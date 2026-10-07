//! "Snapshot Design"/"Compare to Snapshot": a separate design-comparison feature
//! that also lives behind this dialog's own shell (`RetargetModel.compare_open`).

use crate::{
    EditorModel, MainWindow, RetargetModel, RetargetRowItem, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            compare::snapshot_summary,
            native_io,
            state::{EditorState, design_label_text},
            view,
        },
        show_toast,
        tutorial_events::raise,
    },
};
use indicatrix::{geometry::meet_solver::SolvedTier, optics::LightingPreset};
use indicatrix_cut_core::{Design, TierDelta, diff_tiers};
use indicatrix_editor::{
    guide::solving_events::{COMPARE_OPENED, SNAPSHOT_TAKEN},
    snapshot::DesignSnapshot,
};
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

thread_local! {
    /// A design plus its solved masts,
    /// captured on demand by [`setup_snapshot_callbacks`]'s `snapshot_design`
    /// handler and read back by its `compare_to_snapshot` handler. A module-local
    /// `thread_local!` for the same reason [`super::RETARGET_ASYNC`] is one --
    /// rather than a new `EditorState` field, and Slint's single-threaded event
    /// loop makes this sound.
    static DESIGN_SNAPSHOT: RefCell<Option<DesignSnapshot>> = const { RefCell::new(None) };
}

/// The held snapshot's design and label, for the visual compare window's
/// "Compare visually…" entry (`gui::editor::compare`) -- `None` before the first
/// "Snapshot Design" of this session. A clone: the compare window only ever reads
/// it, and the snapshot itself stays exactly as taken.
#[must_use]
pub(in crate::gui::editor) fn snapshot_for_compare() -> Option<(Design, String)> {
    DESIGN_SNAPSHOT.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|snapshot| (snapshot.design.clone(), snapshot.label.clone()))
    })
}

/// Holds `design`, solved to `solved`, as the snapshot "Compare to Snapshot" and "Compare
/// visually..." measure against -- what Snapshot Design does for the open design, without its
/// toast. A guide's "Build this design" lesson holds the library design it rebuilds this way
/// while its last step is open, so the learner's stone can be compared with the original.
pub(in crate::gui::editor) fn hold_reference_snapshot(
    ui: &MainWindow,
    design: Design,
    solved: Vec<SolvedTier>,
    label: &str,
) {
    DESIGN_SNAPSHOT.with(|cell| {
        *cell.borrow_mut() = Some(DesignSnapshot {
            design,
            solved: Some(solved),
            label: label.to_string(),
            from_retarget_apply: false,
        });
    });
    ui.global::<EditorModel>().set_has_snapshot(true);
}

/// The label of the snapshot held on Retarget Apply, e.g. `Before retarget to Sapphire`.
/// Display only: whether a snapshot is the original is [`DesignSnapshot::from_retarget_apply`].
#[must_use]
pub(in crate::gui::editor) fn original_snapshot_label(target: &str) -> String {
    format!("Before retarget to {target}")
}

/// The held snapshot's design and label, only when it is the original kept by a Retarget
/// Apply -- `None` for no snapshot or one the cutter took themselves.
#[must_use]
pub(in crate::gui::editor) fn original_for_compare() -> Option<(Design, String)> {
    DESIGN_SNAPSHOT.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|snapshot| snapshot.from_retarget_apply)
            .map(|snapshot| (snapshot.design.clone(), snapshot.label.clone()))
    })
}

/// Holds the design as it was before a Retarget Apply as the reference snapshot, replacing any
/// earlier one (no database write). `solved` is the cached solve of that design when one is
/// at hand; without it the snapshot's mast diff simply has no old masts.
pub(in crate::gui::editor) fn hold_original_before_retarget(
    ui: &MainWindow,
    design: Design,
    solved: Option<Vec<SolvedTier>>,
    label: &str,
) {
    DESIGN_SNAPSHOT.with(|cell| {
        *cell.borrow_mut() = Some(DesignSnapshot {
            design,
            solved,
            label: label.to_string(),
            from_retarget_apply: true,
        });
    });
    ui.global::<EditorModel>().set_has_snapshot(true);
}

// The pure diff-row view (the status badge, the signed mast delta, the angle texts) moved
// to `indicatrix_editor::snapshot` (shared with the web snapshot dialog); re-exported at
// its old path, which `super::tests` exercises.
pub(super) use indicatrix_editor::snapshot::diff_row_view;

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
pub(super) fn diff_rows_from_deltas(deltas: &[TierDelta]) -> Vec<RetargetRowItem> {
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
            from_retarget_apply: false,
        });
    });
    // "Compare to Snapshot" is
    // gated on this in `editor_command_bar.slint`; a snapshot is never cleared
    // within a session, so this only ever goes `true`.
    ui.global::<EditorModel>().set_has_snapshot(true);
    show_toast(ui, &format!("Snapshot taken: \"{label}\"."), "success");
    raise(ui, SNAPSHOT_TAKEN);
}

/// [`setup_snapshot_callbacks`]'s `compare_to_snapshot` tail -- diffs `snapshot`
/// against `design`'s current state via [`indicatrix_cut_core::diff_tiers`] and
/// opens the Retarget dialog's own shell in its Compare mode
/// (`RetargetModel.compare_open`) -- see `retarget_dialog.slint`'s own `if
/// compare_open` branch. Shared by that callback's own cache-hit/cache-miss
/// paths, the same reasoning [`store_design_snapshot`] documents on itself.
///
/// The label carries a second line, in words, about how the stone differs optically
/// (`gui::editor::compare::snapshot_summary`): the table-up figures of both designs take a
/// few milliseconds, under the viewport's lighting.
fn show_compare_to_snapshot(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
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
    let custom = render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    let lighting =
        LightingPreset::from_index(ui.global::<ViewportModel>().get_selected_lighting_index());
    let optical = snapshot_summary(
        &snapshot.design,
        snapshot.solved.as_deref(),
        design,
        current_solved,
        &custom,
        lighting,
    );
    ui.global::<RetargetModel>()
        .set_compare_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>().set_compare_label(
        format!(
            "\"{}\" vs. current (\"{current_label}\")\n{optical}",
            snapshot.label
        )
        .into(),
    );
    ui.global::<RetargetModel>().set_compare_open(true);
    raise(ui, COMPARE_OPENED);
}

/// "Snapshot Design"/"Compare to Snapshot": registers both
/// halves of the design-comparison feature. The command bar
/// (`editor_command_bar.slint`) triggers `EditorModel.snapshot_design()` and
/// `compare_to_snapshot()`, and `gui::editor` registers this function at startup.
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
    render_ctx: &Arc<Mutex<RenderContext>>,
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
                // the shared cache is now generation-tagged -- only the
                // masts themselves matter here.
                let cached = solid_last_solved
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|(_, s)| s.len() == st.design.tiers.len())
                    .map(|(_, s)| s);
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
            native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
                store_design_snapshot(ui, &design, solved.ok(), &label);
            });
        });
    }

    {
        let state = Rc::clone(state);
        let render_ctx = Arc::clone(render_ctx);
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
                // the shared cache is now generation-tagged -- only the
                // masts themselves matter here.
                let cached = solid_last_solved
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .clone()
                    .filter(|(_, s)| s.len() == st.design.tiers.len())
                    .map(|(_, s)| s);
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
                    &render_ctx,
                    &snapshot,
                    &design,
                    &current_label,
                    Some(current_solved),
                );
                return;
            }
            let render_ctx = Arc::clone(&render_ctx);
            native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
                show_compare_to_snapshot(
                    ui,
                    &render_ctx,
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
