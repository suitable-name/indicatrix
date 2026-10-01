//! "Snapshot Design" / "Compare to Snapshot": the toolkit-free rows of the tier-by-tier
//! comparison between a snapshot taken earlier and the design now.
//!
//! Moved from the desktop's `retarget_actions::snapshot`; the desktop maps
//! [`DiffRowView`] to its Slint row type and the web app to its own. The diff itself is
//! `indicatrix_cut_core::diff_tiers`.

use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, TierDelta, diff_tiers};

/// A tier position where NEITHER the angle, the indices, nor the mast (beyond this
/// tolerance) moved is shown as "Same" rather than "Changed", so a long, mostly-untouched
/// schedule reads at a glance.
pub const MAST_DIFF_TOLERANCE: f64 = 1e-4;

/// A design plus its solved masts, captured on demand ("Snapshot Design") and diffed
/// against the design later ("Compare to Snapshot").
#[derive(Clone)]
pub struct DesignSnapshot {
    /// The snapshotted design.
    pub design: Design,
    /// The design's own solve at snapshot time, when it had one -- `None` for a design
    /// that does not currently solve, in which case the diff simply reports no mast
    /// figures for that side.
    pub solved: Option<Vec<SolvedTier>>,
    /// The design's own label at snapshot time, shown in the compare header so two
    /// different snapshots across one session are never mistaken for each other.
    pub label: String,
}

/// One [`TierDelta`] as a pure, toolkit-free view -- see [`diff_row_view`] for how each
/// field is derived.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffRowView {
    /// The tier position the row is about.
    pub tier_index: usize,
    /// The tier's name.
    pub name: String,
    /// Its angle in the snapshot (`"-"` when the tier did not exist), two decimals.
    pub old_angle: String,
    /// Its angle now (`"-"` when the tier no longer exists), two decimals.
    pub new_angle: String,
    /// The signed mast difference, `"-"` when either side has no mast.
    pub mast_delta: String,
    /// `"Added"`, `"Removed"`, `"Changed"` or `"Same"`.
    pub status_label: &'static str,
    /// The status badge colour, `(r, g, b)`.
    pub status_rgb: (u8, u8, u8),
}

/// One [`TierDelta`]'s badge label and RGB color -- chosen HERE, once, so the label and the
/// color can never drift apart. The RGB values match the desktop theme's
/// `accent-sky` / `accent-ruby` / `accent-amber` / `accent-emerald`.
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

/// The view of one [`TierDelta`]: angles as `"41.20\u{b0}"` (or `"-"`), the mast difference
/// as a signed figure, and the change-status badge.
#[must_use]
pub fn diff_row_view(delta: &TierDelta) -> DiffRowView {
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

/// The comparison rows of `snapshot` against the design now (`design` with its solve
/// `current_solved`, when it has one), one per tier position, in position order.
#[must_use]
pub fn compare_rows(
    snapshot: &DesignSnapshot,
    design: &Design,
    current_solved: Option<&[SolvedTier]>,
) -> Vec<DiffRowView> {
    diff_tiers(
        &snapshot.design.tiers,
        snapshot.solved.as_deref(),
        &design.tiers,
        current_solved,
    )
    .iter()
    .map(diff_row_view)
    .collect()
}

/// The compare header: `"snapshot label" vs. current ("current label")`.
#[must_use]
pub fn compare_label(snapshot_label: &str, current_label: &str) -> String {
    format!("\"{snapshot_label}\" vs. current (\"{current_label}\")")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorSession;

    #[test]
    fn an_untouched_design_compares_all_same_and_a_moved_angle_reads_changed() {
        let session = EditorSession::from_template(
            indicatrix_cut_core::FreshDesignSpec {
                gear_teeth: 96,
                symmetry_order: 8,
                mirror: true,
                material: indicatrix_cut_core::MaterialSelection::none(),
                preform: indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
            },
            1,
        );
        let solved = session.design.solve().ok();
        let snapshot = DesignSnapshot {
            design: session.design.clone(),
            solved: solved.clone(),
            label: "before".to_string(),
        };
        let same = compare_rows(&snapshot, &session.design, solved.as_deref());
        assert_eq!(same.len(), session.design.tiers.len());
        assert!(same.iter().all(|row| row.status_label == "Same"));

        let mut moved = session.design;
        moved.tiers[0].angle_deg += 1.0;
        let rows = compare_rows(&snapshot, &moved, None);
        assert_eq!(rows[0].status_label, "Changed");
        assert_eq!(rows[0].mast_delta, "-");
        assert_eq!(
            compare_label("a", "b"),
            "\"a\" vs. current (\"b\")".to_string()
        );
    }
}
