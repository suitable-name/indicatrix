//! The document's round trip with the library: opening an entry, saving into it, and the
//! recent-items list on the More page.
//!
//! The database work (`load`, `save`) runs on a worker queue; the state is touched only on
//! the UI thread, where [`open_library_entry`] and the save hand their results back.

use std::cell::RefCell;

use indicatrix_mobile_core::{
    document::{self, DocumentOrigin, OpenError},
    layout::Destination,
    library::{EntryId, LibraryDesign},
    settings::{RecentItem, RecentKind},
    tasks::{BackgroundQueue, Generation},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{
    close_gallery, guard,
    handle::{run_on_ui, toast},
};
use crate::{AppWindow, DocumentModel, Shell, design::Shared};

thread_local! {
    /// The queue the library reads and writes run on; created on first use and ended by
    /// [`shutdown`].
    static QUEUE: RefCell<Option<BackgroundQueue>> = const { RefCell::new(None) };
    /// Counts library opens: only the newest answer is applied.
    static LATEST_OPEN: Generation = Generation::default();
    /// The recent items the More page was last given (to skip identical pushes).
    static SHOWN_RECENT: RefCell<Vec<RecentItem>> = const { RefCell::new(Vec::new()) };
}

/// Ends the worker queue. Call from the UI thread at shutdown (`handle::forget`): the
/// queue must not be dropped by the process's thread-local teardown.
pub(super) fn shutdown() {
    let queue = QUEUE.with(|slot| slot.borrow_mut().take());
    drop(queue);
    SHOWN_RECENT.with(|shown| shown.borrow_mut().clear());
}

/// Runs `job` on the worker queue.
fn submit(job: impl FnOnce() + Send + 'static) {
    QUEUE.with(|slot| {
        slot.borrow_mut()
            .get_or_insert_with(|| BackgroundQueue::new("document-library"))
            .submit(job);
    });
}

/// Opens library entry `id` as the current design: settles unsaved changes first, then
/// loads the entry on a worker, installs it with the origin "Library", records it in the
/// recent items and switches to the Design destination.
pub fn open_library_entry(window: &AppWindow, state: &Shared, id: i64) {
    guard::confirm_discard(window, state, guard::Pending::Library(id));
}

/// Loads entry `id` and opens it; the unsaved-changes question is already settled.
pub(super) fn load_and_open(window: &AppWindow, state: &Shared, id: EntryId) {
    let handle = document::library_handle(&mut state.borrow_mut());
    let handle = match handle {
        Ok(handle) => handle,
        Err(message) => {
            toast(window, &message);
            return;
        }
    };
    let generation = LATEST_OPEN.with(Generation::advance);
    let weak = window.as_weak();
    submit(move || {
        let loaded = handle.load(id);
        run_on_ui(&weak, move |window, state| {
            if LATEST_OPEN.with(|latest| latest.is_current(generation)) {
                finish_open(window, state, id, loaded);
            }
        });
    });
}

/// What the snackbar says when the restored design's library entry turns out to be deleted.
const RESTORED_ENTRY_DELETED: &str =
    "Your last design was deleted from the Library. It is open as an unsaved design.";

/// Checks the design the autosave brought back at start-up against the library: when its
/// entry no longer exists (the app died between a delete and the clean-up that follows it,
/// or the autosave was written by a build older than that clean-up), the design is
/// detached from the entry and its autosave is deleted, so it is not restored again and a
/// Save cannot silently create a new entry for a design the user deleted. The design stays
/// open as an unsaved design. The library is asked on the worker queue, and an entry whose
/// row cannot be read counts as existing.
pub(super) fn settle_restored(window: &AppWindow, state: &Shared) {
    let prepared = {
        let mut app = state.borrow_mut();
        let entry = app.document.library_entry();
        entry.and_then(|entry| {
            document::library_handle(&mut app)
                .ok()
                .map(|handle| (entry, handle))
        })
    };
    let Some((entry, handle)) = prepared else {
        return;
    };
    let weak = window.as_weak();
    submit(move || {
        if handle.missing(&[entry]).is_empty() {
            return;
        }
        run_on_ui(&weak, move |window, state| {
            let detached = match state.try_borrow_mut() {
                Ok(mut app) => app.forget_deleted_entries(&[entry]),
                Err(error) => {
                    tracing::warn!(%error, "the app state was busy; a deleted entry stays linked");
                    false
                }
            };
            if detached {
                crate::design::refresh(window, state);
                toast(window, RESTORED_ENTRY_DELETED);
            }
        });
    });
}

/// Installs a loaded entry, or says why it could not be opened.
fn finish_open(
    window: &AppWindow,
    state: &Shared,
    id: EntryId,
    loaded: Result<LibraryDesign, String>,
) {
    let result = loaded
        .map_err(OpenError::new)
        .and_then(|design| document::open_library(&mut state.borrow_mut(), design, id));
    match result {
        Ok(report) => {
            close_gallery(window);
            {
                let mut app = state.borrow_mut();
                app.set_destination(Destination::Design);
            }
            window
                .global::<Shell>()
                .set_destination(Destination::Design.index());
            crate::design::after_edit(window, state);
            toast(window, &report.message());
        }
        Err(error) => toast(window, &error.message),
    }
}

/// "Save to Library" (and "Save" on a design that came from the library): writes the
/// design into its entry, or into a new one, on the worker. `then_pending` also runs the
/// action waiting behind the unsaved-changes question once the write succeeded.
pub(super) fn save(window: &AppWindow, state: &Shared, then_pending: bool) {
    let prepared = {
        let mut app = state.borrow_mut();
        document::library_handle(&mut app).and_then(|handle| {
            let plan = document::plan_library_save(&mut app)?;
            Ok((handle, plan, app.document.origin.clone()))
        })
    };
    let (handle, plan, origin) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            if then_pending {
                guard::clear_pending();
            }
            toast(window, &message);
            return;
        }
    };
    let weak = window.as_weak();
    submit(move || {
        let result = document::write_library_save(&handle, &plan);
        run_on_ui(&weak, move |window, state| {
            saved(window, state, &plan, &origin, result, then_pending);
        });
    });
}

/// The worker finished a save to the library.
fn saved(
    window: &AppWindow,
    state: &Shared,
    plan: &document::LibrarySavePlan,
    origin: &DocumentOrigin,
    result: Result<EntryId, String>,
    then_pending: bool,
) {
    match result {
        Ok(id) => {
            // A different design replaced this one while the write ran: the entry is
            // written, but the open design is not its working copy.
            let same_document = state.borrow().document.origin == *origin;
            if same_document {
                document::finish_library_save(&mut state.borrow_mut(), plan, id);
                crate::design::refresh(window, state);
            }
            let text = plan.warning.as_ref().map_or_else(
                || "Saved to Library".to_owned(),
                |warning| format!("Saved to Library. {warning}"),
            );
            toast(window, &text);
            if then_pending {
                guard::run_pending(window, state);
            }
        }
        Err(message) => {
            if then_pending {
                guard::clear_pending();
            }
            toast(window, &message);
        }
    }
}

/// The dialog id of "Clear Recent Items?". More's row opens that question itself with this
/// id (`ui/pages/more.slint`); keep the two in step. Ids 1001 and 1002 are the
/// unsaved-changes questions (`guard`).
pub(super) const DIALOG_CLEAR_RECENT: i32 = 1003;

/// Answers the recent items' question. Returns false for any other dialog id.
pub(super) fn handle_dialog_result(
    window: &AppWindow,
    state: &Shared,
    id: i32,
    confirmed: bool,
) -> bool {
    if id != DIALOG_CLEAR_RECENT {
        return false;
    }
    if confirmed {
        state.borrow_mut().update_settings(|settings| {
            settings.clear_recent();
        });
        crate::design::refresh(window, state);
        toast(window, "Recent items cleared");
    }
    true
}

/// A row of the Recent list was tapped: a library design reopens, a file asks for the
/// picker (the platform cannot reopen a file by name).
pub(super) fn open_recent(window: &AppWindow, state: &Shared, index: usize) {
    let item = state.borrow().settings.recent.get(index).cloned();
    let Some(item) = item else {
        return;
    };
    match item.kind {
        RecentKind::Library { entry_id } => open_library_entry(window, state, entry_id),
        RecentKind::Document { display_name } => {
            toast(
                window,
                &format!("Choose \"{display_name}\" to open it again."),
            );
            super::request_open(window, state);
        }
    }
}

/// Pushes the recent items and whether the design belongs to the library. Cheap when the
/// recent list did not change.
pub(super) fn push(window: &AppWindow, state: &indicatrix_mobile_core::app_state::AppState) {
    let model = window.global::<DocumentModel>();
    model.set_in_library(state.document.library_entry().is_some());
    let unchanged = SHOWN_RECENT.with(|shown| *shown.borrow() == state.settings.recent);
    if unchanged {
        return;
    }
    let titles: Vec<SharedString> = state
        .settings
        .recent
        .iter()
        .map(|item| item.title.as_str().into())
        .collect();
    let details: Vec<SharedString> = state
        .settings
        .recent
        .iter()
        .map(|item| detail(item).into())
        .collect();
    model.set_recent_titles(ModelRc::new(VecModel::from(titles)));
    model.set_recent_details(ModelRc::new(VecModel::from(details)));
    SHOWN_RECENT.with(|shown| shown.borrow_mut().clone_from(&state.settings.recent));
}

/// The one line under a recent item.
fn detail(item: &RecentItem) -> String {
    match &item.kind {
        RecentKind::Library { .. } => "In your Library".to_owned(),
        RecentKind::Document { display_name } => {
            format!("{display_name} \u{b7} Open the file again")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn details_name_where_the_design_is() {
        let library = RecentItem {
            title: "A".to_owned(),
            opened_at_unix: 0,
            kind: RecentKind::Library { entry_id: 1 },
        };
        let file = RecentItem {
            title: "B".to_owned(),
            opened_at_unix: 0,
            kind: RecentKind::Document {
                display_name: "b.asc".to_owned(),
            },
        };
        assert_eq!(detail(&library), "In your Library");
        assert!(detail(&file).starts_with("b.asc"));
        assert!(detail(&file).ends_with("Open the file again"));
    }
}
