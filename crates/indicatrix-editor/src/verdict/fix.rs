//! Planning the fixes the verdict offers.
//!
//! [`plan_fix`] turns one [`FixAction`] into a [`FixPlan`]: ONE [`Edit`] (a batch where the
//! fix needs several), a sentence for the toast, and the tier to select afterwards. The edit
//! is applied by the caller through the editor session, so it is one undo step.
//!
//! Every plan is checked before it is returned, and a fix that cannot be made safely is
//! refused with a plain sentence rather than applied half-way:
//!
//! - **Snap to teeth** only rounds index positions; nothing needs checking.
//! - **Remove vanished facets** removes only what the finished stone does not show, and
//!   then checks the stone is unchanged (every other tier keeps the same number of live
//!   facets, the girdle and the table keep their size).
//! - **Move after** simulates the move: it must leave fewer tiers meeting a later tier,
//!   and the design must still solve.
//! - **Steepen pavilion** turns the facet about its girdle-side edge (the hinge of
//!   `design::hinge`, through `retarget::anchors`), so the girdle stays, and the result
//!   must pass the Retarget validity gate (`retarget::validity::judge`).
//! - **Add a table** tries the quick-add height first and otherwise scans heights above the
//!   girdle for one that keeps every crown facet, nearest a table of about 56 % of the
//!   stone's width.
//!
//! The planner solves the design (and candidates) itself, so a caller runs it on a worker
//! thread. It never mutates the design it is given.

use super::{FixAction, SAFE_MARGIN_DEG};
use crate::{
    manipulate::{dependents::clear_dependant_edits, tiers_meeting},
    retarget::{
        anchors::anchored_candidate_for,
        validity::{InvalidReason, StoneAnalysis, analyze, judge},
    },
};
use indicatrix::geometry::{
    meet_solver::MeetConstraint,
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh},
};
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, critical_angle_deg,
    design::hinge::FacetSide,
    manufacturability::check_cut_order,
    optics_hints::{MAX_RETARGET_ANGLE_DEG, is_horizontal_angle_deg},
};
use std::{collections::BTreeSet, ops::Range};

/// Shown when the design changed under the verdict (a stale tier position).
const STALE: &str = "The design changed since this was checked. Look at the verdict again.";

/// An index position this close to a whole tooth counts as on the tooth (the manufacturability
/// check's own tolerance).
const GEAR_EPS: f64 = 1e-9;

/// The mast the tier table's quick-add gives a table. The same value, so "Add a table" here
/// and the quick-add button agree on a design where the quick-add height works.
pub const QUICK_ADD_TABLE_MAST: f64 = 0.32;

/// The table width, as a percentage of the stone's width, the height scan aims for.
const TARGET_TABLE_PERCENT: f64 = 56.0;

/// A scanned table narrower than this (percent of the stone's width) is not offered.
const MIN_TABLE_PERCENT: f64 = 20.0;

/// A scanned table wider than this (percent of the stone's width) is not offered.
const MAX_TABLE_PERCENT: f64 = 90.0;

/// How many heights the table scan tries between the girdle and the top of the stone.
const TABLE_SCAN_STEPS: usize = 16;

/// What applying a fix does: the edit, the sentence for the toast, and the tier to select.
#[derive(Debug, Clone, PartialEq)]
pub struct FixPlan {
    /// The one edit to apply (a batch where the fix needs several steps).
    pub edit: Edit,
    /// What changed, as one sentence.
    pub message: String,
    /// The tier to select afterwards, as a position in the design AFTER the edit.
    pub select: Option<usize>,
}

/// The angle a steepened pavilion tier gets for a material of index `n_d`: the critical angle
/// plus [`SAFE_MARGIN_DEG`], rounded UP to 0.01 degree so the margin is never a hair short.
#[must_use]
pub fn steep_target_deg(n_d: f64) -> f64 {
    ((critical_angle_deg(n_d) + SAFE_MARGIN_DEG) * 100.0).ceil() / 100.0
}

/// Plans `action` against `design` for a material of refractive index `n_d` (used by the
/// windowing fix only).
///
/// # Errors
///
/// A plain sentence saying why the fix cannot be made safely (the design changed, a relation
/// drives the angle, the result would break the stone, ...). Nothing is changed then.
pub fn plan_fix(design: &Design, action: &FixAction, n_d: f64) -> Result<FixPlan, String> {
    match action {
        FixAction::SnapToTeeth { tier } => snap_to_teeth(design, *tier),
        FixAction::RemoveVanished { tier } => remove_vanished(design, *tier),
        FixAction::MoveAfter { tier, after } => move_after(design, *tier, *after),
        FixAction::SteepenPavilion { tier } => steepen_pavilion(design, *tier, n_d),
        FixAction::AddTable => add_table(design),
    }
}

/// The name a person reads for the tier: its own name, or its standard code for an unnamed
/// or old-style one.
fn tier_label(design: &Design, tier: usize) -> String {
    // What the tier table calls it: the tier's own name, or its standard code when the name
    // is empty or old-style (`3`, `A`). `Tier N` only when there is no such tier.
    crate::retarget::plan::tier_display_names(design)
        .into_iter()
        .nth(tier)
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| format!("Tier {}", tier + 1))
}

const fn plural(count: usize) -> &'static str {
    if count == 1 { "" } else { "s" }
}

/// An [`InvalidReason`] as a sentence about the CURRENT design (the gate's own wording speaks
/// of a "retargeted" design).
fn invalid_text(reason: &InvalidReason) -> String {
    match reason {
        InvalidReason::DoesNotSolve(why) => {
            format!("The design does not solve ({}).", why.trim_end_matches('.'))
        }
        InvalidReason::NotClosed => "The facets do not enclose a stone.".to_string(),
        other => other.message(),
    }
}

/// The set of plane indices whose face reaches the surface of `mesh`.
pub(super) fn live_planes(mesh: &SolidMesh) -> BTreeSet<usize> {
    mesh.rings
        .iter()
        .filter(|(_, ring)| ring.len() >= 3)
        .map(|&(plane, _)| plane)
        .collect()
}

/// Which index entries of tier `tier` have a facet that is cut away, when that can be told.
///
/// `ranges` are the tier plane ranges ([`indicatrix_cut_core::design::hinge::tier_plane_ranges`])
/// and `live` the planes the stone shows. Entry `j` of the tier is plane `range.start + j`
/// only when the tier contributes exactly one plane per index entry (a duplicated plane makes
/// the ranges shorter than the list), so anything else is `None`, and so is a tier with
/// nothing cut away or everything cut away (the whole tier goes then, not entries).
pub(super) fn vanished_positions_in(
    design: &Design,
    ranges: &[Range<usize>],
    live: &BTreeSet<usize>,
    tier: usize,
) -> Option<Vec<usize>> {
    let tier_ref = design.tiers.get(tier)?;
    let range = ranges.get(tier)?;
    if tier_ref.indices.is_empty() || range.len() != tier_ref.indices.len() {
        return None;
    }
    let gone: Vec<usize> = range
        .clone()
        .enumerate()
        .filter(|(_, plane)| !live.contains(plane))
        .map(|(position, _)| position)
        .collect();
    (!gone.is_empty() && gone.len() < range.len()).then_some(gone)
}

/// The tier's index list with `positions` taken out, as one `Edit::SetIndices`. A detached
/// mark on a removed position goes with it.
pub(super) fn partial_removal_edit(design: &Design, tier: usize, positions: &[usize]) -> Edit {
    let source = &design.tiers[tier];
    let mut removed = Vec::new();
    let mut kept = Vec::new();
    for (position, &value) in source.indices.iter().enumerate() {
        if positions.contains(&position) {
            removed.push(value);
        } else {
            kept.push(value);
        }
    }
    let detached = source
        .detached
        .iter()
        .copied()
        .filter(|mark| !removed.contains(mark) || kept.contains(mark))
        .collect();
    Edit::SetIndices {
        index: tier,
        indices: kept,
        detached,
    }
}

/// `Edit::RemoveTier`, with the other tiers' references to its name cleared first (one batch)
/// so no tier is left meeting a name nothing bears -- what the tier table's Remove does after
/// the cutter confirms.
pub(super) fn remove_tier_edit(design: &Design, tier: usize) -> Edit {
    if tiers_meeting(design, tier).is_empty() {
        Edit::RemoveTier { index: tier }
    } else {
        let mut steps = clear_dependant_edits(design, tier);
        steps.push(Edit::RemoveTier { index: tier });
        Edit::Batch(steps)
    }
}

/// Whether two optional percentages are the same figure.
fn same_figure(a: Option<f64>, b: Option<f64>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => (a - b).abs() <= 1e-6,
        (None, None) => true,
        _ => false,
    }
}

/// Whether `after` shows the same stone as `before`: every tier but `removed` keeps its live
/// facet count and the girdle and table keep their size.
fn same_stone(before: &StoneAnalysis, after: &StoneAnalysis, removed: Option<usize>) -> bool {
    let kept_before: Vec<usize> = before
        .facets
        .tiers
        .iter()
        .enumerate()
        .filter(|&(index, _)| Some(index) != removed)
        .map(|(_, facets)| facets.alive)
        .collect();
    let kept_after: Vec<usize> = after
        .facets
        .tiers
        .iter()
        .map(|facets| facets.alive)
        .collect();
    kept_before == kept_after
        && same_figure(before.girdle_percent, after.girdle_percent)
        && same_figure(before.table_percent, after.table_percent)
}

fn snap_to_teeth(design: &Design, tier: usize) -> Result<FixPlan, String> {
    let source = design.tiers.get(tier).ok_or_else(|| STALE.to_string())?;
    let label = tier_label(design, tier);
    let snap = |values: &[f64]| -> (Vec<f64>, usize) {
        let mut out: Vec<f64> = Vec::with_capacity(values.len());
        let mut moved = 0;
        for &value in values {
            let tooth = value.round();
            let snapped = if (value - tooth).abs() > GEAR_EPS {
                moved += 1;
                tooth
            } else {
                value
            };
            if !out.contains(&snapped) {
                out.push(snapped);
            }
        }
        (out, moved)
    };
    let (indices, moved_indices) = snap(&source.indices);
    let (detached, moved_detached) = snap(&source.detached);
    let moved = moved_indices + moved_detached;
    if moved == 0 {
        return Err(format!(
            "Every index position of {label} is already on a gear tooth."
        ));
    }
    Ok(FixPlan {
        edit: Edit::SetIndices {
            index: tier,
            indices,
            detached,
        },
        message: format!(
            "Snapped {moved} index position{} of {label} to the nearest gear tooth.",
            plural(moved)
        ),
        select: Some(tier),
    })
}

fn remove_vanished(design: &Design, tier: usize) -> Result<FixPlan, String> {
    if tier >= design.tiers.len() {
        return Err(STALE.to_string());
    }
    let label = tier_label(design, tier);
    let before = analyze(design, false).map_err(|reason| invalid_text(&reason))?;
    let facets = before
        .facets
        .tiers
        .get(tier)
        .ok_or_else(|| STALE.to_string())?;
    if facets.total == 0 || facets.alive == facets.total {
        return Err(format!(
            "None of the facets of {label} is cut away any more."
        ));
    }
    let (edit, message, removed) = if facets.alive == 0 {
        (
            remove_tier_edit(design, tier),
            format!(
                "Removed {label}: all {} of its facets were cut away by later tiers.",
                facets.total
            ),
            Some(tier),
        )
    } else {
        let SolidStatus::Closed(mesh) = build_solid_mesh(&before.planes) else {
            return Err("The facets do not enclose a stone.".to_string());
        };
        let live = live_planes(&mesh);
        let positions =
            vanished_positions_in(design, &before.ranges, &live, tier).ok_or_else(|| {
                format!(
                    "{label}: the cut-away facets cannot be told apart from the others here. \
                 Remove them by hand in the tier form."
                )
            })?;
        (
            partial_removal_edit(design, tier, &positions),
            format!(
                "Removed {} cut-away facet{} of {label}; the other {} stay.",
                positions.len(),
                plural(positions.len()),
                facets.total - positions.len()
            ),
            None,
        )
    };

    let mut candidate = design.clone();
    candidate
        .apply_edit(edit.clone())
        .map_err(|error| error.to_string())?;
    let after = analyze(&candidate, false).map_err(|reason| invalid_text(&reason))?;
    if !same_stone(&before, &after, removed) {
        return Err(format!(
            "Removing the cut-away facets of {label} would change other facets, so it is not \
             done automatically. Remove them by hand if you want to."
        ));
    }
    Ok(FixPlan {
        edit,
        message,
        select: None,
    })
}

fn move_after(design: &Design, tier: usize, after: usize) -> Result<FixPlan, String> {
    if after <= tier || after >= design.tiers.len() {
        return Err(STALE.to_string());
    }
    let label = tier_label(design, tier);
    let target = tier_label(design, after);
    let out_of_order_before = check_cut_order(design).len();
    let edit = Edit::MoveTier {
        from: tier,
        to: after,
    };
    let mut candidate = design.clone();
    candidate
        .apply_edit(edit.clone())
        .map_err(|error| error.to_string())?;
    if check_cut_order(&candidate).len() >= out_of_order_before {
        return Err(format!(
            "Moving {label} to just after {target} would put another tier out of order, so it \
             is not done automatically. Reorder the tiers by hand."
        ));
    }
    if let Err(error) = candidate.solve() {
        return Err(format!(
            "Moving {label} to just after {target} would leave a design that does not solve \
             ({error}). Reorder the tiers by hand."
        ));
    }
    Ok(FixPlan {
        edit,
        message: format!("Moved {label} to just after {target}, so {target} is cut first."),
        select: Some(after),
    })
}

fn steepen_pavilion(design: &Design, tier: usize, n_d: f64) -> Result<FixPlan, String> {
    let source = design.tiers.get(tier).ok_or_else(|| STALE.to_string())?;
    let label = tier_label(design, tier);
    if design.is_tier_driven(tier) {
        return Err(format!(
            "{label} follows a relation, so the relation sets its angle. Change the relation \
             instead."
        ));
    }
    if is_horizontal_angle_deg(source.angle_deg) {
        return Err(format!(
            "{label} is a flat facet and has no angle to steepen."
        ));
    }
    if !(n_d.is_finite() && n_d > 1.0) {
        return Err("The material has no usable refractive index.".to_string());
    }
    let target = steep_target_deg(n_d);
    if target > MAX_RETARGET_ANGLE_DEG {
        return Err(format!(
            "{label} would have to be at {target:.1}\u{b0}, steeper than the \
             {MAX_RETARGET_ANGLE_DEG:.1}\u{b0} limit. Choose a material with a higher index, or \
             change the pavilion by hand."
        ));
    }
    let current = source.angle_deg.abs();
    if current >= target {
        return Err(format!(
            "{label} is already at {current:.1}\u{b0}, at or past the safe angle."
        ));
    }
    let new_angle = target.copysign(source.angle_deg);

    let original = analyze(design, true).map_err(|reason| invalid_text(&reason))?;
    let (candidate_design, anchors) =
        anchored_candidate_for(design, &[(tier, new_angle)], &original)
            .map_err(|error| error.to_string())?;
    let candidate = analyze(&candidate_design, false);
    if let Some(first) = judge(design, &original, &candidate).first() {
        return Err(format!(
            "Steepening {label} to {target:.2}\u{b0} would break the stone: {} Change the \
             pavilion by hand.",
            invalid_text(first)
        ));
    }

    let mut edits = vec![Edit::RetargetAngles {
        changes: vec![(tier, source.angle_deg, new_angle)],
    }];
    edits.extend(anchors.iter().map(|anchor| Edit::SetConstraint {
        index: anchor.tier_index,
        constraint: MeetConstraint::ScaleReference(anchor.new_mast),
    }));
    let edit = if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Batch(edits)
    };
    Ok(FixPlan {
        edit,
        message: format!(
            "Steepened {label} from {current:.1}\u{b0} to {target:.2}\u{b0}; it turns about its \
             girdle edge, so the girdle stays."
        ),
        select: Some(tier),
    })
}

/// Whether the design has a table: a flat facet on the crown side.
pub(super) fn has_table(design: &Design) -> bool {
    design.tiers.iter().any(|tier| {
        is_horizontal_angle_deg(tier.angle_deg)
            && FacetSide::of_angle_deg(tier.angle_deg) == FacetSide::Crown
    })
}

/// "Table", or "Table 2", "Table 3", ... when the name is taken.
pub(super) fn unique_table_name(design: &Design) -> String {
    let taken = |name: &str| {
        design
            .tiers
            .iter()
            .any(|tier| tier.name.eq_ignore_ascii_case(name))
    };
    if !taken("Table") {
        return "Table".to_string();
    }
    (2..1000)
        .map(|number| format!("Table {number}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| "Table (new)".to_string())
}

/// The flat crown facet a table fix adds: angle 0, no index positions, pinned at `height`.
fn table_tier(name: &str, height: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg: 0.0,
        name: name.to_string(),
        indices: Vec::new(),
        constraint: MeetConstraint::ScaleReference(height),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// The table's width (percent of the stone's width) when a table at `height` is acceptable:
/// the stone stays valid by the Retarget gate, the table is live and its width is plausible.
fn table_width_at(
    design: &Design,
    original: &StoneAnalysis,
    name: &str,
    height: f64,
) -> Option<f64> {
    let mut candidate = design.clone();
    candidate
        .apply_edit(Edit::AddTier {
            index: design.tiers.len(),
            tier: table_tier(name, height),
        })
        .ok()?;
    let analysis = analyze(&candidate, false);
    if !judge(design, original, &analysis).is_empty() {
        return None;
    }
    let analysis = analysis.ok()?;
    if analysis.facets.tiers.last()?.alive == 0 {
        return None;
    }
    analysis
        .table_percent
        .filter(|percent| (MIN_TABLE_PERCENT..=MAX_TABLE_PERCENT).contains(percent))
}

fn add_table(design: &Design) -> Result<FixPlan, String> {
    if has_table(design) {
        return Err("The design already has a table.".to_string());
    }
    let original = analyze(design, false).map_err(|reason| invalid_text(&reason))?;
    let name = unique_table_name(design);

    let found = table_width_at(design, &original, &name, QUICK_ADD_TABLE_MAST)
        .map(|percent| (QUICK_ADD_TABLE_MAST, percent))
        .or_else(|| scan_table(design, &original, &name));
    let Some((height, percent)) = found else {
        return Err(
            "No table height keeps every crown facet, so no table is added automatically. Add \
             one by hand in the tier form."
                .to_string(),
        );
    };
    let index = design.tiers.len();
    Ok(FixPlan {
        edit: Edit::AddTier {
            index,
            tier: table_tier(&name, height),
        },
        message: format!(
            "Added a table at height {height:.3}: {percent:.0} % of the stone's width."
        ),
        select: Some(index),
    })
}

/// The table height above the girdle that keeps every crown facet and comes nearest a table of
/// [`TARGET_TABLE_PERCENT`], with its width. `None` when no scanned height works.
fn scan_table(design: &Design, original: &StoneAnalysis, name: &str) -> Option<(f64, f64)> {
    let SolidStatus::Closed(mesh) = build_solid_mesh(&original.planes) else {
        return None;
    };
    let top = mesh
        .positions
        .iter()
        .map(|vertex| vertex.y)
        .fold(f64::NEG_INFINITY, f64::max);
    let girdle_top = original.girdle_band.map_or(0.0, |(_, high)| high);
    if !top.is_finite() || top <= girdle_top {
        return None;
    }
    (1..=TABLE_SCAN_STEPS)
        .filter_map(|step| {
            let fraction = step as f64 / (TABLE_SCAN_STEPS + 1) as f64;
            let height = fraction.mul_add(top - girdle_top, girdle_top);
            table_width_at(design, original, name, height).map(|percent| (height, percent))
        })
        .min_by(|a, b| {
            (a.1 - TARGET_TABLE_PERCENT)
                .abs()
                .total_cmp(&(b.1 - TARGET_TABLE_PERCENT).abs())
        })
}
