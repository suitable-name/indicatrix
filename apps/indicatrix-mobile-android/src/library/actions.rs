//! What the Library does for the user: import, tags, ignore, delete and share.
//!
//! Each action reads what it needs from the runtime on the UI thread, runs the database
//! part on the runtime's queue and finishes on the UI thread, where it reloads the list
//! and tells the user what happened. Nothing here blocks the UI thread.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::Arc,
};

use indicatrix_mobile_core::{
    library::{EntryId, ImportReport, LibraryHandle},
    platform::{OpenKind, OpenRequest, PickedFile, Platform},
};
use slint::{ComponentHandle, SharedString, VecModel};

use super::{
    format,
    list::{self, Reload},
    runtime::{self, Runtime},
    tags::{self, Chip, SheetState},
};
use crate::{AppWindow, LibraryModel, Shell, TagChipData, design::Shared, push::toast};

/// The dialog id of "Delete Designs?" (ids 3000 to 3099 belong to the Library).
pub(super) const DIALOG_DELETE: i32 = 3001;
/// The dialog id of the list of files an import could not read.
pub(super) const DIALOG_IMPORT_FAILED: i32 = 3002;

/// The platform services, from the app state.
fn platform_of(rt: &Runtime) -> Option<Arc<dyn Platform>> {
    rt.shared
        .try_borrow()
        .ok()
        .map(|app| Arc::clone(app.platform()))
}

// ---------------------------------------------------------------------------------
// Import
// ---------------------------------------------------------------------------------

/// Asks for design files and imports them. The library is opened first when it has not
/// been shown yet.
pub(super) fn import_designs(window: &AppWindow) {
    let Some(handle) = list::ensure_open(window) else {
        toast::show(window, "The library could not be opened.");
        return;
    };
    let Some(platform) = runtime::with(|rt| platform_of(rt)).flatten() else {
        return;
    };
    let weak = window.as_weak();
    platform.open_documents(
        OpenRequest {
            kind: OpenKind::Designs,
            allow_multiple: true,
        },
        Box::new(move |result| {
            let delivered = weak.upgrade_in_event_loop(move |window| match result {
                Ok(files) if files.is_empty() => {}
                Ok(files) => start_import(&window, handle, files),
                Err(error) => toast::show(&window, &error.message),
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "picked files were dropped: the UI thread is gone");
            }
        }),
    );
}

/// Imports `files` on the queue and reports when done.
fn start_import(window: &AppWindow, handle: LibraryHandle, files: Vec<PickedFile>) {
    let count = files.len();
    let started =
        runtime::with(|rt| {
            rt.busy = Some(format::importing_text(count));
            list::sync(rt, window);
            let weak = rt.weak.clone();
            let Some(queue) = rt.queue.as_ref() else {
                return false;
            };
            queue.submit(move || {
                let report = catch_unwind(AssertUnwindSafe(|| handle.import(files)))
                    .unwrap_or_else(|_| ImportReport {
                        failed: vec![(
                            "Import".to_owned(),
                            "The import stopped unexpectedly.".to_owned(),
                        )],
                        ..ImportReport::default()
                    });
                let delivered = weak.upgrade_in_event_loop(move |window| {
                    import_done(&window, &report);
                });
                if let Err(error) = delivered {
                    tracing::debug!(%error, "an import result was dropped: the UI thread is gone");
                }
            });
            true
        });
    if started != Some(true) {
        runtime::with(|rt| {
            rt.busy = None;
            list::sync(rt, window);
        });
        toast::show(window, "The library is not open.");
    }
}

/// An import finished: reload the list and say what happened.
fn import_done(window: &AppWindow, report: &ImportReport) {
    runtime::with(|rt| {
        rt.busy = None;
        let revision = list::current_revision(rt);
        list::reload(rt, revision, Reload::Refresh);
        list::sync(rt, window);
    });
    let summary = format::import_message(report);
    if report.failed.is_empty() {
        toast::show(window, &summary);
    } else {
        let body = format!("{summary}\n\n{}", format::failure_details(report, 5));
        window.global::<Shell>().invoke_show_dialog(
            DIALOG_IMPORT_FAILED,
            "Some Files Were Not Imported".into(),
            body.into(),
            "OK".into(),
            "".into(),
        );
    }
}

// ---------------------------------------------------------------------------------
// Writes on the selection
// ---------------------------------------------------------------------------------

/// Runs `job` on the queue and finishes on the UI thread: on success the selection ends,
/// the list reloads and `done_text` is shown; on failure the error is shown and the
/// selection stays.
fn run_write(
    window: &AppWindow,
    done_text: String,
    job: impl FnOnce(&LibraryHandle) -> Result<(), String> + Send + 'static,
) {
    let queued = runtime::with(|rt| {
        let (Some(handle), Some(queue)) = (rt.handle.clone(), rt.queue.as_ref()) else {
            return false;
        };
        let weak = rt.weak.clone();
        queue.submit(move || {
            let result = catch_unwind(AssertUnwindSafe(|| job(&handle)))
                .unwrap_or_else(|_| Err("The change stopped unexpectedly.".to_owned()));
            let delivered = weak.upgrade_in_event_loop(move |window| {
                write_done(&window, &result, &done_text);
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "a library change was dropped: the UI thread is gone");
            }
        });
        true
    });
    if queued != Some(true) {
        toast::show(window, "The library is not open.");
    }
}

/// A write finished on the queue.
fn write_done(window: &AppWindow, result: &Result<(), String>, done_text: &str) {
    match result {
        Ok(()) => {
            runtime::with(|rt| {
                list::clear_selection(rt, window);
                let revision = list::current_revision(rt);
                list::reload(rt, revision, Reload::Refresh);
                list::sync(rt, window);
            });
            toast::show(window, done_text);
        }
        Err(message) => toast::show(window, message),
    }
}

/// "n design(s)".
fn designs(count: usize) -> String {
    format!("{count} {}", if count == 1 { "design" } else { "designs" })
}

/// Ignores the selected designs, or stops ignoring them when all of them are ignored.
pub(super) fn toggle_ignore_selected(window: &AppWindow) {
    let chosen = runtime::with(|rt| {
        let cards = list::selected_cards(rt);
        let ids: Vec<EntryId> = cards.iter().map(|card| card.id).collect();
        let ignore = !cards.iter().all(|card| card.ignored);
        (ids, ignore)
    });
    let Some((ids, ignore)) = chosen else {
        return;
    };
    if ids.is_empty() {
        return;
    }
    let text = if ignore {
        format!("Ignored {}", designs(ids.len()))
    } else {
        format!("Stopped ignoring {}", designs(ids.len()))
    };
    run_write(window, text, move |handle| handle.set_ignored(&ids, ignore));
}

/// Asks before the selected designs are deleted.
pub(super) fn ask_delete_selected(window: &AppWindow) {
    let ids = runtime::with(|rt| {
        let ids: Vec<EntryId> = list::selected_cards(rt)
            .iter()
            .map(|card| card.id)
            .collect();
        rt.pending_delete.clone_from(&ids);
        ids
    })
    .unwrap_or_default();
    if ids.is_empty() {
        return;
    }
    let body = format!(
        "{} will be removed from this device's library, together with the cutting progress and \
         recent items kept for {}. This cannot be undone.",
        designs(ids.len()),
        if ids.len() == 1 { "it" } else { "them" },
    );
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_DELETE,
        "Delete Designs?".into(),
        body.into(),
        "Delete".into(),
        "Cancel".into(),
    );
}

/// Answers the Library's dialogs. Returns false for an id that is not the Library's.
pub(super) fn handle_dialog_result(window: &AppWindow, id: i32, confirmed: bool) -> bool {
    match id {
        DIALOG_DELETE => {
            let ids =
                runtime::with(|rt| std::mem::take(&mut rt.pending_delete)).unwrap_or_default();
            if confirmed && !ids.is_empty() {
                run_delete(window, ids);
            }
            true
        }
        DIALOG_IMPORT_FAILED => true,
        _ => false,
    }
}

/// Deletes `ids` on the queue. Back on the UI thread the app state forgets the entries that
/// are gone (their recent items, an autosave of them, the open design's link to them), the
/// list reloads and the user is told.
fn run_delete(window: &AppWindow, ids: Vec<EntryId>) {
    let queued = runtime::with(|rt| {
        let (Some(handle), Some(queue)) = (rt.handle.clone(), rt.queue.as_ref()) else {
            return false;
        };
        let weak = rt.weak.clone();
        queue.submit(move || {
            let result = catch_unwind(AssertUnwindSafe(|| handle.delete(&ids)))
                .unwrap_or_else(|_| Err("The change stopped unexpectedly.".to_owned()));
            // Not `ids`: a delete that stopped early left the later entries in place.
            let gone = catch_unwind(AssertUnwindSafe(|| handle.missing(&ids))).unwrap_or_default();
            let delivered = weak.upgrade_in_event_loop(move |window| {
                delete_done(&window, &result, &gone);
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "a library delete was dropped: the UI thread is gone");
            }
        });
        true
    });
    if queued != Some(true) {
        toast::show(window, "The library is not open.");
    }
}

/// A delete finished on the queue; `gone` are the entries that no longer exist.
fn delete_done(window: &AppWindow, result: &Result<(), String>, gone: &[EntryId]) {
    let shared = runtime::with(|rt| Rc::clone(&rt.shared));
    let detached = shared
        .as_ref()
        .is_some_and(|shared| forget_deleted(shared, gone));
    write_done(window, result, &deleted_text(gone.len(), detached));
    if let Some(shared) = shared {
        // The More page's recent items, the Save rows and the Cut page follow the state.
        crate::design::refresh(window, &shared);
    }
}

/// Lets the app state forget the deleted entries `gone`. Returns whether the open design
/// was one of them.
fn forget_deleted(shared: &Shared, gone: &[EntryId]) -> bool {
    let Ok(mut app) = shared.try_borrow_mut() else {
        tracing::warn!("the app state was busy; the deleted designs were not forgotten");
        return false;
    };
    app.forget_deleted_entries(gone)
}

/// What the snackbar says after `count` designs were deleted; `detached` says that the open
/// design was one of them and stays open, now as an unsaved design.
fn deleted_text(count: usize, detached: bool) -> String {
    let deleted = format!("Deleted {}", designs(count));
    if detached {
        format!("{deleted}. The design you have open stays open and is now unsaved.")
    } else {
        deleted
    }
}

/// Offers the selected design's file through the share sheet.
pub(super) fn share_selected(window: &AppWindow) {
    let job = runtime::with(|rt| {
        if rt.selected.len() != 1 {
            return None;
        }
        let id = *rt.selected.iter().next()?;
        Some((id, rt.handle.clone()?, platform_of(rt)?, rt.weak.clone()))
    })
    .flatten();
    let Some((id, handle, platform, weak)) = job else {
        return;
    };
    let queued = runtime::with(|rt| {
        let Some(queue) = rt.queue.as_ref() else {
            return false;
        };
        queue.submit(move || {
            let made = handle.export_bytes(id);
            let delivered = weak.upgrade_in_event_loop(move |window| match made {
                Ok(file) => {
                    if let Err(error) = platform.share(file) {
                        toast::show(&window, &error.message);
                    }
                }
                Err(message) => toast::show(&window, &message),
            });
            if let Err(error) = delivered {
                tracing::debug!(%error, "a shared design was dropped: the UI thread is gone");
            }
        });
        true
    });
    if queued != Some(true) {
        toast::show(window, "The library is not open.");
    }
}

// ---------------------------------------------------------------------------------
// The tag sheet
// ---------------------------------------------------------------------------------

/// The chip data for a tag chip.
fn chip_data(chip: &Chip) -> TagChipData {
    TagChipData {
        text: SharedString::from(chip.text.as_str()),
        on: chip.on,
    }
}

/// Brings `model` in line with `chips`, touching only the rows that differ so a chip
/// row that is being scrolled keeps its place.
fn refresh_chips(model: &VecModel<TagChipData>, chips: &[Chip]) {
    use slint::Model;
    if model.row_count() != chips.len() {
        model.set_vec(chips.iter().map(chip_data).collect::<Vec<_>>());
        return;
    }
    for (index, chip) in chips.iter().enumerate() {
        let differs = model
            .row_data(index)
            .is_none_or(|row| row.on != chip.on || row.text.as_str() != chip.text.as_str());
        if differs {
            model.set_row_data(index, chip_data(chip));
        }
    }
}

/// Writes the sheet's chips to the page.
fn sync_sheet(rt: &Runtime) {
    refresh_chips(&rt.tag_add_model, &rt.tag_sheet.add);
    refresh_chips(&rt.tag_remove_model, &rt.tag_sheet.remove);
}

/// Opens the tag sheet for the selected designs.
pub(super) fn open_tag_sheet(window: &AppWindow) {
    runtime::with(|rt| {
        let carried: Vec<Vec<String>> = list::selected_cards(rt)
            .iter()
            .map(|card| card.tags.clone())
            .collect();
        if carried.is_empty() {
            return;
        }
        rt.tag_sheet = SheetState::open(&rt.options.tags, &carried);
        rt.tag_sheet_open = true;
        sync_sheet(rt);
        window.global::<LibraryModel>().set_tag_new_text("".into());
        list::sync(rt, window);
    });
}

/// Closes the tag sheet without applying it.
pub(super) fn close_tag_sheet(window: &AppWindow) {
    runtime::with(|rt| {
        rt.tag_sheet_open = false;
        list::sync(rt, window);
    });
}

/// Flips an "add" chip.
pub(super) fn toggle_add(index: i32) {
    runtime::with(|rt| {
        if let Ok(index) = usize::try_from(index) {
            rt.tag_sheet.toggle_add(index);
            sync_sheet(rt);
        }
    });
}

/// Flips a "remove" chip.
pub(super) fn toggle_remove(index: i32) {
    runtime::with(|rt| {
        if let Ok(index) = usize::try_from(index) {
            rt.tag_sheet.toggle_remove(index);
            sync_sheet(rt);
        }
    });
}

/// Adds the typed tag name to the chips to add.
pub(super) fn add_new_tag(window: &AppWindow) {
    let model = window.global::<LibraryModel>();
    let typed = model.get_tag_new_text();
    runtime::with(|rt| {
        if rt.tag_sheet.add_new(&typed) {
            sync_sheet(rt);
            model.set_tag_new_text("".into());
        }
    });
}

/// Applies the sheet's choices to the selected designs.
pub(super) fn apply_tags(window: &AppWindow) {
    // A name typed but not yet added still counts.
    add_new_tag(window);
    let changes = runtime::with(|rt| {
        let cards: Vec<(EntryId, Vec<String>)> = list::selected_cards(rt)
            .iter()
            .map(|card| (card.id, card.tags.clone()))
            .collect();
        let changes = tags::plan(
            &cards,
            &rt.tag_sheet.chosen_add(),
            &rt.tag_sheet.chosen_remove(),
        );
        rt.tag_sheet_open = false;
        list::sync(rt, window);
        changes
    })
    .unwrap_or_default();
    if changes.is_empty() {
        toast::show(window, "No tags changed");
        return;
    }
    let touched: usize = changes.iter().map(|change| change.ids.len()).sum();
    run_write(
        window,
        format!("Tagged {}", designs(touched)),
        move |handle| {
            for change in &changes {
                handle.set_tags(&change.ids, &change.tags)?;
            }
            Ok(())
        },
    );
}

/// Opens a design: the shared document code asks about unsaved changes and installs it.
pub(super) fn open_entry(window: &AppWindow, state: &Shared, id: EntryId) {
    crate::design::document::open_library_entry(window, state, id);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delete_message_counts_and_names_the_open_design() {
        assert_eq!(deleted_text(1, false), "Deleted 1 design");
        assert_eq!(deleted_text(3, false), "Deleted 3 designs");
        assert_eq!(
            deleted_text(1, true),
            "Deleted 1 design. The design you have open stays open and is now unsaved."
        );
    }
}
