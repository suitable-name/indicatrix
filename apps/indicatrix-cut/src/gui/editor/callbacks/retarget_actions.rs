//! Wires `super::super::retarget` (the "Retarget for material" proposal
//! builder/applier) and `ui/components/retarget_dialog.slint` into the Edit tab --
//! four `setup_*` functions, one per `RetargetDialog`/`EditorView` callback, mirroring
//! `solve_actions.rs`'s Optimize callbacks in shape.
//!
//! # Where the target material comes from
//!
//! [`resolve_target_material`] resolves the proposal's TARGET from whatever the
//! design settings panel's material combo/RI-override fields currently show, NOT
//! from `state.design.material` directly: the design's current material is unchanged
//! by this dialog (`retarget::apply` only ever produces `Edit::RetargetAngles`), while
//! the combo can show a different, not-yet-applied selection. Opening Retarget without
//! changing the combo first resolves target == current material (a no-op Shift
//! proposal) -- expected, not a bug: the interesting case is picking a different
//! material first, then opening Retarget to preview the angle changes before
//! committing it via the design settings panel's own "Apply".
//!
//! # Synchronous, like `deep_solve`/`optimize_solve` were on their first pass
//!
//! `RetargetMode::Optimize` runs a real `optimize_design` search, but this first
//! wiring pass calls it synchronously on the UI thread inside
//! [`setup_retarget_proposal_changed_callback`] rather than spawning a worker thread --
//! `editor_retarget_busy` is still toggled around the call so a later pass can move
//! it off-thread without any Slint-side change.
//!
//! # Slint-free view-model split, for testability
//!
//! [`retarget_view`]/[`row_view`]/[`resolve_target_material`]/[`apply_pending_retarget`]
//! take and return only plain Rust types, so this group's test module can exercise the
//! actual decision logic directly -- `push_retarget_view`/`push_target_readout` are
//! the only two functions that touch a Slint type, thin enough not to need tests.

use super::super::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    retarget::{self, CrownShift, RetargetError, RetargetMode, RetargetProposal, RetargetRow},
    state::{EditorState, design_material_options, parse_design_material_form},
    view::{self, parse_optimize_weights},
};
use crate::{
    EditorModel, MainWindow, RetargetModel, RetargetRowItem,
    bridge::render_thread::RenderContext,
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix::{geometry::meet_solver::Block, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    Design, EditError, ObjectiveWeights, OptimizeConfig, ResolvedMaterial, Risk,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

/// Resolves the retarget target material from `design`'s current material (used for
/// `specific_gravity_override` passthrough and as the fallback) plus whatever the
/// design settings panel's material combo and RI-override text field currently show
/// -- see this module's doc comment ("Where the target material comes from"). An
/// unparseable `ri_override_text` falls back to `design.material` unchanged rather
/// than surfacing a second error path: the design settings panel's own "Apply"
/// already validates that text before it reaches `Design`.
///
/// [`resolved_gem_material`] (not plain `MaterialSelection::resolve`) is used for the
/// returned `gem`, so an RI override shows up in the target's own dispersion (matters
/// for `RetargetMode::Optimize`'s objective).
fn resolve_target_material(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> ResolvedMaterial {
    let options = design_material_options(custom);
    let selection =
        parse_design_material_form(combo_index, ri_override_text, &options, &design.material)
            .unwrap_or_else(|_| design.material.clone());
    let lookup = EditorMaterialLookup::new(custom);
    let resolved = selection.resolve(&lookup);
    let gem = resolved_gem_material(&selection, &lookup);
    ResolvedMaterial { gem, ..resolved }
}

/// Pure, Slint-free view of one [`RetargetRow`] -- see this module's doc comment
/// ("Slint-free view-model split").
struct RetargetRowView {
    tier_index: usize,
    block: &'static str,
    name: String,
    old_angle: String,
    new_angle: String,
    margin: String,
    risk_label: &'static str,
    risk_rgb: (u8, u8, u8),
}

/// [`Risk`]'s label plus its RGB badge color -- chosen HERE, once, so the two can
/// never drift apart, matching `Theme.accent-emerald`/`accent-amber`/`accent-ruby`
/// exactly.
const fn risk_label_and_rgb(risk: Risk) -> (&'static str, (u8, u8, u8)) {
    match risk {
        Risk::Safe => ("Safe", (0x10, 0xb9, 0x81)),
        Risk::Marginal => ("Marginal", (0xf5, 0x9e, 0x0b)),
        Risk::Windows => ("Windows", (0xf4, 0x3f, 0x5e)),
    }
}

fn row_view(row: &RetargetRow) -> RetargetRowView {
    let block = match row.block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        // Never actually produced by `retarget::build_proposal` (girdle tiers are
        // never listed), but this stays total rather than panicking on a future change.
        Block::Girdle => "Girdle",
    };
    let (risk_label, risk_rgb) = risk_label_and_rgb(row.risk);
    RetargetRowView {
        tier_index: row.tier_index,
        block,
        name: row.name.clone(),
        old_angle: format!("{:.2}\u{b0}", row.old_angle),
        new_angle: format!("{:.2}\u{b0}", row.new_angle),
        margin: format!("{:+.2}\u{b0}", row.margin_deg),
        risk_label,
        risk_rgb,
    }
}

/// Pure, Slint-free view of one [`retarget::build_proposal`] call -- exactly what
/// [`push_retarget_view`] pushes into `MainWindow`'s `editor_retarget_*` properties,
/// computed here so the routing decision (which of `rows`/`notes`/`anchored_errors`/
/// `solve_error` gets populated) is unit-tested directly. The second element of the
/// returned pair is the real [`RetargetProposal`] to stash in
/// `EditorState::pending_retarget`, `None` for either [`RetargetError`] variant.
fn retarget_view(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    mode: RetargetMode,
) -> (RetargetView, Option<RetargetProposal>) {
    match retarget::build_proposal(design, target, crown, mode) {
        Ok(proposal) => {
            let rows = proposal.rows.iter().map(row_view).collect();
            let view = RetargetView {
                rows,
                notes: proposal.notes.clone(),
                anchored_errors: Vec::new(),
                solve_error: String::new(),
            };
            (view, Some(proposal))
        }
        Err(RetargetError::AnchoredTiers(tiers)) => {
            let anchored_errors = tiers
                .iter()
                .map(|(index, name)| format!("#{index} \"{name}\""))
                .collect();
            let view = RetargetView {
                rows: Vec::new(),
                notes: Vec::new(),
                anchored_errors,
                solve_error: String::new(),
            };
            (view, None)
        }
        Err(RetargetError::Solve(err)) => {
            let view = RetargetView {
                rows: Vec::new(),
                notes: Vec::new(),
                anchored_errors: Vec::new(),
                solve_error: err.to_string(),
            };
            (view, None)
        }
    }
}

struct RetargetView {
    rows: Vec<RetargetRowView>,
    notes: Vec<String>,
    anchored_errors: Vec<String>,
    solve_error: String,
}

/// Pushes [`RetargetView`] into `MainWindow`'s `editor_retarget_rows`/`_notes`/
/// `_anchored_errors`/`_solve_error` -- the only place a [`RetargetRowView`] becomes
/// a real `RetargetRowItem`.
fn push_retarget_view(ui: &MainWindow, view: RetargetView) {
    let rows: Vec<RetargetRowItem> = view
        .rows
        .into_iter()
        .map(|r| RetargetRowItem {
            tier_index: r.tier_index as i32,
            block: r.block.into(),
            name: r.name.into(),
            old_angle: r.old_angle.into(),
            new_angle: r.new_angle.into(),
            margin: r.margin.into(),
            risk_label: r.risk_label.into(),
            risk_color: Color::from_rgb_u8(r.risk_rgb.0, r.risk_rgb.1, r.risk_rgb.2),
        })
        .collect();
    ui.global::<RetargetModel>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>()
        .set_notes(ModelRc::new(VecModel::from(
            view.notes
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_anchored_tier_errors(ModelRc::new(VecModel::from(
            view.anchored_errors
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_solve_error(view.solve_error.into());
}

/// Pushes the target material readout -- fixed for the lifetime of one dialog
/// session, but re-pushed on every rebuild anyway since it costs nothing.
fn push_target_readout(ui: &MainWindow, target: &ResolvedMaterial) {
    ui.global::<RetargetModel>()
        .set_target_material_name(target.gem.name.clone().into());
    ui.global::<RetargetModel>()
        .set_target_material_ri(format!("{:.4}", target.n_d).into());
    ui.global::<RetargetModel>()
        .set_target_critical_angle(format!("{:.2}\u{b0}", target.critical_angle_deg).into());
}

/// `RetargetMode::Optimize`'s config, reusing the standalone Optimize panel's own
/// weight fields when they parse, else [`OptimizeConfig::default`]'s weights.
/// Best-effort: a malformed weight field isn't this dialog's form to validate (the
/// Optimize panel's own button already does), so this never surfaces a second error.
fn optimize_config_from_ui(ui: &MainWindow) -> OptimizeConfig {
    let weights = parse_optimize_weights(
        &ui.global::<EditorModel>().get_optimize_weight_windowing(),
        &ui.global::<EditorModel>().get_optimize_weight_extinction(),
        &ui.global::<EditorModel>()
            .get_optimize_weight_tilt_brilliance(),
    )
    .unwrap_or_else(|_| ObjectiveWeights::default());
    OptimizeConfig {
        weights,
        ..OptimizeConfig::default()
    }
}

/// Reads `render_ctx`'s current custom materials, resolves the target against them
/// plus `ui`'s current combo/RI-override fields, then rebuilds and pushes a full
/// [`RetargetView`] -- the common body [`setup_retarget_open_callback`]/
/// [`setup_retarget_proposal_changed_callback`] both need.
fn rebuild_and_push(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
    crown: CrownShift,
    mode: RetargetMode,
) -> Option<RetargetProposal> {
    let target = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resolve_target_material(
            design,
            &ctx.custom_materials,
            ui.global::<EditorModel>().get_material_combo_index(),
            &ui.global::<EditorModel>().get_ri_override_text(),
        )
    };
    push_target_readout(ui, &target);
    let (view, proposal) = retarget_view(design, &target, crown, mode);
    push_retarget_view(ui, view);
    proposal
}

/// "Retarget for material...": opens the dialog and builds the first proposal
/// (Shift mode, default crown settings -- reset here even if a previous session left
/// the dialog's `in-out` properties on Optimize/a nonzero crown fraction).
pub(in crate::gui::editor) fn setup_retarget_open_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_open(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        ui.global::<RetargetModel>().set_mode_index(0);
        ui.global::<RetargetModel>().set_crown_fraction(0.0);
        ui.global::<RetargetModel>().set_scale_crown_by_ratio(false);
        let generation = st.generation.load(AtomicOrdering::Relaxed);
        let proposal = rebuild_and_push(
            &ui,
            &render_ctx,
            &st.design,
            CrownShift::default(),
            RetargetMode::Shift,
        );
        st.pending_retarget = proposal.map(|p| (p, generation));
        ui.global::<RetargetModel>().set_is_open(true);
    });
}

/// The mode/crown-handling controls changed: rebuilds the proposal from their
/// current values. See this module's doc comment ("Synchronous...") for why
/// `RetargetMode::Optimize` still runs inline here rather than on a worker thread.
pub(in crate::gui::editor) fn setup_retarget_proposal_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_proposal_changed(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let crown = CrownShift {
            fraction: f64::from(ui.global::<RetargetModel>().get_crown_fraction()),
            scale_by_ratio: ui.global::<RetargetModel>().get_scale_crown_by_ratio(),
        };
        let mode = if ui.global::<RetargetModel>().get_mode_index() == 1 {
            RetargetMode::Optimize(optimize_config_from_ui(&ui))
        } else {
            RetargetMode::Shift
        };
        ui.global::<RetargetModel>().set_is_busy(true);
        let generation = st.generation.load(AtomicOrdering::Relaxed);
        let proposal = rebuild_and_push(&ui, &render_ctx, &st.design, crown, mode);
        st.pending_retarget = proposal.map(|p| (p, generation));
        ui.global::<RetargetModel>().set_is_busy(false);
    });
}

/// Why [`apply_pending_retarget`] applied nothing.
enum RetargetApplyError {
    /// The design changed (another edit, Undo/Redo) since this proposal was built --
    /// the same stale-generation guard `setup_optimize_apply_callback` documents for
    /// the identical race.
    Stale,
    Edit(EditError),
}

/// Applies `pending`'s [`RetargetProposal`] through `state` iff its generation still
/// matches, via one [`retarget::apply`]/[`EditorState::apply`] call (one
/// `Edit::RetargetAngles` through `History`, the ONLY way this module mutates
/// `design`). Returns the number of tiers the pushed edit named on success.
fn apply_pending_retarget(
    state: &mut EditorState,
    pending: (RetargetProposal, u64),
) -> Result<usize, RetargetApplyError> {
    let (proposal, started_generation) = pending;
    if state.generation.load(AtomicOrdering::Relaxed) != started_generation {
        return Err(RetargetApplyError::Stale);
    }
    let edit = retarget::apply(&state.design, &proposal);
    let applied = proposal.rows.len();
    state
        .apply(edit)
        .map(|()| applied)
        .map_err(RetargetApplyError::Edit)
}

/// "Apply": commits the held proposal, then re-solves and refreshes the shared
/// viewport via [`view::refresh_all`] -- the same Solve path `setup_solve_callback`
/// uses, NOT `refresh_editor_panel_stale`: every pavilion/crown angle just moved, so
/// the design needs a real re-solve shown immediately, not left marked stale.
pub(in crate::gui::editor) fn setup_retarget_apply_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_apply(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let mut st = state.borrow_mut();
        let Some(pending) = st.pending_retarget.take() else {
            return;
        };
        let target_name = pending.0.target.gem.name.clone();
        match apply_pending_retarget(&mut st, pending) {
            Ok(applied) => {
                view::refresh_all(&ui, &render_ctx, &preview_state, &solid_last_solved, &st);
                ui.global::<RetargetModel>().set_is_open(false);
                show_toast(
                    &ui,
                    &format!("Retargeted {applied} tier(s) for {target_name}."),
                    "success",
                );
            }
            Err(RetargetApplyError::Stale) => {
                show_toast(
                    &ui,
                    "The design changed since this retarget proposal was built -- \
                     re-open Retarget for material.",
                    "error",
                );
            }
            Err(RetargetApplyError::Edit(e)) => {
                show_toast(&ui, &e.to_string(), "error");
            }
        }
    });
}

/// "Cancel"/the backdrop click: discards whatever proposal was pending and closes
/// the dialog -- no edit was ever applied, so there is nothing to undo.
pub(in crate::gui::editor) fn setup_retarget_close_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_close(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        state.borrow_mut().pending_retarget = None;
        ui.global::<RetargetModel>().set_is_open(false);
    });
}

#[cfg(test)]
mod tests {
    use super::{super::super::state::design_material_index_from_name, *};
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
    };

    fn diamond_design() -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::default(),
            vec![ConstraintTier {
                angle_deg: -40.0,
                name: "P1".to_string(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                detached: Vec::new(),
            }],
        );
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        design
    }

    // --- resolve_target_material ---

    #[test]
    fn resolve_target_material_defaults_to_the_designs_own_current_material() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let own_index = design_material_index_from_name(design.material.name.as_deref(), &options);
        let target = resolve_target_material(&design, &custom, own_index, "");
        assert_eq!(target.gem.name, "Diamond");
        assert!((target.n_d - design.effective_refractive_index()).abs() < 1e-9);
    }

    #[test]
    fn resolve_target_material_resolves_a_different_combo_selection() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        assert_eq!(target.gem.name, "Quartz");
        assert!((target.n_d - design.effective_refractive_index()).abs() > 0.1);
    }

    #[test]
    fn resolve_target_material_falls_back_when_the_ri_override_text_is_invalid() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let own_index = design_material_index_from_name(design.material.name.as_deref(), &options);
        let target = resolve_target_material(&design, &custom, own_index, "not-a-number");
        assert_eq!(target.gem.name, "Diamond");
    }

    // --- retarget_view routing ---

    #[test]
    fn retarget_view_shift_mode_populates_rows_not_errors() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        let (view, proposal) =
            retarget_view(&design, &target, CrownShift::default(), RetargetMode::Shift);
        assert!(!view.rows.is_empty());
        assert_eq!(view.anchored_errors, Vec::<String>::new());
        assert_eq!(view.solve_error, "");
        assert!(proposal.is_some());
    }

    #[test]
    fn retarget_view_optimize_mode_populates_anchored_errors_not_rows_when_anchored() {
        // `diamond_design`'s one tier is a `ScaleReference` -- always anchored, so
        // `RetargetMode::Optimize` must refuse rather than silently seed-and-stop.
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        let config = OptimizeConfig {
            max_evaluations: 4,
            ..OptimizeConfig::default()
        };
        let (view, proposal) = retarget_view(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Optimize(config),
        );
        assert!(view.rows.is_empty());
        assert_ne!(view.anchored_errors, Vec::<String>::new());
        assert!(view.anchored_errors[0].contains("P1"));
        assert!(proposal.is_none());
    }

    // --- apply_pending_retarget ---

    fn fresh_state_with(design: Design) -> EditorState {
        let mut state = EditorState::fresh();
        state.design = design;
        state.history = History::new();
        state
    }

    #[test]
    fn apply_pending_retarget_pushes_exactly_one_retarget_angles_edit_and_solves() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        let (_, proposal) =
            retarget_view(&design, &target, CrownShift::default(), RetargetMode::Shift);
        let proposal = proposal.expect("shift mode never fails");

        let mut state = fresh_state_with(design);
        let generation = state.generation.load(AtomicOrdering::Relaxed);
        let applied = apply_pending_retarget(&mut state, (proposal, generation))
            .unwrap_or_else(|_| panic!("a fresh, matching-generation proposal must apply"));
        assert_eq!(applied, 1);
        assert!(state.history.can_undo());
        assert!(
            state.design.solve().is_ok(),
            "the retargeted design must still solve"
        );
    }

    #[test]
    fn apply_pending_retarget_refuses_a_stale_generation() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        let (_, proposal) =
            retarget_view(&design, &target, CrownShift::default(), RetargetMode::Shift);
        let proposal = proposal.expect("shift mode never fails");

        let mut state = fresh_state_with(design);
        let stale_generation = state.generation.load(AtomicOrdering::Relaxed) + 1;
        let result = apply_pending_retarget(&mut state, (proposal, stale_generation));
        assert!(matches!(result, Err(RetargetApplyError::Stale)));
        assert!(
            !state.history.can_undo(),
            "nothing should have been applied"
        );
    }
}
