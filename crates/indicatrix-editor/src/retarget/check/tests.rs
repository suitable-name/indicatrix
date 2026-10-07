//! Tests of the retarget check.

use super::*;
use crate::retarget::{
    CrownShift,
    anchors::anchored_candidate,
    apply_with_anchors, build_plan,
    validity::{KNIFE_EDGE_PERCENT, MIN_GIRDLE_FRACTION, ValidityStatus},
};
use indicatrix::geometry::stone_metrics::{SolidStatus, build_solid_mesh};
use indicatrix_cut_core::{
    BuiltinMaterials, ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
    design::{
        girdle_band::{corner_resolution, girdle_walls, wall_thinnest_within},
        hinge::FacetSide,
    },
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

fn check_against_material(
    design: &Design,
    target: &indicatrix_cut_core::ResolvedMaterial,
    crown: CrownShift,
) -> (RetargetPlan, RetargetCheck) {
    let plan = build_plan(design, target, crown, &[]);
    let current = indicatrix::optics::materials::GemMaterial::diamond();
    let inputs = CheckInputs {
        girdle: None,
        design,
        plan: &plan,
        current_gem: Some(&current),
        lighting: LightingPreset::RingLights,
    };
    let result = check_retarget(&inputs);
    (plan, result)
}

fn check(design: &Design, target: &str, crown: CrownShift) -> (RetargetPlan, RetargetCheck) {
    let target = selection(target).resolve(&BuiltinMaterials);
    check_against_material(design, &target, crown)
}

/// A check against a bare refractive index (no named material).
fn check_index(design: &Design, n_d: f64, crown: CrownShift) -> (RetargetPlan, RetargetCheck) {
    let target = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(n_d),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
    .resolve(&BuiltinMaterials);
    check_against_material(design, &target, crown)
}

/// The brilliant as a 1.72 stone, the owner's starting point for the emerald and
/// sapphire cases.
fn brilliant_at_172() -> Design {
    let mut design = brilliant();
    design.material = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.72),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

fn tier_named(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .expect("the tier exists")
}

fn is_structural_failure(reason: &InvalidReason) -> bool {
    matches!(
        reason,
        InvalidReason::GirdleGone
            | InvalidReason::NotClosed
            | InvalidReason::DoesNotSolve(_)
            | InvalidReason::FlatOffGirdle { .. }
    )
}

#[test]
fn a_same_material_retarget_is_valid_and_changes_no_mast() {
    let design = brilliant();
    let (_, result) = check(&design, "Diamond", CrownShift::default());
    assert_eq!(result.validity.status, ValidityStatus::Valid);
    for anchor in &result.anchors {
        assert!(
            (anchor.new_mast - anchor.old_mast).abs() < 1e-5,
            "tier {}",
            anchor.tier_index
        );
    }
}

/// The gate's promise about the girdle's corners, whatever the verdict: the thinnest point
/// stays at least half as thick as it was (and never a knife edge), or the verdict names
/// it. A valid verdict can therefore never hide a girdle that has run out between the walls.
fn assert_corners_kept(result: &RetargetCheck) {
    let figures = result.validity.figures;
    let kept = match (figures.thinnest_was, figures.thinnest_now) {
        (Some(was), Some(now)) => {
            was <= KNIFE_EDGE_PERCENT
                || (now > KNIFE_EDGE_PERCENT && now >= MIN_GIRDLE_FRACTION * was)
        }
        _ => true,
    };
    let refused = result
        .validity
        .reasons
        .iter()
        .any(|reason| matches!(reason, InvalidReason::GirdleThinAtCorners { .. }));
    assert_eq!(
        kept, !refused,
        "thinnest point {:?} -> {:?}, verdict {:?}",
        figures.thinnest_was, figures.thinnest_now, result.validity.reasons
    );
    if result.validity.status == ValidityStatus::Valid {
        assert!(kept, "a valid verdict must keep the corners");
    }
}

/// The brilliant with every crown and pavilion facet at the position of a girdle wall, so
/// each one cuts its wall along a level line: the girdle is as thick between the walls as
/// anywhere else. A 1.72 stone, like [`brilliant_at_172`].
fn level_brilliant_at_172() -> Design {
    const WALLS: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    let pinned = |name: &str, angle_deg: f64, indices: &[f64], mast: f64| ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    };
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        vec![
            pinned("Table", 0.0, &[], 0.32),
            pinned("Crown", 34.5, &WALLS, 0.59),
            pinned("Girdle", 90.0, &WALLS, 1.0),
            pinned("Pavilion", -41.0, &WALLS, 0.67),
            pinned("Culet", -0.0, &[], 0.88),
        ],
    );
    design.material = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(1.72),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

#[test]
fn diamond_to_sapphire_keeps_the_girdle_and_closes() {
    // The owner's sapphire case in spirit: a higher-index to a lower-index material with
    // part of the shift applied to the crown.
    let design = brilliant();
    let crown = CrownShift {
        fraction: 0.33,
        scale_by_ratio: false,
        follow_pavilion: false,
    };
    let (plan, result) = check(&design, "Sapphire", crown);
    assert!(
        result
            .validity
            .reasons
            .iter()
            .all(|reason| !is_structural_failure(reason)),
        "{:?}",
        result.validity.reasons
    );

    // The retargeted stone closes and has a girdle of the thickness the stone had: every
    // facet turns about its girdle edge. (Whether the corners keep their thickness is the
    // gate's separate question, `assert_corners_kept`.)
    let figures = result.validity.figures;
    let was = figures.girdle_was.expect("the design has a girdle");
    let now = figures
        .girdle_now
        .expect("the retargeted stone closes and has a girdle");
    assert!((was - now).abs() < 1e-3, "girdle {was} % became {now} %");
    assert!(result.metrics.retargeted_in_target.is_some());
    let mut applied = design.clone();
    let edit = apply_with_anchors(&design, &plan.proposal(), &result.anchors);
    History::new()
        .apply(&mut applied, edit)
        .expect("the batch applies");
    let after = analyze(&applied, false).expect("the applied stone closes");
    assert!(after.girdle_percent.is_some(), "the girdle stays alive");
    assert_corners_kept(&result);
    // The table never moves: it is listed, flagged as not moving, and its angle is kept.
    let table = plan
        .rows
        .iter()
        .find(|row| row.name == "Table")
        .expect("the table is listed");
    assert!(!table.moves);
    assert!(table.new_angle.abs() < 1e-12 && table.new_angle.is_sign_positive());
}

#[test]
fn the_owners_sapphire_case_keeps_the_table_on_the_crown_side() {
    // n_D 1.72 to 1.7681 with a third of the shift on the crown: the table used to go
    // from 0.00 to -0.37 degrees and onto the pavilion side.
    let design = brilliant_at_172();
    let crown = CrownShift {
        fraction: 0.33,
        scale_by_ratio: false,
        follow_pavilion: false,
    };
    let (plan, result) = check_index(&design, 1.7681, crown);
    let table = plan
        .rows
        .iter()
        .find(|row| row.name == "Table")
        .expect("the table is listed");
    assert!(!table.moves);
    assert!(table.new_angle.abs() < 1e-12 && table.new_angle.is_sign_positive());
    assert!(
        result
            .validity
            .reasons
            .iter()
            .all(|reason| !is_structural_failure(reason)),
        "{:?}",
        result.validity.reasons
    );
    // A change this small must end valid with Shift alone.
    assert_eq!(
        result.validity.status,
        ValidityStatus::Valid,
        "{:?}",
        result.validity.reasons
    );
    // The headline reports the girdle's thinnest point next to its overall figure, so
    // "girdle 6.0 % (was 6.0 %)" can no longer hide what happens between the walls.
    let figures = result.validity.figures;
    let (was, now) = (
        figures
            .thinnest_was
            .expect("the thinnest point is measured"),
        figures
            .thinnest_now
            .expect("the thinnest point is measured"),
    );
    assert!(
        now > KNIFE_EDGE_PERCENT && now >= MIN_GIRDLE_FRACTION * was,
        "thinnest point {was} % became {now} %"
    );
    let headline = result.validity.headline();
    assert!(headline.contains("thinnest point"), "{headline}");
    assert_corners_kept(&result);
    let culet = tier_named(&design, "Culet");

    // The hinge step never touches a flat: only crown and pavilion facets turn about the
    // girdle. (A flat can still appear in `result.anchors`: that is the height refit
    // below, which keeps the table and the culet the size they were.)
    let original = analyze(&design, true).expect("the template solves and closes");
    let (hinged, hinge_anchors) = anchored_candidate(&design, &plan, &original);
    for flat in [table.tier_index, culet] {
        assert!(
            hinge_anchors.iter().all(|anchor| anchor.tier_index != flat),
            "tier {flat} must not be a hinge anchor"
        );
        assert_eq!(hinged.tiers[flat], design.tiers[flat], "tier {flat}");
    }

    // The full Shift path: the plan's angles plus the checked masts, as the one edit that
    // Apply commits. Whatever the checked masts say about the table, its ANGLE is the
    // one it had, bit for bit, so it is still a level plane.
    let mut applied = design.clone();
    let edit = apply_with_anchors(&design, &plan.proposal(), &result.anchors);
    History::new()
        .apply(&mut applied, edit)
        .expect("the batch applies");
    for flat in [table.tier_index, culet] {
        assert_eq!(
            applied.tiers[flat].angle_deg.to_bits(),
            design.tiers[flat].angle_deg.to_bits(),
            "tier {flat} keeps its angle"
        );
    }

    // ...and the table is still its own facet, on the crown side, above the girdle.
    let after = analyze(&applied, false).expect("the shifted stone solves and closes");
    let ring = after
        .flats
        .iter()
        .find(|flat| flat.tier_index == table.tier_index)
        .expect("the table keeps its own facet");
    let (_, girdle_top) = after.girdle_band.expect("the girdle stays alive");
    assert_eq!(ring.side, FacetSide::Crown);
    assert!(
        ring.height > girdle_top,
        "table at {} under the girdle top {girdle_top}",
        ring.height
    );
}

#[test]
fn follow_pavilion_moves_the_table_height_with_the_crown() {
    // The default policy: the crown follows the pavilion's stretch, so the whole stone is
    // the original stretched vertically. The table keeps its angle and its size; its
    // height follows the crown (the refit puts it where the stretched crown has it).
    let design = brilliant_at_172();
    let (plan, result) = check_index(&design, 1.7681, CrownShift::default());
    assert!(plan.crown.follow_pavilion);
    assert_eq!(
        result.validity.status,
        ValidityStatus::Valid,
        "{:?}",
        result.validity.reasons
    );

    let table = tier_named(&design, "Table");
    assert!(
        result
            .anchors
            .iter()
            .any(|anchor| anchor.tier_index == table),
        "the table's height is refitted along with the crown: {:?}",
        result.anchors
    );

    let figures = result.validity.figures;
    let (table_was, table_now) = (
        figures.table_was.expect("the table is measured"),
        figures.table_now.expect("the table is measured"),
    );
    assert!(
        (table_now - table_was).abs() <= 0.01 * table_was,
        "table {table_was} % became {table_now} %"
    );
    let (ratio_was, ratio_now) = (
        figures.ratio_was.expect("the silhouette is measured"),
        figures.ratio_now.expect("the silhouette is measured"),
    );
    assert!(
        (ratio_now - ratio_was).abs() <= 0.03 * ratio_was,
        "crown-to-pavilion {ratio_was} became {ratio_now}"
    );
    let (depth_was, depth_now) = (
        figures.depth_was.expect("the depth is measured"),
        figures.depth_now.expect("the depth is measured"),
    );
    assert!(
        depth_now < depth_was,
        "a higher index gives a shallower stone: {depth_was} % became {depth_now} %"
    );
    let headline = result.validity.headline();
    assert!(headline.contains(", depth "), "{headline}");

    // The table's ANGLE is the one it had, bit for bit.
    let mut applied = design.clone();
    let edit = apply_with_anchors(&design, &plan.proposal(), &result.anchors);
    History::new()
        .apply(&mut applied, edit)
        .expect("the batch applies");
    assert_eq!(
        applied.tiers[table].angle_deg.to_bits(),
        design.tiers[table].angle_deg.to_bits(),
        "the table keeps its angle"
    );
}

#[test]
fn the_owners_emerald_case_keeps_a_live_girdle_and_closes() {
    // n_D 1.72 to 1.5791 scaling the crown by the critical-angle ratio: the crown and
    // pavilion went steeper at fixed masts and the girdle disappeared.
    let design = brilliant_at_172();
    let crown = CrownShift {
        fraction: 0.0,
        scale_by_ratio: true,
        follow_pavilion: false,
    };
    let (plan, result) = check_index(&design, 1.5791, crown);
    assert!(
        result
            .validity
            .reasons
            .iter()
            .all(|reason| !is_structural_failure(reason)),
        "{:?}",
        result.validity.reasons
    );
    assert!(
        !result.anchors.is_empty(),
        "the pinned facets must be re-anchored"
    );

    // The girdle is live in the retargeted stone and its overall figure is exactly what it
    // was: every facet turns about the wall vertices that figure is read from. That figure
    // cannot see the band run out between the walls, and the Shift result does exactly
    // that, so the gate must refuse it for the corners.
    let figures = result.validity.figures;
    let was = figures.girdle_was.expect("the design has a girdle");
    let now = figures.girdle_now.expect("the girdle stays alive");
    assert!((was - now).abs() < 1e-3, "girdle {was} % became {now} %");
    assert_eq!(result.validity.status, ValidityStatus::Invalid);
    assert!(
        result
            .validity
            .reasons
            .iter()
            .any(|reason| matches!(reason, InvalidReason::GirdleThinAtCorners { .. })),
        "{:?}",
        result.validity.reasons
    );
    assert_corners_kept(&result);
    assert_ends_valid(&design, &plan, &result);
}

#[test]
fn a_valid_verdict_never_hides_a_pinched_girdle() {
    // Retargets of the 1.72 stone both ways, small and large, crown moved and not: the
    // verdict is valid only when the corners keep their thickness, and refuses with that
    // reason when they do not.
    let design = brilliant_at_172();
    let ratio = CrownShift {
        fraction: 0.0,
        scale_by_ratio: true,
        follow_pavilion: false,
    };
    let third = CrownShift {
        fraction: 0.33,
        scale_by_ratio: false,
        follow_pavilion: false,
    };
    for (n_to, crown) in [
        (1.5791, ratio),
        (1.7681, third),
        (1.62, CrownShift::default()),
        (1.9, CrownShift::default()),
        (1.45, ratio),
    ] {
        let (_, result) = check_index(&design, n_to, crown);
        assert_corners_kept(&result);
    }
}

/// How far apart two readings of a level girdle's thickness may be, in percent of the width.
///
/// The planes are `f32`, so the corner vertices of a level girdle stand a few 1e-7 off the
/// ideal level line (about 1.6e-5 % of this 2.0 wide stone, and the extreme vertices that the
/// overall figure reads differ from the corners by that much). A thousandth of a percent is
/// some 60 times that noise and still 2000 times under the 2.36 % being compared.
const LEVEL_NOISE_PERCENT: f64 = 1e-3;

/// The girdle walls of `design` whose corners read under half of the thickest wall's, one per
/// line with plane, normal, reading and ring, for a failure message. Empty when every wall
/// reads alike.
fn thin_walls_report(design: &Design) -> String {
    let Ok(analysis) = analyze(design, false) else {
        return "the design does not close".to_string();
    };
    let SolidStatus::Closed(mesh) = build_solid_mesh(&analysis.planes) else {
        return "the design does not close".to_string();
    };
    let resolution = corner_resolution(&analysis.planes);
    let readings: Vec<_> = girdle_walls(design, &analysis.ranges, &analysis.planes, &mesh)
        .filter_map(|wall| {
            let thinnest = wall_thinnest_within(wall.ring, wall.normal, resolution)?;
            Some((wall, thinnest))
        })
        .collect();
    let thickest = readings.iter().map(|&(_, t)| t).fold(0.0_f64, f64::max);
    readings
        .iter()
        .filter(|&&(_, thinnest)| thinnest < 0.5 * thickest)
        .map(|(wall, thinnest)| {
            format!(
                "plane {}: normal {:?}, thinnest {thinnest}, ring {:?}",
                wall.plane, wall.normal, wall.ring
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn a_level_girdle_keeps_its_thinnest_point_through_a_shift_either_way() {
    // Every facet cuts its wall along a level line, and a facet turned about its girdle
    // edge keeps that line: the band is as thick between the walls after the retarget as it
    // was, whichever way the pavilion turns.
    let design = level_brilliant_at_172();
    let original = analyze(&design, false).expect("the level brilliant closes");
    let (average, thinnest) = (
        original.girdle_percent.expect("a girdle"),
        original
            .girdle_thinnest_percent
            .expect("a live girdle wall"),
    );
    assert!(
        (average - thinnest).abs() < LEVEL_NOISE_PERCENT,
        "a level girdle is as thin at its corners as it measures: {thinnest} % against \
         {average} %; the walls that read thin:\n{}",
        thin_walls_report(&design)
    );
    for n_to in [1.7681, 1.65] {
        let (_, result) = check_index(&design, n_to, CrownShift::default());
        let figures = result.validity.figures;
        let (was, now) = (
            figures.thinnest_was.expect("measured"),
            figures.thinnest_now.expect("the retargeted stone closes"),
        );
        assert!(
            (was - now).abs() < LEVEL_NOISE_PERCENT,
            "n {n_to}: thinnest point {was} % became {now} %"
        );
        assert!(
            result
                .validity
                .reasons
                .iter()
                .all(|reason| !matches!(reason, InvalidReason::GirdleThinAtCorners { .. })),
            "n {n_to}: {:?}",
            result.validity.reasons
        );
    }
}

/// The owner's rule: a retarget ends valid in both modes. Shift is valid, or it is not
/// and the dialog says to use Optimize, and Optimize then offers only valid options.
fn assert_ends_valid(design: &Design, plan: &RetargetPlan, result: &RetargetCheck) {
    use crate::retarget::{
        search::{SearchInputs, SearchSettings, run_search},
        view::CheckView,
    };
    match result.validity.status {
        ValidityStatus::Valid => {}
        ValidityStatus::Invalid => {
            let view = CheckView::from_check(result).with_optimize_hint();
            assert!(
                view.details
                    .iter()
                    .any(|line| line.contains("Shift alone is not valid here")),
                "{:?}",
                view.details
            );
            let settings = SearchSettings {
                evaluations: 24,
                keep: 2,
                ..SearchSettings::default()
            };
            let inputs = SearchInputs {
                design,
                plan,
                current_gem: None,
                lighting: LightingPreset::RingLights,
                settings: &settings,
            };
            let report = run_search(
                &inputs,
                &std::sync::atomic::AtomicBool::new(false),
                &|_, _| {},
            )
            .expect("the search runs");
            assert_ne!(report.candidates, Vec::new(), "{:?}", report.notes());
            for candidate in &report.candidates {
                assert_eq!(candidate.validity.status, ValidityStatus::Valid);
            }
        }
        ValidityStatus::Unchecked => panic!("the design can be analysed"),
    }
}

#[test]
fn the_hinge_keeps_the_girdle_where_the_plain_shift_destroys_it() {
    let design = brilliant();
    let original = analyze(&design, true).unwrap();
    let target = selection("Quartz").resolve(&BuiltinMaterials);
    let crown = CrownShift {
        fraction: 0.0,
        scale_by_ratio: true,
        follow_pavilion: false,
    };
    let plan = build_plan(&design, &target, crown, &[]);

    // The plain shift: angles only, masts left alone.
    let mut plain = design.clone();
    for row in plan.rows.iter().filter(|row| row.moves) {
        plain.tiers[row.tier_index].angle_deg = row.new_angle;
    }
    let plain_reasons = judge(&design, &original, &analyze(&plain, false));
    assert!(
        !plain_reasons.is_empty(),
        "leaving the masts alone must wreck this stone"
    );

    // The anchored candidate.
    let current = indicatrix::optics::materials::GemMaterial::diamond();
    let inputs = CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: Some(&current),
        lighting: LightingPreset::RingLights,
    };
    let result = check_retarget(&inputs);
    assert_ne!(result.anchors, Vec::new());
    // Whatever the verdict, the anchored candidate keeps a live girdle.
    assert!(
        !result.validity.reasons.contains(&InvalidReason::GirdleGone),
        "{:?}",
        result.validity.reasons
    );
}

#[test]
fn applying_the_checked_anchors_is_one_undo_step() {
    let mut design = brilliant();
    let original = design.clone();
    let (plan, result) = check(&design, "Sapphire", CrownShift::default());
    let edit = apply_with_anchors(&design, &plan.proposal(), &result.anchors);
    let mut history = History::new();
    history.apply(&mut design, edit).expect("applies");
    assert!(history.undo(&mut design).unwrap());
    assert_eq!(design, original);
    assert!(!history.can_undo());
}

#[test]
fn metrics_cover_all_three_columns_when_the_design_names_a_material() {
    let design = brilliant();
    let (_, result) = check(&design, "Sapphire", CrownShift::default());
    assert!(result.metrics.current_in_current.is_some());
    assert!(result.metrics.current_in_target.is_some());
    assert!(result.metrics.retargeted_in_target.is_some());
    assert_eq!(result.metrics.cells().len(), 9);
}

#[test]
fn without_a_current_material_the_first_column_is_missing() {
    let design = brilliant();
    let target = selection("Sapphire").resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &target, CrownShift::default(), &[]);
    let inputs = CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    };
    let result = check_retarget(&inputs);
    assert!(result.metrics.current_in_current.is_none());
    assert!(result.metrics.current_in_target.is_some());
}

#[test]
fn a_design_that_does_not_solve_is_unchecked_and_keeps_the_plain_edit() {
    // A pavilion tier with no anchor at all: the design has no scale reference.
    let mut design = brilliant();
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::MeetExisting;
    }
    let (_, result) = check(&design, "Sapphire", CrownShift::default());
    assert_eq!(result.validity.status, ValidityStatus::Unchecked);
    assert_eq!(result.anchors, Vec::new());
    assert!(result.allows_apply());
}

#[test]
fn a_stop_request_abandons_the_check() {
    let design = brilliant();
    let target = selection("Sapphire").resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &target, CrownShift::default(), &[]);
    let inputs = CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    };
    assert!(run_check(&inputs, None, &|| true).is_none());
}

#[test]
fn a_cached_analysis_is_reused_and_returned() {
    let design = brilliant();
    let target = selection("Sapphire").resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &target, CrownShift::default(), &[]);
    let inputs = CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    };
    let (first, _) = run_check(&inputs, None, &|| false).unwrap();
    let (second, _) = run_check(&inputs, Some(Arc::clone(&first)), &|| false).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn the_ladder_is_skipped_when_even_the_largest_thickening_cannot_cure_the_overall_girdle() {
    let allowance = GirdleAllowance::standard();
    let thin = |was_percent: f64, now_percent: f64| InvalidReason::GirdleTooThin {
        was_percent,
        now_percent,
    };
    // Lost over 60 % of a 4 % band: 1.5 + 0.4 stays under half of 4.0.
    assert!(!thickening_can_cure(&thin(4.0, 1.5), allowance));
    // Close enough (1.7 + 0.4 = 2.1 >= 2.0): the +10 % rung can lift it over.
    assert!(thickening_can_cure(&thin(4.0, 1.7), allowance));
    // The corner and gone cases always keep the ladder.
    assert!(thickening_can_cure(
        &InvalidReason::GirdleThinAtCorners {
            was_percent: 4.0,
            now_percent: 0.0,
        },
        allowance
    ));
    assert!(thickening_can_cure(&InvalidReason::GirdleGone, allowance));
    // A reason a thicker girdle has nothing to do with.
    assert!(!thickening_can_cure(&InvalidReason::NotClosed, allowance));
}
