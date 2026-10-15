//! The Stone tab's design-side wiring: "Back to design" after a material preview, the one
//! stone width (the design's girdle diameter, one undoable edit), the per-design render effects
//! (debounced commit, reset, one-shot migration of the old app-level values), and the
//! printed-proportions panel.

use std::{
    cell::{Cell, RefCell},
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

use indicatrix_cut_core::{Edit, StoneRenderEffects};
use slint::{ComponentHandle, SharedString, Timer, TimerMode};

use crate::{
    EditorModel, MainWindow, ParametersModel, SettingsModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::EditorState,
            view::{
                SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan,
                sync_viewport_material_link,
            },
        },
        render::{render_body_color, stone_effects},
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
    settings::SettingsPersister,
};

/// How long a Stone-tab control must rest before its value is written to the design.
const EFFECTS_COMMIT_DELAY: Duration = Duration::from_millis(350);
/// How long to wait before trying again when the editor state is busy.
const EFFECTS_RETRY_DELAY: Duration = Duration::from_millis(200);
/// Shown when an action finds the editor state borrowed by another running action.
const BUSY_MESSAGE: &str = "The design is busy right now. Try again in a moment.";

/// Everything the effects commit needs, shared by its timer callbacks.
struct EffectsCommit {
    ui_weak: slint::Weak<MainWindow>,
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    /// `EditorState::design_epoch` when the first uncommitted control change was made: a commit
    /// whose design was replaced since then must not be written into the new design.
    baseline_epoch: Cell<Option<u64>>,
}

/// Whether a pending commit still belongs to the design on screen. `captured` is the design
/// epoch when the change was made (`None` = unknown, accept); `current` is the epoch now.
fn commit_still_applies(captured: Option<u64>, current: u64) -> bool {
    captured.is_none_or(|epoch| epoch == current)
}

/// Writes the Stone tab's effect controls into the design as ONE undoable edit (nothing when
/// the design already has them) and refreshes the panel. Retries while the editor state is
/// busy; the pending flag stays set meanwhile, so the controls keep their value. A commit
/// whose design was replaced meanwhile is dropped and the new design's own values are shown.
fn commit_effects(commit: Rc<EffectsCommit>) {
    let Some(ui) = commit.ui_weak.upgrade() else {
        commit.baseline_epoch.set(None);
        stone_effects::clear_live_edit_pending();
        return;
    };
    // A library design took the viewport while the change waited: the controls now show that
    // row's own view-only values and must not be written into the open design.
    if !ui.global::<ViewportModel>().get_shows_editor_design() {
        commit.baseline_epoch.set(None);
        stone_effects::clear_live_edit_pending();
        return;
    }
    let Ok(mut st) = commit.state.try_borrow_mut() else {
        Timer::single_shot(EFFECTS_RETRY_DELAY, move || commit_effects(commit));
        return;
    };
    let captured = commit.baseline_epoch.take();
    if !commit_still_applies(captured, st.design_epoch.load(Ordering::Relaxed)) {
        // Another design was opened since the control moved: its own effects are shown.
        stone_effects::clear_live_edit_pending();
        refresh_editor_panel_stale(&ui, &commit.render_ctx, &st, &BTreeSet::new());
        return;
    }
    let effects = stone_effects::effects_from_controls(&ui);
    let edit = stone_effects::effects_edit(&st.design, effects);
    // Cleared before the refresh so it may push the design's (now equal) values.
    stone_effects::clear_live_edit_pending();
    if let Some(edit) = edit {
        if let Err(error) = st.apply(edit) {
            show_toast(&ui, &error.to_string(), "error");
        }
        // Effects never move a tier's own mast.
        refresh_editor_panel_stale(&ui, &commit.render_ctx, &st, &BTreeSet::new());
    }
}

/// Sets the design's girdle diameter from the Stone tab's text (arithmetic allowed, blank
/// clears): one undoable edit, nothing when it already is that value.
fn apply_stone_width(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    text: &str,
) {
    let Ok(mut st) = state.try_borrow_mut() else {
        show_toast(ui, BUSY_MESSAGE, "error");
        return;
    };
    let girdle_diameter_mm =
        match indicatrix_editor::material::parse_yield_form(text, 0, "", &st.design.material) {
            Ok((mm, _)) => mm,
            Err(message) => {
                show_toast(ui, &message, "error");
                return;
            }
        };
    if girdle_diameter_mm.map(f64::to_bits) == st.design.girdle_diameter_mm.map(f64::to_bits) {
        return;
    }
    if let Err(error) = st.apply(Edit::SetGirdleDiameterMm { girdle_diameter_mm }) {
        show_toast(ui, &error.to_string(), "error");
        return;
    }
    // A girdle diameter never moves a tier's own mast.
    refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::new());
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        BTreeSet::new(),
        false,
    );
}

/// The Stone tab's callbacks that need the open design: "Back to design", the stone width, the
/// effects commit and reset, and the one-shot migration. (The printed-proportions panel is
/// [`setup_printed_proportions_callback`].)
pub(in crate::gui::editor) fn setup_stone_tab_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    setup_back_to_design_callback(ui, state, render_ctx);
    setup_stone_width_callbacks(ui, state, render_ctx, preview_state, solid_last_solved);
    let commit = Rc::new(EffectsCommit {
        ui_weak: ui.as_weak(),
        state: Rc::clone(state),
        render_ctx: Arc::clone(render_ctx),
        baseline_epoch: Cell::new(None),
    });
    setup_effects_commit_callback(ui, &commit);
    setup_effects_reset_callback(ui, &commit);
    setup_stone_migration_callback(ui, commit);
}

/// "Back to design": the view follows the design's material again and any preview colour is
/// forgotten. The material sync runs at once so the picture does not wait for an edit.
fn setup_back_to_design_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state_back = Rc::clone(state);
    let render_ctx_back = Arc::clone(render_ctx);
    let ui_weak_back = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_preview_back_to_design(move || {
            let Some(ui) = ui_weak_back.upgrade() else {
                return;
            };
            ui.global::<ViewportModel>()
                .set_viewport_material_linked(true);
            render_body_color::clear_view_color(&render_ctx_back);
            if let Ok(st) = state_back.try_borrow() {
                let selected_material_index = {
                    let mut ctx = render_ctx_back
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    sync_viewport_material_link(&ui, &mut ctx, &st.design)
                };
                drop(st);
                // Set only after the `render_ctx` guard above is dropped -- see
                // `sync_viewport_material_link`'s own doc comment.
                if let Some(idx) = selected_material_index {
                    ui.global::<ViewportModel>()
                        .set_selected_material_index(idx);
                }
            }
            ui.global::<ViewportModel>().invoke_render_color_refresh();
            show_toast(&ui, "Back to the design's own material.", "info");
        });
}

/// The one stone width: the Stone tab's field and the Preform tab's Girdle Diameter are the
/// same design value; the planner's adopt action sets it through `stone_width_changed`.
fn setup_stone_width_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let (state_width, ctx_width, preview_width, solved_width) = (
        Rc::clone(state),
        Arc::clone(render_ctx),
        Arc::clone(preview_state),
        Arc::clone(solid_last_solved),
    );
    let ui_weak_width = ui.as_weak();
    ui.global::<EditorModel>()
        .on_apply_stone_width(move |text: SharedString| {
            if let Some(ui) = ui_weak_width.upgrade() {
                apply_stone_width(
                    &ui,
                    &state_width,
                    &ctx_width,
                    &preview_width,
                    &solved_width,
                    &text,
                );
            }
        });
    // `SettingsModel.stone_width_changed(mm)`: a positive width goes to the design; zero is
    // ignored, since clearing the width is the field's blank text.
    let (state_set, ctx_set, preview_set, solved_set) = (
        Rc::clone(state),
        Arc::clone(render_ctx),
        Arc::clone(preview_state),
        Arc::clone(solid_last_solved),
    );
    let ui_weak_set = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_stone_width_changed(move |width_mm: f32| {
            if width_mm <= 0.0 {
                return;
            }
            if let Some(ui) = ui_weak_set.upgrade() {
                apply_stone_width(
                    &ui,
                    &state_set,
                    &ctx_set,
                    &preview_set,
                    &solved_set,
                    &f64::from(width_mm).to_string(),
                );
            }
        });
}

/// Effects: a control changed the picture; write the design once it rests.
fn setup_effects_commit_callback(ui: &MainWindow, commit: &Rc<EffectsCommit>) {
    let timer = Timer::default();
    let commit_edited = Rc::clone(commit);
    ui.global::<SettingsModel>()
        .on_stone_effects_edited(move || {
            // The design the change belongs to is the one on screen at the FIRST uncommitted
            // change; later nudges of the same burst keep that baseline.
            if commit_edited.baseline_epoch.get().is_none() {
                let epoch = commit_edited
                    .state
                    .try_borrow()
                    .ok()
                    .map(|st| st.design_epoch.load(Ordering::Relaxed));
                commit_edited.baseline_epoch.set(epoch);
            }
            let commit = Rc::clone(&commit_edited);
            timer.start(TimerMode::SingleShot, EFFECTS_COMMIT_DELAY, move || {
                commit_effects(Rc::clone(&commit));
            });
        });
}

/// "Reset this tab" of the Stone tab (`ParametersModel.reset_stone_effects`): the default
/// effects as ONE undoable edit; material and geometry are never touched.
fn setup_effects_reset_callback(ui: &MainWindow, commit: &Rc<EffectsCommit>) {
    let reset = Rc::clone(commit);
    ui.global::<ParametersModel>()
        .on_reset_stone_effects(move || {
            let Some(ui) = reset.ui_weak.upgrade() else {
                return;
            };
            let defaults = StoneRenderEffects::default();
            // A library design on the viewport has no design to write: its view-only effects
            // simply go back to the defaults below.
            if ui.global::<ViewportModel>().get_shows_editor_design() {
                let Ok(mut st) = reset.state.try_borrow_mut() else {
                    show_toast(&ui, BUSY_MESSAGE, "error");
                    return;
                };
                stone_effects::clear_live_edit_pending();
                reset.baseline_epoch.set(None);
                if let Some(edit) = stone_effects::effects_edit(&st.design, defaults) {
                    if let Err(error) = st.apply(edit) {
                        show_toast(&ui, &error.to_string(), "error");
                        return;
                    }
                    refresh_editor_panel_stale(&ui, &reset.render_ctx, &st, &BTreeSet::new());
                }
            } else {
                stone_effects::clear_live_edit_pending();
                reset.baseline_epoch.set(None);
            }
            // Also undo a drag that never reached the design.
            {
                let mut ctx = RenderContext::lock(&reset.render_ctx);
                if !stone_effects::context_matches(&ctx, &defaults) {
                    stone_effects::apply_effects_to_context(&mut ctx, &defaults);
                }
            }
            stone_effects::push_effects_to_controls(&ui, &defaults);
            show_toast(&ui, "Stone effects reset.", "info");
        });
}

/// One-shot migration: the old app-level stone size and effects become the starting values
/// of the first design that is opened (an undoable edit), then are never read again.
fn setup_stone_migration_callback(ui: &MainWindow, migrate: Rc<EffectsCommit>) {
    ui.global::<EditorModel>().on_stone_design_opened(move || {
        let Some(persister) = SettingsPersister::installed_for_this_thread() else {
            return;
        };
        let settings = persister.snapshot().settings;
        if settings.migrated_stone_effects {
            return;
        }
        let Some(ui) = migrate.ui_weak.upgrade() else {
            return;
        };
        let Ok(mut st) = migrate.state.try_borrow_mut() else {
            return;
        };
        if !st.has_design {
            return;
        }
        if let Some(legacy) = settings.legacy_stone_defaults() {
            let edits = stone_effects::legacy_migration_edits(&st.design, &legacy);
            if !edits.is_empty() && st.apply(Edit::Batch(edits)).is_ok() {
                refresh_editor_panel_stale(&ui, &migrate.render_ctx, &st, &BTreeSet::new());
                show_toast(
                    &ui,
                    "Your earlier stone size and effects were carried onto this design.",
                    "info",
                );
            }
        }
        persister.update(|file| file.settings.mark_stone_effects_migrated());
    });
}

/// The printed-proportions panel. Wired to `EditorModel.apply_printed_proportions`
/// (`ui/models/editor.slint`), called from `editor_design_settings.slint`.
pub(in crate::gui::editor) fn setup_printed_proportions_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state_props = Rc::clone(state);
    let render_ctx_props = Arc::clone(render_ctx);
    let ui_weak_props = ui.as_weak();
    ui.global::<EditorModel>().on_apply_printed_proportions(
        move |vol_w3: SharedString,
              lw: SharedString,
              cw: SharedString,
              pw: SharedString,
              hw: SharedString| {
            let Some(ui) = ui_weak_props.upgrade() else {
                return;
            };
            let props = match parse_printed_proportions_form(&vol_w3, &lw, &cw, &pw, &hw) {
                Ok(props) => props,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let has_any = props.vol_w3.is_some()
                || props.lw.is_some()
                || props.cw.is_some()
                || props.pw.is_some()
                || props.hw.is_some();
            let mut st = state_props.borrow_mut();
            st.printed_proportions = has_any.then_some(props);
            // A printed-proportions edit never moves a tier's own mast.
            refresh_editor_panel_stale(&ui, &render_ctx_props, &st, &BTreeSet::new());
            drop(st);
            show_toast(
                &ui,
                if has_any {
                    "Printed proportions saved -- Deep Solve can now verify against them."
                } else {
                    "Printed proportions cleared."
                },
                "success",
            );
        },
    );
}

// The printed-proportions parsers moved to `indicatrix_editor::printed_proportions`
// (shared with the web design settings); re-exported at their old path, which
// `super::tests` exercises.
#[cfg(test)]
pub(super) use indicatrix_editor::printed_proportions::parse_printed_proportions_field;
pub(super) use indicatrix_editor::printed_proportions::parse_printed_proportions_form;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_commit_with_an_unknown_baseline_or_the_same_design_applies() {
        assert!(commit_still_applies(None, 7));
        assert!(commit_still_applies(Some(7), 7));
    }

    /// A slider nudge on one design, then another design opens before the debounce fires: the
    /// commit must be dropped instead of writing the old slider values into the new design.
    #[test]
    fn a_stale_commit_is_dropped_after_a_design_switch() {
        let mut state = EditorState::fresh();
        let captured = state.design_epoch.load(Ordering::Relaxed);
        assert!(commit_still_applies(
            Some(captured),
            state.design_epoch.load(Ordering::Relaxed)
        ));
        // An ordinary edit of the same design keeps the commit valid.
        state
            .apply(Edit::SetGirdleDiameterMm {
                girdle_diameter_mm: Some(6.0),
            })
            .expect("the width applies");
        assert!(commit_still_applies(
            Some(captured),
            state.design_epoch.load(Ordering::Relaxed)
        ));
        // Opening / loading / creating another design replaces the state wholesale.
        state.replace_wholesale(EditorState::fresh());
        assert!(!commit_still_applies(
            Some(captured),
            state.design_epoch.load(Ordering::Relaxed)
        ));
    }
}
