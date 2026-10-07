//! Tests for [`super::build_proposal`]/[`super::apply`], the plan and their shared helpers.

use super::{
    view::{NO_VALUE, NOT_CHANGED_LABEL, plan_row_view},
    *,
};
use crate::view_model::row_format::representative_crown_and_pavilion_angles_deg;
use indicatrix_cut_core::{
    BuiltinMaterials, ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
    crown_window_margin_deg, optics_hints::AngleGuard, retarget_angle_deg, tier_margin_deg,
};

/// "RBC-445" (PC 13.156) -- a small, genuinely meet-derived design (not a
/// synthetic worst case), the right fixture for exercising real editor behavior.
const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
     H PC 13.156  RBC-445\n\
     H Richard B Conley, Facets, Oct 2013 p10\n\
     a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
     a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
     a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
     a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
     a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
     a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
     a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
     a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
     a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
     a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
     a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
     a 0.000000 0.40674031 96 n E\n";

/// Builds a real `Design` with genuine meet-derived structure from a raw `.asc`
/// text: every tier keeps its file's real `MeetConstraint`, except that each
/// crown/pavilion/girdle `Block` present gets exactly one bootstrapped
/// `ScaleReference` (that block's first tier's real recorded mast) when the
/// file stated no explicit anchor of its own.
fn design_with_real_meet_structure(text: &str) -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(text).expect("fixture must parse");
    let mut inputs = indicatrix::geometry::meet_solver::meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && is_scale_reference(&t.constraint));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

fn rbc_445() -> Design {
    // The raw fixture text only carries the legacy `I 1.54` schedule field;
    // giving the design a real Diamond `MaterialSelection` makes
    // `effective_refractive_index` return Diamond's own resolved n_D (~2.417)
    // instead of the unrelated legacy 1.54 figure.
    let mut design = design_with_real_meet_structure(RBC_445);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

fn quartz() -> ResolvedMaterial {
    MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
    .resolve(&BuiltinMaterials)
}

fn diamond_n_d() -> f64 {
    indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap()
}

/// The RBC-445 tiers `classify_blocks` actually reports as `Block::Pavilion` --
/// notably NOT the two tiers authored at exactly -90 degrees (indices 2 and
/// 3): a facet at plus or minus 90 degrees from the girdle plane classifies as
/// `Block::Girdle`, not as an extreme pavilion angle.
fn true_pavilion_indices(design: &Design) -> Vec<usize> {
    let blocks = classify_blocks(&design.meet_tier_inputs());
    (0..design.tiers.len())
        .filter(|&i| blocks[i] == Block::Pavilion)
        .collect()
}

// --- RBC-445 diamond -> quartz lists every pavilion tier with the expected shift ---

#[test]
fn shift_mode_lists_every_true_pavilion_tier_with_the_expected_shift() {
    let design = rbc_445();
    let pavilion_indices = true_pavilion_indices(&design);
    assert_eq!(
        pavilion_indices,
        vec![0, 1, 4, 5, 6],
        "RBC-445's own -90 degree tiers (indices 2, 3) must classify as Girdle, not Pavilion"
    );

    let target = quartz();
    let n_from = design.effective_refractive_index();
    assert!((n_from - diamond_n_d()).abs() < 1e-9);
    let n_to = target.n_d;

    let proposal = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    )
    .expect("Shift mode never fails");

    for &index in &pavilion_indices {
        let row = proposal
            .rows
            .iter()
            .find(|r| r.tier_index == index)
            .unwrap_or_else(|| panic!("tier {index} must be in the proposal"));
        assert_eq!(row.block, Block::Pavilion);
        let old_angle = design.tiers[index].angle_deg;
        let expected = retarget_angle_deg(old_angle, n_from, n_to).clamp(-89.5, 89.5);
        assert!(
            (row.new_angle - expected).abs() < 1e-9,
            "tier {index}: old={old_angle} new={} expected={expected}",
            row.new_angle
        );
        // The margin-preserving formula must actually hold for every
        // tier that was not clamped.
        if expected.abs() < 89.5 - 1e-9 {
            let margin_before = tier_margin_deg(old_angle, n_from);
            let margin_after = tier_margin_deg(row.new_angle, n_to);
            assert!(
                (margin_after - margin_before).abs() < 1e-9,
                "tier {index}: margin drifted, before={margin_before} after={margin_after}"
            );
        }
    }

    // Girdle tiers (the two -90 degree ones) are never listed at all.
    assert!(
        !proposal
            .rows
            .iter()
            .any(|r| r.tier_index == 2 || r.tier_index == 3)
    );

    // The pavilion rows above do not depend on the crown policy; under the fixed policy
    // every crown tier's angle is unchanged.
    let fixed = build_proposal(
        &design,
        &target,
        CrownShift::fixed(),
        RetargetMode::Shift,
        &[],
    )
    .expect("Shift mode never fails");
    for row in fixed.rows.iter().filter(|r| r.block == Block::Crown) {
        assert!(
            (row.new_angle - row.old_angle).abs() < 1e-12,
            "tier {}: crown must stay put with CrownShift::fixed()",
            row.tier_index
        );
    }
}

#[test]
fn shift_mode_crown_fraction_moves_crown_by_the_same_constant_delta() {
    let design = rbc_445();
    let target = quartz();
    let n_from = design.effective_refractive_index();
    let n_to = target.n_d;
    let delta = critical_angle_deg(n_to) - critical_angle_deg(n_from);

    let crown = CrownShift {
        fraction: 0.5,
        ..CrownShift::fixed()
    };
    let proposal = build_proposal(&design, &target, crown, RetargetMode::Shift, &[]).unwrap();

    for row in proposal.rows.iter().filter(|r| r.block == Block::Crown) {
        let expected = 0.5f64.mul_add(delta, row.old_angle).clamp(-89.5, 89.5);
        assert!(
            (row.new_angle - expected).abs() < 1e-9,
            "tier {}",
            row.tier_index
        );
    }
}

#[test]
fn shift_mode_scale_by_ratio_scales_the_raw_crown_angle() {
    let design = rbc_445();
    let target = quartz();
    let n_from = design.effective_refractive_index();
    let n_to = target.n_d;
    let ratio = critical_angle_deg(n_to) / critical_angle_deg(n_from);

    let crown = CrownShift {
        scale_by_ratio: true,
        ..CrownShift::fixed()
    };
    let proposal = build_proposal(&design, &target, crown, RetargetMode::Shift, &[]).unwrap();

    for row in proposal.rows.iter().filter(|r| r.block == Block::Crown) {
        let expected = (row.old_angle * ratio).clamp(-89.5, 89.5);
        assert!(
            (row.new_angle - expected).abs() < 1e-9,
            "tier {}",
            row.tier_index
        );
    }
}

// --- Apply then undo leaves the design byte-identical ---

#[test]
fn apply_then_undo_leaves_the_design_byte_identical() {
    let mut design = rbc_445();
    let original = design.clone();
    let target = quartz();

    let proposal = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    )
    .unwrap();
    assert_ne!(proposal.rows, Vec::new());

    let edit = apply(&design, &proposal);
    let mut history = History::new();
    history
        .apply(&mut design, edit)
        .expect("retarget must apply");
    assert_ne!(
        design, original,
        "apply must have actually changed something"
    );

    assert!(history.undo(&mut design).unwrap());
    assert_eq!(
        design, original,
        "undo must restore the design byte-identically"
    );
}

// --- Risk badges at boundaries ---

#[test]
fn risk_badges_match_the_windowing_risk_boundaries() {
    // Same index in and out (a no-op shift), so each tier's angle IS its own
    // margin over the critical angle, chosen exactly at each boundary.
    let n = 1.5;
    let crit = critical_angle_deg(n);
    let cases = [
        (crit - 1.0, Risk::Windows),
        (crit + 0.0, Risk::Marginal),
        (crit + 1.999, Risk::Marginal),
        (crit + 2.0, Risk::Safe),
    ];

    let tiers = cases
        .iter()
        .enumerate()
        .map(|(i, &(theta, _))| ConstraintTier {
            angle_deg: -theta,
            name: format!("P{i}"),
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(1.0),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    let design = Design {
        material: MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: Some(n),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        },
        ..Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::default(),
            tiers,
        )
    };
    let target = ResolvedMaterial {
        gem: indicatrix::optics::materials::GemMaterial::diamond(),
        n_d: n,
        critical_angle_deg: crit,
    };

    let proposal = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    )
    .unwrap();
    for (row, &(_, expected_risk)) in proposal.rows.iter().zip(&cases) {
        assert_eq!(
            row.risk, expected_risk,
            "tier {}: margin {}",
            row.tier_index, row.margin_deg
        );
    }
}

// --- The anchored-tier refusal ---

#[test]
fn optimize_mode_refuses_anchored_tiers_and_lists_them() {
    let design = rbc_445();
    let target = quartz();
    let config = OptimizeConfig {
        max_evaluations: 4,
        ..OptimizeConfig::default()
    };

    let err = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Optimize(config),
        &[],
    )
    .expect_err("RBC-445's bootstrapped anchors must trigger a refusal");

    match err {
        RetargetError::AnchoredTiers(tiers) => {
            let indices: Vec<usize> = tiers.iter().map(|&(i, _)| i).collect();
            // Index 0 (pavilion "1") and index 7 (crown "A") are each the first
            // tier of their own block -- what the bootstrap pins as that
            // block's sole `ScaleReference` anchor.
            assert_eq!(indices, vec![0, 7]);
        }
        RetargetError::Solve(e) => panic!("expected AnchoredTiers, got Solve({e})"),
    }
}

#[test]
fn optimize_mode_succeeds_when_nothing_in_scope_is_anchored() {
    // A design with only a Girdle tier: `scope` is empty, so the anchored
    // check passes vacuously and `optimize_design` runs against an empty free
    // set rather than refusing.
    let tiers = vec![ConstraintTier {
        angle_deg: 90.0,
        name: "Girdle".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }];
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::default(),
        tiers,
    );
    let blocks = classify_blocks(&design.meet_tier_inputs());
    assert_eq!(blocks, vec![Block::Girdle]);

    let target = quartz();
    let config = OptimizeConfig {
        max_evaluations: 4,
        ..OptimizeConfig::default()
    };
    let proposal = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Optimize(config),
        &[],
    )
    .expect("no anchored tiers are in scope, so this must succeed");
    assert_eq!(proposal.rows, Vec::new());
}

// --- Golden `.asc` export for a retargeted design ---

#[test]
fn golden_asc_export_reflects_the_retargeted_pavilion_angles() {
    let mut design = rbc_445();
    let target = quartz();
    let n_from = design.effective_refractive_index();
    let n_to = target.n_d;

    let proposal = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    )
    .unwrap();
    let edit = apply(&design, &proposal);
    let mut history = History::new();
    history
        .apply(&mut design, edit)
        .expect("retarget must apply");

    let schedule = design
        .to_asc_schedule()
        .expect("retargeted design must still solve");
    let text = indicatrix_formats::asc::to_asc_string(&schedule)
        .expect("fixture tier names carry no whitespace, so the writer accepts them");

    for &index in &true_pavilion_indices(&design) {
        let expected = retarget_angle_deg(
            // The ORIGINAL angle: read it back from the proposal row, since
            // `design` has already been mutated in place.
            proposal
                .rows
                .iter()
                .find(|r| r.tier_index == index)
                .unwrap()
                .old_angle,
            n_from,
            n_to,
        )
        .clamp(-89.5, 89.5);
        assert!(
            (schedule.tiers[index].angle_deg - expected).abs() < 1e-6,
            "tier {index}: exported angle {} != expected {expected}",
            schedule.tiers[index].angle_deg
        );
    }
    // A real, non-empty `.asc` schedule carrying the retargeted design's header.
    assert!(text.contains("RBC-445"));
}

// --- build_proposal resolves a custom catalogue material's real RI instead
//     of the built-ins-only fallback ---

#[test]
fn build_proposal_uses_the_custom_materials_own_ri() {
    let mut design = rbc_445();
    // A custom catalogue material this design is "currently on" -- not one
    // of the thirteen built-ins `effective_refractive_index` alone can
    // resolve, so the plain (non-`_with`) path falls through to the
    // legacy `ScheduleMeta::refractive_index` (1.54 for this fixture)
    // instead of this material's real, much higher n_D.
    let my_garnet = GemMaterial::new_custom("My Garnet", 1.74, 0.024, 0.0, [0.0, 0.0, 0.0]);
    design.material = MaterialSelection {
        name: Some("My Garnet".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    let custom = vec![my_garnet];

    let target = quartz();
    let n_from_builtins_only = design.effective_refractive_index();
    let n_from_with_custom = design.effective_refractive_index_with(&custom);
    // Confirms the fixture actually exercises the bug: the two resolutions
    // must disagree, or this test would not catch a regression back to
    // the plain built-ins-only path.
    assert!((n_from_builtins_only - n_from_with_custom).abs() > 0.1);

    let plain = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    )
    .expect("Shift mode never fails");
    let fixed = build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &custom,
    )
    .expect("Shift mode never fails");

    // Same tiers, different angles: `build_proposal` (unchanged, still
    // built-ins-only) must NOT match `build_proposal` once
    // the custom material's RI actually differs from the legacy fallback.
    assert_eq!(plain.rows.len(), fixed.rows.len());
    assert!(
        plain
            .rows
            .iter()
            .zip(&fixed.rows)
            .any(|(p, f)| (p.new_angle - f.new_angle).abs() > 1e-6),
        "build_proposal must retarget from the custom material's own RI"
    );

    // And the fixed path's angles must match what shifting from the
    // custom-aware n_from directly would produce.
    let blocks = classify_blocks(&design.meet_tier_inputs());
    for row in &fixed.rows {
        let expected = shifted_angle(
            &design,
            row.tier_index,
            blocks[row.tier_index],
            n_from_with_custom,
            target.n_d,
            CrownShift::default(),
            plan::pavilion_stretch(&design, n_from_with_custom, target.n_d),
        );
        assert!((row.new_angle - expected).abs() < 1e-9);
    }
}

#[test]
fn retarget_scope_holds_only_flat_tier_positions_when_concave_tiers_exist() {
    let design = indicatrix_cut_core::Design::concave_fixture();
    let (scope, blocks) = retarget_scope(&design);
    assert_eq!(blocks.len(), design.tiers.len());
    assert!(scope.iter().all(|&i| i < design.tiers.len()));
    // The girdle (tier 0) is excluded; every other flat tier is in.
    assert_eq!(scope, vec![1, 2, 3, 4]);
}

// --- Flat tiers are listed, never moved ---

/// Every kind of crown policy. The first is the default (follow the pavilion), then the
/// explicit fractions (`follow_pavilion` off), the ratio rules, the fixed crown, and a
/// fraction with `follow_pavilion` left on (which must behave as follow).
fn crown_policies() -> [CrownShift; 7] {
    [
        CrownShift::default(),
        CrownShift {
            fraction: 0.33,
            ..CrownShift::fixed()
        },
        CrownShift {
            fraction: 1.0,
            ..CrownShift::fixed()
        },
        CrownShift {
            scale_by_ratio: true,
            ..CrownShift::fixed()
        },
        CrownShift {
            fraction: 0.5,
            scale_by_ratio: true,
            follow_pavilion: false,
        },
        CrownShift::fixed(),
        CrownShift {
            fraction: 0.33,
            follow_pavilion: true,
            ..CrownShift::fixed()
        },
    ]
}

fn quartz_design() -> Design {
    let mut design = rbc_445();
    design.material = MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

fn diamond_resolved() -> ResolvedMaterial {
    MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
    .resolve(&BuiltinMaterials)
}

#[test]
fn the_table_is_listed_but_no_crown_policy_ever_moves_it() {
    let design = rbc_445();
    let target = quartz();
    for crown in crown_policies() {
        let plan = build_plan(&design, &target, crown, &[]);
        let table = plan
            .rows
            .iter()
            .find(|row| row.name == "E")
            .expect("the table is listed");
        assert!(!table.moves, "{crown:?}");
        assert!(
            table.new_angle.abs() < 1e-12 && table.new_angle.is_sign_positive(),
            "{crown:?}: table went to {}",
            table.new_angle
        );
        assert!(table.margin_deg.is_none() && table.risk.is_none());

        let proposal = plan.proposal();
        assert!(
            proposal
                .rows
                .iter()
                .all(|row| row.tier_index != table.tier_index),
            "the proposal must not carry the table"
        );
        assert!(
            plan.moving_angles()
                .iter()
                .all(|&(index, _)| index != table.tier_index)
        );
        let legacy = build_proposal(&design, &target, crown, RetargetMode::Shift, &[]).unwrap();
        assert!(
            legacy
                .rows
                .iter()
                .all(|row| !is_horizontal_angle_deg(row.old_angle)),
            "the older entry point must not list flat tiers either"
        );
        assert_eq!(legacy.rows, proposal.rows);
    }
}

#[test]
fn a_culet_at_minus_zero_keeps_its_sign_and_its_value() {
    let shifted = plan::shift_angle(
        -0.0,
        Block::Pavilion,
        1.72,
        1.5791,
        CrownShift::default(),
        1.0,
    );
    assert!(shifted.angle_deg.abs() < 1e-12);
    assert!(shifted.angle_deg.is_sign_negative());
    assert_eq!(shifted.guard, AngleGuard::Within);

    let table = plan::shift_angle(0.0, Block::Crown, 1.72, 1.7681, crown_policies()[1], 1.0);
    assert!(table.angle_deg.abs() < 1e-12);
    assert!(table.angle_deg.is_sign_positive());
}

// --- A shifted angle never crosses the horizontal or leaves 1..89.5 degrees ---

fn assert_every_moving_row_is_in_range(plan: &RetargetPlan) {
    for row in plan.rows.iter().filter(|row| row.moves) {
        assert_eq!(
            row.new_angle.is_sign_negative(),
            row.old_angle.is_sign_negative(),
            "{}: {} became {}",
            row.name,
            row.old_angle,
            row.new_angle
        );
        let magnitude = row.new_angle.abs();
        assert!(
            (1.0 - 1e-9..=89.5 + 1e-9).contains(&magnitude),
            "{}: {} became {}",
            row.name,
            row.old_angle,
            row.new_angle
        );
    }
}

#[test]
fn no_policy_in_either_direction_crosses_zero_or_leaves_the_range() {
    for crown in crown_policies() {
        let down = build_plan(&rbc_445(), &quartz(), crown, &[]);
        assert_every_moving_row_is_in_range(&down);
        let up = build_plan(&quartz_design(), &diamond_resolved(), crown, &[]);
        assert_every_moving_row_is_in_range(&up);
    }
}

#[test]
fn a_crown_pushed_past_the_horizontal_is_held_and_says_so() {
    // Diamond to quartz moves the critical angle by about +16 degrees; minus five times
    // that on a 20-31 degree crown would land far below zero.
    let crown = CrownShift {
        fraction: -5.0,
        ..CrownShift::fixed()
    };
    let plan = build_plan(&rbc_445(), &quartz(), crown, &[]);
    assert_every_moving_row_is_in_range(&plan);
    let crown_rows: Vec<&PlanRow> = plan
        .rows
        .iter()
        .filter(|row| row.block == Block::Crown && row.moves)
        .collect();
    assert_ne!(crown_rows, Vec::<&PlanRow>::new());
    for row in &crown_rows {
        assert_eq!(row.guard, AngleGuard::HeldAtMinimum, "{}", row.name);
        assert!((row.new_angle - 1.0).abs() < 1e-9, "{}", row.name);
        let view = plan_row_view(row);
        assert!(view.new_angle.ends_with("(held)"), "{}", view.new_angle);
    }
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("past the horizontal")),
        "{:?}",
        plan.notes
    );
}

#[test]
fn a_crown_pushed_past_the_steep_limit_is_held_at_the_maximum() {
    let crown = CrownShift {
        fraction: 6.0,
        ..CrownShift::fixed()
    };
    let plan = build_plan(&rbc_445(), &quartz(), crown, &[]);
    assert_every_moving_row_is_in_range(&plan);
    assert!(
        plan.rows
            .iter()
            .filter(|row| row.block == Block::Crown && row.moves)
            .all(
                |row| row.guard == AngleGuard::HeldAtMaximum && (row.new_angle - 89.5).abs() < 1e-9
            )
    );
    assert!(
        plan.notes.iter().any(|note| note.contains("steeper than")),
        "{:?}",
        plan.notes
    );
}

// --- Risk: the crown rows read the crown estimate, not the pavilion formula ---

#[test]
fn crown_rows_read_the_crown_window_estimate_against_the_candidate_pavilion() {
    let design = rbc_445();
    let plan = build_plan(&design, &quartz(), crown_policies()[1], &[]);

    let mut candidate = design;
    for (index, angle) in plan.moving_angles() {
        candidate.tiers[index].angle_deg = angle;
    }
    let partner = representative_crown_and_pavilion_angles_deg(&candidate)
        .1
        .expect("the fixture has a pavilion");

    let crown_rows: Vec<&PlanRow> = plan
        .rows
        .iter()
        .filter(|row| row.block == Block::Crown && row.moves)
        .collect();
    assert_ne!(crown_rows, Vec::<&PlanRow>::new());
    for row in crown_rows {
        let expected = crown_window_margin_deg(partner, row.new_angle, plan.n_to);
        let margin = row.margin_deg.expect("a crown row carries an estimate");
        assert!((margin - expected).abs() < 1e-9, "{}", row.name);
        assert!(row.margin_is_estimate, "{}", row.name);
        assert!(
            (margin - tier_margin_deg(row.new_angle, plan.n_to)).abs() > 1.0,
            "{}: the pavilion formula must not be what a crown row shows",
            row.name
        );
        let view = plan_row_view(row);
        assert!(view.margin.ends_with(" est."), "{}", view.margin);
    }

    for row in plan
        .rows
        .iter()
        .filter(|row| row.block == Block::Pavilion && row.moves)
    {
        assert!(!row.margin_is_estimate, "{}", row.name);
        let margin = row.margin_deg.expect("a pavilion row has a margin");
        assert!((margin - tier_margin_deg(row.new_angle, plan.n_to)).abs() < 1e-9);
    }
}

#[test]
fn a_plan_that_shows_crown_estimates_says_plainly_what_the_estimate_looks_at() {
    let design = rbc_445();
    for crown in [crown_policies()[0], crown_policies()[1]] {
        let plan = build_plan(&design, &quartz(), crown, &[]);
        assert!(plan.rows.iter().any(|row| row.margin_is_estimate));
        let note = plan
            .notes
            .iter()
            .find(|note| note.contains("rough estimates"))
            .unwrap_or_else(|| panic!("no crown-estimate note in {:?}", plan.notes));
        assert!(note.contains("same side"), "{note}");
        assert!(note.contains("not a measurement"), "{note}");

        // A result that came from somewhere else (an Optimize option) says it too.
        let angles = plan.moving_angles();
        let rebuilt = plan.with_angles(&design, &angles);
        assert!(
            rebuilt
                .notes
                .iter()
                .any(|note| note.contains("rough estimates")),
            "{:?}",
            rebuilt.notes
        );
    }
}

#[test]
fn a_design_with_no_pavilion_gives_its_crown_rows_no_risk() {
    let tiers = vec![ConstraintTier {
        angle_deg: 30.0,
        name: "Crown".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }];
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::default(),
        tiers,
    );
    let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
    let row = &plan.rows[0];
    assert_eq!(row.block, Block::Crown);
    assert!(row.margin_deg.is_none() && row.risk.is_none());
    let view = plan_row_view(row);
    assert_eq!(view.margin, NO_VALUE);
    assert_eq!(view.risk_label, NO_VALUE);
    assert!(view.changed);
}

// --- Row views ---

#[test]
fn a_flat_row_reads_not_changed_and_has_no_margin() {
    let plan = build_plan(&rbc_445(), &quartz(), CrownShift::default(), &[]);
    let table = plan
        .rows
        .iter()
        .find(|row| row.name == "E")
        .expect("the table is listed");
    let view = plan_row_view(table);
    assert!(!view.changed);
    assert_eq!(view.risk_label, NOT_CHANGED_LABEL);
    assert_eq!(view.margin, NO_VALUE);
    assert_eq!(view.old_angle, view.new_angle);
    assert!(plan.notes.iter().any(|note| note.contains("Flat facets")));
}

#[test]
fn a_moving_row_view_carries_a_risk_label_and_a_signed_margin() {
    let plan = build_plan(&rbc_445(), &quartz(), CrownShift::default(), &[]);
    for row in plan.rows.iter().filter(|row| row.moves) {
        let view = plan_row_view(row);
        assert!(view.changed);
        assert!(["Safe", "Marginal", "Windows"].contains(&view.risk_label));
        assert!(
            view.margin.starts_with('+') || view.margin.starts_with('-'),
            "{}",
            view.margin
        );
    }
}

#[test]
fn retarget_rows_show_positive_angles_and_standard_tier_codes() {
    // RBC-445 names its tiers the old way ("1".."7" on the pavilion, "A".."E" above), and
    // stores the pavilion angles as negative numbers.
    let design = rbc_445();
    let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
    let view = super::view::plan_view(&plan).with_tier_names(&design);
    let labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    assert!(!view.rows.is_empty());
    for row in &view.rows {
        assert_eq!(
            row.name, labels[row.tier_index].code,
            "an old-style name reads as the standard code"
        );
        assert!(
            !row.old_angle.starts_with('-') && !row.new_angle.starts_with('-'),
            "{} reads {} -> {}",
            row.name,
            row.old_angle,
            row.new_angle
        );
    }
    // Tier 0 is stored as -50.4 degrees; it reads 50.40 degrees, on the pavilion side.
    let first = view
        .rows
        .iter()
        .find(|row| row.tier_index == 0)
        .expect("the first pavilion tier moves");
    assert_eq!(first.block, "Pavilion");
    assert_eq!(first.old_angle, "50.40\u{b0}");
    // A margin is a difference, so it keeps an explicit sign.
    assert!(first.margin.starts_with('+') || first.margin.starts_with('-'));
}

#[test]
fn the_legacy_proposal_rows_also_read_as_positive_angles_with_codes() {
    let design = rbc_445();
    let target = quartz();
    let (view, proposal) = view::retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    );
    assert!(proposal.is_some());
    let labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    for row in &view.rows {
        assert_eq!(row.name, labels[row.tier_index].code);
        assert!(!row.old_angle.starts_with('-') && !row.new_angle.starts_with('-'));
    }
}

#[test]
fn a_held_row_note_names_the_tier_by_code_and_the_held_angle_as_a_magnitude() {
    let crown = CrownShift {
        fraction: -5.0,
        ..CrownShift::fixed()
    };
    let design = rbc_445();
    let plan = build_plan(&design, &quartz(), crown, &[]);
    let labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let held_crown: Vec<&PlanRow> = plan
        .rows
        .iter()
        .filter(|row| row.block == Block::Crown && row.moves)
        .collect();
    assert_ne!(held_crown, Vec::<&PlanRow>::new());
    for row in held_crown {
        let code = &labels[row.tier_index].code;
        let note = format!(
            "{code}: the shift would tilt it past the horizontal, so it is held at 1.00\u{b0}."
        );
        assert!(plan.notes.contains(&note), "{note} not in {:?}", plan.notes);
    }
}

#[test]
fn the_older_proposal_is_the_plans_moving_rows() {
    let design = rbc_445();
    let target = quartz();
    for crown in crown_policies() {
        let plan = build_plan(&design, &target, crown, &[]);
        let proposal = build_proposal(&design, &target, crown, RetargetMode::Shift, &[]).unwrap();
        assert_eq!(proposal, plan.proposal(), "{crown:?}");
    }
}

#[test]
fn the_optimize_hint_is_added_only_to_an_invalid_verdict() {
    use super::view::{CheckState, CheckView, OPTIMIZE_HINT};

    assert!(OPTIMIZE_HINT.contains("Shift alone is not valid here"));
    let invalid = CheckView {
        state: CheckState::Invalid,
        headline: "Not valid: the girdle disappears.".to_string(),
        ..CheckView::default()
    };
    let hinted = invalid.with_optimize_hint();
    assert_eq!(
        hinted.details.last().map(String::as_str),
        Some(OPTIMIZE_HINT)
    );
    for state in [
        CheckState::None,
        CheckState::Checking,
        CheckState::Valid,
        CheckState::Unchecked,
    ] {
        let view = CheckView {
            state,
            ..CheckView::default()
        };
        assert_eq!(view.clone().with_optimize_hint(), view);
    }
}

#[test]
fn a_failed_relation_reads_as_one_plain_sentence() {
    let reason = validity::InvalidReason::RelationFails(
        "Lower Girdle comes out at 91.5 degrees.".to_string(),
    );
    assert_eq!(
        reason.message(),
        "A tier that follows a relation cannot follow it after this change (Lower Girdle comes out at 91.5 degrees)."
    );
    assert!(
        validity::RetargetStrategy::Optimized
            .label()
            .contains("searched")
    );
}

// --- The crown follows the pavilion's stretch ---

/// The standard round brilliant as a stone of index `n_d`.
fn brilliant_at(n_d: f64) -> Design {
    let mut design = Design::new(
        PreformSpec::block(2.0, 1.0, 4.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    );
    design.material = MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(n_d),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    };
    design
}

/// A bare refractive index as a target material.
fn target_at(n_d: f64) -> ResolvedMaterial {
    MaterialSelection {
        name: None,
        specific_gravity_override: None,
        refractive_index_override: Some(n_d),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
    .resolve(&BuiltinMaterials)
}

#[test]
fn follow_pavilion_scales_every_crown_tangent_by_the_pavilion_stretch() {
    let design = brilliant_at(1.72);
    let plan = build_plan(&design, &target_at(1.7681), CrownShift::default(), &[]);
    assert!(plan.stretch < 1.0, "stretch {}", plan.stretch);

    let crown_rows: Vec<&PlanRow> = plan
        .rows
        .iter()
        .filter(|row| row.block == Block::Crown && row.moves)
        .collect();
    assert_ne!(crown_rows, Vec::<&PlanRow>::new());
    for row in crown_rows {
        let ratio = row.new_angle.abs().to_radians().tan() / row.old_angle.abs().to_radians().tan();
        assert!(
            (ratio - plan.stretch).abs() < 1e-9,
            "{}: tan ratio {ratio} against stretch {}",
            row.name,
            plan.stretch
        );
        assert_eq!(
            row.new_angle.is_sign_negative(),
            row.old_angle.is_sign_negative()
        );
    }

    let main = plan
        .rows
        .iter()
        .find(|row| row.name == "Pavilion Main")
        .expect("the pavilion main is listed");
    let from_row =
        main.new_angle.abs().to_radians().tan() / main.old_angle.abs().to_radians().tan();
    assert!(
        (from_row - plan.stretch).abs() < 1e-9,
        "{from_row} against {}",
        plan.stretch
    );
    assert_eq!(
        plan.stretch.to_bits(),
        plan::pavilion_stretch(&design, plan.n_from, plan.n_to).to_bits()
    );
}

#[test]
fn follow_pavilion_is_the_default_and_fraction_zero_no_longer_is() {
    assert!(CrownShift::default().follow_pavilion);
    assert!(!CrownShift::fixed().follow_pavilion);
    assert_eq!(CrownShift::fixed().fraction.to_bits(), 0.0_f64.to_bits());
    assert!(!CrownShift::fixed().scale_by_ratio);

    let design = brilliant_at(1.72);
    let target = target_at(1.7681);
    let fixed = build_plan(&design, &target, CrownShift::fixed(), &[]);
    let crown_rows: Vec<&PlanRow> = fixed
        .rows
        .iter()
        .filter(|row| row.block == Block::Crown && row.moves)
        .collect();
    assert_ne!(crown_rows, Vec::<&PlanRow>::new());
    for row in crown_rows {
        assert_eq!(
            row.new_angle.to_bits(),
            row.old_angle.to_bits(),
            "{}",
            row.name
        );
    }

    let followed = build_plan(&design, &target, CrownShift::default(), &[]);
    assert!(
        followed
            .rows
            .iter()
            .filter(|row| row.block == Block::Crown && row.moves)
            .all(|row| (row.new_angle - row.old_angle).abs() > 0.1),
        "the default crown must move with the pavilion"
    );
}

#[test]
fn a_fraction_next_to_follow_pavilion_behaves_as_follow() {
    let design = brilliant_at(1.72);
    let target = target_at(1.7681);
    let plain = build_plan(&design, &target, CrownShift::default(), &[]);
    let with_fraction = build_plan(
        &design,
        &target,
        CrownShift {
            fraction: 0.33,
            follow_pavilion: true,
            ..CrownShift::fixed()
        },
        &[],
    );
    let angles = |plan: &RetargetPlan| -> Vec<u64> {
        plan.rows
            .iter()
            .map(|row| row.new_angle.to_bits())
            .collect()
    };
    assert_eq!(angles(&plain), angles(&with_fraction));
}

#[test]
fn scale_by_ratio_wins_over_follow_pavilion() {
    let design = rbc_445();
    let target = quartz();
    let ratio =
        critical_angle_deg(target.n_d) / critical_angle_deg(design.effective_refractive_index());
    let crown = CrownShift {
        fraction: 0.4,
        scale_by_ratio: true,
        follow_pavilion: true,
    };
    let proposal = build_proposal(&design, &target, crown, RetargetMode::Shift, &[]).unwrap();
    let crown_rows: Vec<&RetargetRow> = proposal
        .rows
        .iter()
        .filter(|row| row.block == Block::Crown)
        .collect();
    assert_ne!(crown_rows, Vec::<&RetargetRow>::new());
    for row in crown_rows {
        let expected = (row.old_angle * ratio).clamp(-89.5, 89.5);
        assert!(
            (row.new_angle - expected).abs() < 1e-9,
            "tier {}",
            row.tier_index
        );
    }
}

#[test]
fn a_design_without_a_pavilion_has_a_stretch_of_one() {
    let tiers = vec![ConstraintTier {
        angle_deg: 34.0,
        name: "Crown Main".to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }];
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::default(),
        tiers,
    );
    assert_eq!(
        plan::pavilion_stretch(&design, 1.54, 1.77).to_bits(),
        1.0_f64.to_bits()
    );
    let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
    assert_eq!(plan.stretch.to_bits(), 1.0_f64.to_bits());
    assert_eq!(
        plan.rows[0].new_angle.to_bits(),
        plan.rows[0].old_angle.to_bits()
    );
    assert!(
        !plan
            .notes
            .iter()
            .any(|note| note.contains("Crown tiers follow the pavilion")),
        "{:?}",
        plan.notes
    );
}

#[test]
fn a_same_material_retarget_has_a_stretch_of_one() {
    let design = brilliant_at(1.72);
    let plan = build_plan(&design, &target_at(1.72), CrownShift::default(), &[]);
    assert_eq!(plan.stretch.to_bits(), 1.0_f64.to_bits());
    for row in plan.rows.iter().filter(|row| row.block == Block::Crown) {
        assert_eq!(
            row.new_angle.to_bits(),
            row.old_angle.to_bits(),
            "{}",
            row.name
        );
    }
}

#[test]
fn the_follow_note_names_the_stretch() {
    let design = brilliant_at(1.72);
    let target = target_at(1.7681);
    let plan = build_plan(&design, &target, CrownShift::default(), &[]);
    let note = plan
        .notes
        .iter()
        .find(|note| note.contains("Crown tiers follow the pavilion"))
        .unwrap_or_else(|| panic!("no follow note in {:?}", plan.notes));
    assert!(note.contains(&format!("{:.3}", plan.stretch)), "{note}");
    assert!(note.contains("silhouette"), "{note}");

    // The older rules say nothing of the kind.
    for crown in [
        CrownShift::fixed(),
        crown_policies()[1],
        crown_policies()[3],
    ] {
        let other = build_plan(&design, &target, crown, &[]);
        assert!(
            !other
                .notes
                .iter()
                .any(|note| note.contains("Crown tiers follow the pavilion")),
            "{crown:?}: {:?}",
            other.notes
        );
    }
}

#[test]
fn the_stretch_grows_with_a_lower_index_target_and_shrinks_with_a_higher_one() {
    let design = brilliant_at(1.72);
    assert!(plan::pavilion_stretch(&design, 1.72, 1.55) > 1.0);
    assert!(plan::pavilion_stretch(&design, 1.72, 1.9) < 1.0);
}
