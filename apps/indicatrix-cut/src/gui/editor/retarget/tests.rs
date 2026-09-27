//! Tests for [`super::build_proposal`]/[`super::apply`] and their shared helpers.

use super::*;
use indicatrix_cut_core::{
    BuiltinMaterials, ConstraintTier, History, MaterialSelection, PreformSpec, ScheduleMeta,
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
    };
    design
}

fn quartz() -> ResolvedMaterial {
    MaterialSelection {
        name: Some("Quartz".to_string()),
        specific_gravity_override: None,
        refractive_index_override: None,
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

    // Default crown fraction is 0: every crown tier's angle is unchanged.
    for row in proposal.rows.iter().filter(|r| r.block == Block::Crown) {
        assert!(
            (row.new_angle - row.old_angle).abs() < 1e-12,
            "tier {}: crown must stay put with the default CrownShift",
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
        scale_by_ratio: false,
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
        fraction: 0.0,
        scale_by_ratio: true,
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
    let text = indicatrix_formats::asc::to_asc_string(&schedule);

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
        );
        assert!((row.new_angle - expected).abs() < 1e-9);
    }
}
