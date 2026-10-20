
/// What the older entry point answers, or `None` when its meet gate refuses the change.
///
/// `build_proposal` validates the angle-only proposal against the design's own meets. The pinned
/// RBC-445 turned from Diamond to Quartz is a change it may refuse, because turning pinned
/// facets without their masts splits the meets they take part in (the checked path carries the
/// masts). A refusal must be the gate's (`Solve`), never another error; the rows themselves are
/// then pinned through the plan, which the gate does not touch. The tests that compare the
/// older entry point with the plan also run it on the unchanged Diamond-to-Diamond retarget,
/// which the gate always lets through, so that equality is never skipped.
fn legacy_shift_proposal(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
) -> Option<RetargetProposal> {
    match build_proposal(design, target, crown, RetargetMode::Shift, &[]) {
        Ok(proposal) => Some(proposal),
        Err(RetargetError::Solve(_)) => None,
        Err(other) => panic!("{crown:?}: only the meet gate may refuse a Shift: {other:?}"),
    }
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
        let flat_free = |legacy: &RetargetProposal| {
            legacy
                .rows
                .iter()
                .all(|row| !is_horizontal_angle_deg(row.old_angle))
        };
        // Quartz: when the gate lets it through, the older entry point lists the plan's rows
        // and no flat tier; when it refuses (`None`), the plan's rows above are the check.
        if let Some(legacy) = legacy_shift_proposal(&design, &target, crown) {
            assert!(flat_free(&legacy), "the older entry point must not list flat tiers either");
            assert_eq!(legacy.rows, proposal.rows);
        }
        // Diamond to Diamond changes no angle, so the gate always passes: this comparison runs.
        let same = diamond_resolved();
        let legacy = legacy_shift_proposal(&design, &same, crown)
            .expect("an unchanged retarget keeps every meet");
        assert!(flat_free(&legacy), "the older entry point must not list flat tiers either");
        assert_eq!(legacy.rows, build_plan(&design, &same, crown, &[]).proposal().rows);
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
    // The display does not depend on the angles changing, and Diamond to Quartz is a change the
    // pinned RBC-445 is refused (no rows, no proposal), so the unchanged retarget carries it.
    let target = diamond_resolved();
    let (view, proposal) = view::retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Shift,
        &[],
    );
    assert!(proposal.is_some());
    assert_ne!(view.rows.len(), 0, "the rows below must not be vacuous");
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
    let same = diamond_resolved();
    for crown in crown_policies() {
        let plan = build_plan(&design, &target, crown, &[]);
        // Quartz: equal when the meet gate lets the change through; a refusal is the gate's.
        if let Some(proposal) = legacy_shift_proposal(&design, &target, crown) {
            assert_eq!(proposal, plan.proposal(), "{crown:?}");
        }
        // Diamond to Diamond changes no angle, so the gate always passes: this one always runs.
        let plan = build_plan(&design, &same, crown, &[]);
        let proposal = legacy_shift_proposal(&design, &same, crown)
            .expect("an unchanged retarget keeps every meet");
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
fn only_a_successful_check_enables_apply() {
    use super::view::CheckState;

    assert!(CheckState::Valid.allows_apply());
    for state in [
        CheckState::None,
        CheckState::Checking,
        CheckState::Invalid,
        CheckState::Unchecked,
    ] {
        assert!(!state.allows_apply(), "{state:?} must not enable Apply");
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
    // The arithmetic of the plan; the pinned RBC-445 is refused by the legacy entry point's gate.
    let proposal = shift_rows(&design, &target, crown, &[]);
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
