//! Wires `super::super::retarget` (the "Retarget for material" proposal
//! builder/applier) and `ui/components/retarget_dialog.slint` into the Edit tab --
//! `setup_*` functions, one per `RetargetDialog`/`EditorView` callback, mirroring
//! `solve_actions.rs`'s Optimize callbacks in shape.
//!
//! # Where the target material comes from
//!
//! [`resolve_target_material`] resolves the proposal's TARGET from `RetargetModel`'s
//! OWN `target_material_index`/`target_ri_override_text` properties, NOT from
//! `state.design.material` or the design settings panel's combo: this dialog owns its
//! target end to end, seeded from the design's current material on `open()` (see
//! [`setup_retarget_open_callback`]) and changed only by this dialog's own picker.
//! `retarget::apply` still only ever produces `Edit::RetargetAngles` -- see
//! [`setup_retarget_apply_callback`] for how a target material change actually reaches
//! `design.material`.
//!
//! # `RetargetMode::Optimize` runs off the UI thread
//!
//! Shift mode is pure angle arithmetic and stays synchronous. `RetargetMode::Optimize`
//! runs a real `optimize_design` search, so [`setup_retarget_proposal_changed_callback`]
//! hands it to [`super::super::optimize_solve::spawn_optimize_solve`] -- the exact same
//! worker/cancel/progress machinery the standalone Optimize panel uses
//! (`solve_actions::setup_optimize_callback`) -- rather than blocking the UI thread for
//! however long the search takes. [`RETARGET_ASYNC`] is this dialog's own
//! module-local run tracker: deliberately NOT a new `EditorState` field (that group is
//! owned elsewhere) and NOT threaded through any `setup_retarget_*` call site (`gui::
//! editor::mod`'s wiring is likewise owned elsewhere) -- a `thread_local!` is sound
//! here because Slint's event loop is single-threaded, exactly like `EditorState`'s own
//! `RefCell`. The worker's completion handler is `Send` (a hard requirement of
//! [`super::super::optimize_solve::spawn_optimize_solve`]'s own signature) so it cannot
//! capture `Rc<RefCell<EditorState>>` directly; it stashes the finished proposal into
//! [`RETARGET_ASYNC`] instead, and [`setup_retarget_apply_callback`] reads it back out
//! on the main thread exactly like it already reads `EditorState::pending_retarget` for
//! a Shift-mode proposal.
//!
//! # Slint-free view-model split, for testability
//!
//! [`retarget_view`]/[`row_view`]/[`resolve_target_material`]/[`apply_pending_retarget`]
//! take and return only plain Rust types, so this group's test module can exercise the
//! actual decision logic directly -- `push_retarget_view`/`push_target_readout` are
//! the only two functions that touch a Slint type, thin enough not to need tests.

use super::super::{
    auto_solve,
    edit_intent::{EditIntent, EditIntentQueue},
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    optimize_solve::{self, OptimizeSolveHandle, OptimizeSolveOutcome},
    retarget::{self, CrownShift, RetargetError, RetargetMode, RetargetProposal, RetargetRow},
    stale::{self, ResultKind},
    stall_guard::stall_guard,
    state::{
        EditorState, design_label_text, design_material_index_from_name, design_material_options,
        parse_design_material_form,
    },
    view::{self, parse_optimize_weights},
};
use crate::{
    EditorModel, MainWindow, RetargetModel, RetargetRowItem,
    bridge::render_thread::RenderContext,
    gui::{show_toast, solid_preview::preview_state::SolidPreviewState},
};
use indicatrix::{
    geometry::meet_solver::{Block, SolvedTier},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, Edit, EditError, MaterialSelection, ObjectiveWeights, OptimizeConfig, ResolvedMaterial,
    Risk, TierDelta, diff_tiers,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering as AtomicOrdering},
};

thread_local! {
    /// This dialog's own async `RetargetMode::Optimize` run tracker -- see this
    /// module's doc comment ("`RetargetMode::Optimize` runs off the UI thread") for
    /// why this is a module-local `thread_local!` rather than a new `EditorState`
    /// field.
    static RETARGET_ASYNC: RefCell<RetargetAsyncRun> = RefCell::new(RetargetAsyncRun::default());
}

/// [`RETARGET_ASYNC`]'s payload.
#[derive(Default)]
struct RetargetAsyncRun {
    /// Cancels the in-flight worker -- `None` whenever nothing is running.
    handle: Option<OptimizeSolveHandle>,
    /// Bumped every time a run starts, is cancelled, or is superseded by a newer
    /// one, so a worker that finishes after being superseded can tell (by comparing
    /// the `run_id` it was launched with) and discard its own result instead of
    /// overwriting a newer run's.
    run_id: u64,
    /// The finished proposal an async Optimize run produced, paired with the design
    /// generation it ran against -- exactly [`EditorState::pending_retarget`]'s own
    /// shape, read back by [`setup_retarget_apply_callback`] the same way.
    pending: Option<(RetargetProposal, u64)>,
    /// The [`crate::ActivityModel`]
    /// id [`start_optimize_run`] registered for the currently running search, if
    /// any -- finished by [`Self::cancel_and_supersede`] on every path that stops
    /// tracking `handle` (cancel, supersede, dialog close), so an abandoned run
    /// never leaves a chip behind in the status strip.
    activity_id: Option<u64>,
}

impl RetargetAsyncRun {
    /// Cancels whatever is running (if anything) and bumps `run_id`, so any result
    /// still in flight is discarded on arrival -- shared by every place that starts
    /// a new run, cancels one outright, or closes the dialog.
    fn cancel_and_supersede(&mut self) {
        if let Some(handle) = self.handle.take() {
            handle.cancel();
        }
        self.run_id = self.run_id.wrapping_add(1);
        self.pending = None;
        if let (Some(activity), Some(id)) = (auto_solve::activity(), self.activity_id.take()) {
            activity.finish(id);
        }
    }
}

/// Just the [`MaterialSelection`] half of [`resolve_target_material`], split out so
/// the async `RetargetMode::Optimize` wiring (which needs a real
/// [`MaterialSelection`] to hand [`optimize_solve::spawn_optimize_solve`], not a
/// pre-resolved [`ResolvedMaterial`]) doesn't reimplement this parsing.
///
/// # Errors
///
/// [`parse_design_material_form`]'s own message when `ri_override_text` is non-empty
/// and does not parse as a finite refractive index greater than 1.0 -- see
/// [`target_material_selection`] for the lenient wrapper most callers actually want.
fn resolve_target_selection(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> Result<MaterialSelection, String> {
    let options = design_material_options(custom);
    parse_design_material_form(combo_index, ri_override_text, &options, &design.material)
}

/// [`resolve_target_selection`], falling back to `design.material` unchanged on a
/// parse error rather than surfacing it -- used wherever a caller needs SOME target
/// selection unconditionally (`start_optimize_run`'s own search input). The two
/// dialog-readout call sites ([`rebuild_and_push`]/[`start_optimize_run`]'s own
/// up-front check) call [`resolve_target_selection`] directly instead, so a parse
/// error actually reaches the cutter -- see [`push_target_error`].
fn target_material_selection(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> MaterialSelection {
    resolve_target_selection(design, custom, combo_index, ri_override_text)
        .unwrap_or_else(|_| design.material.clone())
}

/// Resolves an already-known [`MaterialSelection`] into a [`ResolvedMaterial`]
/// against `custom`. [`resolved_gem_material`] (not plain `MaterialSelection::resolve`)
/// is used for the returned `gem`, so an RI override shows up in the target's own
/// dispersion (matters for `RetargetMode::Optimize`'s objective) -- shared by every
/// caller here that already has a `MaterialSelection` in hand, so this one three-line
/// pattern isn't repeated per call site.
fn resolved_material_from_selection(
    selection: &MaterialSelection,
    custom: &[GemMaterial],
) -> ResolvedMaterial {
    let lookup = EditorMaterialLookup::new(custom);
    let resolved = selection.resolve(&lookup);
    let gem = resolved_gem_material(selection, &lookup);
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
    // The catalogue's custom materials, so the design's CURRENT refractive index
    // resolves through the same lookup the rest of the editor uses (
    // 53). Without it a design whose material is a custom catalogue entry had its
    // source RI read from the built-in table alone, which silently fell back to a
    // different number -- and every proposed angle is a shift from that number.
    custom_materials: &[GemMaterial],
) -> (RetargetView, Option<RetargetProposal>) {
    match retarget::build_proposal(design, target, crown, mode, custom_materials) {
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

/// The readout label for a retarget target: `target.gem.name` is
/// [`resolved_gem_material`]'s pick of an actual [`GemMaterial`] to render/compute
/// dispersion against, which falls back to Diamond (`material.rs`'s own default) for
/// both "(none)" and a typed custom RI -- neither of which the cutter ever asked to
/// move toward Diamond. Derived from the [`MaterialSelection`] that was actually
/// resolved instead: the name when one was picked, `"Custom RI"` for a typed
/// override with no name, `"(none)"` for neither.
fn target_display_name(selection: &MaterialSelection) -> String {
    selection.name.clone().unwrap_or_else(|| {
        if selection.refractive_index_override.is_some() {
            "Custom RI".to_string()
        } else {
            "(none)".to_string()
        }
    })
}

/// Pushes the target material readout -- fixed for the lifetime of one dialog
/// session, but re-pushed on every rebuild anyway since it costs nothing. Also clears
/// `RetargetModel.target_material_error` (see [`push_target_error`]): reaching this
/// function at all means [`resolve_target_selection`] just succeeded, so any earlier
/// parse error no longer applies.
fn push_target_readout(ui: &MainWindow, selection: &MaterialSelection, target: &ResolvedMaterial) {
    ui.global::<RetargetModel>()
        .set_target_material_name(target_display_name(selection).into());
    ui.global::<RetargetModel>()
        .set_target_material_ri(format!("{:.4}", target.n_d).into());
    ui.global::<RetargetModel>()
        .set_target_critical_angle(format!("{:.2}\u{b0}", target.critical_angle_deg).into());
    ui.global::<RetargetModel>()
        .set_target_material_error("".into());
}

/// `ri_override_text` failed to parse -- pushes `message` into
/// `RetargetModel.target_material_error` (shown above the proposal table,
/// `retarget_dialog.slint`) and clears `rows`/`notes` so nothing on screen implies a
/// proposal was actually built against this text. The readout above the error
/// (`target_material_name`/`_ri`/`_critical_angle`) is deliberately left as-is --
/// whatever the last valid target was -- rather than reset, matching the error
/// banner's own "showing the last valid target" wording.
fn push_target_error(ui: &MainWindow, message: &str) {
    ui.global::<RetargetModel>()
        .set_target_material_error(message.into());
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: Vec::new(),
            anchored_errors: Vec::new(),
            solve_error: String::new(),
        },
    );
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
        ui.global::<EditorModel>().get_optimize_weight_yield(),
    )
    .unwrap_or_else(|_| ObjectiveWeights::default());
    OptimizeConfig {
        weights,
        ..OptimizeConfig::default()
    }
}

/// Shows `design` retargeted by `proposal` as a ghost overlay
/// in the shared solid viewport, iff `RetargetModel.preview_enabled` is on AND
/// `proposal` is `Some` (a `None` proposal -- an anchored-tier refusal, a solve
/// error -- has nothing to preview). Returns whether a ghost was actually shown;
/// the caller resubmits the real, live design through the ordinary
/// [`view::submit_preview_replan`] path when it wasn't (see both call sites).
///
/// Builds the candidate via [`Design::apply_edit`] on a clone -- explicitly
/// documented as safe for exactly this ("usable standalone by a caller that wants
/// edit/undo without a `History` stack") -- rather than [`EditorState::apply`], so
/// this never touches `History`/`is_dirty`/the design generation: a preview must
/// never look like a real edit to anything else in this crate.
fn apply_ghost_preview_or_revert(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    design: &Design,
    proposal: Option<&RetargetProposal>,
) -> bool {
    if !ui.global::<RetargetModel>().get_preview_enabled() {
        return false;
    }
    let Some(proposal) = proposal else {
        return false;
    };
    let mut candidate = design.clone();
    let edit = retarget::apply(design, proposal);
    if candidate.apply_edit(edit).is_err() {
        return false;
    }
    view::submit_design_ghost_preview(ui, render_ctx, preview_state, &candidate)
}

/// Reads `render_ctx`'s current custom materials, resolves the target against them
/// plus `RetargetModel`'s own target picker fields, then rebuilds and pushes a full
/// [`RetargetView`] -- the common body [`setup_retarget_open_callback`] and Shift
/// mode's branch of [`setup_retarget_proposal_changed_callback`] both need. Only
/// ever called with [`RetargetMode::Shift`] since `RetargetMode::Optimize` moved
/// off-thread -- see this module's doc comment.
fn rebuild_and_push(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    design: &Design,
    crown: CrownShift,
    mode: RetargetMode,
) -> Option<RetargetProposal> {
    let combo_index = ui.global::<RetargetModel>().get_target_material_index();
    let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
    // Cloned out of the lock alongside the resolution below, rather than re-locking
    // for the proposal: `retarget_view` needs the same catalogue the target was
    // resolved against, and re-locking could observe a different
    // one if a custom material were saved in between.
    let (selection, target, custom_materials) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // Checked with the Result-returning resolver FIRST, so an
        // unparseable override is reported instead of silently resolved against
        // `design.material` -- see `push_target_error`'s own doc comment.
        match resolve_target_selection(design, &ctx.custom_materials, combo_index, &ri_text) {
            Ok(selection) => {
                let target = resolved_material_from_selection(&selection, &ctx.custom_materials);
                let custom_materials = ctx.custom_materials.as_ref().clone();
                (selection, target, custom_materials)
            }
            Err(message) => {
                drop(ctx);
                push_target_error(ui, &message);
                return None;
            }
        }
    };
    push_target_readout(ui, &selection, &target);
    let (view, proposal) = retarget_view(design, &target, crown, mode, &custom_materials);
    push_retarget_view(ui, view);
    proposal
}

/// The target combo's initial index for `material` -- the design's own current
/// material by name, except a name-less selection carrying an RI override (the
/// "Custom RI…" case, see [`design_material_name_from_index`]'s own doc comment)
/// which has no name to look up at all and must instead seed the trailing sentinel
/// entry [`design_material_options`] always appends.
fn initial_target_index(material: &MaterialSelection, options: &[String]) -> i32 {
    if material.name.is_none() && material.refractive_index_override.is_some() {
        return i32::try_from(options.len()).unwrap_or(i32::MAX) - 1;
    }
    design_material_index_from_name(material.name.as_deref(), options)
}

/// "Retarget for material...": opens the dialog and builds the first proposal
/// (Shift mode, default crown settings -- reset here even if a previous session left
/// the dialog's `in-out` properties on Optimize/a nonzero crown fraction). Also
/// seeds the target picker (see this module's doc comment, "Where the target
/// material comes from") from `design`'s own current material, and drops whatever a
/// previous session's async Optimize run left in [`RETARGET_ASYNC`].
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
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
        stale::clear(ResultKind::Retarget);

        let mut st = state.borrow_mut();
        ui.global::<RetargetModel>().set_mode_index(0);
        ui.global::<RetargetModel>().set_crown_fraction(0.0);
        ui.global::<RetargetModel>().set_scale_crown_by_ratio(false);
        ui.global::<RetargetModel>().set_is_busy(false);
        ui.global::<RetargetModel>().set_optimize_evaluations(0);
        ui.global::<RetargetModel>().set_optimize_max_evaluations(0);
        // Each session starts with the preview off -- a
        // previous session's choice is not assumed to still be wanted, and the
        // viewport must show the real design (not a stale ghost) the instant this
        // dialog reopens, before any proposal has even been rebuilt.
        ui.global::<RetargetModel>().set_preview_enabled(false);

        let options = {
            let ctx = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            design_material_options(&ctx.custom_materials)
        };
        ui.global::<RetargetModel>()
            .set_target_material_index(initial_target_index(&st.design.material, &options));
        ui.global::<RetargetModel>().set_target_ri_override_text(
            st.design
                .material
                .refractive_index_override
                .map(|v| format!("{v}"))
                .unwrap_or_default()
                .into(),
        );
        ui.global::<RetargetModel>()
            .set_material_options(ModelRc::new(VecModel::from(
                options
                    .into_iter()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));

        let generation = st.generation.load(AtomicOrdering::Relaxed);
        let proposal = rebuild_and_push(
            &ui,
            &render_ctx,
            &st.design,
            CrownShift::default(),
            RetargetMode::Shift,
        );
        if let Some(proposal) = proposal {
            stale::stamp(ResultKind::Retarget, generation);
            st.pending_retarget = Some((proposal, generation));
        } else {
            st.pending_retarget = None;
        }
        ui.global::<RetargetModel>().set_is_open(true);
    });
}

/// The mode/crown/target controls changed: rebuilds the proposal from their current
/// values. Shift mode stays synchronous (pure angle arithmetic); `RetargetMode::
/// Optimize` hands off to [`start_optimize_run`] instead -- see this module's doc
/// comment ("`RetargetMode::Optimize` runs off the UI thread").
///
/// Also registers [`RetargetModel::cancel_optimize`]'s handler: both callbacks are
/// wired from this one `setup_*` function (rather than a separate one) so this
/// group's async run tracking stays entirely inside this function's own closures,
/// with no new `setup_retarget_*` call site needed in `gui::editor::mod` (owned
/// elsewhere) to wire it up.
///
/// `preview_state`/`solid_last_solved` (added beyond this function's original
/// signature): every rebuild here also shows or reverts the live ghost preview via
/// [`apply_ghost_preview_or_revert`], matching `RetargetModel.preview_enabled`'s
/// current value.
///
/// `retarget_dialog.slint`'s crown
/// slider fires its `changed(value)` interaction callback on every pixel of drag
/// (`ui/components/retarget_dialog.slint`'s own `crown_slider`), so calling
/// `RetargetModel.proposal_changed()` straight into a synchronous `rebuild_and_push`
/// plus ghost-preview/replan resubmit on every tick would rebuild the WHOLE
/// proposal once per pixel dragged. The SHIFT-mode branch (the one a crown-slider
/// drag actually takes) instead posts an [`EditIntent::RetargetCrown`] into a
/// queue this function builds once, draining at most once per 16ms tick -- see
/// [`edit_intent::EditIntentQueue`]'s own doc comment. The OPTIMIZE-mode branch is
/// unaffected: it already hands off to an off-thread, cancellable search
/// ([`start_optimize_run`]), so there is nothing to coalesce there.
pub(in crate::gui::editor) fn setup_retarget_proposal_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    {
        let intent_queue = {
            let state = Rc::clone(state);
            let render_ctx = Arc::clone(render_ctx);
            let preview_state = Arc::clone(preview_state);
            let solid_last_solved = Arc::clone(solid_last_solved);
            let ui_weak = ui.as_weak();
            EditIntentQueue::new(move |_intent| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                apply_retarget_shift_intent(
                    &ui,
                    &state,
                    &render_ctx,
                    &preview_state,
                    &solid_last_solved,
                );
            })
        };
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
        let preview_state = Arc::clone(preview_state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_proposal_changed(move || {
            stall_guard("retarget_on_proposal_changed", || {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                // A new request always supersedes whatever Optimize run was in flight.
                RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());

                if ui.global::<RetargetModel>().get_mode_index() == 1 {
                    let mut st = state.borrow_mut();
                    let crown = CrownShift {
                        fraction: f64::from(ui.global::<RetargetModel>().get_crown_fraction()),
                        scale_by_ratio: ui.global::<RetargetModel>().get_scale_crown_by_ratio(),
                    };
                    // An Optimize run "intentionally receives no
                    // further update" to `st.pending_retarget` on success
                    // (`start_optimize_run`'s own doc comment) -- the result instead
                    // reaches `RETARGET_ASYNC::pending`, which `setup_apply_callback`
                    // prefers. But a STALE Shift proposal left in `pending_retarget`
                    // from before the user switched modes was never cleared by that
                    // path, so switching Shift -> Optimize and clicking Apply before
                    // the search finished silently applied the old Shift angles
                    // instead of refusing (there is nothing yet to apply) or waiting.
                    // Cleared here, unconditionally, so Apply can only ever take an
                    // Optimize result from `RETARGET_ASYNC::pending` once this branch
                    // has run -- the Shift branch below is untouched, it still writes
                    // its own proposal into this same field.
                    st.pending_retarget = None;
                    start_optimize_run(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &mut st,
                        crown,
                    );
                } else {
                    intent_queue.post(EditIntent::RetargetCrown);
                }
            });
        });
    }

    {
        let state = Rc::clone(state);
        let ui_weak = ui.as_weak();
        ui.global::<RetargetModel>().on_cancel_optimize(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // A stray Shift-mode `pending_retarget` cannot actually be present here
            // (this button only matters `while is_busy`, which only Optimize mode
            // sets, and that branch already clears `pending_retarget` before
            // dispatching -- see `on_proposal_changed`'s own comment on why),
            // but cleared defensively anyway since it costs nothing.
            state.borrow_mut().pending_retarget = None;
            cancel_optimize_run(&ui);
        });
    }
}

/// The shared body of "cancel the in-flight `RetargetMode::Optimize` search":
/// [`setup_retarget_proposal_changed_callback`]'s `on_cancel_optimize` handler (the
/// dialog's own Cancel button while busy) and [`start_optimize_run`]'s own
/// `ActivityRegistry` cancel closure (the status strip's `ActivityChip`, which has
/// no `Rc<RefCell<EditorState>>` to reach `state.pending_retarget` with -- see
/// [`RetargetAsyncRun::cancel_and_supersede`]'s own doc comment for why that alone
/// is sufficient: `RETARGET_ASYNC::pending`, not `state.pending_retarget`, is what
/// actually holds an Optimize-mode proposal). Both reach the exact same effect
/// through this one function.
fn cancel_optimize_run(ui: &MainWindow) {
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
    stale::clear(ResultKind::Retarget);
    ui.global::<RetargetModel>().set_is_busy(false);
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: vec!["Optimize cancelled.".to_string()],
            anchored_errors: Vec::new(),
            solve_error: String::new(),
        },
    );
}

/// [`setup_retarget_proposal_changed_callback`]'s Shift-mode body, run once per
/// drained [`EditIntent::RetargetCrown`] instead of once per crown-slider tick --
/// see that function's own doc comment. Reads `RetargetModel.crown_fraction`/
/// `scale_crown_by_ratio` fresh (exactly as the original per-tick call did), so a
/// coalesced burst always rebuilds against the LATEST slider position, not
/// whatever it was when the first tick of the burst posted.
fn apply_retarget_shift_intent(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
) {
    let mut st = state.borrow_mut();
    let crown = CrownShift {
        fraction: f64::from(ui.global::<RetargetModel>().get_crown_fraction()),
        scale_by_ratio: ui.global::<RetargetModel>().get_scale_crown_by_ratio(),
    };
    ui.global::<RetargetModel>().set_is_busy(false);
    let generation = st.generation.load(AtomicOrdering::Relaxed);
    let proposal = rebuild_and_push(ui, render_ctx, &st.design, crown, RetargetMode::Shift);
    if !apply_ghost_preview_or_revert(ui, render_ctx, preview_state, &st.design, proposal.as_ref())
    {
        view::submit_preview_replan(
            ui,
            render_ctx,
            preview_state,
            solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    }
    // Same stamping as the Optimize
    // completion path (`finish_optimize_run`) -- see `stale::ResultKind::Retarget`'s
    // own doc comment.
    if let Some(proposal) = proposal {
        stale::stamp(ResultKind::Retarget, generation);
        st.pending_retarget = Some((proposal, generation));
    } else {
        stale::clear(ResultKind::Retarget);
        st.pending_retarget = None;
    }
}

/// Pushes an `anchored_tier_errors` view (see `RetargetError::AnchoredTiers`'s own
/// doc comment) and clears any pending proposal -- [`start_optimize_run`]'s up-front
/// refusal path, pulled out to keep that function under clippy's function-length
/// lint.
fn push_anchored_refusal(ui: &MainWindow, st: &mut EditorState, anchored: &[(usize, String)]) {
    let anchored_errors = anchored
        .iter()
        .map(|(index, name)| format!("#{index} \"{name}\""))
        .collect();
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: Vec::new(),
            anchored_errors,
            solve_error: String::new(),
        },
    );
    st.pending_retarget = None;
    ui.global::<RetargetModel>().set_is_busy(false);
}

/// Pushes `RetargetModel.optimize_evaluations`/`optimize_max_evaluations`, clamping
/// each to `i32`'s range (real evaluation counts never come close) -- shared by
/// [`start_optimize_run`]'s own initial `0`/`max_evaluations` push and the running
/// search's own progress ticks. Also feeds the SAME fraction into this run's own
/// `ActivityRegistry` entry, if one is registered (
/// section 3.3/8, BUILD item 1) -- `max_evaluations == 0` (not yet known) reports
/// indeterminate rather than a division by zero.
fn set_optimize_progress(ui: &MainWindow, evaluations: usize, max_evaluations: usize) {
    ui.global::<RetargetModel>()
        .set_optimize_evaluations(i32::try_from(evaluations).unwrap_or(i32::MAX));
    ui.global::<RetargetModel>()
        .set_optimize_max_evaluations(i32::try_from(max_evaluations).unwrap_or(i32::MAX));
    if let (Some(activity), Some(id)) = (
        auto_solve::activity(),
        RETARGET_ASYNC.with(|cell| cell.borrow().activity_id),
    ) {
        let fraction = if max_evaluations == 0 {
            super::super::activity::INDETERMINATE
        } else {
            evaluations as f32 / max_evaluations as f32
        };
        activity.progress(id, fraction);
    }
}

/// [`setup_retarget_proposal_changed_callback`]'s `RetargetMode::Optimize` branch:
/// runs the cheap, synchronous part of `retarget::build_proposal`'s Optimize path
/// (target resolution, scope, the up-front anchored-tier check, the shift seed)
/// here on the UI thread, then hands the actual `optimize_design` search off to
/// [`optimize_solve::spawn_optimize_solve`]. `st` is the already-borrowed
/// `EditorState` (borrowed by the caller so the anchored-tier/no-op cases can update
/// `pending_retarget` without a second borrow).
///
/// `preview_state`/`solid_last_solved`: reverts the viewport
/// to the real, live design the moment a fresh search starts (so a stale ghost from
/// a previous proposal never lingers through several seconds of search with no
/// candidate of its own yet) -- [`finish_optimize_run`] shows the new ghost once
/// the search actually has a result, via the same `Arc` clones stashed on
/// [`OptimizeRunContext`].
fn start_optimize_run(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &view::SolidLastSolved,
    st: &mut EditorState,
    crown: CrownShift,
) {
    let custom = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let combo_index = ui.global::<RetargetModel>().get_target_material_index();
    let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
    // Checked up front so an unparseable override refuses the run with an
    // explicit error, matching `rebuild_and_push`'s own guard, instead of silently
    // searching against `st.design.material` unchanged.
    let selection = match resolve_target_selection(&st.design, &custom, combo_index, &ri_text) {
        Ok(selection) => selection,
        Err(message) => {
            push_target_error(ui, &message);
            // Matches `push_anchored_refusal`'s own clearing: `RETARGET_ASYNC.pending`
            // is already `None` (the caller cancelled/superseded before calling this
            // function), but `st.pending_retarget` could still hold an earlier,
            // genuinely valid Optimize proposal -- Apply must not offer THAT one up
            // as if it answered this now-unparseable text.
            st.pending_retarget = None;
            ui.global::<RetargetModel>().set_is_busy(false);
            return;
        }
    };
    let target = resolved_material_from_selection(&selection, &custom);
    push_target_readout(ui, &selection, &target);

    let (scope, blocks) = retarget::retarget_scope(&st.design);
    let anchored = retarget::anchored_tiers_in(&st.design, &scope);
    if !anchored.is_empty() {
        push_anchored_refusal(ui, st, &anchored);
        return;
    }

    // `_with(&custom)`, not the bare (built-ins-and-override-only) accessor: `st.
    // design.material.name` may name a CUSTOM catalogue material this session
    // defined, which the bare accessor cannot see at all and would silently score
    // against the wrong index for -- see `Design::effective_refractive_index_with`'s
    // own doc comment.
    let n_from = st.design.effective_refractive_index_with(&custom);
    let n_to = target.n_d;
    let seeded = retarget::seed_shift_design(&st.design, &scope, &blocks, n_from, n_to, crown);
    let ctx = OptimizeRunContext {
        original: st.design.clone(),
        seeded: seeded.clone(),
        scope,
        blocks,
        n_to,
        target,
        custom: custom.clone(),
        render_ctx: Arc::clone(render_ctx),
        preview_state: Arc::clone(preview_state),
    };
    let config = optimize_config_from_ui(ui);
    let max_evaluations = config.max_evaluations;

    ui.global::<RetargetModel>().set_is_busy(true);
    // Registered
    // before the worker spawns (and before the first `set_optimize_progress`
    // call just below, so that call's own activity-progress push actually has
    // an id to reach), so the status strip's activity list shows it from the
    // first frame -- same `auto_solve::activity()` stashed-handle pattern
    // `solve_actions::setup_optimize_callback` uses for the standalone Optimize
    // panel. Cancelling from the activity chip runs [`cancel_optimize_run`], the
    // exact same reset `on_cancel_optimize` (below) applies.
    let activity_id = auto_solve::activity().map(|a| {
        let ui_weak = ui.as_weak();
        a.start(
            "retarget_optimize",
            "Retarget Optimize",
            Some(Box::new(move || {
                // `cancel_optimize_run` reaches `ActivityRegistry::finish` (via
                // `RetargetAsyncRun::cancel_and_supersede`), and THIS closure
                // runs synchronously from inside `ActivityRegistry::cancels.
                // borrow().invoke(id)` (`ActivityModel.cancel`'s own handler,
                // `activity.rs::ActivityRegistry::new`) -- calling `finish` (its
                // own `self.cancels.borrow_mut()`) right here would panic on a
                // re-entrant borrow. Deferred one event-loop tick via a
                // single-shot `Timer` (the same "run after this handler returns"
                // idiom `auto_solve::schedule_idle_replan_if_stale` already
                // uses), by which point `invoke`'s own borrow has been dropped.
                let ui_weak = ui_weak.clone();
                let timer = slint::Timer::default();
                timer.start(
                    slint::TimerMode::SingleShot,
                    std::time::Duration::ZERO,
                    move || {
                        if let Some(ui) = ui_weak.upgrade() {
                            cancel_optimize_run(&ui);
                        }
                    },
                );
            })),
        )
    });
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().activity_id = activity_id);
    set_optimize_progress(ui, 0, max_evaluations);
    // A stale ghost from a previous proposal must not sit in
    // the viewport for the several seconds this search can take before it has any
    // candidate of its own to show -- `finish_optimize_run` shows the new one once
    // the search actually has a result.
    view::submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        st,
        BTreeSet::new(),
        false,
    );

    let run_id = RETARGET_ASYNC.with(|cell| {
        let mut run = cell.borrow_mut();
        run.run_id = run.run_id.wrapping_add(1);
        run.run_id
    });
    let started_generation = st.generation.load(AtomicOrdering::Relaxed);

    // `on_done` below must be `Send` (`spawn_optimize_solve`'s own bound, since it
    // crosses into the worker thread before hopping back to the UI thread) --
    // everything it captures is therefore plain owned data (bundled into `ctx`),
    // never `Rc<RefCell<EditorState>>`. See this module's doc comment.
    let handle = optimize_solve::spawn_optimize_solve(
        ui.as_weak(),
        seeded,
        selection,
        custom,
        config,
        |ui: &MainWindow, progress: optimize_solve::OptimizeSolveProgress| {
            set_optimize_progress(ui, progress.evaluations, progress.max_evaluations);
        },
        move |ui: &MainWindow, outcome: OptimizeSolveOutcome| {
            finish_optimize_run(ui, outcome, run_id, started_generation, &ctx);
        },
    );
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().handle = Some(handle));
    // `st` (the caller's already-borrowed `EditorState`) intentionally receives no
    // further update here: the async result reaches `EditorState::pending_retarget`
    // only through `setup_retarget_apply_callback` reading `RETARGET_ASYNC::pending`
    // back out on the main thread once the worker finishes, never from this
    // function's own return.
}

/// Everything [`finish_optimize_run`] needs about the run it is completing, bundled
/// purely to keep that function's own signature short -- the same reasoning
/// `optimize_solve::OptimizeJob` documents on itself. Built once by
/// [`start_optimize_run`] and moved, whole, into the `Send`-bound completion closure.
struct OptimizeRunContext {
    /// The design as it stood before the shift seed/search ran -- every row's
    /// `old_angle` reads from this, per [`retarget::rows_from_outcome`].
    original: Design,
    /// `original` with every `scope` tier's angle already shifted -- the baseline
    /// [`optimize_solve::spawn_optimize_solve`]'s own worker searched from.
    seeded: Design,
    /// The pavilion/crown tier indices this run retargets (see
    /// [`retarget::retarget_scope`]).
    scope: Vec<usize>,
    /// `scope`'s own [`Block`] classification, same order.
    blocks: Vec<Block>,
    /// The target material's refractive index -- `retarget::build_notes`/
    /// `retarget::rows_from_outcome` both need it.
    n_to: f64,
    /// The resolved target material this run searched against.
    target: ResolvedMaterial,
    /// The custom catalogue materials in scope when this run started -- carried
    /// through so [`finish_optimize_run`] can also resolve `original`'s effective RI
    /// via `Design::effective_refractive_index_with` rather than the bare,
    /// custom-catalogue-blind accessor (see [`start_optimize_run`]'s matching
    /// comment on `n_from`).
    custom: Vec<GemMaterial>,
    /// The shared render context, carried through so [`finish_optimize_run`] can show the
    /// finished search's own candidate as a ghost preview -- both are plain `Arc`
    /// clones, `Send`-safe like every other field here.
    render_ctx: Arc<Mutex<RenderContext>>,
    /// See [`Self::render_ctx`].
    preview_state: Arc<SolidPreviewState>,
}

/// [`start_optimize_run`]'s completion handler. Runs on the UI thread (via
/// [`optimize_solve::spawn_optimize_solve`]'s own `upgrade_in_event_loop` hop), but
/// reached through a `Send`-bound closure (see this module's doc comment) that
/// cannot capture `Rc<RefCell<EditorState>>` -- so the finished proposal goes into
/// [`RETARGET_ASYNC`], not straight into `EditorState`.
///
/// Discards the result (leaving `RetargetModel`/`RETARGET_ASYNC` exactly as whatever
/// superseded this run already left them) when `run_id` no longer matches
/// [`RETARGET_ASYNC`]'s current one -- this run was cancelled or superseded before it
/// finished.
fn finish_optimize_run(
    ui: &MainWindow,
    outcome: OptimizeSolveOutcome,
    run_id: u64,
    started_generation: u64,
    ctx: &OptimizeRunContext,
) {
    let still_current = RETARGET_ASYNC.with(|cell| cell.borrow().run_id == run_id);
    if !still_current {
        return;
    }
    RETARGET_ASYNC.with(|cell| cell.borrow_mut().handle = None);
    // This run is finishing on its
    // own (a cancelled/superseded run's own activity was already finished by
    // `RetargetAsyncRun::cancel_and_supersede`, which also cleared `activity_id`,
    // so `take()` here is a harmless no-op on that path).
    if let (Some(activity), Some(id)) = (
        auto_solve::activity(),
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().activity_id.take()),
    ) {
        activity.finish(id);
    }

    let (rows, notes, solve_error) = match outcome {
        OptimizeSolveOutcome::Completed { outcome }
        | OptimizeSolveOutcome::Cancelled { outcome } => {
            let rows = retarget::rows_from_outcome(
                &ctx.original,
                &ctx.seeded,
                &ctx.scope,
                &ctx.blocks,
                ctx.n_to,
                &outcome.changes,
            );
            let n_from = ctx.original.effective_refractive_index_with(&ctx.custom);
            let notes = retarget::build_notes(
                RetargetMode::Optimize(OptimizeConfig::default()),
                n_from,
                ctx.n_to,
            );
            (rows, notes, String::new())
        }
        OptimizeSolveOutcome::Failed { error } => (Vec::new(), Vec::new(), error.to_string()),
    };

    let row_views: Vec<RetargetRowView> = rows.iter().map(row_view).collect();
    let proposal = (!rows.is_empty()).then(|| RetargetProposal {
        rows,
        target: ctx.target.clone(),
        notes: notes.clone(),
    });
    // Shows the ghost preview for THIS finished search's own
    // candidate (when the toggle is on and a proposal actually built), against the
    // SAME `original` design `apply_pending_retarget` would retarget from --
    // before `proposal` is moved into `RETARGET_ASYNC` below. No live
    // `EditorState` is reachable from this `Send`-bound closure (see this module's
    // doc comment) to revert through the ordinary `submit_preview_replan` path if
    // this shows nothing, but there is nothing stale to revert either:
    // `start_optimize_run` already put the real design back the moment this
    // search started.
    let _ = apply_ghost_preview_or_revert(
        ui,
        &ctx.render_ctx,
        &ctx.preview_state,
        &ctx.original,
        proposal.as_ref(),
    );
    let has_proposal = proposal.is_some();
    RETARGET_ASYNC.with(|cell| {
        cell.borrow_mut().pending = proposal.map(|p| (p, started_generation));
    });
    // Stamps this result's own
    // generation so `push_stale_content` (`view.rs`) can badge it the instant a
    // FURTHER edit lands -- see `stale::ResultKind::Retarget`'s own doc comment.
    if has_proposal {
        stale::stamp(ResultKind::Retarget, started_generation);
    } else {
        stale::clear(ResultKind::Retarget);
    }

    let view = RetargetView {
        rows: row_views,
        notes,
        anchored_errors: Vec::new(),
        solve_error,
    };
    push_retarget_view(ui, view);
    ui.global::<RetargetModel>().set_is_busy(false);
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
/// matches, via exactly one [`EditorState::apply`] call -- the ONLY way this module
/// mutates `design`. When `material_change` is `Some`, the angle retarget and the
/// material change are combined into one `Edit::Batch` (see that variant's own doc
/// comment: it validates both sub-edits before writing anything back, so a stale
/// tier index can never leave the design half-retargeted), so the whole "Retarget for
/// material" action is ONE undo step; `None` (the target already matches
/// `state.design.material`, e.g. a Shift-mode preview the user never actually
/// retargeted) pushes the plain `Edit::RetargetAngles` alone, exactly as before.
/// Returns the number of tiers the retarget named on success.
fn apply_pending_retarget(
    state: &mut EditorState,
    pending: (RetargetProposal, u64),
    material_change: Option<MaterialSelection>,
) -> Result<usize, RetargetApplyError> {
    let (proposal, started_generation) = pending;
    if state.generation.load(AtomicOrdering::Relaxed) != started_generation {
        return Err(RetargetApplyError::Stale);
    }
    let angle_edit = retarget::apply(&state.design, &proposal);
    let applied = proposal.rows.len();
    let edit = match material_change {
        Some(material) => Edit::Batch(vec![angle_edit, Edit::SetMaterial { material }]),
        None => angle_edit,
    };
    state
        .apply(edit)
        .map(|()| applied)
        .map_err(RetargetApplyError::Edit)
}

/// "Apply": commits the target material (see below) together with the held
/// proposal, then re-solves and refreshes the shared viewport via
/// [`view::refresh_all`] -- the same Solve path `setup_solve_callback` uses, NOT
/// `refresh_editor_panel_stale`: every pavilion/crown angle just moved, so the design
/// needs a real re-solve shown immediately, not left marked stale.
///
/// # Also commits the target material, as ONE undo step
///
/// Moving every tier's angle without also committing `design.material` would leave
/// critical-angle-shifted tiers reading as "Windows" against the design's OLD
/// material -- e.g. Diamond-shifted angles judged against Quartz's own, much
/// shallower critical angle -- until the cutter separately pressed "Apply" on the
/// design settings panel's own material combo, so both commit together as one
/// step instead. The target material is resolved from `RetargetModel`'s own picker fields
/// BEFORE calling [`apply_pending_retarget`] (its own stale-generation check still
/// runs first inside that call, against the generation the proposal was actually
/// built against), then handed to it as `material_change` -- `Some` only when it
/// actually differs from `state.design.material` (e.g. never for a Shift-mode
/// preview the cutter didn't retarget). `apply_pending_retarget` combines the angle
/// retarget and the material change into one `Edit::Batch` (see that variant's own
/// doc comment), so pressing Undo once fully reverts a retarget that also changed
/// material, not twice.
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
        stall_guard("retarget_on_apply", || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            // A Shift-mode proposal lives in `EditorState::pending_retarget`; an
            // Optimize-mode one (built off-thread) lives in `RETARGET_ASYNC::pending`
            // instead -- see this module's doc comment.
            let pending = st
                .pending_retarget
                .take()
                .or_else(|| RETARGET_ASYNC.with(|cell| cell.borrow_mut().pending.take()));
            let Some(pending) = pending else {
                return;
            };
            // The pending result is being consumed right now (applied, or attempted
            // and refused) either way -- nothing is left to badge as stale.
            stale::clear(ResultKind::Retarget);

            // Resolved BEFORE `apply_pending_retarget` -- see this function's own doc
            // comment ("Also commits the target material, as ONE undo step"). Reading
            // `st.design.material` here (rather than after the retarget applies) is safe:
            // a retarget's own `Edit::RetargetAngles`/`Edit::Batch` never touches
            // `design.material` itself, only tier angles.
            let custom = render_ctx
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .custom_materials
                .as_ref()
                .clone();
            let combo_index = ui.global::<RetargetModel>().get_target_material_index();
            let ri_text = ui.global::<RetargetModel>().get_target_ri_override_text();
            let material_selection =
                target_material_selection(&st.design, &custom, combo_index, &ri_text);
            // Named from the `MaterialSelection` the cutter actually picked, not
            // `pending.0.target.gem.name` (which falls back to Diamond for "(none)"/a
            // typed custom RI -- see `target_display_name`'s own doc comment), same
            // as the dialog's own live readout.
            let target_name = target_display_name(&material_selection);
            let material_change =
                (material_selection != st.design.material).then_some(material_selection);

            match apply_pending_retarget(&mut st, pending, material_change) {
                Ok(applied) => {
                    // `view::
                    // refresh_all` now takes `Rc<RefCell<EditorState>>` under the
                    // name `view::refresh_all_now` (see that function's own doc
                    // comment) -- this call site's own logic is otherwise untouched.
                    drop(st);
                    view::refresh_all_now(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &state,
                        false,
                    );
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
    });
}

/// "Cancel"/the backdrop click: discards whatever proposal was pending and closes
/// the dialog -- no edit was ever applied, so there is nothing to undo.
///
/// `render_ctx`/`preview_state`/`solid_last_solved` (added beyond this function's
/// original signature): closing the dialog must never leave a candidate's ghost
/// geometry stuck in the shared viewport, so this always resubmits the real, live
/// design on the way out, regardless of how `RetargetModel.preview_enabled` was
/// left.
pub(in crate::gui::editor) fn setup_retarget_close_callback(
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
    ui.global::<RetargetModel>().on_close(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        // Also abandons any in-flight `RetargetMode::Optimize` worker -- closing the
        // dialog must not leave a search running (and later overwriting
        // `RETARGET_ASYNC::pending`) behind an already-dismissed proposal.
        RETARGET_ASYNC.with(|cell| cell.borrow_mut().cancel_and_supersede());
        state.borrow_mut().pending_retarget = None;
        stale::clear(ResultKind::Retarget);
        ui.global::<RetargetModel>().set_is_open(false);
        let st = state.borrow();
        view::submit_preview_replan(
            &ui,
            &render_ctx,
            &preview_state,
            &solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
    });
}

thread_local! {
    /// A design plus its solved masts,
    /// captured on demand by [`setup_snapshot_callbacks`]'s `snapshot_design`
    /// handler and read back by its `compare_to_snapshot` handler. A module-local
    /// `thread_local!` for the same reason [`RETARGET_ASYNC`] is one -- rather than
    /// a new `EditorState` field, and Slint's single-threaded event loop makes this
    /// sound.
    static DESIGN_SNAPSHOT: RefCell<Option<DesignSnapshot>> = const { RefCell::new(None) };
}

/// [`DESIGN_SNAPSHOT`]'s payload.
#[derive(Clone)]
struct DesignSnapshot {
    design: Design,
    /// The design's own solve at snapshot time, when it had one -- `None` for a
    /// design that does not currently solve (a `MissingAnchor`), in which case
    /// [`diff_tiers`] simply reports no mast figures for that side, same as it
    /// would for any other caller with nothing to compare.
    solved: Option<Vec<SolvedTier>>,
    /// The design's own label at snapshot time (`state::design_label_text`), shown
    /// in the compare header so two different snapshots across one session are
    /// never mistaken for each other.
    label: String,
}

/// One [`TierDelta`] as a pure, Slint-free view -- see [`diff_rows_from_deltas`]'s
/// own doc comment for exactly how each field maps onto [`RetargetRowItem`].
struct DiffRowView {
    tier_index: usize,
    name: String,
    old_angle: String,
    new_angle: String,
    mast_delta: String,
    status_label: &'static str,
    status_rgb: (u8, u8, u8),
}

/// A tier position where NEITHER the angle, the indices, nor the mast (beyond
/// [`MAST_DIFF_TOLERANCE`]) moved -- shown as "Same" rather than "Changed" so a
/// long, mostly-untouched schedule reads at a glance.
const MAST_DIFF_TOLERANCE: f64 = 1e-4;

/// One [`TierDelta`]'s badge label and RGB color -- chosen HERE, once, matching
/// `risk_label_and_rgb`'s own "choose it in exactly one place" reasoning, so the
/// label and the color can never drift apart. The RGB values match
/// `Theme.accent-sky`/`accent-ruby`/`accent-amber`/`accent-emerald` (this module
/// has no access to the `Theme` global from plain Rust, so the numbers are
/// restated here, same as `risk_label_and_rgb` already does for `Risk`).
fn diff_status_label_and_rgb(delta: &TierDelta) -> (&'static str, (u8, u8, u8)) {
    if delta.added() {
        ("Added", (0x38, 0xbd, 0xf8))
    } else if delta.removed() {
        ("Removed", (0xf4, 0x3f, 0x5e))
    } else if delta.angle_changed()
        || delta.indices_changed()
        || delta.mast_changed(MAST_DIFF_TOLERANCE)
    {
        ("Changed", (0xf5, 0x9e, 0x0b))
    } else {
        ("Same", (0x10, 0xb9, 0x81))
    }
}

fn diff_row_view(delta: &TierDelta) -> DiffRowView {
    let angle_text =
        |a: Option<f64>| a.map_or_else(|| "-".to_string(), |v| format!("{v:.2}\u{b0}"));
    let mast_delta = match (delta.mast_before, delta.mast_after) {
        (Some(before), Some(after)) => format!("{:+.4}", after - before),
        _ => "-".to_string(),
    };
    let (status_label, status_rgb) = diff_status_label_and_rgb(delta);
    DiffRowView {
        tier_index: delta.index,
        name: delta.name.clone(),
        old_angle: angle_text(delta.angle_before),
        new_angle: angle_text(delta.angle_after),
        mast_delta,
        status_label,
        status_rgb,
    }
}

/// `deltas` (`indicatrix_cut_core::diff_tiers`'s own output)
/// rendered into [`RetargetModel::compare_rows`], reusing this dialog's existing
/// [`RetargetRowItem`] row shape rather than a second table type (this shape
/// already fits): `block` is
/// left blank (a design comparison has no crown/pavilion grouping of its own),
/// `old_angle`/`new_angle` are this tier's angle in the snapshot vs. now,
/// `margin` is repurposed to show the signed MAST delta (there is no target
/// material here to measure a critical-angle margin against), and
/// `risk_label`/`risk_color` become a plain change-status badge ("Same" /
/// "Changed" / "Added" / "Removed") instead of a windowing risk.
#[must_use]
fn diff_rows_from_deltas(deltas: &[TierDelta]) -> Vec<RetargetRowItem> {
    deltas
        .iter()
        .map(diff_row_view)
        .map(|v| RetargetRowItem {
            tier_index: i32::try_from(v.tier_index).unwrap_or(i32::MAX),
            block: "".into(),
            name: v.name.into(),
            old_angle: v.old_angle.into(),
            new_angle: v.new_angle.into(),
            margin: v.mast_delta.into(),
            risk_label: v.status_label.into(),
            risk_color: Color::from_rgb_u8(v.status_rgb.0, v.status_rgb.1, v.status_rgb.2),
        })
        .collect()
}

/// [`setup_snapshot_callbacks`]'s `snapshot_design` tail -- stashes `design`'s
/// snapshot into [`DESIGN_SNAPSHOT`] and reports it. Shared by that callback's
/// own cache-hit (synchronous) and cache-miss
/// (`native_io::resolve_solved_then`'s background-solve continuation) paths, so
/// the two can never store or report a snapshot differently.
fn store_design_snapshot(
    ui: &MainWindow,
    design: &Design,
    solved: Option<Vec<SolvedTier>>,
    label: &str,
) {
    DESIGN_SNAPSHOT.with(|cell| {
        *cell.borrow_mut() = Some(DesignSnapshot {
            design: design.clone(),
            solved,
            label: label.to_string(),
        });
    });
    // "Compare to Snapshot" is
    // gated on this in `editor_command_bar.slint`; a snapshot is never cleared
    // within a session, so this only ever goes `true`.
    ui.global::<EditorModel>().set_has_snapshot(true);
    show_toast(ui, &format!("Snapshot taken: \"{label}\"."), "success");
}

/// [`setup_snapshot_callbacks`]'s `compare_to_snapshot` tail -- diffs `snapshot`
/// against `design`'s current state via [`indicatrix_cut_core::diff_tiers`] and
/// opens the Retarget dialog's own shell in its Compare mode
/// (`RetargetModel.compare_open`) -- see `retarget_dialog.slint`'s own `if
/// compare_open` branch. Shared by that callback's own cache-hit/cache-miss
/// paths, the same reasoning [`store_design_snapshot`] documents on itself.
fn show_compare_to_snapshot(
    ui: &MainWindow,
    snapshot: &DesignSnapshot,
    design: &Design,
    current_label: &str,
    current_solved: Option<&[SolvedTier]>,
) {
    let deltas = diff_tiers(
        &snapshot.design.tiers,
        snapshot.solved.as_deref(),
        &design.tiers,
        current_solved,
    );
    let rows = diff_rows_from_deltas(&deltas);
    ui.global::<RetargetModel>()
        .set_compare_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>().set_compare_label(
        format!("\"{}\" vs. current (\"{current_label}\")", snapshot.label).into(),
    );
    ui.global::<RetargetModel>().set_compare_open(true);
}

/// "Snapshot Design"/"Compare to Snapshot": registers both
/// halves of the design-comparison feature. Neither callback is wired to a visible
/// button anywhere in this app yet -- the command bar/menu trigger for
/// `EditorModel.snapshot_design()`/`compare_to_snapshot()`, and the one-line
/// `gui::editor::mod` registration this function itself needs, are still to add.
///
/// `snapshot_design` captures `state.design` plus its current solved masts (from
/// `solid_last_solved`'s cache when it is aligned with the design, else a
/// background solve via `native_io::resolve_solved_then`, so a large,
/// not-yet-cached design snapshots without blocking the UI thread with a
/// synchronous `Design::solve()` right here) into [`DESIGN_SNAPSHOT`].
/// `compare_to_snapshot` diffs that snapshot against the design's CURRENT state
/// via [`indicatrix_cut_core::diff_tiers`] and opens the Retarget dialog's own
/// shell in its Compare mode (`RetargetModel.compare_open`) -- see
/// `retarget_dialog.slint`'s own `if compare_open` branch.
pub(in crate::gui::editor) fn setup_snapshot_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    solid_last_solved: &view::SolidLastSolved,
) {
    {
        let state = Rc::clone(state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        ui.global::<EditorModel>().on_snapshot_design(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (design, label, cached) = {
                let st = state.borrow();
                let cached = solid_last_solved
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|s| s.len() == st.design.tiers.len());
                (
                    Arc::new(st.design.clone()),
                    design_label_text(st.asc_filename.as_deref()),
                    cached,
                )
            };
            // A cache hit stays synchronous (no perceptible cost); a miss goes
            // through `native_io::resolve_solved_then`'s SAME cached-or-background
            // resolution the write/export paths already use, so a large,
            // not-yet-cached design snapshots without a multi-second freeze.
            if let Some(solved) = cached {
                store_design_snapshot(&ui, &design, Some(solved), &label);
                return;
            }
            super::super::native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
                store_design_snapshot(ui, &design, solved.ok(), &label);
            });
        });
    }

    {
        let state = Rc::clone(state);
        let solid_last_solved = Arc::clone(solid_last_solved);
        let ui_weak = ui.as_weak();
        ui.global::<EditorModel>().on_compare_to_snapshot(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let snapshot = DESIGN_SNAPSHOT.with(|cell| cell.borrow().clone());
            let Some(snapshot) = snapshot else {
                show_toast(
                    &ui,
                    "No snapshot taken yet -- use Snapshot Design first.",
                    "error",
                );
                return;
            };
            let (design, current_label, cached) = {
                let st = state.borrow();
                let cached = solid_last_solved
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|s| s.len() == st.design.tiers.len());
                (
                    Arc::new(st.design.clone()),
                    design_label_text(st.asc_filename.as_deref()),
                    cached,
                )
            };
            // Same cached-or-background resolution as `on_snapshot_design` just
            // above, for the same reason.
            if let Some(current_solved) = &cached {
                show_compare_to_snapshot(
                    &ui,
                    &snapshot,
                    &design,
                    &current_label,
                    Some(current_solved),
                );
                return;
            }
            super::super::native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
                show_compare_to_snapshot(
                    ui,
                    &snapshot,
                    &design,
                    &current_label,
                    solved.ok().as_deref(),
                );
            });
        });
    }

    let ui_weak = ui.as_weak();
    ui.global::<RetargetModel>().on_compare_close(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<RetargetModel>().set_compare_open(false);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::{super::super::state::design_material_index_from_name, *};
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{
        ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
    };

    /// Test-only convenience composing [`target_material_selection`] (the lenient,
    /// production wrapper) with [`resolved_material_from_selection`] -- these tests
    /// exercise the RESOLVED material, not the [`MaterialSelection`] on its own, and
    /// production code no longer has a use for that exact composition (both real call
    /// sites need the `Result`-returning [`resolve_target_selection`] instead, for
    /// error surfacing).
    fn resolve_target_material(
        design: &Design,
        custom: &[GemMaterial],
        combo_index: i32,
        ri_override_text: &str,
    ) -> ResolvedMaterial {
        let selection = target_material_selection(design, custom, combo_index, ri_override_text);
        resolved_material_from_selection(&selection, custom)
    }

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
                original_notes: None,
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
        let (view, proposal) = retarget_view(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Shift,
            &[],
        );
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
            &[],
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
        let (_, proposal) = retarget_view(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Shift,
            &[],
        );
        let proposal = proposal.expect("shift mode never fails");

        let mut state = fresh_state_with(design);
        let generation = state.generation.load(AtomicOrdering::Relaxed);
        let applied = apply_pending_retarget(&mut state, (proposal, generation), None)
            .unwrap_or_else(|_| panic!("a fresh, matching-generation proposal must apply"));
        assert_eq!(applied, 1);
        assert!(state.history.can_undo());
        assert!(
            state.design.solve().is_ok(),
            "the retargeted design must still solve"
        );
    }

    #[test]
    fn apply_pending_retarget_combines_a_material_change_into_one_undo_step() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        let (_, proposal) = retarget_view(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Shift,
            &[],
        );
        let proposal = proposal.expect("shift mode never fails");

        let mut state = fresh_state_with(design);
        let generation = state.generation.load(AtomicOrdering::Relaxed);
        let material_change = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        apply_pending_retarget(&mut state, (proposal, generation), Some(material_change))
            .unwrap_or_else(|_| panic!("a fresh, matching-generation proposal must apply"));
        assert_eq!(state.design.material.name.as_deref(), Some("Quartz"));

        // ONE undo step reverts both the angle retarget and the material change.
        assert!(state.undo().unwrap());
        assert_eq!(state.design.material.name.as_deref(), Some("Diamond"));
        assert!(!state.history.can_undo());
    }

    #[test]
    fn apply_pending_retarget_refuses_a_stale_generation() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        let target = resolve_target_material(&design, &custom, quartz_index, "");
        let (_, proposal) = retarget_view(
            &design,
            &target,
            CrownShift::default(),
            RetargetMode::Shift,
            &[],
        );
        let proposal = proposal.expect("shift mode never fails");

        let mut state = fresh_state_with(design);
        let stale_generation = state.generation.load(AtomicOrdering::Relaxed) + 1;
        let result = apply_pending_retarget(&mut state, (proposal, stale_generation), None);
        assert!(matches!(result, Err(RetargetApplyError::Stale)));
        assert!(
            !state.history.can_undo(),
            "nothing should have been applied"
        );
    }

    // --- initial_target_index ---

    #[test]
    fn initial_target_index_finds_the_designs_own_named_material() {
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let material = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        let quartz_index = design_material_index_from_name(Some("Quartz"), &options);
        assert_eq!(initial_target_index(&material, &options), quartz_index);
    }

    #[test]
    fn initial_target_index_seeds_the_custom_ri_sentinel_for_a_nameless_override() {
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let material = MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(1.6),
        };
        assert_eq!(
            initial_target_index(&material, &options),
            i32::try_from(options.len()).unwrap() - 1
        );
        assert_eq!(
            options.last().map(String::as_str),
            Some("Custom RI\u{2026}")
        );
    }

    #[test]
    fn initial_target_index_falls_back_to_none_for_a_plain_nameless_material() {
        let custom: [GemMaterial; 0] = [];
        let options = design_material_options(&custom);
        let material = MaterialSelection::default();
        assert_eq!(initial_target_index(&material, &options), 0);
    }

    // --- target_material_selection ---

    #[test]
    fn target_material_selection_falls_back_to_the_current_material_on_unparseable_ri() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let selection = target_material_selection(&design, &custom, 0, "not-a-number");
        assert_eq!(selection, design.material);
    }

    // --- resolve_target_selection ---

    #[test]
    fn resolve_target_selection_reports_an_unparseable_ri_override_instead_of_falling_back() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let err = resolve_target_selection(&design, &custom, 0, "1,74")
            .expect_err("a comma-separated RI override must not silently parse");
        assert!(
            err.contains("1,74"),
            "error should name the bad text: {err}"
        );
    }

    #[test]
    fn resolve_target_selection_reports_a_non_positive_ri_override() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let err = resolve_target_selection(&design, &custom, 0, "0.5")
            .expect_err("an RI override of 0.5 is not physically valid");
        assert!(err.contains("greater than 1.0"), "got: {err}");
    }

    #[test]
    fn resolve_target_selection_accepts_a_valid_ri_override() {
        let design = diamond_design();
        let custom: [GemMaterial; 0] = [];
        let selection = resolve_target_selection(&design, &custom, 0, "1.74")
            .expect("a well-formed RI override must parse");
        assert_eq!(selection.refractive_index_override, Some(1.74));
    }

    // --- target_display_name ---

    #[test]
    fn target_display_name_shows_none_for_a_nameless_uncustomized_selection() {
        let selection = MaterialSelection::default();
        assert_eq!(target_display_name(&selection), "(none)");
    }

    #[test]
    fn target_display_name_shows_custom_ri_for_a_nameless_override() {
        let selection = MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(1.74),
        };
        assert_eq!(target_display_name(&selection), "Custom RI");
    }

    #[test]
    fn target_display_name_shows_the_picked_material_name() {
        let selection = MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
        };
        assert_eq!(target_display_name(&selection), "Quartz");
    }

    // --- diff_row_view / diff_rows_from_deltas ---

    fn tier_for_diff(name: &str, angle_deg: f64) -> ConstraintTier {
        ConstraintTier {
            angle_deg,
            name: name.to_string(),
            indices: vec![0.0],
            constraint: MeetConstraint::ScaleReference(0.5),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    #[test]
    fn diff_row_view_labels_an_unchanged_position_same() {
        let before = vec![tier_for_diff("Table", 0.0)];
        let after = before.clone();
        let deltas = diff_tiers(&before, None, &after, None);
        let row = diff_row_view(&deltas[0]);
        assert_eq!(row.status_label, "Same");
        assert_eq!(row.mast_delta, "-");
    }

    #[test]
    fn diff_row_view_labels_a_moved_angle_changed() {
        let before = vec![tier_for_diff("Star", 15.0)];
        let mut after = before.clone();
        after[0].angle_deg = 16.0;
        let deltas = diff_tiers(&before, None, &after, None);
        let row = diff_row_view(&deltas[0]);
        assert_eq!(row.status_label, "Changed");
        assert_eq!(row.old_angle, "15.00\u{b0}");
        assert_eq!(row.new_angle, "16.00\u{b0}");
    }

    #[test]
    fn diff_row_view_reports_a_signed_mast_delta_when_both_masts_are_known() {
        use indicatrix::geometry::meet_solver::SolveStrategy;
        let before = vec![tier_for_diff("Table", 0.0)];
        let after = before.clone();
        let before_solved = vec![SolvedTier {
            mast: 0.5,
            strategy: SolveStrategy::ScaleReference,
            detail: String::new(),
        }];
        let after_solved = vec![SolvedTier {
            mast: 0.55,
            strategy: SolveStrategy::ScaleReference,
            detail: String::new(),
        }];
        let deltas = diff_tiers(&before, Some(&before_solved), &after, Some(&after_solved));
        let row = diff_row_view(&deltas[0]);
        assert_eq!(row.mast_delta, "+0.0500");
    }

    #[test]
    fn diff_row_view_labels_added_and_removed_positions() {
        let before = vec![tier_for_diff("Table", 0.0)];
        let after = vec![tier_for_diff("Table", 0.0), tier_for_diff("Star", 15.0)];
        let deltas = diff_tiers(&before, None, &after, None);
        assert_eq!(diff_row_view(&deltas[0]).status_label, "Same");
        assert_eq!(diff_row_view(&deltas[1]).status_label, "Added");

        let deltas_reverse = diff_tiers(&after, None, &before, None);
        assert_eq!(diff_row_view(&deltas_reverse[1]).status_label, "Removed");
    }

    #[test]
    fn diff_rows_from_deltas_carries_the_tier_index_and_name_through() {
        let before = vec![tier_for_diff("Table", 0.0), tier_for_diff("Star", 15.0)];
        let mut after = before.clone();
        after[1].angle_deg = 16.0;
        let deltas = diff_tiers(&before, None, &after, None);
        let rows = diff_rows_from_deltas(&deltas);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1].tier_index, 1);
        assert_eq!(rows[1].name.as_str(), "Star");
        assert_eq!(rows[1].risk_label.as_str(), "Changed");
    }
}
