//! "Snapshot Design" / "Compare to Snapshot": the toolkit-free rows of the tier-by-tier
//! comparison between a snapshot taken earlier and the design now.
//!
//! Moved from the desktop's `retarget_actions::snapshot`; the desktop maps
//! [`DiffRowView`] to its Slint row type and the web app to its own. The diff itself is
//! `indicatrix_cut_core::diff_tiers`.

use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{ConcaveTierDelta, Design, TierDelta, diff_concave_tiers, diff_tiers};

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
    /// `true` only for the "Before retarget" snapshot a Retarget Apply holds; the Optimize
    /// comparison offers "Original" only for such a snapshot. Never set for a snapshot the
    /// cutter took themselves, whatever its label says.
    pub from_retarget_apply: bool,
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
    /// The status badge color, `(r, g, b)`.
    pub status_rgb: (u8, u8, u8),
    /// A concave tier's tool badge (`"CYL"`, or `"CYL -> CON"` when the tool itself
    /// changed); empty for a flat tier. A concave row's [`Self::tier_index`] is its
    /// position in `design.concave_tiers`, and its angles are the facet's own.
    pub badge: String,
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

/// An angle as the compare window shows it: `"41.20\u{b0}"`, or `"-"` for a side that has
/// no such tier. Always the magnitude, because a person reads positive angles and the
/// block (crown, pavilion, girdle) names the side; the stored sign stays inside the file.
fn angle_text(angle_deg: Option<f64>) -> String {
    angle_deg.map_or_else(|| "-".to_string(), |v| format!("{:.2}\u{b0}", v.abs()))
}

/// The view of one [`TierDelta`]: angles as `"41.20\u{b0}"` (or `"-"`, always positive),
/// the mast difference as a signed figure, and the change-status badge.
#[must_use]
pub fn diff_row_view(delta: &TierDelta) -> DiffRowView {
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
        badge: String::new(),
    }
}

/// The view of one [`ConcaveTierDelta`], flagged with the tool's badge.
///
/// A concave tier has no mast, so `mast_delta` is `"-"`. The status is `"Added"`,
/// `"Removed"` or `"Changed"`: [`diff_concave_tiers`] only reports positions that
/// differ, and a difference in ANY field (tool, theta, displacement, diameter,
/// angle, motion, indices) is a change.
#[must_use]
pub fn concave_diff_row_view(delta: &ConcaveTierDelta) -> DiffRowView {
    let (status_label, status_rgb) = match (&delta.before, &delta.after) {
        (None, _) => ("Added", (0x38, 0xbd, 0xf8)),
        (_, None) => ("Removed", (0xf4, 0x3f, 0x5e)),
        _ => ("Changed", (0xf5, 0x9e, 0x0b)),
    };
    let badge = match (&delta.before, &delta.after) {
        (Some(before), Some(after)) if before.tool != after.tool => {
            format!("{} -> {}", before.tool.code(), after.tool.code())
        }
        (Some(t), _) | (None, Some(t)) => t.tool.code().to_string(),
        (None, None) => String::new(),
    };
    DiffRowView {
        tier_index: delta.position,
        name: delta
            .after
            .as_ref()
            .or(delta.before.as_ref())
            .map_or_else(String::new, |t| t.name.clone()),
        old_angle: angle_text(delta.before.as_ref().map(|t| t.angle_deg)),
        new_angle: angle_text(delta.after.as_ref().map(|t| t.angle_deg)),
        mast_delta: "-".to_string(),
        status_label,
        status_rgb,
        badge,
    }
}

/// The comparison rows of `snapshot` against the design now.
///
/// `design` comes with its solve `current_solved`, when it has one. There is one row per
/// flat tier position, in position order, then one per concave tier position that
/// differs ([`concave_diff_row_view`]).
///
/// The concave rows are what makes a compare between two snapshots that differ only in
/// a concave tier show that difference instead of "no change".
#[must_use]
pub fn compare_rows(
    snapshot: &DesignSnapshot,
    design: &Design,
    current_solved: Option<&[SolvedTier]>,
) -> Vec<DiffRowView> {
    let flat = diff_tiers(
        &snapshot.design.tiers,
        snapshot.solved.as_deref(),
        &design.tiers,
        current_solved,
    );
    let concave = diff_concave_tiers(&snapshot.design.concave_tiers, &design.concave_tiers);
    flat.iter()
        .map(diff_row_view)
        .chain(concave.iter().map(concave_diff_row_view))
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
            from_retarget_apply: false,
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

    #[test]
    fn compare_rows_include_concave_deltas() {
        let design = Design::concave_fixture();
        let solved = design.solve().ok();
        let snapshot = DesignSnapshot {
            design: design.clone(),
            solved: solved.clone(),
            label: "before".to_string(),
            from_retarget_apply: false,
        };
        let same = compare_rows(&snapshot, &design, solved.as_deref());
        assert_eq!(same.len(), design.tiers.len(), "no concave row when equal");
        assert!(same.iter().all(|row| row.badge.is_empty()));

        let mut edited = design;
        edited.concave_tiers[0].tool = indicatrix_cut_core::design::ConcaveTool::Cone;
        edited.concave_tiers[0].tool_angle_deg = Some(60.0);
        edited.concave_tiers.pop();
        let rows = compare_rows(&snapshot, &edited, solved.as_deref());
        let concave: Vec<&DiffRowView> = rows.iter().filter(|r| !r.badge.is_empty()).collect();
        assert_eq!(concave.len(), 2);
        assert_eq!(concave[0].status_label, "Changed");
        assert_eq!(concave[0].badge, "CYL -> CON");
        assert_eq!(concave[0].name, "Groove");
        assert_eq!(concave[1].status_label, "Removed");
        assert_eq!(concave[1].badge, "SPH");
        assert_eq!(concave[1].new_angle, "-");
    }

    #[test]
    fn compare_angles_read_as_positive_numbers_and_a_missing_side_stays_a_dash() {
        let delta = TierDelta {
            index: 3,
            name: "P1".to_string(),
            angle_before: Some(-40.3),
            angle_after: Some(-41.0),
            indices_before: None,
            indices_after: None,
            mast_before: None,
            mast_after: None,
        };
        let row = diff_row_view(&delta);
        assert_eq!(row.old_angle, "40.30\u{b0}");
        assert_eq!(row.new_angle, "41.00\u{b0}");

        let added = TierDelta {
            angle_before: None,
            angle_after: Some(-39.0),
            ..delta.clone()
        };
        let row = diff_row_view(&added);
        assert_eq!(row.old_angle, "-");
        assert_eq!(row.new_angle, "39.00\u{b0}");

        let crown = TierDelta {
            angle_before: Some(34.5),
            angle_after: Some(35.0),
            ..delta
        };
        let row = diff_row_view(&crown);
        assert_eq!(row.old_angle, "34.50\u{b0}");
        assert_eq!(row.new_angle, "35.00\u{b0}");
    }

    #[test]
    fn a_concave_compare_row_shows_the_facet_angle_as_a_magnitude() {
        let design = Design::concave_fixture();
        let snapshot = DesignSnapshot {
            design: design.clone(),
            solved: None,
            label: "before".to_string(),
            from_retarget_apply: false,
        };
        let mut edited = design;
        edited.concave_tiers[0].angle_deg = -35.0;
        let rows = compare_rows(&snapshot, &edited, None);
        let concave: Vec<&DiffRowView> = rows.iter().filter(|r| !r.badge.is_empty()).collect();
        assert_eq!(
            concave[0].old_angle, "42.00\u{b0}",
            "the fixture's Groove is -42"
        );
        assert_eq!(concave[0].new_angle, "35.00\u{b0}");
    }
}
