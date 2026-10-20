//! Durable vault variants. All database access is worker-owned.
use super::Shared;
use crate::{AppWindow, HistoryModel, VariantsModel};
use indicatrix_cut_core::native::{LoadedDesign, design_from_str};
use indicatrix_mobile_core::library::{NewVariant, VariantSummary};
use indicatrix_mobile_core::{
    library::LibraryHandle,
    tasks::BackgroundQueue,
    variants::{self, VariantCapture, VariantStamp},
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc};

struct Runtime {
    shared: Shared,
    queue: BackgroundQueue,
    rows: Vec<VariantSummary>,
    parent: Option<i64>,
    family: String,
    request: u64,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }
enum Action {
    List,
    Save(VariantCapture, String, String, Option<i64>),
    Open(i64),
    Rename(i64, String),
    Note(i64, String),
    Delete(i64),
}
struct Answer {
    rows: Vec<VariantSummary>,
    loaded: Option<(LoadedDesign, String, i64)>,
    saved: Option<i64>,
}

pub fn wire(window: &AppWindow, shared: &Shared) {
    RUNTIME.with(|value| {
        *value.borrow_mut() = Some(Runtime {
            shared: Rc::clone(shared),
            queue: BackgroundQueue::new("variants"),
            rows: Vec::new(),
            parent: None,
            family: String::new(),
            request: 0,
        });
    });
    let weak = window.as_weak();
    window.global::<VariantsModel>().on_show(move || {
        if let Some(window) = weak.upgrade() {
            window.global::<VariantsModel>().set_open(true);
            submit(&window, Action::List);
        }
    });
    let weak = window.as_weak();
    window.global::<VariantsModel>().on_close(move || {
        // Persist explicitly submitted saves, but never apply an Open after Back.
        // A later visit has its own request; queued answers from this visit are stale.
        RUNTIME.with(|value| {
            if let Some(runtime) = value.borrow_mut().as_mut() {
                runtime.request = runtime
                    .request
                    .checked_add(1)
                    .expect("variant request ID exhausted");
            }
        });
        if let Some(window) = weak.upgrade() {
            window.global::<VariantsModel>().set_open(false);
            window.global::<VariantsModel>().set_busy(false);
        }
    });
    let weak = window.as_weak();
    window.global::<VariantsModel>().on_select(move |index| {
        let Some(window) = weak.upgrade() else { return };
        RUNTIME.with(|value| {
            let guard = value.borrow();
            let Some(row) = guard.as_ref().and_then(|runtime| {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| runtime.rows.get(index))
            }) else {
                return;
            };
            let model = window.global::<VariantsModel>();
            model.set_selected(index);
            model.set_name(row.name.as_str().into());
            model.set_note(row.note.as_deref().unwrap_or("").into());
            model.set_confirm_open(false);
            model.set_confirm_delete(false);
        });
    });
    wire_actions(window);
}

/// Save, and Open, Rename, Update Note and Remove of the selected variant.
fn wire_actions(window: &AppWindow) {
    let weak = window.as_weak();
    window.global::<VariantsModel>().on_save(move |historical| {
        let Some(window) = weak.upgrade() else { return };
        let capture = RUNTIME.with(|value| {
            let mut guard = value.borrow_mut();
            let runtime = guard
                .as_mut()
                .ok_or_else(|| "The editor is closed.".to_owned())?;
            let mut app = runtime.shared.borrow_mut();
            let design = if historical {
                let position = usize::try_from(window.global::<HistoryModel>().get_selected())
                    .map_err(|_| "Select a historical step first.".to_owned())?;
                super::history::selected_design(&app, position)?
            } else {
                app.session.design.clone()
            };
            let capture = VariantCapture::capture(&mut app, &design)?;
            Ok::<_, String>((capture, runtime.parent))
        });
        match capture {
            Ok((capture, parent)) => {
                let model = window.global::<VariantsModel>();
                submit(
                    &window,
                    Action::Save(
                        capture,
                        model.get_name().to_string(),
                        model.get_note().to_string(),
                        parent,
                    ),
                );
            }
            Err(error) => window.global::<VariantsModel>().set_status(error.into()),
        }
    });
    for kind in 0..4 {
        let weak = window.as_weak();
        let action = move || {
            let Some(window) = weak.upgrade() else { return };
            let model = window.global::<VariantsModel>();
            let selected = selected(&window);
            let Some(id) = selected else { return };
            if kind == 0 {
                let dirty = RUNTIME.with(|value| {
                    value
                        .borrow()
                        .as_ref()
                        .is_some_and(|runtime| runtime.shared.borrow().session.is_dirty())
                });
                if dirty && !model.get_confirm_open() {
                    model.set_confirm_open(true);
                    model.set_status("Opening replaces the unsaved working copy. Confirm to proceed; Undo restores it.".into());
                    return;
                }
            }
            let action = match kind {
                0 => Action::Open(id),
                1 => Action::Rename(id, model.get_name().to_string()),
                2 => Action::Note(id, model.get_note().to_string()),
                _ => Action::Delete(id),
            };
            submit(&window, action);
        };
        match kind {
            0 => window.global::<VariantsModel>().on_open_selected(action),
            1 => window.global::<VariantsModel>().on_rename(action),
            2 => window.global::<VariantsModel>().on_update_note(action),
            _ => window.global::<VariantsModel>().on_remove(action),
        }
    }
}

fn selected(window: &AppWindow) -> Option<i64> {
    let index = usize::try_from(window.global::<VariantsModel>().get_selected()).ok()?;
    RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()?
            .rows
            .get(index)
            .map(|row| row.variant_id)
    })
}

/// Called only after candidate provenance/freshness validation by its owning workflow.
pub fn save_candidate(
    window: &AppWindow,
    shared: &Shared,
    design: &indicatrix_cut_core::Design,
    name: &str,
) {
    let capture = VariantCapture::capture(&mut shared.borrow_mut(), design);
    match capture {
        Ok(capture) => {
            let parent =
                RUNTIME.with(|value| value.borrow().as_ref().and_then(|runtime| runtime.parent));
            submit(
                window,
                Action::Save(
                    capture,
                    name.to_owned(),
                    "Saved from a validated operation candidate".into(),
                    parent,
                ),
            );
            window.global::<VariantsModel>().set_open(true);
        }
        Err(error) => crate::push::toast::show(window, &error),
    }
}

fn submit(window: &AppWindow, action: Action) {
    if window.global::<VariantsModel>().get_busy() {
        return;
    }
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else { return };
        let (uuid, stamp, opener) = {
            let mut app = runtime.shared.borrow_mut();
            indicatrix_mobile_core::document::ensure_document_id(&mut app);
            (app.document.metadata.id.clone(), VariantStamp::capture(&app), app.library.opener(app.platform().app_dirs()))
        };
        if uuid != runtime.family { runtime.family.clone_from(&uuid); runtime.parent = None; runtime.rows.clear(); }
        runtime.request += 1;
        let request = runtime.request;
        let weak = window.as_weak();
        window.global::<VariantsModel>().set_busy(true);
        runtime.queue.submit(move || {
            let outcome = opener.open().and_then(|handle| perform(&handle, &uuid, action).map(|answer| (handle, answer)));
            let _queued = weak.upgrade_in_event_loop(move |window| {
                let shared = RUNTIME.with(|value| {
                    let mut guard = value.borrow_mut();
                    let runtime = guard.as_mut()?;
                    if runtime.request != request { return None; }
                    let mut app = runtime.shared.borrow_mut();
                    let model = window.global::<VariantsModel>();
                    model.set_busy(false);
                    if !stamp.is_current(&app) {
                        model.set_status("The working design changed. Refresh variants before opening a result. Completed saves remain in the library.".into());
                        return None;
                    }
                    let mut changed = false;
                    match outcome {
                        Err(error) => model.set_status(error.into()),
                        Ok((handle, answer)) => {
                            app.library.adopt(handle);
                            runtime.rows = answer.rows;
                            if let Some(parent) = answer.saved { runtime.parent = Some(parent); }
                            if let Some((loaded, name, id)) = answer.loaded {
                                match variants::open(&mut app, stamp, &loaded, &name) {
                                    Ok(did_change) => { changed = did_change; runtime.parent = Some(id); model.set_status("Opened variant. Undo restores the previous design.".into()); }
                                    Err(error) => model.set_status(error.into()),
                                }
                            } else { model.set_status("Variants saved in the local library.".into()); }
                            model.set_rows(ModelRc::new(VecModel::from(runtime.rows.iter().map(|row| format!("{}{}", row.name, row.note.as_ref().map_or(String::new(), |note| format!(" · {note}"))).into()).collect::<Vec<slint::SharedString>>())));
                            model.set_selected(-1); model.set_confirm_open(false); model.set_confirm_delete(false);
                        }
                    }
                    Some((Rc::clone(&runtime.shared), changed))
                });
                if let Some((shared, changed)) = shared {
                    if changed { super::after_edit(&window, &shared); }
                    else { super::refresh(&window, &shared); }
                }
            });
        });
    });
}

fn perform(handle: &LibraryHandle, uuid: &str, action: Action) -> Result<Answer, String> {
    let mut loaded = None;
    let mut saved = None;
    match action {
        Action::List => {}
        Action::Save(capture, name, note, parent) => {
            if capture.uuid != uuid {
                return Err("This captured state belongs to another design.".into());
            }
            let parent = parent.filter(|id| handle.load_variant(uuid, *id).is_ok());
            let created_at = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |elapsed| {
                    i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
                });
            saved = Some(handle.save_variant(
                uuid,
                &NewVariant {
                    name: &name,
                    note: Some(&note),
                    parent_variant_id: parent,
                    design_text: &capture.text,
                    thumbnail_png: None,
                    created_at,
                },
            )?);
        }
        Action::Open(id) => {
            let variant = handle.load_variant(uuid, id)?;
            let design =
                design_from_str(&variant.design_text).map_err(|error| error.to_string())?;
            loaded = Some((design, variant.summary.name, id));
        }
        Action::Rename(id, name) => handle.rename_variant(uuid, id, &name)?,
        Action::Note(id, note) => handle.set_variant_note(uuid, id, Some(&note))?,
        Action::Delete(id) => handle.delete_variant(uuid, id)?,
    }
    Ok(Answer {
        rows: handle.list_variants(uuid)?,
        loaded,
        saved,
    })
}

pub fn shutdown() {
    RUNTIME.with(|value| drop(value.borrow_mut().take()));
}
