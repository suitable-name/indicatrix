//! Re-anchoring masts after the angles move.
//!
//! An imported design pins every tier with a `ScaleReference` mast, so changing a
//! facet's angle and leaving its mast alone tilts the facet about the stone's centre
//! line, not about the edge where it meets the girdle: a steeper crown drops its girdle
//! edge, a steeper pavilion lifts its own, and the girdle band can shrink to nothing.
//!
//! A cutter re-cutting a facet at a new angle does the opposite -- the facet turns about
//! its girdle corners, and those corners stay where they are. [`anchored_candidate`] does the
//! same in the model: it reads the pivot of every moving facet from the ORIGINAL solved
//! stone (`indicatrix_cut_core::design::hinge`) and gives each `ScaleReference` tier the
//! mast that keeps that pivot on its new plane. Tiers that meet other tiers
//! (`MeetExisting`, `MeetNamed`) re-solve from those meets as usual, and the girdle's own
//! masts never change.
//!
//! # Following the neighbour
//!
//! A main facet does not meet the girdle itself but the girdle-breaking facets between it
//! and the girdle, and those turn too. So tiers are anchored nearest-the-girdle first, and a
//! pivot that lies on the plane of an already re-anchored neighbour slides straight up or
//! down onto that neighbour's NEW plane before the mast is computed. The two facets keep
//! meeting along an edge at the same place on the stone's outline, instead of the main
//! facet swinging through the girdle.
//!
//! [`apply_with_anchors`] then turns a proposal plus those masts into one `Edit::Batch`,
//! so the whole retarget is a single undo step.

use super::{RetargetProposal, plan::RetargetPlan, validity::StoneAnalysis};
use glam::DVec3;
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    Design, Edit,
    design::{
        RelationError,
        hinge::{FacetSide, TierHinge, mast_through},
    },
};
use std::collections::BTreeMap;

/// The smallest mast a re-anchored tier may get: below this the plane has crossed the
/// stone's centre and no longer describes a facet.
const MIN_ANCHORED_MAST: f64 = 1e-6;

/// A pivot this close to another tier's original plane lies on that plane.
const NEIGHBOUR_TOLERANCE: f64 = 1e-6;

/// A neighbour plane this close to vertical cannot carry a pivot up or down.
const MIN_VERTICAL_COMPONENT: f64 = 1e-9;

/// One `ScaleReference` mast a retarget changes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AnchorChange {
    /// The tier's position in `design.tiers`.
    pub tier_index: usize,
    /// The mast before the retarget.
    pub old_mast: f64,
    /// The mast after it.
    pub new_mast: f64,
}

/// The `ScaleReference` mast of tier `index`, if it has one.
#[must_use]
pub fn scale_reference_mast(design: &Design, index: usize) -> Option<f64> {
    match design.tiers.get(index)?.constraint {
        MeetConstraint::ScaleReference(mast) => Some(mast),
        _ => None,
    }
}

/// A tier that has already been re-anchored: what its planes became.
#[derive(Debug, Clone, Copy)]
struct MovedPlane {
    angle_deg: f64,
    side: FacetSide,
    mast: f64,
}

/// The order tiers are anchored in: first the facets that bound the girdle band (they have
/// a whole edge on a wall and depend on nothing), then the rest, nearest the girdle first.
fn anchor_order(a: &TierHinge, b: &TierHinge) -> std::cmp::Ordering {
    let distance = |hinge: &TierHinge| match hinge.side {
        FacetSide::Crown => hinge.point.y,
        FacetSide::Pavilion => -hinge.point.y,
    };
    (!a.wall_edge)
        .cmp(&!b.wall_edge)
        .then_with(|| distance(a).total_cmp(&distance(b)))
}

/// `hinge.point`, slid vertically onto the new plane of every already re-anchored tier whose
/// original plane passed through it (the lowest such height for a crown facet, the highest
/// for a pavilion facet, so the point stays inside every neighbour). Unchanged when no
/// re-anchored tier touched it.
fn follow_neighbours(
    hinge: &TierHinge,
    own_tier: usize,
    original: &StoneAnalysis,
    moved: &BTreeMap<usize, MovedPlane>,
) -> DVec3 {
    let mut heights = Vec::new();
    for (&tier, plane) in moved {
        if tier == own_tier {
            continue;
        }
        let Some(range) = original.ranges.get(tier) else {
            continue;
        };
        let (sin_t, cos_t) = plane.angle_deg.abs().to_radians().sin_cos();
        let vertical = match plane.side {
            FacetSide::Crown => cos_t,
            FacetSide::Pavilion => -cos_t,
        };
        if vertical.abs() < MIN_VERTICAL_COMPONENT {
            continue;
        }
        for index in range.clone() {
            let Some(&(normal, offset)) = original.planes.get(index) else {
                continue;
            };
            if (normal.dot(hinge.point) - offset).abs() > NEIGHBOUR_TOLERANCE {
                continue;
            }
            let azimuth = normal.z.atan2(normal.x);
            let radial = azimuth
                .cos()
                .mul_add(hinge.point.x, azimuth.sin() * hinge.point.z);
            heights.push(sin_t.mul_add(-radial, plane.mast) / vertical);
        }
    }
    let height = match hinge.side {
        FacetSide::Crown => heights.into_iter().reduce(f64::min),
        FacetSide::Pavilion => heights.into_iter().reduce(f64::max),
    };
    height.map_or(hinge.point, |y| DVec3::new(hinge.point.x, y, hinge.point.z))
}

/// Sets every tier of `design` that follows a relation to the angle its relation gives.
/// Returns the positions of the followers whose angle changed.
///
/// A design without relations is left alone.
///
/// # Errors
///
/// The relation engine's error when a relation cannot be satisfied (the result is not a
/// facet angle, or relations read each other in a loop). `design` is not changed then.
pub fn fold_relations(design: &mut Design) -> Result<Vec<usize>, RelationError> {
    let updates = design.evaluate_relations()?;
    let mut changed = Vec::new();
    for (index, angle) in updates {
        if let Some(tier) = design.tiers.get_mut(index)
            && tier.angle_deg.to_bits() != angle.to_bits()
        {
            tier.angle_deg = angle;
            changed.push(index);
        }
    }
    Ok(changed)
}

/// `design` with `moves` (`(tier_index, new_angle)`) applied and every moving
/// `ScaleReference` tier re-anchored to its pivot, plus the masts that changed.
///
/// The tiers that follow a relation are never in `moves`; they take the angle their relation
/// gives once the others have moved ([`fold_relations`]), and are re-anchored like any other
/// moving tier.
///
/// A moving tier with no pivot (it has no live facet in the original stone) keeps its mast,
/// and so does any tier whose re-anchored mast would be non-finite or not positive.
///
/// # Errors
///
/// The relation engine's error when the relations cannot be satisfied after the move.
pub fn anchored_candidate_for(
    design: &Design,
    moves: &[(usize, f64)],
    original: &StoneAnalysis,
) -> Result<(Design, Vec<AnchorChange>), RelationError> {
    anchored_candidate_with(design, moves, original, false)
}

/// [`anchored_candidate_for`] against a stone whose girdle band was thickened.
///
/// The band is thickened by [`StoneAnalysis::split_at_girdle`]: every hinge moved, so EVERY tier that has a hinge is
/// re-anchored to it, not only the ones that change angle, and the table and culet masts move
/// by the half-thickness their side was translated by (`flat_shift` is that magnitude, taken
/// from the unsplit stone's flats).
///
/// # Errors
///
/// The relation engine's error when the relations cannot be satisfied after the move.
pub fn anchored_candidate_split(
    design: &Design,
    moves: &[(usize, f64)],
    split: &StoneAnalysis,
    flat_shift: f64,
) -> Result<(Design, Vec<AnchorChange>), RelationError> {
    let (mut candidate, mut anchors) = anchored_candidate_with(design, moves, split, true)?;
    // A flat's mast is its distance from the centre: the table rose and the culet fell by the
    // same half-thickness, so both masts grow by it. The refit that follows finds the exact
    // height; this only starts it near.
    for flat in &split.flats {
        let Some(old_mast) = scale_reference_mast(design, flat.tier_index) else {
            continue;
        };
        let new_mast = old_mast + flat_shift;
        if !new_mast.is_finite() || new_mast < MIN_ANCHORED_MAST {
            continue;
        }
        candidate.tiers[flat.tier_index].constraint = MeetConstraint::ScaleReference(new_mast);
        anchors.retain(|anchor| anchor.tier_index != flat.tier_index);
        anchors.push(AnchorChange {
            tier_index: flat.tier_index,
            old_mast,
            new_mast,
        });
    }
    anchors.sort_by_key(|anchor| anchor.tier_index);
    Ok((candidate, anchors))
}

fn anchored_candidate_with(
    design: &Design,
    moves: &[(usize, f64)],
    original: &StoneAnalysis,
    reanchor_all: bool,
) -> Result<(Design, Vec<AnchorChange>), RelationError> {
    let mut candidate = design.clone();
    for &(index, angle) in moves {
        if let Some(tier) = candidate.tiers.get_mut(index) {
            tier.angle_deg = angle;
        }
    }
    let followers = fold_relations(&mut candidate)?;
    let mut moving: Vec<(usize, f64)> = moves.to_vec();
    moving.extend(
        followers
            .into_iter()
            .map(|index| (index, candidate.tiers[index].angle_deg)),
    );
    if reanchor_all {
        // BTreeMap keys: ascending tier order, so the result is deterministic.
        for &index in original.hinges.keys() {
            if !moving.iter().any(|&(moved, _)| moved == index) {
                moving.push((index, candidate.tiers[index].angle_deg));
            }
        }
    }

    // Nearest the girdle first, so a facet can follow the new plane of the one it meets.
    let mut order: Vec<((usize, f64), &TierHinge)> = moving
        .iter()
        .filter_map(|&(index, angle)| {
            original
                .hinges
                .get(&index)
                .map(|hinge| ((index, angle), hinge))
        })
        .collect();
    order.sort_by(|a, b| anchor_order(a.1, b.1));

    let mut placed = BTreeMap::new();
    let mut anchors = Vec::new();
    for ((tier_index, new_angle), hinge) in order {
        let Some(old_mast) = scale_reference_mast(design, tier_index) else {
            continue;
        };
        let pivot = follow_neighbours(hinge, tier_index, original, &placed);
        let new_mast = mast_through(new_angle, hinge.azimuth_rad, hinge.side, pivot);
        if !new_mast.is_finite() || new_mast < MIN_ANCHORED_MAST {
            continue;
        }
        candidate.tiers[tier_index].constraint = MeetConstraint::ScaleReference(new_mast);
        placed.insert(
            tier_index,
            MovedPlane {
                angle_deg: new_angle,
                side: hinge.side,
                mast: new_mast,
            },
        );
        anchors.push(AnchorChange {
            tier_index,
            old_mast,
            new_mast,
        });
    }
    anchors.sort_by_key(|anchor| anchor.tier_index);
    Ok((candidate, anchors))
}

/// [`anchored_candidate_for`] for a plan's moving rows.
///
/// When the relations cannot be satisfied the candidate carries the plan's angles only (no
/// followers, no masts); [`anchored_candidate_for`] reports why.
#[must_use]
pub fn anchored_candidate(
    design: &Design,
    plan: &RetargetPlan,
    original: &StoneAnalysis,
) -> (Design, Vec<AnchorChange>) {
    let moves = plan.moving_angles();
    anchored_candidate_for(design, &moves, original).unwrap_or_else(|_| {
        let mut candidate = design.clone();
        for &(index, angle) in &moves {
            if let Some(tier) = candidate.tiers.get_mut(index) {
                tier.angle_deg = angle;
            }
        }
        (candidate, Vec::new())
    })
}

/// The masts that differ between `before` and `after` (same tiers, same order): every tier
/// that is a `ScaleReference` in both and whose mast changed.
#[must_use]
pub fn mast_differences(before: &Design, after: &Design) -> Vec<AnchorChange> {
    before
        .tiers
        .iter()
        .zip(&after.tiers)
        .enumerate()
        .filter_map(
            |(tier_index, (was, now))| match (&was.constraint, &now.constraint) {
                (
                    MeetConstraint::ScaleReference(old_mast),
                    MeetConstraint::ScaleReference(new_mast),
                ) if old_mast.to_bits() != new_mast.to_bits() => Some(AnchorChange {
                    tier_index,
                    old_mast: *old_mast,
                    new_mast: *new_mast,
                }),
                _ => None,
            },
        )
        .collect()
}

/// `(tier_index, new_angle)` of every tier whose angle differs between `before` and `after`.
///
/// Both designs have the same tiers in the same order. The tiers that follow a relation in
/// `before` are left out: the editor session works those out again, in the same undo step,
/// and refuses a direct change.
#[must_use]
pub fn angle_differences(before: &Design, after: &Design) -> Vec<(usize, f64)> {
    before
        .tiers
        .iter()
        .zip(&after.tiers)
        .enumerate()
        .filter(|&(index, (was, now))| {
            was.angle_deg.to_bits() != now.angle_deg.to_bits() && !before.is_tier_driven(index)
        })
        .map(|(index, (_, now))| (index, now.angle_deg))
        .collect()
}

/// Turns `proposal`'s angles and `anchors`' masts into ONE atomic `Edit::Batch` (one
/// `ModifyTier` per tier that changes, relation followers included).
///
/// It captures the original meet network of `design`, materialises the angles and masts on
/// a scratch copy, settles the relations, and validates the result under the Retarget
/// preservation policy. Nothing is written to `design`; the returned edit is what the caller
/// pushes through `History`, and its inverse restores every angle and mast together, so one
/// Undo reverts the entire retarget.
///
/// With no anchors it is the angle-only change [`super::apply`] makes, and a pinned design
/// refuses it (turning a pinned facet without moving its mast splits its meets).
///
/// # Errors
///
/// The [`PreservationViolation`](indicatrix_cut_core::meet_preservation::PreservationViolation)
/// that refuses the change. A row or anchor that no longer matches `design` (a tier that is
/// gone, an angle or mast that is not the one the proposal started from, relations that
/// cannot be satisfied) is `StaleCandidate`; nothing is silently dropped. A row for a tier
/// that follows a relation is `Relation`.
#[must_use = "an edit that is not applied changes nothing"]
pub fn apply_with_anchors(
    design: &Design,
    proposal: &RetargetProposal,
    anchors: &[AnchorChange],
) -> Result<Edit, indicatrix_cut_core::meet_preservation::PreservationViolation> {
    let angles: Vec<_> = proposal
        .rows
        .iter()
        .map(|row| indicatrix_cut_core::AngleChange {
            index: row.tier_index,
            from_deg: row.old_angle,
            to_deg: row.new_angle,
        })
        .collect();
    let masts: Vec<_> = anchors
        .iter()
        .map(|anchor| indicatrix_cut_core::MastChange {
            index: anchor.tier_index,
            from_mast: anchor.old_mast,
            to_mast: anchor.new_mast,
        })
        .collect();
    let baseline =
        indicatrix_cut_core::meet_preservation::MeetPreservationBaseline::capture(design)?;
    indicatrix_cut_core::optimize::materialize_preserved_changes(
        design,
        &angles,
        &masts,
        Some(&baseline),
        indicatrix_cut_core::meet_preservation::PreservationPolicy::RETARGET,
    )
    .map(|(_, edit, _)| edit)
    .map_err(|error| match error {
        indicatrix_cut_core::optimize::MaterializeError::Preservation(reason) => reason,
        indicatrix_cut_core::optimize::MaterializeError::Edit(_) => {
            indicatrix_cut_core::meet_preservation::PreservationViolation::StaleCandidate
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retarget::{
        CrownShift, PlanRow, build_plan,
        check::{CheckInputs, RetargetCheck, check_retarget},
        validity::analyze,
    };
    use indicatrix::optics::LightingPreset;
    use indicatrix_cut_core::{
        BuiltinMaterials, ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
        meet_preservation::{MeetPreservationBaseline, PreservationPolicy, PreservationViolation},
    };

    fn selection(name: &str) -> MaterialSelection {
        MaterialSelection {
            name: Some(name.to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        }
    }

    fn brilliant() -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.material = selection("Diamond");
        design
    }

    fn quartz() -> indicatrix_cut_core::ResolvedMaterial {
        selection("Quartz").resolve(&BuiltinMaterials)
    }

    fn tier_named(design: &Design, name: &str) -> usize {
        design.tiers.iter().position(|t| t.name == name).unwrap()
    }

    #[test]
    fn anchored_candidate_moves_angles_and_masts_of_scale_reference_tiers_only() {
        let design = brilliant();
        let original = analyze(&design, true).expect("every tier is pinned");
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let (candidate, anchors) = anchored_candidate(&design, &plan, &original);

        // Every moving tier carries its planned angle in the candidate.
        let moving: Vec<&PlanRow> = plan.rows.iter().filter(|r| r.moves).collect();
        assert_ne!(moving, Vec::<&PlanRow>::new());
        for row in &moving {
            assert!(
                (candidate.tiers[row.tier_index].angle_deg - row.new_angle).abs() < 1e-12,
                "{}",
                row.name
            );
        }
        // Table, culet and girdle are untouched.
        for name in ["Table", "Culet", "Girdle"] {
            let index = tier_named(&design, name);
            assert_eq!(candidate.tiers[index], design.tiers[index], "{name}");
            assert!(anchors.iter().all(|a| a.tier_index != index), "{name}");
        }
        // Every anchor is a real change to a ScaleReference mast.
        for anchor in &anchors {
            assert_eq!(
                scale_reference_mast(&candidate, anchor.tier_index),
                Some(anchor.new_mast)
            );
            assert_eq!(
                scale_reference_mast(&design, anchor.tier_index),
                Some(anchor.old_mast)
            );
        }
    }

    #[test]
    fn a_plan_that_changes_nothing_anchors_every_mast_where_it_was() {
        let design = brilliant();
        let original = analyze(&design, true).expect("every tier is pinned");
        // Same material in and out: no angle moves.
        let same = selection("Diamond").resolve(&BuiltinMaterials);
        let plan = build_plan(&design, &same, CrownShift::default(), &[]);
        let (_, anchors) = anchored_candidate(&design, &plan, &original);
        assert_ne!(anchors, Vec::new());
        for anchor in anchors {
            assert!(
                (anchor.new_mast - anchor.old_mast).abs() < 1e-5,
                "tier {}: {} -> {}",
                anchor.tier_index,
                anchor.old_mast,
                anchor.new_mast
            );
        }
    }

    #[test]
    fn a_pavilion_that_gets_steeper_keeps_the_girdle_edge_of_its_break_facet() {
        let design = brilliant();
        let original = analyze(&design, true).expect("every tier is pinned");
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let (candidate, _) = anchored_candidate(&design, &plan, &original);
        let lower = tier_named(&design, "Lower Girdle");
        let hinge = original.hinges[&lower];

        // The pivot of the lower-girdle facet still lies on its new plane.
        let new_angle = candidate.tiers[lower].angle_deg;
        assert!(
            new_angle < design.tiers[lower].angle_deg,
            "steeper pavilion"
        );
        let mast = scale_reference_mast(&candidate, lower).unwrap();
        let through = mast_through(new_angle, hinge.azimuth_rad, hinge.side, hinge.point);
        assert!((mast - through).abs() < 1e-9, "{mast} vs {through}");
    }

    #[test]
    fn a_main_facet_follows_the_new_plane_of_the_break_facet_it_meets() {
        let design = brilliant();
        let original = analyze(&design, true).expect("every tier is pinned");
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let (candidate, _) = anchored_candidate(&design, &plan, &original);
        let main = tier_named(&design, "Pavilion Main");
        let lower = tier_named(&design, "Lower Girdle");
        let hinge = original.hinges[&main];

        // The main facet's pivot is on the lower girdle facet's ORIGINAL plane...
        let on_original = original.ranges[lower].clone().any(|index| {
            let (normal, offset) = original.planes[index];
            (normal.dot(hinge.point) - offset).abs() < 1e-6
        });
        assert!(on_original, "the pivot should lie on the break facet");

        // ...and the new main plane passes through the point at the same place on the outline
        // that lies on the break facet's NEW plane.
        let (normal, _) = original.ranges[lower]
            .clone()
            .map(|index| original.planes[index])
            .find(|(normal, offset)| (normal.dot(hinge.point) - offset).abs() < 1e-6)
            .expect("the pivot lies on a break facet");
        let azimuth = normal.z.atan2(normal.x);
        let (sin_t, cos_t) = candidate.tiers[lower]
            .angle_deg
            .abs()
            .to_radians()
            .sin_cos();
        let lower_mast = scale_reference_mast(&candidate, lower).unwrap();
        let radial = azimuth
            .cos()
            .mul_add(hinge.point.x, azimuth.sin() * hinge.point.z);
        let followed_height = sin_t.mul_add(-radial, lower_mast) / -cos_t;
        let followed = DVec3::new(hinge.point.x, followed_height, hinge.point.z);
        let main_mast = scale_reference_mast(&candidate, main).unwrap();
        let through = mast_through(
            candidate.tiers[main].angle_deg,
            hinge.azimuth_rad,
            hinge.side,
            followed,
        );
        assert!(
            (main_mast - through).abs() < 1e-9,
            "main mast {main_mast} vs {through}"
        );
    }

    fn index_selection(n_d: f64) -> MaterialSelection {
        MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(n_d),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        }
    }

    /// The standard brilliant as a 1.72 stone (every tier pinned, like an imported design),
    /// its plan to 1.7681 with a third of the shift on the crown, and the check of that plan:
    /// the case the check's own tests pin as valid by Shift alone.
    fn checked_sapphire_case() -> (Design, RetargetPlan, RetargetCheck) {
        let mut design = brilliant();
        design.material = index_selection(1.72);
        let target = index_selection(1.7681).resolve(&BuiltinMaterials);
        let crown = CrownShift {
            fraction: 0.33,
            scale_by_ratio: false,
            follow_pavilion: false,
        };
        let plan = build_plan(&design, &target, crown, &[]);
        let check = check_retarget(&CheckInputs {
            girdle: None,
            design: &design,
            plan: &plan,
            current_gem: None,
            lighting: LightingPreset::RingLights,
        });
        (design, plan, check)
    }

    /// The edit a front end that transports the check's angles and masts commits.
    fn transported_edit(
        design: &Design,
        plan: &RetargetPlan,
        check: &RetargetCheck,
    ) -> Result<Edit, PreservationViolation> {
        let candidate = check
            .candidate
            .as_ref()
            .expect("a valid check carries its candidate");
        let proposal = plan
            .with_angles(design, &angle_differences(design, candidate))
            .proposal();
        apply_with_anchors(design, &proposal, &check.anchors)
    }

    #[test]
    fn apply_with_anchors_is_one_undo_step_and_undoes_exactly() {
        let (mut design, plan, check) = checked_sapphire_case();
        assert!(check.allows_apply(), "{}", check.validity.headline());
        let original_design = design.clone();
        let anchors = check.anchors.clone();
        assert_ne!(anchors, Vec::new());

        let edit = transported_edit(&design, &plan, &check).expect("preserved retarget");
        assert!(matches!(edit, Edit::Batch(_)));

        let mut history = History::new();
        history.apply(&mut design, edit).expect("the batch applies");
        assert_ne!(design, original_design);
        for anchor in &anchors {
            assert_eq!(
                scale_reference_mast(&design, anchor.tier_index),
                Some(anchor.new_mast)
            );
        }

        assert!(history.undo(&mut design).unwrap());
        assert_eq!(
            design, original_design,
            "one undo restores angles and masts"
        );
        assert!(!history.can_undo(), "the retarget was one undo step");
    }

    #[test]
    fn a_retarget_batch_reads_as_a_retarget_in_the_undo_list() {
        // This assertion used to sit in `apply_with_anchors_is_one_undo_step_and_undoes_exactly`.
        // `Edit::describe` only recognises a batch of `RetargetAngles` and `SetConstraint`; the
        // atomic batch of `ModifyTier`s that `apply_with_anchors` now builds reads as "N combined
        // edits", so it stays red until `describe_anchored_retarget` (indicatrix-cut-core,
        // edit/edit_type.rs) learns the new shape. Kept as its own test so the undo-step test
        // above says only what it is named for.
        let (design, plan, check) = checked_sapphire_case();
        let edit = transported_edit(&design, &plan, &check).expect("preserved retarget");
        assert_eq!(edit.describe(&design), "Retarget angles");
    }

    #[test]
    fn apply_with_anchors_reproduces_the_checked_candidate_and_keeps_the_original_meets() {
        let (design, plan, check) = checked_sapphire_case();
        assert!(check.allows_apply(), "{}", check.validity.headline());
        assert_ne!(
            check.anchors,
            Vec::new(),
            "a pinned design needs its masts moved"
        );
        let candidate = check
            .candidate
            .as_ref()
            .expect("a valid check carries its candidate");

        // The check's own edit.
        let mut direct = design.clone();
        let checked = check.validated_edit(&design).expect("preserved retarget");
        History::new()
            .apply(&mut direct, checked)
            .expect("the batch applies");

        // The same state rebuilt from what a front end transports: the candidate's angles
        // as a proposal, plus the masts.
        let mut transported = design.clone();
        let edit = transported_edit(&design, &plan, &check)
            .expect("the transported angles and masts are preserved");
        History::new()
            .apply(&mut transported, edit)
            .expect("the batch applies");

        assert_eq!(transported, direct, "both routes give one state");
        assert_eq!(&transported, candidate, "and it is the checked candidate");
        MeetPreservationBaseline::capture(&design)
            .expect("the original captures")
            .validate(&transported, PreservationPolicy::RETARGET)
            .expect("the original meets survive");
    }

    #[test]
    fn an_angle_only_proposal_on_a_pinned_design_is_refused() {
        // Turning pinned facets and leaving their masts alone splits the meets they take part
        // in (`the_hinge_keeps_the_girdle_where_the_plain_shift_destroys_it` pins the stone
        // damage); the edit must not be produced.
        let design = brilliant();
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let result = apply_with_anchors(&design, &plan.proposal(), &[]);
        assert!(result.is_err(), "{result:?}");
    }

    #[test]
    fn an_anchor_for_a_missing_tier_is_stale() {
        let design = brilliant();
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let proposal = plan.proposal();
        let stale = [AnchorChange {
            tier_index: 99,
            old_mast: 1.0,
            new_mast: 1.1,
        }];
        let result = apply_with_anchors(&design, &proposal, &stale);
        assert!(
            matches!(result, Err(PreservationViolation::StaleCandidate)),
            "{result:?}"
        );
    }
}
