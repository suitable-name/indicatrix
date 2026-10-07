//! "Edit as Text" (Edit > Edit as Text..., and the command palette): the open design's
//! cutting instructions as `.asc` text in a box you can edit, a live check of the text, an
//! Apply that turns the text into the design as one undoable change, Revert, and a line
//! comparison with the saved file or the snapshot.
//!
//! The text, the merge back into the design and the line diff are in
//! `indicatrix_editor::raw_text` (shared and tested there); [`logic`] decides what the
//! dialog says about an edited text; this module is the glue to the dialog
//! (`ui/components/raw_text_dialog.slint`, state in `ui/models/raw_text.slint`).
//!
//! # How the pieces fit
//!
//! - **Opening** clones the design, notes the edit generation and design epoch it was
//!   cloned at and the custom materials, gets its masts from the cached or a background
//!   solve (`native_io::resolve_solved_then`, never on the UI thread's own stack), and
//!   writes the text exactly as Export .asc does. A design that does not solve is told so
//!   in the dialog instead of showing text with made-up depths.
//! - **Checking** runs a moment after the last keystroke (a single-shot timer, restarted by
//!   every edit): `plan_apply` of the text against the design, shown as one sentence with
//!   the line of a problem, and the lists of what Apply would change and lose.
//! - **Applying** checks again with the latest text (the debounce may not have fired), and
//!   refuses when the design moved since the text was written. The change goes through the
//!   session as one `Edit::ReplaceSchedule`, so one Undo takes it back; the refresh is the
//!   one Undo and Redo end in, and the selection is cleared because a whole replacement
//!   leaves the old tier numbers meaningless.
//! - **Comparing** reads the other text off the UI thread where it needs a file, solves it
//!   like the design's own, and diffs it against the text in the box. Answers that arrive
//!   after the dialog closed, the text was written again or another comparison was asked
//!   for are dropped (the tokens in [`Dialog`]).
//!
//! No `EditorState` borrow is held across a call into Slint other than the refresh Undo
//! itself makes while holding one.

mod logic;
#[cfg(test)]
mod tests;

use super::{
    callbacks::{refresh_after_history_move, snapshot_for_compare},
    native_io,
    state::EditorState,
};
use crate::{
    EditorModel, GuideModel, MainWindow, RawTextDiffRow, RawTextModel,
    bridge::render_thread::RenderContext,
    gui::{
        show_toast,
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
        tutorial_events::raise,
    },
};
use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::{Design, native::design_from_str};
use indicatrix_editor::{
    guide::solving_events::RAW_TEXT_DIFF_SHOWN,
    raw_text::{apply_plan, diff_lines, generate_text, normalize_text, omitted_line},
};
use logic::{
    Outcome, PanelView, RowView, compare_failure_view, compare_title, compare_view,
    design_moved_problem, evaluate, panel_view, same_lines,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    path::Path,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError, atomic::Ordering},
    time::Duration,
};

/// How long after the last keystroke the text is checked.
const CHECK_DELAY: Duration = Duration::from_millis(250);

/// `RawTextModel.compare`'s argument for the saved file on disk (the other is the snapshot).
const SOURCE_SAVED_FILE: i32 = 0;

/// What the open dialog knows between callbacks. UI thread only.
#[derive(Default)]
struct Dialog {
    /// Whether the dialog is showing.
    open: bool,
    /// Whether [`Self::base_text`] has been written, so the text can be checked.
    ready: bool,
    /// Bumped whenever the text is written again and when the dialog closes: the answer of
    /// a solve that was asked for before that is dropped.
    token: u64,
    /// The same for comparisons.
    compare_token: u64,
    /// The design's edit generation when the text was written.
    generation: u64,
    /// The design epoch when the text was written.
    epoch: u64,
    /// The custom materials the text was written with.
    custom: Vec<GemMaterial>,
    /// The text as the design wrote it.
    base_text: String,
    /// The text as it is in the box.
    edited_text: String,
}

/// What the callbacks share.
struct Deps {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
    dialog: RefCell<Dialog>,
}

thread_local! {
    /// The shared state, for the continuations that come back from a background thread
    /// (`Rc` cannot cross to it). UI-thread-only.
    static LIVE_DEPS: RefCell<Option<Rc<Deps>>> = const { RefCell::new(None) };

    /// The timer behind [`CHECK_DELAY`]. UI-thread-only.
    static CHECK_TIMER: slint::Timer = slint::Timer::default();
}

/// The shared state, once [`setup_raw_text_dialog`] has run.
fn live_deps() -> Option<Rc<Deps>> {
    LIVE_DEPS.with(|cell| cell.borrow().clone())
}

/// The texts of a check or an apply, and what they were written from.
struct CheckInput {
    base: String,
    edited: String,
    generation: u64,
    epoch: u64,
    custom: Vec<GemMaterial>,
}

impl Deps {
    /// The texts as they are now; `None` while the dialog is closed or still writing.
    fn check_input(&self) -> Option<CheckInput> {
        let dialog = self.dialog.borrow();
        (dialog.open && dialog.ready).then(|| CheckInput {
            base: dialog.base_text.clone(),
            edited: dialog.edited_text.clone(),
            generation: dialog.generation,
            epoch: dialog.epoch,
            custom: dialog.custom.clone(),
        })
    }

    /// Runs `read` on the editor state; `None` when it is in use (a callback is in the middle
    /// of changing it).
    fn with_state<R>(&self, read: impl FnOnce(&EditorState) -> R) -> Option<R> {
        self.state.try_borrow().ok().map(|state| read(&state))
    }
}

/// What the text amounts to for the design as it is now.
fn outcome_for(state: &EditorState, input: &CheckInput) -> Outcome {
    let moved = state.current_generation() != input.generation
        || state.design_epoch.load(Ordering::Relaxed) != input.epoch;
    if moved {
        return Outcome::Problem(design_moved_problem());
    }
    evaluate(&state.design, &input.base, &input.edited, &input.custom)
}

/// The custom material catalogue the render thread holds.
fn custom_materials(render_ctx: &Arc<Mutex<RenderContext>>) -> Vec<GemMaterial> {
    render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone()
}

/// `lines` as a Slint list of strings.
fn lines_model(lines: &[String]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        lines
            .iter()
            .map(|line| SharedString::from(line.as_str()))
            .collect::<Vec<_>>(),
    ))
}

/// `rows` as the comparison list.
fn rows_model(rows: &[RowView]) -> ModelRc<RawTextDiffRow> {
    ModelRc::new(VecModel::from(
        rows.iter()
            .map(|row| RawTextDiffRow {
                kind: row.kind,
                old_line: row.old_line.as_str().into(),
                new_line: row.new_line.as_str().into(),
                text: row.text.as_str().into(),
            })
            .collect::<Vec<_>>(),
    ))
}

/// Shows the live check.
fn publish_panel(ui: &MainWindow, view: &PanelView) {
    let model = ui.global::<RawTextModel>();
    model.set_status(view.status.as_str().into());
    model.set_status_kind(view.kind);
    model.set_changed_lines(lines_model(&view.changed));
    model.set_lost_lines(lines_model(&view.lost));
    model.set_can_apply(view.can_apply);
}

/// Puts every `RawTextModel` property back to "closed and empty".
fn reset_model(ui: &MainWindow) {
    let model = ui.global::<RawTextModel>();
    model.set_is_open(false);
    model.set_text(SharedString::new());
    model.set_busy(false);
    model.set_load_error(SharedString::new());
    model.set_omitted(SharedString::new());
    model.set_text_differs(false);
    model.set_confirm_discard(false);
    model.set_compare_open(false);
    model.set_compare_busy(false);
    model.set_compare_title(SharedString::new());
    model.set_compare_summary(SharedString::new());
    model.set_diff_rows(ModelRc::default());
    publish_panel(ui, &PanelView::empty());
}

// --- opening, writing, reverting, closing ---------------------------------------------------

/// Opens the dialog and starts writing the text.
fn open(ui: &MainWindow, deps: &Rc<Deps>) {
    if deps.dialog.borrow().open || !ui.global::<GuideModel>().invoke_allows_advanced() {
        return;
    }
    let Some(has_design) = deps.with_state(|state| state.has_design) else {
        return;
    };
    if !has_design {
        show_toast(ui, "Open or create a design first.", "info");
        return;
    }
    reset_model(ui);
    deps.dialog.borrow_mut().open = true;
    ui.global::<RawTextModel>().set_is_open(true);
    write_text(ui, deps);
}

/// Writes the text from the design as it is now (opening, and Revert).
fn write_text(ui: &MainWindow, deps: &Rc<Deps>) {
    CHECK_TIMER.with(slint::Timer::stop);
    let model = ui.global::<RawTextModel>();
    let Some((design, generation, epoch)) = deps.with_state(|state| {
        (
            Arc::new(state.design.clone()),
            state.current_generation(),
            state.design_epoch.load(Ordering::Relaxed),
        )
    }) else {
        model.set_load_error(
            "The editor is busy. Close this window and open it again in a moment.".into(),
        );
        return;
    };
    let custom = custom_materials(&deps.render_ctx);
    let token = {
        let mut dialog = deps.dialog.borrow_mut();
        dialog.token = dialog.token.wrapping_add(1);
        dialog.compare_token = dialog.compare_token.wrapping_add(1);
        dialog.ready = false;
        dialog.generation = generation;
        dialog.epoch = epoch;
        dialog.custom = custom;
        dialog.base_text.clear();
        dialog.edited_text.clear();
        dialog.token
    };
    model.set_busy(true);
    model.set_load_error(SharedString::new());
    model.set_confirm_discard(false);
    model.set_compare_open(false);
    model.set_text_differs(false);
    publish_panel(ui, &PanelView::empty());
    let deps = Rc::clone(deps);
    native_io::resolve_solved_then(ui, design, move |ui, design, result| {
        finish_write(ui, &deps, token, &design, result);
    });
}

/// [`write_text`]'s continuation: the masts are in (or the design does not solve).
fn finish_write(
    ui: &MainWindow,
    deps: &Deps,
    token: u64,
    design: &Design,
    result: Result<Vec<SolvedTier>, String>,
) {
    let custom = {
        let dialog = deps.dialog.borrow();
        if !dialog.open || dialog.token != token {
            return;
        }
        dialog.custom.clone()
    };
    let model = ui.global::<RawTextModel>();
    model.set_busy(false);
    match result.and_then(|solved| generate_text(design, &solved, &custom)) {
        Ok(text) => {
            {
                let mut dialog = deps.dialog.borrow_mut();
                dialog.base_text.clone_from(&text);
                dialog.edited_text.clone_from(&text);
                dialog.ready = true;
            }
            model.set_omitted(omitted_line(design).into());
            publish_panel(ui, &panel_view(&Outcome::Unchanged));
            model.set_text(text.into());
        }
        Err(reason) => {
            model.set_text(SharedString::new());
            model.set_load_error(reason.into());
        }
    }
}

/// Closes the dialog; edits that were not applied are dropped.
fn close(ui: &MainWindow, deps: &Deps) {
    CHECK_TIMER.with(slint::Timer::stop);
    {
        let mut dialog = deps.dialog.borrow_mut();
        dialog.token = dialog.token.wrapping_add(1);
        dialog.compare_token = dialog.compare_token.wrapping_add(1);
        *dialog = Dialog {
            token: dialog.token,
            compare_token: dialog.compare_token,
            ..Dialog::default()
        };
    }
    reset_model(ui);
}

// --- checking ----------------------------------------------------------------------------

/// The text in the box was edited: remembers it and checks it after a short pause.
fn text_edited(ui: &MainWindow, deps: &Rc<Deps>, text: &str) {
    let differs = {
        let mut dialog = deps.dialog.borrow_mut();
        if !dialog.open || !dialog.ready {
            return;
        }
        dialog.edited_text.clear();
        dialog.edited_text.push_str(text);
        !same_lines(&dialog.base_text, &dialog.edited_text)
    };
    let model = ui.global::<RawTextModel>();
    model.set_text_differs(differs);
    // The last check described the text before this keystroke.
    model.set_can_apply(false);
    schedule_check(ui, deps);
}

/// (Re)starts the pause before the text is checked.
fn schedule_check(ui: &MainWindow, deps: &Rc<Deps>) {
    let weak = ui.as_weak();
    let deps = Rc::clone(deps);
    CHECK_TIMER.with(|timer| {
        timer.start(slint::TimerMode::SingleShot, CHECK_DELAY, move || {
            if let Some(ui) = weak.upgrade() {
                check_now(&ui, &deps);
            }
        });
    });
}

/// Checks the text in the box and shows the answer.
fn check_now(ui: &MainWindow, deps: &Rc<Deps>) {
    let Some(input) = deps.check_input() else {
        return;
    };
    let Some(outcome) = deps.with_state(|state| outcome_for(state, &input)) else {
        // The editor was in use: try again in a moment.
        schedule_check(ui, deps);
        return;
    };
    publish_panel(ui, &panel_view(&outcome));
}

// --- applying ----------------------------------------------------------------------------

/// What an Apply click came to.
enum Applied {
    /// The design now holds the text.
    Done,
    /// The text changes nothing.
    Nothing,
    /// The text has a problem.
    Problem(Outcome),
    /// The session refused the change, with its reason.
    Refused(String),
}

/// No tier is selected: after a whole replacement the old tier numbers mean nothing.
fn clear_selection(ui: &MainWindow) {
    let editor = ui.global::<EditorModel>();
    editor.set_selected_tier_index(-1);
    editor.set_form_reset_pulse(editor.get_form_reset_pulse().wrapping_add(1));
}

/// Turns the text in the box into the design, as one undoable change.
fn apply(ui: &MainWindow, deps: &Deps) {
    let Some(input) = deps.check_input() else {
        return;
    };
    CHECK_TIMER.with(slint::Timer::stop);
    let applied = {
        let Ok(mut state) = deps.state.try_borrow_mut() else {
            show_toast(
                ui,
                "The editor is busy. Try Apply again in a moment.",
                "info",
            );
            return;
        };
        match outcome_for(&state, &input) {
            Outcome::Plan(plan) if !plan.is_noop() => match apply_plan(&mut state.session, *plan) {
                Ok(_) => {
                    refresh_after_history_move(
                        ui,
                        &deps.render_ctx,
                        &deps.preview_state,
                        &deps.solid_last_solved,
                        &state,
                    );
                    Applied::Done
                }
                Err(error) => Applied::Refused(error.to_string()),
            },
            Outcome::Plan(_) | Outcome::Unchanged => Applied::Nothing,
            problem @ Outcome::Problem(_) => Applied::Problem(problem),
        }
    };
    match applied {
        Applied::Done => {
            clear_selection(ui);
            close(ui, deps);
            show_toast(
                ui,
                "Applied the text. Undo takes it back in one step.",
                "success",
            );
        }
        Applied::Nothing => {
            publish_panel(ui, &panel_view(&Outcome::Unchanged));
            show_toast(
                ui,
                "Nothing to apply. The text changes nothing in the design.",
                "info",
            );
        }
        Applied::Problem(outcome) => publish_panel(ui, &panel_view(&outcome)),
        Applied::Refused(reason) => publish_panel(
            ui,
            &PanelView::problem(format!("The design cannot take this text. {reason}")),
        ),
    }
}

// --- comparing ---------------------------------------------------------------------------

/// Whether the comparison asked for as `token` is still the one the dialog wants.
fn compare_current(deps: &Deps, token: u64) -> bool {
    let dialog = deps.dialog.borrow();
    dialog.open && dialog.compare_token == token
}

/// Swaps the box for a comparison of the text in it with the saved file (`source` 0) or the
/// snapshot.
fn compare(ui: &MainWindow, deps: &Rc<Deps>, source: i32) {
    let token = {
        let mut dialog = deps.dialog.borrow_mut();
        if !dialog.open || !dialog.ready {
            return;
        }
        dialog.compare_token = dialog.compare_token.wrapping_add(1);
        dialog.compare_token
    };
    let model = ui.global::<RawTextModel>();
    model.set_compare_open(true);
    model.set_compare_busy(true);
    model.set_compare_title(SharedString::new());
    model.set_compare_summary(SharedString::new());
    model.set_diff_rows(ModelRc::default());
    if source == SOURCE_SAVED_FILE {
        compare_with_saved_file(ui, deps, token);
    } else {
        compare_with_snapshot(ui, deps, token);
    }
}

/// Gives up on a comparison before it started, with a toast saying why.
fn abandon_compare(ui: &MainWindow, message: &str) {
    let model = ui.global::<RawTextModel>();
    model.set_compare_open(false);
    model.set_compare_busy(false);
    show_toast(ui, message, "info");
}

/// The design inside the `.indicatrix` file at `path`.
fn read_design_file(path: &Path) -> Result<Design, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("The file cannot be read. {error}"))?;
    design_from_str(&text)
        .map(|loaded| loaded.design)
        .map_err(|error| format!("The file cannot be opened as a design. {error}"))
}

/// The saved `.indicatrix` file, or for a design that came from a bare `.asc` the text of
/// that file.
fn compare_with_saved_file(ui: &MainWindow, deps: &Deps, token: u64) {
    let model = ui.global::<RawTextModel>();
    if let Some(path) = native_io::current_design_file() {
        let name = path
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
        let label = format!("the saved file {name}");
        model.set_compare_title(compare_title(&label).into());
        let weak = ui.as_weak();
        std::thread::spawn(move || {
            let loaded = read_design_file(&path);
            let _ = weak.upgrade_in_event_loop(move |ui| {
                if let Some(deps) = live_deps() {
                    continue_compare(&ui, &deps, token, &label, loaded);
                }
            });
        });
        return;
    }
    let original = deps
        .with_state(|state| state.original_asc_text.clone())
        .flatten();
    let Some(original) = original else {
        abandon_compare(
            ui,
            "This design has not been saved yet, so there is no saved file to compare with.",
        );
        return;
    };
    let label = "the original .asc file";
    model.set_compare_title(compare_title(label).into());
    let text = normalize_text(&original).map_err(|problem| problem.to_string());
    finish_compare(ui, deps, token, label, text);
}

/// The snapshot taken with Snapshot Design.
fn compare_with_snapshot(ui: &MainWindow, deps: &Rc<Deps>, token: u64) {
    let Some((design, name)) = snapshot_for_compare() else {
        abandon_compare(ui, "There is no snapshot yet.");
        return;
    };
    let label = format!("the snapshot of \"{name}\"");
    ui.global::<RawTextModel>()
        .set_compare_title(compare_title(&label).into());
    continue_compare(ui, deps, token, &label, Ok(design));
}

/// Solves the other design (or reports why there is none) and writes its text.
fn continue_compare(
    ui: &MainWindow,
    deps: &Rc<Deps>,
    token: u64,
    label: &str,
    other: Result<Design, String>,
) {
    if !compare_current(deps, token) {
        return;
    }
    match other {
        Ok(design) => {
            let deps = Rc::clone(deps);
            let label = label.to_owned();
            native_io::resolve_solved_then(ui, Arc::new(design), move |ui, design, result| {
                let custom = deps.dialog.borrow().custom.clone();
                let text = result.and_then(|solved| generate_text(&design, &solved, &custom));
                finish_compare(ui, &deps, token, &label, text);
            });
        }
        Err(reason) => finish_compare(ui, deps, token, label, Err(reason)),
    }
}

/// Shows the comparison of `other` (the text of what the box is compared with, or why there
/// is none) with the text in the box.
fn finish_compare(
    ui: &MainWindow,
    deps: &Deps,
    token: u64,
    label: &str,
    other: Result<String, String>,
) {
    if !compare_current(deps, token) {
        return;
    }
    let compared = other.is_ok();
    let view = match other {
        Ok(other) => {
            let edited = deps.dialog.borrow().edited_text.clone();
            compare_view(label, &diff_lines(&other, &edited))
        }
        Err(reason) => compare_failure_view(label, &reason),
    };
    let model = ui.global::<RawTextModel>();
    model.set_compare_busy(false);
    model.set_compare_title(view.title.into());
    model.set_compare_summary(view.summary.into());
    model.set_diff_rows(rows_model(&view.rows));
    if compared {
        raise(ui, RAW_TEXT_DIFF_SHOWN);
    }
}

// --- wiring ------------------------------------------------------------------------------

/// Wires the Edit as Text dialog. Called once from `setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_raw_text_dialog(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let deps = Rc::new(Deps {
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
        solid_last_solved: Arc::clone(solid_last_solved),
        dialog: RefCell::new(Dialog::default()),
    });
    LIVE_DEPS.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&deps)));
    let model = ui.global::<RawTextModel>();

    let (weak, shared) = (ui.as_weak(), Rc::clone(&deps));
    model.on_open(move || {
        if let Some(ui) = weak.upgrade() {
            open(&ui, &shared);
        }
    });

    let (weak, shared) = (ui.as_weak(), Rc::clone(&deps));
    model.on_close(move || {
        if let Some(ui) = weak.upgrade() {
            close(&ui, &shared);
        }
    });

    let (weak, shared) = (ui.as_weak(), Rc::clone(&deps));
    model.on_text_edited(move |text| {
        if let Some(ui) = weak.upgrade() {
            text_edited(&ui, &shared, text.as_str());
        }
    });

    let (weak, shared) = (ui.as_weak(), Rc::clone(&deps));
    model.on_apply(move || {
        if let Some(ui) = weak.upgrade() {
            apply(&ui, &shared);
        }
    });

    let (weak, shared) = (ui.as_weak(), Rc::clone(&deps));
    model.on_revert(move || {
        if let Some(ui) = weak.upgrade() {
            write_text(&ui, &shared);
        }
    });

    let (weak, shared) = (ui.as_weak(), deps);
    model.on_compare(move |source| {
        if let Some(ui) = weak.upgrade() {
            compare(&ui, &shared, source);
        }
    });
}
