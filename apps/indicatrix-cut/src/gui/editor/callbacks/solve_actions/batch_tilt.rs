//! "Compute Tilt Curves" for the design currently open in the editor.

use super::RunProvenance;
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        batch::tilt,
        editor::{
            auto_solve,
            material_lookup::{EditorMaterialLookup, resolved_gem_material},
            state::EditorState,
        },
        show_toast,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::Design;
use indicatrix_vault::{db::sqlite::Database, model::tilt_curves::TiltPerformanceCurves};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering as AtomicOrdering},
    },
    thread,
};

/// Computes the full 4-axis tilt-performance sweep for the
/// design currently open in the editor -- not whatever a catalogue row currently
/// stores -- via `gui::batch::tilt::tilt_curves_for_planes`, entirely off the UI
/// thread (~1.36s, see that function's own doc comment). Closes the crate's last
/// 3 clippy warnings: `tilt_curves_for_planes`/`save_tilt_curves_for_entry` were
/// real and tested but had no production caller before this.
///
/// `run_epoch` (the same superseded-run guard [`super::deep_solve_run::setup_deep_solve_callback`]/
/// [`super::optimize_run::setup_optimize_callback`] use) is what keeps a superseded run from ever
/// overwriting a fresher result: a second click bumps it, and the FIRST run's
/// eventual completion becomes a no-op the moment [`apply_batch_tilt_result`]
/// checks back in. Body split into [`begin_batch_tilt_for_open_design`]/
/// [`run_batch_tilt_for_open_design`]/[`apply_batch_tilt_result`] purely to keep
/// each piece under clippy's function-length lint.
pub(in crate::gui::editor) fn setup_batch_tilt_for_open_design_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let db = Arc::clone(db);
    let ui_weak = ui.as_weak();
    let run_epoch = Arc::new(AtomicU64::new(0));
    ui.global::<EditorModel>()
        .on_compute_tilt_curves_for_open_design(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            begin_batch_tilt_for_open_design(&ui, &state, &render_ctx, &db, &run_epoch);
        });
}

/// Everything one "Compute Tilt Curves" run needs, snapshotted once on the UI
/// thread by [`begin_batch_tilt_for_open_design`] and carried unchanged through
/// [`run_batch_tilt_for_open_design`]/[`apply_batch_tilt_result`] -- bundled into
/// one struct purely to keep those two functions' signatures under clippy's
/// argument-count lint.
struct BatchTiltRun {
    /// A snapshot of `state.design` at dispatch time -- solved by
    /// [`run_batch_tilt_for_open_design`], off the UI thread.
    design: Design,
    /// A snapshot of `RenderContext::custom_materials` at dispatch time.
    custom_materials: Vec<GemMaterial>,
    /// `state.source_entry_id` at dispatch time -- `None` means this design has
    /// never been saved to the catalogue, so a finished sweep has nothing to
    /// save against.
    source_entry_id: Option<i64>,
    /// Whether `state.design` has changed since this run started -- see
    /// [`RunProvenance::is_stale`].
    provenance: RunProvenance,
    /// The shared superseded-run counter -- see
    /// [`setup_batch_tilt_for_open_design_callback`]'s own doc comment.
    run_epoch_done: Arc<AtomicU64>,
    /// This run's own epoch, captured at dispatch time.
    this_run: u64,
    /// The shared design database -- only ever reached for a save, and only once
    /// this run is confirmed current and non-stale.
    db: Arc<Mutex<Database>>,
    /// The window this run reports back to, once finished.
    ui_weak: slint::Weak<MainWindow>,
}

impl BatchTiltRun {
    /// Whether a newer "Compute Tilt Curves" click has started since this run
    /// was dispatched -- `false` once that happens, meaning this run's result
    /// must never touch `EditorModel`/the database at all.
    fn is_current(&self) -> bool {
        self.run_epoch_done.load(AtomicOrdering::Relaxed) == self.this_run
    }
}

/// `on_compute_tilt_curves_for_open_design`'s actual body -- snapshots
/// `state.design` (and everything else [`BatchTiltRun`] bundles) on the UI
/// thread, cheap enough to do inline, then hands the real work to
/// [`run_batch_tilt_for_open_design`] on a background thread -- matching
/// `gui::tilt::tilt_profile::spawn_tilt_profile_sweep`'s own `thread::spawn` +
/// `upgrade_in_event_loop` shape rather than inventing a second pattern.
fn begin_batch_tilt_for_open_design(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    db: &Arc<Mutex<Database>>,
    run_epoch: &Arc<AtomicU64>,
) {
    let st = state.borrow();
    let design = st.design.clone();
    let source_entry_id = st.source_entry_id;
    let provenance = RunProvenance::capture(&st);
    drop(st);

    let custom_materials = render_ctx
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();

    let this_run = run_epoch.fetch_add(1, AtomicOrdering::Relaxed) + 1;
    let run = BatchTiltRun {
        design,
        custom_materials,
        source_entry_id,
        provenance,
        run_epoch_done: Arc::clone(run_epoch),
        this_run,
        db: Arc::clone(db),
        ui_weak: ui.as_weak(),
    };
    show_toast(
        ui,
        "Computing tilt-performance curves for this design...",
        "info",
    );
    thread::spawn(move || run_batch_tilt_for_open_design(run));
}

/// The background half of [`begin_batch_tilt_for_open_design`]: solves
/// `run.design` (the one real, otherwise-unavoidable ~1.36s+ solve cost of this
/// action), builds its planes/material through
/// [`EditorMaterialLookup`]/[`resolved_gem_material`] (the same precedence
/// `bridge::render_thread::context::resolve_material` uses), runs the
/// local-only sweep (`worker: None` -- this action has no worker-settings
/// parameter to dispatch remotely against), and hands the result to
/// [`apply_batch_tilt_result`] back on the UI thread. A `MissingAnchor` solve
/// failure is reported the same superseded-run-aware way a successful sweep is,
/// rather than silently dropped.
fn run_batch_tilt_for_open_design(run: BatchTiltRun) {
    let solved = match run.design.solve() {
        Ok(solved) => solved,
        Err(e) => {
            let ui_weak = run.ui_weak.clone();
            let run_epoch_done = Arc::clone(&run.run_epoch_done);
            let this_run = run.this_run;
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                if run_epoch_done.load(AtomicOrdering::Relaxed) == this_run {
                    show_toast(&ui, &format!("Cannot compute tilt curves: {e}"), "error");
                }
            });
            return;
        }
    };
    let planes = auto_solve::design_to_gpu_planes_from_solved(&run.design, &solved);
    let lookup = EditorMaterialLookup::new(&run.custom_materials);
    let material = resolved_gem_material(&run.design.material, &lookup);
    let cancel = AtomicBool::new(false);
    let curves = tilt::tilt_curves_for_planes(&planes, &material, None, &cancel);

    let ui_weak = run.ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        apply_batch_tilt_result(&ui, curves, &run);
    });
}

/// The UI-thread completion half of [`run_batch_tilt_for_open_design`]: drops a
/// superseded run's result outright ([`BatchTiltRun::is_current`] -- see
/// [`setup_batch_tilt_for_open_design_callback`]'s own doc comment), reports a
/// sweep failure, and -- for a real result from the CURRENT run -- either saves
/// it via `gui::batch::tilt::save_tilt_curves_for_entry` (when
/// `run.source_entry_id` names a real catalogue row and the design has not
/// changed since this run started) or explains why it wasn't saved. A design
/// that changed mid-run is deliberately shown but never persisted: the curves
/// are honest for the design AS IT WAS when the run started, and saving them
/// now would silently attach a stale sweep to whatever the catalogue row
/// currently is.
fn apply_batch_tilt_result(
    ui: &MainWindow,
    curves: Option<TiltPerformanceCurves>,
    run: &BatchTiltRun,
) {
    if !run.is_current() {
        return;
    }
    let Some(curves) = curves else {
        show_toast(
            ui,
            "Tilt-curve computation failed for this design.",
            "error",
        );
        return;
    };
    if run.provenance.is_stale() {
        // Wording consistent with every other generation guard in this file --
        // see `setup_optimize_apply_callback`'s matching fix.
        // "Result discarded" wording and amber class as every other generation
        // guard in this file -- see `setup_optimize_apply_callback`'s matching
        // fix. The curves themselves are still shown (never saved, per this
        // function's own doc comment), so "discarded" here means specifically
        // "not saved," which the sentence's own second half already says.
        show_toast(
            ui,
            "Result discarded: the design changed while computing tilt curves. Re-run to \
             save an up-to-date result.",
            "warning",
        );
        return;
    }
    match run.source_entry_id {
        Some(entry_id) => {
            if tilt::save_tilt_curves_for_entry(&run.db, entry_id, &curves) {
                show_toast(
                    ui,
                    "Saved tilt-performance curves for this design.",
                    "success",
                );
            } else {
                show_toast(
                    ui,
                    "Could not save tilt-performance curves for this design.",
                    "error",
                );
            }
        }
        None => show_toast(
            ui,
            "Computed tilt-performance curves -- save this design to the \
             catalogue to keep them.",
            "info",
        ),
    }
}
