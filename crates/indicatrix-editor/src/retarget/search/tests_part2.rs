
#[test]
fn keep_look_off_reproduces_the_old_ranking_inputs() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let original = analyze(&design, false).expect("the brilliant solves");
    let on = quick();
    let off = SearchSettings {
        keep_look: false,
        ..quick()
    };
    assert!(on.keep_look, "the default keeps the look");
    let target = shape_target(&on, &original).expect("keep the look builds a target");
    assert_eq!(target.table_percent, original.table_percent);
    assert_eq!(target.crown_to_pavilion, original.crown_to_pavilion);
    assert!((target.weight - KEEP_LOOK_WEIGHT).abs() < f32::EPSILON);
    assert_eq!(shape_target(&off, &original), None);

    let options_for = |settings: &SearchSettings| {
        let inputs = SearchInputs {
            design: &design,
            plan: &plan,
            current_gem: None,
            lighting: LightingPreset::RingLights,
            settings,
        };
        search_options(
            &inputs,
            &original,
            &design,
            &[],
            &BTreeMap::new(),
            GuardFractions {
                overall: 0.5,
                thinnest: 0.5,
            },
        )
    };
    assert_eq!(options_for(&off).shape_target, None);
    assert_eq!(options_for(&on).shape_target, Some(target));
}

#[test]
fn the_notes_mention_the_penalty_only_when_the_look_was_kept() {
    let mut report = bare_report(StartPoint::Shift, 4);
    assert!(
        !report
            .notes()
            .iter()
            .any(|note| note.contains("keep the look"))
    );
    report.keep_look = true;
    assert!(
        report
            .notes()
            .iter()
            .any(|note| note.contains("keep the look"))
    );
    // Nothing was searched, so there was nothing to penalise.
    report.free_tiers = 0;
    assert!(
        !report
            .notes()
            .iter()
            .any(|note| note.contains("keep the look"))
    );
}

#[test]
fn applying_a_candidate_is_one_undo_step_and_undoes_exactly() {
    let design = brilliant_at_172();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");
    assert_ne!(report.candidates, Vec::new());
    for candidate in &report.candidates {
        let proposal = plan.with_angles(&design, &candidate.angles).proposal();
        let edit = apply_with_anchors(&design, &proposal, &candidate.anchors).expect("preserved retarget");
        let mut applied = design.clone();
        let mut history = History::new();
        history
            .apply(&mut applied, edit)
            .expect("the batch applies");
        assert_eq!(applied.tiers, candidate.design.tiers);

        assert!(history.undo(&mut applied).expect("undo works"));
        assert_eq!(applied, design, "one undo restores angles and masts");
        assert!(!history.can_undo(), "the retarget was one undo step");
    }
}

#[test]
fn a_shift_that_is_not_valid_never_leaves_an_invalid_option() {
    // A crown fraction this large pushes every crown tier to the steep limit.
    let design = brilliant_at_172();
    let crown = CrownShift {
        fraction: 6.0,
        scale_by_ratio: false,
        follow_pavilion: false,
    };
    let plan = build_plan(&design, &target_at(1.544), crown, &[]);
    let settings = SearchSettings {
        evaluations: 12,
        keep: 1,
        ..SearchSettings::default()
    };
    let report = run(&design, &plan, &settings).expect("the search runs");

    if report.shift_validity.status == ValidityStatus::Invalid {
        assert_ne!(report.start, StartPoint::Shift);
        assert!(
            report
                .notes()
                .iter()
                .any(|note| note.contains("Shift alone is not valid here")),
            "{:?}",
            report.notes()
        );
    } else {
        assert_eq!(report.start, StartPoint::Shift);
    }
    for candidate in &report.candidates {
        assert_eq!(candidate.validity.status, ValidityStatus::Valid);
    }
}

// --- Tiers that follow a relation ---

#[test]
fn a_follower_row_shows_its_relation_and_stays_out_of_the_proposal() {
    let (design, main, lower) = brilliant_with_a_follower();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let row = plan
        .rows
        .iter()
        .find(|row| row.tier_index == lower)
        .expect("the follower is listed");
    let main_row = plan
        .rows
        .iter()
        .find(|row| row.tier_index == main)
        .expect("the main is listed");

    assert!(row.follows.is_some());
    assert!(!row.moves);
    assert!(row.new_angle.is_sign_negative());
    assert!(
        (row.new_angle.abs() - (main_row.new_angle.abs() + 1.5)).abs() < 1e-9,
        "{} against {}",
        row.new_angle,
        main_row.new_angle
    );
    assert_ne!(
        row.new_angle, row.old_angle,
        "the follower moves with the main"
    );
    assert!(
        plan.proposal()
            .rows
            .iter()
            .all(|proposed| proposed.tier_index != lower)
    );
    assert!(
        plan.moving_angles()
            .iter()
            .all(|&(index, _)| index != lower)
    );
    assert_eq!(plan.relation_error, None);
    assert!(plan.notes.iter().any(|note| note.contains("follows")));

    let view = plan_row_view(row);
    assert_eq!(view.risk_label, FOLLOWS_LABEL);
    assert!(view.margin.starts_with("= "), "{}", view.margin);
    assert!(view.changed);
}

#[test]
fn shift_keeps_the_relation_and_stays_valid() {
    let (design, main, lower) = brilliant_with_a_follower();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let original = analyze(&design, true).expect("the design solves and closes");
    let attempt = best_attempt(&design, &plan.moving_angles(), &original, None, &|| false)
        .expect("the relation can be followed")
        .expect("not stopped");

    assert_eq!(
        attempt.reasons,
        Vec::new(),
        "the Shift change must be valid"
    );
    let main_angle = attempt.design.tiers[main].angle_deg;
    let lower_angle = attempt.design.tiers[lower].angle_deg;
    assert!(
        (lower_angle.abs() - (main_angle.abs() + 1.5)).abs() < 1e-9,
        "{lower_angle} against {main_angle}"
    );
    assert!(
        attempt
            .anchors
            .iter()
            .any(|anchor| anchor.tier_index == lower),
        "the follower is re-anchored like any other facet that turns"
    );
}

#[test]
fn applying_through_the_session_makes_the_follower_follow_in_the_same_undo_step() {
    let (design, _, _) = brilliant_with_a_follower();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let original = analyze(&design, true).expect("the design solves and closes");
    let attempt = best_attempt(&design, &plan.moving_angles(), &original, None, &|| false)
        .expect("the relation can be followed")
        .expect("not stopped");

    let mut session = EditorSession::with_history(design.clone(), History::new());
    // The projection may turn a moving tier by a little more or less than the plan asked for,
    // so what is applied is the state that was validated: the attempt's own angles and masts,
    // never the plan's angles combined with the attempt's masts.
    let proposal = plan
        .with_angles(&design, &angle_differences(&design, &attempt.design))
        .proposal();
    let edit = apply_with_anchors(&design, &proposal, &attempt.anchors).expect("preserved retarget");
    // The checked edit carries the follower's settled angle. Every caller (the desktop's Apply,
    // the CLI, the web app, mobile) hands it to the session through this conversion, which
    // leaves the follower's angle to the session's own relation fold so that the edit does not
    // read as a direct change of a driven tier.
    let edit = crate::optimize_view::edits_for_relation_session(&design, edit);
    session.apply(edit).expect("the retarget applies");
    assert_eq!(session.design.tiers, attempt.design.tiers);

    assert!(session.undo().expect("undo works").is_some());
    assert_eq!(session.design, design);
    assert!(!session.history.can_undo(), "one undo step");
}

#[test]
fn a_search_keeps_the_relation_in_every_option() {
    let (design, main, lower) = brilliant_with_a_follower();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let report = run(&design, &plan, &quick()).expect("the search runs");
    assert_ne!(report.candidates, Vec::new());
    for candidate in &report.candidates {
        let main_angle = candidate.design.tiers[main].angle_deg;
        let lower_angle = candidate.design.tiers[lower].angle_deg;
        assert!(
            (lower_angle.abs() - (main_angle.abs() + 1.5)).abs() < 1e-9,
            "{lower_angle} against {main_angle}"
        );
        assert!(
            candidate.angles.iter().all(|&(index, _)| index != lower),
            "the follower never travels in the proposal"
        );
        assert_eq!(candidate.validity.status, ValidityStatus::Valid);
    }
}

#[test]
fn a_moved_follower_is_reanchored_and_the_delivered_stone_is_what_gets_scored() {
    let (design, main, lower) = brilliant_with_a_follower();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let settings = quick();
    let inputs = SearchInputs {
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
        settings: &settings,
    };
    let original = analyze(&design, true).expect("the design solves and closes");
    let hinges = tier_hinges(&design, &original.solved);

    // The stone as the optimizer scores it: the main is steeper and the lower girdle follows it
    // with the mast it started with.
    let mut as_scored = design.clone();
    as_scored.tiers[main].angle_deg = -43.0;
    fold_relations(&mut as_scored).expect("the relation can be followed");
    assert!((as_scored.tiers[lower].angle_deg + 44.5).abs() < 1e-9);

    // The stone that is delivered: the follower turns about its girdle edge like any facet.
    let mut delivered = as_scored.clone();
    assert!(
        reanchor_followers(&mut delivered, &design, &hinges),
        "the follower moved, so its mast moves"
    );
    assert_ne!(
        delivered.tiers[lower].constraint, as_scored.tiers[lower].constraint,
        "the follower is on a new mast"
    );
    // A second pass finds nothing left to do.
    assert!(!reanchor_followers(&mut delivered, &design, &hinges));

    let scored_stone = analyze(&as_scored, false).expect("closes");
    let delivered_stone = analyze(&delivered, false).expect("closes");
    let on_scored = delivered_numbers(&inputs, &original, &as_scored, &scored_stone);
    let on_delivered = delivered_numbers(&inputs, &original, &delivered, &delivered_stone);
    assert_ne!(
        on_delivered, on_scored,
        "the two stones differ, so their scores do"
    );
}

#[test]
fn every_option_carries_the_numbers_of_the_stone_that_is_delivered() {
    // With a relation in the design the optimizer's own score is of a stone with the follower
    // on its start mast. The report's score, ranking and figures describe the delivered stone,
    // which is the design in the option, so scoring that design again must give them back.
    let (design, _, _) = brilliant_with_a_follower();
    let plan = build_plan(&design, &target_at(1.7681), SAPPHIRE_CROWN, &[]);
    let settings = quick();
    let report = run(&design, &plan, &settings).expect("the search runs");
    assert_ne!(report.candidates, Vec::new());
    let inputs = SearchInputs {
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
        settings: &settings,
    };
    let original = analyze(&design, true).expect("the design solves and closes");
    for candidate in &report.candidates {
        let stone = analyze(&candidate.design, false).expect("an option closes");
        let again = delivered_numbers(&inputs, &original, &candidate.design, &stone);
        assert_eq!(
            candidate.numbers, again,
            "{:?}: the option's figures are not those of its own design",
            candidate.kind
        );
    }
    assert!(
        report
            .candidates
            .windows(2)
            .all(|pair| pair[0].numbers.score <= pair[1].numbers.score),
        "best first, by the delivered stone's score"
    );
}

#[test]
fn a_plan_says_when_a_relation_cannot_be_followed() {
    // Lower Girdle = Pavilion Main + 47: 88 degrees now, but the shift to a lower index makes
    // the pavilion main steeper than 43 degrees and the sum leaves the 90 degree limit.
    let mut design = brilliant_at_172();
    let lower = tier_named(&design, "Lower Girdle");
    design.tiers[lower].angle_deg = -88.0;
    follow(&mut design, lower, "[Pavilion Main] + 47");
    let plan = build_plan(&design, &target_at(1.544), CrownShift::default(), &[]);
    assert!(plan.relation_error.is_some(), "{:?}", plan.notes);
    assert!(
        plan.notes
            .iter()
            .any(|note| note.contains("cannot follow it after this change")),
        "{:?}",
        plan.notes
    );
}

#[test]
fn an_attempt_whose_relation_cannot_be_followed_is_an_error_not_a_result() {
    let (design, main, _) = brilliant_with_a_follower();
    let original = analyze(&design, true).expect("the design solves and closes");
    // 89 degrees plus 1.5 is past the 90 degree limit.
    let outcome = best_attempt(&design, &[(main, -89.0)], &original, None, &|| false);
    assert!(outcome.is_err());
}

// --- The notes ---

fn bare_report(start: StartPoint, free_tiers: usize) -> SearchReport {
    SearchReport {
        start,
        shift_validity: RetargetValidity::unchecked(&InvalidReason::GirdleGone),
        start_numbers: CandidateNumbers {
            score: 10.0,
            windowing_pct: 5.0,
            brilliance_pct: 80.0,
            extinction_pct: 3.0,
            yield_loss_pct: 40.0,
        },
        candidates: Vec::new(),
        dropped: Vec::new(),
        evaluations: 0,
        free_tiers,
        keep_look: false,
        baseline: None,
    }
}

#[test]
fn the_notes_say_where_a_search_started_when_shift_alone_is_not_valid() {
    let partial = bare_report(StartPoint::Partial { percent: 40 }, 3).notes();
    assert_eq!(
        partial[0],
        "Shift alone is not valid here. The girdle disappears. The search started from 40 % of the Shift change."
    );
    let unchanged = bare_report(StartPoint::Unchanged, 3).notes();
    assert_eq!(
        unchanged[0],
        "Shift alone is not valid here. The girdle disappears. The search started from the design as it is."
    );
    assert!(
        bare_report(StartPoint::Shift, 3)
            .notes()
            .iter()
            .all(|note| !note.contains("Shift alone"))
    );
}

#[test]
fn the_notes_count_what_was_dropped_and_why() {
    let mut report = bare_report(StartPoint::Shift, 3);
    report.dropped = vec![vec![InvalidReason::GirdleGone]];
    let one = report.notes();
    assert!(
        one.iter().any(|note| note
            == "1 result of the search was dropped because it is not valid: The girdle disappears."),
        "{one:?}"
    );

    report.dropped = vec![
        vec![InvalidReason::GirdleGone],
        vec![
            InvalidReason::GirdleGone,
            InvalidReason::TierLost {
                name: "Star".to_string(),
            },
        ],
    ];
    let two = report.notes();
    assert!(
        two.iter().any(|note| note
            == "2 results of the search were dropped because they are not valid: The girdle disappears. Star: all its facets disappear."),
        "{two:?}"
    );
}

#[test]
fn the_notes_say_what_to_try_when_nothing_better_was_found() {
    let nothing = bare_report(StartPoint::Unchanged, 3).notes();
    assert!(nothing.iter().any(|note| note.contains("no valid option")));
    assert!(nothing.iter().any(|note| note.contains("wider range")));

    let no_free = bare_report(StartPoint::Unchanged, 0).notes();
    assert!(
        no_free
            .iter()
            .any(|note| note.contains("nothing to search"))
    );
    assert!(no_free.iter().all(|note| !note.contains("no valid option")));
}
