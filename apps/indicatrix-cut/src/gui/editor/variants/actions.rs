//! What each button of the Variants view does: opening the forms, saving, renaming, changing a
//! note, deleting and opening a variant. Comparing two designs is in [`super::comparing`].
//!
//! Every function runs on the UI thread. The ones that need the library start a worker thread
//! ([`spawn_job`]) and finish in a continuation that finds the shared state again through
//! [`live_deps`]. `VariantsModel.busy` is on from the click until the continuation runs, and
//! no `EditorState` borrow is held across a call into Slint other than the refresh that Undo
//! itself makes while holding one.

use super::{
    Deps, Identity, VARIANTS_VIEW, adopt_open_design, apply, capture,
    form::{self, Form, FormView},
    identity_of, live_deps, publish, refresh, rows, spawn_job, store, unix_now, with_db,
};
use crate::{
    GuideModel, MainWindow, VariantsModel,
    gui::{
        editor::{
            callbacks::{HistoryMove, refresh_after_renumbering_move},
            state::EditorState,
        },
        show_toast,
        tutorial_events::raise,
    },
};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    EditorSession, guide::solving_events::VARIANT_SAVED, session::SessionEditError,
};
use indicatrix_vault::model::design_variant::VariantSummary;
use slint::{ComponentHandle, SharedString};
use std::{rc::Rc, sync::Arc};

pub(super) const NO_DESIGN_TEXT: &str = "Open or create a design first.";
const WAIT_TEXT: &str = "Wait a moment. The last action is still being worked out.";
pub(super) const EDITOR_BUSY_TEXT: &str = "The editor is busy. Try again in a moment.";
const STEP_GONE_TEXT: &str = "That step is not in the history any more.";
const STEP_CHANGED_TEXT: &str =
    "The history changed since you chose this step. Choose the step again.";
/// Said when the design was opened again, or another one opened, while the save form was up.
const FORM_DESIGN_CHANGED_TEXT: &str =
    "The design changed since this form was opened. Choose Save as a variant again.";
pub(super) const VARIANT_GONE_TEXT: &str = "That variant is no longer in the list.";

// --- small helpers -------------------------------------------------------------------------

/// Whether a save, rename, delete, open or comparison is still being worked out; says so if it
/// is.
pub(super) fn is_busy(ui: &MainWindow, deps: &Deps) -> bool {
    let busy = deps.page.borrow().busy;
    if busy {
        show_toast(ui, WAIT_TEXT, "info");
    }
    busy
}

pub(super) fn set_busy(ui: &MainWindow, deps: &Deps, busy: bool) {
    deps.page.borrow_mut().busy = busy;
    ui.global::<VariantsModel>().set_busy(busy);
}

/// Shows the form for `form` with the words in `view`, and switches the History tab to the
/// Variants view.
fn show_form(ui: &MainWindow, deps: &Deps, form: Form, view: &FormView) {
    let mode = form.code();
    deps.page.borrow_mut().form = form;
    let model = ui.global::<VariantsModel>();
    model.set_form_error(SharedString::new());
    model.set_form_title(view.title.as_str().into());
    model.set_form_hint(view.hint.as_str().into());
    model.set_form_ok(view.ok.into());
    model.set_form_name(view.name.as_str().into());
    model.set_form_note(view.note.as_str().into());
    model.set_form_mode(mode);
    model.set_view(VARIANTS_VIEW);
}

fn close_form(ui: &MainWindow, deps: &Deps) {
    deps.page.borrow_mut().form = Form::Closed;
    let model = ui.global::<VariantsModel>();
    model.set_form_mode(0);
    model.set_form_error(SharedString::new());
}

/// Puts `reason` under the form's fields and leaves the form open.
fn form_failed(ui: &MainWindow, deps: &Deps, reason: &str) {
    set_busy(ui, deps, false);
    ui.global::<VariantsModel>().set_form_error(reason.into());
}

/// The variant with `key` in the list on screen. A key that is not there (the variant was
/// deleted elsewhere) says so and reads the list again.
fn find_variant(ui: &MainWindow, deps: &Rc<Deps>, key: &str) -> Option<VariantSummary> {
    let found = key.parse::<i64>().ok().and_then(|id| {
        deps.page
            .borrow()
            .list
            .iter()
            .find(|variant| variant.variant_id == id)
            .cloned()
    });
    if found.is_none() {
        show_toast(ui, VARIANT_GONE_TEXT, "info");
        refresh(ui, deps);
    }
    found
}

/// The variants a new name must not repeat: read from the library if its lock is free right
/// now (it never waits), otherwise as last listed.
fn known_variants(deps: &Deps, uuid: &str) -> Vec<VariantSummary> {
    deps.db
        .try_lock()
        .ok()
        .and_then(|db| store::list(&db, uuid).ok())
        .unwrap_or_else(|| deps.page.borrow().list.clone())
}

// --- opening the forms -----------------------------------------------------------------------

/// A history step as the save form names it.
struct StepInfo {
    number: usize,
    label: String,
    /// The step's revision; `None` for the start, which never changes.
    revision: Option<u64>,
}

/// What opening the save form needs from the editor.
struct SavePlan {
    uuid: String,
    /// The design epoch the form is opened at, see [`Form::Save`].
    epoch: u64,
    step: Option<StepInfo>,
}

fn step_info(state: &EditorState, number: usize) -> Result<StepInfo, String> {
    if number == 0 {
        return Ok(StepInfo {
            number,
            label: String::new(),
            revision: None,
        });
    }
    state
        .history_entries()
        .into_iter()
        .find(|entry| entry.position == number)
        .map(|entry| StepInfo {
            number,
            label: entry.label,
            revision: Some(entry.revision),
        })
        .ok_or_else(|| STEP_GONE_TEXT.to_owned())
}

fn prepare_save(state: &EditorState, step: Option<usize>) -> Result<SavePlan, String> {
    let identity = identity_of(state);
    let uuid = identity.uuid.ok_or_else(|| NO_DESIGN_TEXT.to_owned())?;
    let step = step.map(|number| step_info(state, number)).transpose()?;
    Ok(SavePlan {
        uuid,
        epoch: identity.epoch,
        step,
    })
}

/// Opens the save form. `position` is the history step to save (the History tab's row
/// button), or negative for the design as it is now.
pub(super) fn begin_save(ui: &MainWindow, deps: &Rc<Deps>, position: i32) {
    if is_busy(ui, deps) {
        return;
    }
    let Some(plan) = deps.with_state(|state| prepare_save(state, usize::try_from(position).ok()))
    else {
        show_toast(ui, EDITOR_BUSY_TEXT, "info");
        return;
    };
    let plan = match plan {
        Ok(plan) => plan,
        Err(reason) => {
            show_toast(ui, &reason, "info");
            return;
        }
    };
    adopt_open_design(ui, deps);
    let name = rows::default_name(&known_variants(deps, &plan.uuid));
    let step = plan.step.as_ref();
    let view = form::save_view(step.map(|info| (info.number, info.label.as_str())), name);
    let form = Form::Save {
        position: step.map(|info| info.number),
        revision: step.and_then(|info| info.revision),
        design: (plan.uuid, plan.epoch),
    };
    show_form(ui, deps, form, &view);
}

pub(super) fn begin_rename(ui: &MainWindow, deps: &Rc<Deps>, key: &str) {
    if is_busy(ui, deps) {
        return;
    }
    if let Some(variant) = find_variant(ui, deps, key) {
        let form = Form::Rename(variant.variant_id);
        show_form(ui, deps, form, &form::rename_view(&variant));
    }
}

pub(super) fn begin_note(ui: &MainWindow, deps: &Rc<Deps>, key: &str) {
    if is_busy(ui, deps) {
        return;
    }
    if let Some(variant) = find_variant(ui, deps, key) {
        let form = Form::Note(variant.variant_id);
        show_form(ui, deps, form, &form::note_view(&variant));
    }
}

pub(super) fn begin_delete(ui: &MainWindow, deps: &Rc<Deps>, key: &str) {
    if is_busy(ui, deps) {
        return;
    }
    if let Some(variant) = find_variant(ui, deps, key) {
        let form = Form::Delete(variant.variant_id);
        show_form(ui, deps, form, &form::delete_view(&variant));
    }
}

pub(super) fn cancel_form(ui: &MainWindow, deps: &Rc<Deps>) {
    close_form(ui, deps);
}

// --- the form's confirm button -----------------------------------------------------------------

/// The form's confirm button: does what the form is for.
pub(super) fn submit_form(ui: &MainWindow, deps: &Rc<Deps>) {
    if deps.page.borrow().busy {
        return;
    }
    let form = deps.page.borrow().form.clone();
    match form {
        Form::Closed => {}
        Form::Save {
            position,
            revision,
            design,
        } => save(ui, deps, position, revision, (&design.0, design.1)),
        Form::Rename(id) => rename(ui, deps, id),
        Form::Note(id) => change_note(ui, deps, id),
        Form::Delete(id) => remove(ui, deps, id),
    }
}

/// The design a save keeps, and which design it belongs to.
pub(super) struct Captured {
    pub(super) uuid: String,
    pub(super) epoch: u64,
    pub(super) design: Design,
}

fn capture_for_save(
    state: &EditorState,
    position: Option<usize>,
    revision: Option<u64>,
    opened_for: (&str, u64),
) -> Result<Captured, String> {
    capture_from(
        &state.session,
        &identity_of(state),
        position,
        revision,
        opened_for,
    )
}

/// The design a save keeps, if the editor still holds the design the form was opened for
/// (`opened_for`: its UUID and design epoch) and, for a history step, that same step.
///
/// The step is named by its position and revision, and every new history numbers its
/// revisions from zero again, so the pair alone could name a step of another design. The
/// design check comes first for that reason.
pub(super) fn capture_from(
    session: &EditorSession,
    identity: &Identity,
    position: Option<usize>,
    revision: Option<u64>,
    opened_for: (&str, u64),
) -> Result<Captured, String> {
    let uuid = identity
        .uuid
        .clone()
        .ok_or_else(|| NO_DESIGN_TEXT.to_owned())?;
    if !identity.is_design(opened_for.0, opened_for.1) {
        return Err(FORM_DESIGN_CHANGED_TEXT.to_owned());
    }
    if let (Some(step), Some(expected)) = (position, revision) {
        let unchanged = session
            .history_entries()
            .iter()
            .any(|entry| entry.position == step && entry.revision == expected);
        if !unchanged {
            return Err(STEP_CHANGED_TEXT.to_owned());
        }
    }
    let design = capture::design_at_step(session, position)?;
    Ok(Captured {
        uuid,
        epoch: identity.epoch,
        design,
    })
}

fn save(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    position: Option<usize>,
    revision: Option<u64>,
    opened_for: (&str, u64),
) {
    let model = ui.global::<VariantsModel>();
    let name = match rows::clean_name(model.get_form_name().as_str()) {
        Ok(name) => name,
        Err(reason) => return model.set_form_error(reason.into()),
    };
    let note = match rows::clean_note(model.get_form_note().as_str()) {
        Ok(note) => note,
        Err(reason) => return model.set_form_error(reason.into()),
    };
    let Some(captured) =
        deps.with_state(|state| capture_for_save(state, position, revision, opened_for))
    else {
        return model.set_form_error(EDITOR_BUSY_TEXT.into());
    };
    let Captured {
        uuid,
        epoch,
        design,
    } = match captured {
        Ok(captured) => captured,
        Err(reason) => return model.set_form_error(reason.into()),
    };
    let parent = deps.page.borrow().branch.get(&uuid, epoch);
    set_busy(ui, deps, true);
    model.set_form_error(SharedString::new());
    let db = Arc::clone(&deps.db);
    let (job_uuid, job_name, now) = (uuid.clone(), name.clone(), unix_now());
    let started = spawn_job(
        ui,
        "variant-save",
        move || {
            let text = capture::design_text(&design)?;
            let picture = capture::picture_png(&design);
            with_db(&db, |db| {
                store::save(
                    db,
                    &store::SaveRequest {
                        uuid: &job_uuid,
                        name: &job_name,
                        note: note.as_deref(),
                        design_text: &text,
                        picture_png: picture.as_deref(),
                        parent,
                        now,
                    },
                )
            })
        },
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish_save(ui, &deps, (&uuid, epoch), &name, answer);
            }
        },
    );
    if let Err(reason) = started {
        form_failed(ui, deps, &reason);
    }
}

/// [`save`]'s continuation. `design` is the design's UUID and epoch at the click.
fn finish_save(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    design: (&str, u64),
    name: &str,
    answer: Result<i64, String>,
) {
    match answer {
        Ok(id) => {
            set_busy(ui, deps, false);
            deps.page.borrow_mut().branch.set(design.0, design.1, id);
            close_form(ui, deps);
            show_toast(ui, &format!("Saved \"{name}\" as a variant."), "success");
            refresh(ui, deps);
            raise(ui, VARIANT_SAVED);
        }
        Err(reason) => form_failed(ui, deps, &reason),
    }
}

fn rename(ui: &MainWindow, deps: &Rc<Deps>, id: i64) {
    let model = ui.global::<VariantsModel>();
    let name = match rows::clean_name(model.get_form_name().as_str()) {
        Ok(name) => name,
        Err(reason) => return model.set_form_error(reason.into()),
    };
    set_busy(ui, deps, true);
    let db = Arc::clone(&deps.db);
    let started = spawn_job(
        ui,
        "variant-rename",
        move || with_db(&db, |db| store::rename(db, id, &name)),
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish_edit(ui, &deps, "Renamed the variant.", answer);
            }
        },
    );
    if let Err(reason) = started {
        form_failed(ui, deps, &reason);
    }
}

fn change_note(ui: &MainWindow, deps: &Rc<Deps>, id: i64) {
    let model = ui.global::<VariantsModel>();
    let note = match rows::clean_note(model.get_form_note().as_str()) {
        Ok(note) => note,
        Err(reason) => return model.set_form_error(reason.into()),
    };
    set_busy(ui, deps, true);
    let db = Arc::clone(&deps.db);
    let message = if note.is_some() {
        "Saved the note."
    } else {
        "Cleared the note."
    };
    let started = spawn_job(
        ui,
        "variant-note",
        move || with_db(&db, |db| store::set_note(db, id, note.as_deref())),
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish_edit(ui, &deps, message, answer);
            }
        },
    );
    if let Err(reason) = started {
        form_failed(ui, deps, &reason);
    }
}

fn remove(ui: &MainWindow, deps: &Rc<Deps>, id: i64) {
    set_busy(ui, deps, true);
    let db = Arc::clone(&deps.db);
    let started = spawn_job(
        ui,
        "variant-delete",
        move || with_db(&db, |db| store::delete(db, id)),
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                deps.page.borrow_mut().branch.forget(id);
                finish_edit(ui, &deps, "Deleted the variant.", answer);
            }
        },
    );
    if let Err(reason) = started {
        form_failed(ui, deps, &reason);
    }
}

/// The continuation of a rename, a note change or a delete: closes the form and reads the
/// list again, or says what went wrong.
fn finish_edit(ui: &MainWindow, deps: &Rc<Deps>, message: &str, answer: Result<(), String>) {
    match answer {
        Ok(()) => {
            set_busy(ui, deps, false);
            close_form(ui, deps);
            show_toast(ui, message, "success");
        }
        Err(reason) => form_failed(ui, deps, &reason),
    }
    refresh(ui, deps);
}

// --- opening a variant -----------------------------------------------------------------------

/// How putting a variant in place ended.
enum Placed {
    /// The open design is the variant now.
    Done,
    /// It already was the same.
    AlreadyThere,
    /// It could not be done, with a sentence for the cutter.
    Refused(String),
}

/// The refusal for an edit the session turned down.
///
/// A variant brings its own relations with it, so a tier that follows a relation in the open
/// design is no obstacle: the variant's relations replace the open design's. What is left to
/// refuse is a variant whose relations cannot hold (a loop, an angle out of range).
pub(super) fn refusal_text(error: &SessionEditError) -> String {
    format!("The design cannot take this variant. {error}")
}

/// Makes the open design the variant's, as one undo step worded `Open variant "<name>"`, if
/// it is still the design the click was made on.
///
/// The selected tier row follows its tier through the change, as it does through an Undo: a
/// variant that replaces the tier list leaves no row to follow, one that changes only the
/// material or the rough keeps the selection where it was.
fn put_in_place(
    ui: &MainWindow,
    deps: &Deps,
    design: (&str, u64),
    variant: &Design,
    name: &str,
) -> Placed {
    let Ok(mut state) = deps.state.try_borrow_mut() else {
        return Placed::Refused(EDITOR_BUSY_TEXT.to_owned());
    };
    if !identity_of(&state).is_design(design.0, design.1) {
        return Placed::Refused(
            "The design changed while the variant was loading. Open the variant again.".to_owned(),
        );
    }
    match apply::replacement_edit(&state.design, variant) {
        Ok(None) => Placed::AlreadyThere,
        Ok(Some(edit)) => {
            let label = apply::open_label(name);
            match state.session.try_apply_mapped(edit, Some(&label)) {
                Ok((change, renumbering)) => {
                    refresh_after_renumbering_move(
                        ui,
                        &deps.render_ctx,
                        &deps.preview_state,
                        &deps.solid_last_solved,
                        &state,
                        &HistoryMove::new(change, renumbering),
                    );
                    Placed::Done
                }
                Err(error) => Placed::Refused(refusal_text(&error)),
            }
        }
        Err(reason) => Placed::Refused(reason),
    }
}

/// "Open" on a row: replaces the open design with the variant.
pub(super) fn open_variant(ui: &MainWindow, deps: &Rc<Deps>, key: &str) {
    if is_busy(ui, deps) || !ui.global::<GuideModel>().invoke_allows_history() {
        return;
    }
    let Some(variant) = find_variant(ui, deps, key) else {
        return;
    };
    let Some(identity) = deps.with_state(identity_of) else {
        show_toast(ui, EDITOR_BUSY_TEXT, "info");
        return;
    };
    let Some(uuid) = identity.uuid else {
        show_toast(ui, NO_DESIGN_TEXT, "info");
        return;
    };
    set_busy(ui, deps, true);
    let db = Arc::clone(&deps.db);
    let (job_uuid, id, epoch) = (uuid.clone(), variant.variant_id, identity.epoch);
    let started = spawn_job(
        ui,
        "variant-open",
        move || {
            with_db(&db, |db| store::load_design(db, &job_uuid, id))
                .map_err(|problem| problem.to_string())
        },
        move |ui, answer| {
            if let Some(deps) = live_deps() {
                finish_open(ui, &deps, (&uuid, epoch), answer);
            }
        },
    );
    if let Err(reason) = started {
        set_busy(ui, deps, false);
        show_toast(ui, &reason, "error");
    }
}

/// [`open_variant`]'s continuation. `design` is the design's UUID and epoch at the click.
fn finish_open(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    design: (&str, u64),
    answer: Result<(VariantSummary, Design), String>,
) {
    set_busy(ui, deps, false);
    let (summary, variant) = match answer {
        Ok(loaded) => loaded,
        Err(reason) => {
            show_toast(ui, &reason, "error");
            refresh(ui, deps);
            return;
        }
    };
    let name = &summary.name;
    match put_in_place(ui, deps, design, &variant, name) {
        Placed::Done => {
            deps.page
                .borrow_mut()
                .branch
                .set(design.0, design.1, summary.variant_id);
            show_toast(
                ui,
                &format!("Opened \"{name}\". Undo takes it back in one step."),
                "success",
            );
        }
        Placed::AlreadyThere => {
            deps.page
                .borrow_mut()
                .branch
                .set(design.0, design.1, summary.variant_id);
            show_toast(
                ui,
                &format!("The design is already the same as \"{name}\"."),
                "info",
            );
        }
        Placed::Refused(reason) => show_toast(ui, &reason, "warning"),
    }
    publish(ui, deps);
}
