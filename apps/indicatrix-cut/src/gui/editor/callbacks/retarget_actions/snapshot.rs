//! "Snapshot Design"/"Compare to Snapshot": a separate design-comparison feature
//! that also lives behind this dialog's own shell (`RetargetModel.compare_open`).

use crate::{
    EditorModel, MainWindow, RetargetModel, RetargetRowItem,
    gui::{
        editor::{
            native_io,
            state::{EditorState, design_label_text},
            view,
        },
        show_toast,
    },
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, TierDelta, diff_tiers};
use slint::{Color, ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc, sync::Arc};

thread_local! {
    /// A design plus its solved masts,
    /// captured on demand by [`setup_snapshot_callbacks`]'s `snapshot_design`
    /// handler and read back by its `compare_to_snapshot` handler. A module-local
    /// `thread_local!` for the same reason [`super::RETARGET_ASYNC`] is one --
    /// rather than a new `EditorState` field, and Slint's single-threaded event
    /// loop makes this sound.
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
pub(super) struct DiffRowView {
    pub(super) tier_index: usize,
    pub(super) name: String,
    pub(super) old_angle: String,
    pub(super) new_angle: String,
    pub(super) mast_delta: String,
    pub(super) status_label: &'static str,
    pub(super) status_rgb: (u8, u8, u8),
}

/// A tier position where NEITHER the angle, the indices, nor the mast (beyond
/// [`MAST_DIFF_TOLERANCE`]) moved -- shown as "Same" rather than "Changed" so a
/// long, mostly-untouched schedule reads at a glance.
const MAST_DIFF_TOLERANCE: f64 = 1e-4;

/// One [`TierDelta`]'s badge label and RGB color -- chosen HERE, once, matching
/// `proposal_view::risk_label_and_rgb`'s own "choose it in exactly one place"
/// reasoning, so the label and the color can never drift apart. The RGB values
/// match `Theme.accent-sky`/`accent-ruby`/`accent-amber`/`accent-emerald` (this
/// module has no access to the `Theme` global from plain Rust, so the numbers are
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

pub(super) fn diff_row_view(delta: &TierDelta) -> DiffRowView {
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
            native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
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
            native_io::resolve_solved_then(&ui, design, move |ui, design, solved| {
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
