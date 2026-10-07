//! Tests of the retarget validity gate.

use super::*;
use crate::retarget::{CrownShift, build_plan};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{
    BuiltinMaterials, ConstraintTier, MaterialSelection, PreformSpec, ScheduleMeta,
    design::hinge::TierFacets,
};

fn brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn two_tier_design() -> Design {
    let tier = |name: &str, angle: f64| ConstraintTier {
        angle_deg: angle,
        name: name.to_string(),
        indices: Vec::new(),
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    };
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::default(),
        vec![tier("Table", 0.0), tier("Crown Main", 34.0)],
    )
}

/// A hand-built analysis of a two-tier stone: a table that is alive and a crown main
/// with eight live facets, a 2 % girdle and a 56 % table.
fn good_analysis() -> StoneAnalysis {
    StoneAnalysis {
        solved: Vec::new(),
        planes: Vec::new(),
        ranges: Vec::new(),
        girdle_percent: Some(2.0),
        girdle_thinnest_percent: Some(0.8),
        table_percent: Some(56.0),
        crown_height: Some(0.16),
        pavilion_depth: Some(0.44),
        crown_to_pavilion: Some(0.16 / 0.44),
        depth_percent: Some(60.0),
        facets: SolidFacets {
            tiers: vec![
                TierFacets {
                    total: 1,
                    alive: 1,
                    first_live_normal_y: Some(1.0),
                },
                TierFacets {
                    total: 8,
                    alive: 8,
                    first_live_normal_y: Some(0.8),
                },
            ],
            preform_alive: 0,
        },
        flats: vec![FlatRing {
            tier_index: 0,
            side: FacetSide::Crown,
            area: 1.0,
            height: 0.4,
        }],
        girdle_band: Some((-0.01, 0.02)),
        undersized: BTreeSet::new(),
        hinges: BTreeMap::new(),
    }
}

#[test]
fn identical_stones_are_valid() {
    let design = two_tier_design();
    let original = good_analysis();
    let reasons = judge(&design, &original, &Ok(good_analysis()));
    assert_eq!(reasons, Vec::new());
}

#[test]
fn a_vanished_girdle_is_reported_in_plain_english() {
    let design = two_tier_design();
    let original = good_analysis();
    let mut candidate = good_analysis();
    candidate.girdle_percent = None;
    let reasons = judge(&design, &original, &Ok(candidate));
    assert_eq!(reasons, vec![InvalidReason::GirdleGone]);
    assert_eq!(reasons[0].message(), "The girdle disappears.");
}

#[test]
fn a_girdle_under_half_of_the_original_is_too_thin_but_half_is_fine() {
    let design = two_tier_design();
    let original = good_analysis();

    let mut thin = good_analysis();
    thin.girdle_percent = Some(0.9);
    let reasons = judge(&design, &original, &Ok(thin));
    assert_eq!(
        reasons,
        vec![InvalidReason::GirdleTooThin {
            was_percent: 2.0,
            now_percent: 0.9
        }]
    );
    assert!(reasons[0].message().contains("0.9"));
    assert!(reasons[0].message().contains("2.0"));

    let mut half = good_analysis();
    half.girdle_percent = Some(1.0);
    assert_eq!(judge(&design, &original, &Ok(half)), Vec::new());
}

#[test]
fn a_girdle_that_pinches_out_at_its_corners_is_refused_though_its_overall_figure_stays() {
    let design = two_tier_design();
    let original = good_analysis();
    // The overall girdle figure is exactly what it was; the thinnest point is gone.
    let mut pinched = good_analysis();
    pinched.girdle_thinnest_percent = Some(0.0);
    assert_eq!(pinched.girdle_percent, original.girdle_percent);
    let reasons = judge(&design, &original, &Ok(pinched));
    assert_eq!(
        reasons,
        vec![InvalidReason::GirdleThinAtCorners {
            was_percent: 0.8,
            now_percent: 0.0
        }]
    );
    let message = reasons[0].message();
    assert!(message.contains("knife edge"), "{message}");
    assert!(message.contains("0.80"), "{message}");
}

#[test]
fn a_thinnest_point_under_half_of_the_original_is_too_thin_but_half_is_fine() {
    let design = two_tier_design();
    let original = good_analysis();

    let mut thin = good_analysis();
    thin.girdle_thinnest_percent = Some(0.39);
    let reasons = judge(&design, &original, &Ok(thin));
    assert_eq!(
        reasons,
        vec![InvalidReason::GirdleThinAtCorners {
            was_percent: 0.8,
            now_percent: 0.39
        }]
    );
    let message = reasons[0].message();
    assert!(message.contains("too thin at its corners"), "{message}");
    assert!(message.contains("0.39"), "{message}");
    assert!(message.contains("0.80"), "{message}");

    let mut half = good_analysis();
    half.girdle_thinnest_percent = Some(0.4);
    assert_eq!(judge(&design, &original, &Ok(half)), Vec::new());
    let mut thicker = good_analysis();
    thicker.girdle_thinnest_percent = Some(1.5);
    assert_eq!(judge(&design, &original, &Ok(thicker)), Vec::new());
}

#[test]
fn a_girdle_that_was_a_knife_edge_already_or_has_no_thinnest_point_is_not_judged_on_it() {
    let design = two_tier_design();
    let mut knife = good_analysis();
    knife.girdle_thinnest_percent = Some(0.0);
    assert_eq!(judge(&design, &knife, &Ok(good_analysis())), Vec::new());
    assert_eq!(judge(&design, &knife, &Ok(knife.clone())), Vec::new());

    let original = good_analysis();
    let mut unmeasured = good_analysis();
    unmeasured.girdle_thinnest_percent = None;
    assert_eq!(judge(&design, &original, &Ok(unmeasured)), Vec::new());
    assert_eq!(
        judge(&design, &unmeasured_original(), &Ok(knife)),
        Vec::new()
    );
}

/// An original stone whose thinnest point could not be measured.
fn unmeasured_original() -> StoneAnalysis {
    StoneAnalysis {
        girdle_thinnest_percent: None,
        ..good_analysis()
    }
}

#[test]
fn the_headline_names_the_thinnest_point_only_when_it_differs_from_the_girdle_figure() {
    let valid = |figures: ValidityFigures| RetargetValidity {
        status: ValidityStatus::Valid,
        reasons: Vec::new(),
        warnings: Vec::new(),
        figures,
        strategy: RetargetStrategy::Anchored,
    };
    let mut figures = ValidityFigures {
        girdle_was: Some(5.9),
        girdle_now: Some(5.9),
        thinnest_was: Some(0.25),
        thinnest_now: Some(0.4),
        table_was: Some(56.0),
        table_now: Some(56.0),
        ..ValidityFigures::default()
    };
    assert_eq!(
        valid(figures).headline(),
        "Valid: girdle 5.9 % (was 5.9 %), thinnest point 0.40 % (was 0.25 %), table 56 % (was 56 %)"
    );

    // A thinnest point that is the girdle figure says nothing new.
    figures.thinnest_now = Some(5.93);
    assert_eq!(
        valid(figures).headline(),
        "Valid: girdle 5.9 % (was 5.9 %), table 56 % (was 56 %)"
    );
    // So does a stone with no measured thinnest point.
    figures.thinnest_now = None;
    assert_eq!(
        valid(figures).headline(),
        "Valid: girdle 5.9 % (was 5.9 %), table 56 % (was 56 %)"
    );
}

#[test]
fn the_headline_closes_with_the_depth_clause_only_when_both_depth_figures_are_known() {
    let valid = |figures: ValidityFigures| RetargetValidity {
        status: ValidityStatus::Valid,
        reasons: Vec::new(),
        warnings: Vec::new(),
        figures,
        strategy: RetargetStrategy::Anchored,
    };
    let mut figures = ValidityFigures {
        girdle_was: Some(2.3),
        girdle_now: Some(2.1),
        table_was: Some(56.0),
        table_now: Some(56.0),
        depth_was: Some(58.0),
        depth_now: Some(61.0),
        ..ValidityFigures::default()
    };
    assert_eq!(
        valid(figures).headline(),
        "Valid: girdle 2.1 % (was 2.3 %), table 56 % (was 56 %), depth 61 % (was 58 %)"
    );

    // The clause follows the thinnest-point clause and the table clause, in that order.
    figures.girdle_now = Some(5.9);
    figures.girdle_was = Some(5.9);
    figures.thinnest_was = Some(0.25);
    figures.thinnest_now = Some(0.4);
    assert_eq!(
        valid(figures).headline(),
        "Valid: girdle 5.9 % (was 5.9 %), thinnest point 0.40 % (was 0.25 %), table 56 % (was 56 %), depth 61 % (was 58 %)"
    );

    // One missing figure drops the clause and leaves the old headline exactly.
    figures.depth_now = None;
    assert_eq!(
        valid(figures).headline(),
        "Valid: girdle 5.9 % (was 5.9 %), thinnest point 0.40 % (was 0.25 %), table 56 % (was 56 %)"
    );
}

#[test]
fn the_detail_lines_quote_the_crown_to_pavilion_ratio_before_the_warnings() {
    let verdict = |status: ValidityStatus, figures: ValidityFigures| RetargetValidity {
        status,
        reasons: vec![InvalidReason::GirdleGone, InvalidReason::NotClosed],
        warnings: vec!["Careful.".to_string()],
        figures,
        strategy: RetargetStrategy::Anchored,
    };
    let with_ratio = ValidityFigures {
        ratio_was: Some(0.36),
        ratio_now: Some(0.351),
        ..ValidityFigures::default()
    };
    let valid = verdict(ValidityStatus::Valid, with_ratio).detail_lines();
    assert_eq!(valid.last().map(String::as_str), Some("Careful."));
    assert!(
        valid.contains(&"Crown-to-pavilion ratio 0.35 (was 0.36).".to_string()),
        "{valid:?}"
    );
    let ratio_at = valid
        .iter()
        .position(|line| line.starts_with("Crown-to"))
        .unwrap();
    assert_eq!(
        ratio_at + 2,
        valid.len(),
        "the ratio comes just before the warnings"
    );

    let invalid = verdict(ValidityStatus::Invalid, with_ratio).detail_lines();
    assert_eq!(invalid[0], InvalidReason::NotClosed.message());
    assert!(
        invalid[1].starts_with("Crown-to-pavilion ratio"),
        "{invalid:?}"
    );

    // Without both ratios, or when unchecked, there is no ratio line.
    let without = verdict(ValidityStatus::Valid, ValidityFigures::default()).detail_lines();
    assert!(!without.iter().any(|line| line.starts_with("Crown-to")));
    let unchecked = verdict(ValidityStatus::Unchecked, with_ratio).detail_lines();
    assert_eq!(unchecked, vec!["Careful.".to_string()]);
}

#[test]
fn the_brilliants_analysis_carries_the_silhouette_figures() {
    let analysis = analyze(&brilliant(), false).expect("the template solves and closes");
    let crown = analysis.crown_height.expect("a crown height");
    let pavilion = analysis.pavilion_depth.expect("a pavilion depth");
    let ratio = analysis.crown_to_pavilion.expect("a ratio");
    assert!((ratio - crown / pavilion).abs() < 1e-12);
    let depth = analysis.depth_percent.expect("a depth");
    assert!((40.0..120.0).contains(&depth), "{depth}");
}

#[test]
fn a_table_that_drops_below_the_girdle_is_reported() {
    let design = two_tier_design();
    let original = good_analysis();
    let mut candidate = good_analysis();
    candidate.flats[0].height = 0.01;
    let reasons = judge(&design, &original, &Ok(candidate));
    assert_eq!(
        reasons,
        vec![InvalidReason::FlatOffGirdle {
            side: FacetSide::Crown
        }]
    );
    assert_eq!(
        reasons[0].message(),
        "The table would sit below the top of the girdle."
    );
}

#[test]
fn a_tier_that_loses_all_or_some_facets_is_named() {
    let design = two_tier_design();
    let original = good_analysis();

    let mut all = good_analysis();
    all.facets.tiers[1].alive = 0;
    assert_eq!(
        judge(&design, &original, &Ok(all)),
        vec![InvalidReason::TierLost {
            name: "Crown Main".to_string()
        }]
    );

    let mut some = good_analysis();
    some.facets.tiers[1].alive = 5;
    let reasons = judge(&design, &original, &Ok(some));
    assert_eq!(
        reasons,
        vec![InvalidReason::TierPartlyLost {
            name: "Crown Main".to_string(),
            lost: 3,
            total: 8
        }]
    );
    assert_eq!(reasons[0].message(), "Crown Main: 3 of 8 facets disappear.");
}

#[test]
fn a_tier_that_was_already_missing_a_facet_is_not_blamed_again() {
    let design = two_tier_design();
    let mut original = good_analysis();
    original.facets.tiers[1].alive = 5;
    let mut candidate = good_analysis();
    candidate.facets.tiers[1].alive = 5;
    assert_eq!(judge(&design, &original, &Ok(candidate)), Vec::new());
}

#[test]
fn newly_undersized_facets_are_reported_but_old_ones_are_not() {
    let design = two_tier_design();
    let mut original = good_analysis();
    original.undersized.insert(0);
    let mut candidate = good_analysis();
    candidate.undersized.insert(0);
    candidate.undersized.insert(1);
    assert_eq!(
        judge(&design, &original, &Ok(candidate)),
        vec![InvalidReason::FacetsTooSmall {
            name: "Crown Main".to_string()
        }]
    );
}

#[test]
fn a_candidate_that_does_not_close_has_exactly_that_reason() {
    let design = two_tier_design();
    let original = good_analysis();
    let reasons = judge(&design, &original, &Err(InvalidReason::NotClosed));
    assert_eq!(reasons, vec![InvalidReason::NotClosed]);
}

#[test]
fn the_headline_quotes_the_figures_or_the_first_reason() {
    let valid = RetargetValidity {
        status: ValidityStatus::Valid,
        reasons: Vec::new(),
        warnings: Vec::new(),
        figures: ValidityFigures {
            girdle_was: Some(2.3),
            girdle_now: Some(2.1),
            table_was: Some(56.0),
            table_now: Some(56.0),
            ..ValidityFigures::default()
        },
        strategy: RetargetStrategy::Anchored,
    };
    assert_eq!(
        valid.headline(),
        "Valid: girdle 2.1 % (was 2.3 %), table 56 % (was 56 %)"
    );

    let invalid = RetargetValidity {
        status: ValidityStatus::Invalid,
        reasons: vec![
            InvalidReason::GirdleGone,
            InvalidReason::TierLost {
                name: "Star".to_string(),
            },
        ],
        warnings: vec!["Careful.".to_string()],
        figures: ValidityFigures::default(),
        strategy: RetargetStrategy::Anchored,
    };
    assert_eq!(invalid.headline(), "Not valid: The girdle disappears.");
    assert_eq!(
        invalid.detail_lines(),
        vec![
            "Star: all its facets disappear.".to_string(),
            "Careful.".to_string()
        ]
    );
    assert!(!invalid.allows_apply());
    assert!(valid.allows_apply());
}

#[test]
fn polygon_area_of_a_unit_square_is_one() {
    let square = [
        DVec3::new(0.0, 1.0, 0.0),
        DVec3::new(1.0, 1.0, 0.0),
        DVec3::new(1.0, 1.0, 1.0),
        DVec3::new(0.0, 1.0, 1.0),
    ];
    assert!((polygon_area(&square) - 1.0).abs() < 1e-12);
}

#[test]
fn analysing_the_brilliant_finds_a_girdle_a_table_and_a_culet() {
    let design = brilliant();
    let analysis = analyze(&design, true).expect("the template solves and closes");
    assert!(analysis.girdle_percent.is_some());
    assert!(analysis.table_percent.is_some());
    assert_eq!(analysis.flats.len(), 2);
    assert!(analysis.girdle_band.is_some());
    assert_eq!(analysis.facets.preform_alive, 0);
    assert!(!analysis.hinges.is_empty());
    let table = analysis
        .flats
        .iter()
        .find(|flat| flat.side == FacetSide::Crown)
        .unwrap();
    let (_, high) = analysis.girdle_band.unwrap();
    assert!(table.height > high);
}

#[test]
fn the_brilliants_thinnest_point_is_far_under_its_overall_girdle_figure() {
    // The break facets stand between the walls' positions, so the band is some 6 % of the
    // width at its thickest vertex pair and a fraction of a percent at the corners.
    let analysis = analyze(&brilliant(), false).expect("the template solves and closes");
    let average = analysis.girdle_percent.expect("a girdle");
    let thinnest = analysis
        .girdle_thinnest_percent
        .expect("a live girdle wall");
    assert!(thinnest > KNIFE_EDGE_PERCENT, "{thinnest}");
    assert!(
        thinnest < average / 5.0,
        "thinnest {thinnest} % against {average} %"
    );
}

#[test]
fn an_unanchored_steep_crown_is_judged_invalid_and_the_untouched_design_valid() {
    let design = brilliant();
    let original = analyze(&design, true).unwrap();

    // The same design is its own best candidate.
    let same = analyze(&design, false);
    assert_eq!(judge(&design, &original, &same), Vec::new());

    // A crown main tilted to 60 degrees with its mast left where it was cuts into the
    // girdle band: the old behaviour of the dialog.
    let mut steep = design.clone();
    let main = steep
        .tiers
        .iter()
        .position(|tier| tier.name == "Crown Main")
        .unwrap();
    steep.tiers[main].angle_deg = 60.0;
    let reasons = judge(&design, &original, &analyze(&steep, false));
    assert!(
        !reasons.is_empty(),
        "a crown cutting into the girdle must be refused"
    );
}

#[test]
fn a_plan_on_the_same_material_is_valid_without_any_change() {
    let mut design = brilliant();
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let same = design.material.resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &same, CrownShift::default(), &[]);
    assert!(
        plan.rows
            .iter()
            .all(|row| (row.new_angle - row.old_angle).abs() < 1e-12)
    );
}

#[test]
fn split_at_girdle_moves_crown_hinges_up_and_pavilion_hinges_down_by_half() {
    let design = brilliant();
    let original = analyze(&design, true).expect("the brilliant solves and closes");
    let split = original.split_at_girdle(0.1);
    assert_ne!(original.hinges.len(), 0);
    for (index, hinge) in &original.hinges {
        let moved = split.hinges[index].point;
        let expected = match hinge.side {
            FacetSide::Crown => 0.05,
            FacetSide::Pavilion => -0.05,
        };
        assert!(
            (moved.y - hinge.point.y - expected).abs() < 1e-12,
            "tier {index}"
        );
        assert_eq!(moved.x.to_bits(), hinge.point.x.to_bits());
        assert_eq!(moved.z.to_bits(), hinge.point.z.to_bits());
    }
}

#[test]
fn split_at_girdle_keeps_every_plane_through_its_shifted_hinge() {
    let design = brilliant();
    let original = analyze(&design, true).expect("the brilliant solves and closes");
    let split = original.split_at_girdle(0.1);
    let mut checked = 0;
    for (index, hinge) in &original.hinges {
        for plane in original.ranges[*index].clone() {
            let (normal, offset) = original.planes[plane];
            if (normal.dot(hinge.point) - offset).abs() > 1e-6 {
                continue;
            }
            let (split_normal, split_offset) = split.planes[plane];
            assert_eq!(split_normal, normal);
            let residual = split_normal.dot(split.hinges[index].point) - split_offset;
            assert!(residual.abs() < 1e-9, "tier {index}: {residual}");
            checked += 1;
        }
    }
    assert_ne!(checked, 0, "some hinge lies on a facet plane");
}

#[test]
fn a_split_stone_keeps_its_figures_and_the_band_grows_by_the_change() {
    let design = brilliant();
    let original = analyze(&design, true).expect("the brilliant solves and closes");
    let split = original.split_at_girdle(0.1);
    assert_eq!(split.table_percent, original.table_percent);
    assert_eq!(split.crown_to_pavilion, original.crown_to_pavilion);
    let ((lo, hi), (split_lo, split_hi)) = (
        original.girdle_band.expect("a girdle"),
        split.girdle_band.expect("a girdle"),
    );
    assert!(((split_hi - split_lo) - (hi - lo) - 0.1).abs() < 1e-12);
}

#[test]
fn split_at_girdle_shifts_a_near_vertical_plane_by_its_tiers_side_not_its_lean() {
    let design = brilliant();
    let mut original = analyze(&design, true).expect("the brilliant solves and closes");
    let (&index, hinge) = original
        .hinges
        .iter()
        .find(|(_, hinge)| hinge.side == FacetSide::Pavilion)
        .expect("the brilliant has a pavilion hinge");
    let side = hinge.side;
    let plane = original.ranges[index].start;
    // A pavilion plane whose normal leans a hair UP (above the old 1e-9 threshold): the old
    // rule moved it with the crown half, the hinge moves with the pavilion half.
    let lean = 1e-8;
    original.planes[plane].0 = glam::DVec3::new(0.0, lean, 1.0).normalize();
    let normal_y = original.planes[plane].0.y;
    let offset = original.planes[plane].1;
    let split = original.split_at_girdle(0.1);
    let expected_shift = match side {
        FacetSide::Crown => 0.05,
        FacetSide::Pavilion => -0.05,
    };
    let moved = split.planes[plane].1 - offset;
    assert!(
        normal_y.mul_add(-expected_shift, moved).abs() < 1e-14,
        "{moved} vs {}",
        normal_y * expected_shift
    );
}
