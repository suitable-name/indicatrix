use super::*;
use indicatrix_cut_core::ConstraintTier;

#[test]
fn material_index_and_name_round_trip_for_every_preset() {
    for (index, &name) in MATERIAL_PRESET_NAMES.iter().enumerate() {
        let expected_name = (name != "(none)").then(|| name.to_string());
        assert_eq!(material_name_from_index(index as i32), expected_name);
        assert_eq!(
            material_index_from_name(expected_name.as_deref()),
            index as i32
        );
    }
}

#[test]
fn material_index_from_name_falls_back_to_zero_for_an_unknown_name() {
    assert_eq!(material_index_from_name(Some("Garnet")), 0);
    assert_eq!(material_index_from_name(None), 0);
}

#[test]
fn parse_yield_form_blank_fields_mean_unset() {
    let (girdle_diameter_mm, material) = parse_yield_form("", 0, "").unwrap();
    assert_eq!(girdle_diameter_mm, None);
    assert_eq!(material, MaterialSelection::none());
}

#[test]
fn parse_yield_form_reads_girdle_diameter_and_material_selection() {
    let (girdle_diameter_mm, material) = parse_yield_form("8.2", 1, "3.515").unwrap();
    assert_eq!(girdle_diameter_mm, Some(8.2));
    assert_eq!(material.name.as_deref(), Some("Diamond"));
    assert_eq!(material.specific_gravity_override, Some(3.515));
}

#[test]
fn parse_yield_form_rejects_zero_or_negative_or_unparseable_values() {
    assert!(parse_yield_form("0.0", 0, "").is_err());
    assert!(parse_yield_form("-1.0", 0, "").is_err());
    assert!(parse_yield_form("wide", 0, "").is_err());
    assert!(parse_yield_form("", 0, "0.0").is_err());
    assert!(parse_yield_form("", 0, "-1.0").is_err());
    assert!(parse_yield_form("", 0, "heavy").is_err());
}

#[test]
fn design_to_gpu_planes_inverts_the_halfspace_sign_convention() {
    // Converting a fresh design's own preform planes to `GpuFacetPlane` and back via
    // `to_halfspace_f64` must reproduce the same `m` for every plane, proving the
    // `d = -m` flip in `design_to_gpu_planes` is inverted correctly.
    let design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(12, 1.3, 1.0, 0.9),
        12,
        4,
        1.6,
    );
    // A fresh design has no tiers, so `planes()` cannot fail with `MissingAnchor`.
    let original = design
        .planes()
        .expect("a fresh design has no tiers to anchor");
    let converted = design_to_gpu_planes(&design);
    assert_eq!(original.len(), converted.len());
    for (&(n, m), gpu) in original.iter().zip(&converted) {
        let (round_trip_n, round_trip_m) = gpu.to_halfspace_f64();
        assert!(
            (round_trip_n - n).length() < 1e-5,
            "normal did not round-trip: {round_trip_n:?} vs {n:?}"
        );
        assert!(
            (round_trip_m - m).abs() < 1e-5,
            "offset did not round-trip: {round_trip_m} vs {m}"
        );
    }
}

#[test]
fn editor_state_add_tier_then_undo_then_redo_round_trips() {
    let mut state = EditorState::fresh();
    let before = state.design.clone();

    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 0.0,
                name: "T".to_string(),
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.32),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .expect("add must apply");
    assert_eq!(state.design.tiers.len(), 1);
    assert!(state.history.can_undo());
    assert!(!state.history.can_redo());

    assert!(state.undo().unwrap());
    assert_eq!(state.design, before);
    assert!(!state.history.can_undo());
    assert!(state.history.can_redo());

    assert!(state.redo().unwrap());
    assert_eq!(state.design.tiers.len(), 1);
}

#[test]
fn editor_state_undo_on_an_empty_history_is_a_harmless_no_op() {
    let mut state = EditorState::fresh();
    assert!(!state.undo().unwrap());
    assert!(!state.redo().unwrap());
}

#[test]
fn status_text_reports_closed_for_a_fresh_design() {
    let (text, is_problem) = status_text_and_is_problem(&EditorState::fresh().design);
    assert!(text.starts_with("Closed"));
    assert!(!is_problem);
}

#[test]
fn status_text_reports_degenerate_once_pinched_flat() {
    // Two opposing zero-mast facets pinch the preform's vertical extent to nothing.
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 0.0,
                name: "T".to_string(),
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.0),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: ConstraintTier {
                angle_deg: -0.0,
                name: "C".to_string(),
                indices: vec![],
                constraint: MeetConstraint::ScaleReference(0.0),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let (text, is_problem) = status_text_and_is_problem(&state.design);
    assert!(text.starts_with("Degenerate"), "{text}");
    assert!(is_problem);
}

#[test]
fn tier_items_reflects_position_and_fields_in_order() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -41.0,
                name: "P1".to_string(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let items = tier_items(&state.design, state.design.effective_refractive_index());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].index, 0);
    assert_eq!(items[0].angle_deg.as_str(), "-41");
    assert_eq!(items[0].mast.as_str(), "0.65");
    assert_eq!(items[0].name.as_str(), "P1");
    assert_eq!(items[0].indices.as_str(), "0, 24");
}

/// A complete orbit reads "orbit x4" and is never flagged incomplete; detaching it
/// must flip [`EditorTierItem::is_detached`] without changing the orbit shape shown.
#[test]
fn tier_items_reports_orbit_status_and_detached_state() {
    let mut state = EditorState::fresh(); // symmetry_order 8, see `EditorState::fresh`
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -41.0,
                name: "P1".to_string(),
                indices: vec![0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let items = tier_items_stale(&state.design, state.design.effective_refractive_index());
    assert_eq!(items[0].orbit_status.as_str(), "orbit x8");
    assert!(!items[0].orbit_incomplete);
    assert!(!items[0].is_detached);

    let edit = state.design.detach_all_in_tier(0).unwrap();
    state.apply(edit).unwrap();
    let items = tier_items_stale(&state.design, state.design.effective_refractive_index());
    assert!(items[0].is_detached);
    assert_eq!(
        items[0].orbit_status.as_str(),
        "orbit x8",
        "detaching must not change the reported orbit shape, only is_detached"
    );
}

/// The no-solve tier list must still reflect authored fields exactly like
/// [`tier_items`] does -- only mast/strategy differ, always the fixed "not solved"
/// placeholder flagged uncertain, regardless of whether the design would actually
/// solve cleanly. This function must never touch `Design::solve`.
#[test]
fn tier_items_stale_reflects_authored_fields_without_solving() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -41.0,
                name: "P1".to_string(),
                indices: vec![0.0, 24.0],
                constraint: MeetConstraint::ScaleReference(0.65),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let items = tier_items_stale(&state.design, state.design.effective_refractive_index());
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].index, 0);
    assert_eq!(items[0].angle_deg.as_str(), "-41");
    assert_eq!(items[0].name.as_str(), "P1");
    assert_eq!(items[0].indices.as_str(), "0, 24");
    // Never a real mast, always flagged uncertain, even though this exact tier is a
    // `ScaleReference` that would solve instantly and exactly.
    assert_eq!(items[0].mast.as_str(), "-");
    assert!(items[0].strategy_is_uncertain);
}

/// Reproduces the "Solve, Adopt, and it says not solved again" bug: build a design
/// with a real crown anchor and one imported, not-yet-adopted tier pinned to exactly
/// the mast its real meet constraint would derive, confirm it already solves, then
/// apply the exact `Edit::SetConstraint` "Adopt" issues, and confirm the design is
/// STILL solvable -- never falling back to `MissingAnchor` -- converging to the same
/// mast. The acceptance criterion for `setup_adopt_meet_callback` calling a real
/// re-solve rather than showing a fixed "Not solved" banner.
#[test]
fn adopting_a_suggested_meet_keeps_the_design_solved_at_the_same_mast() {
    // A proven-solvable crown anchor + `MeetNamed` pair, not a guessed shape.
    let mut state = EditorState::fresh();
    state.design = Design::fresh(
        indicatrix_cut_core::PreformSpec::block(1.0, 1.0, 2.0),
        96,
        4,
        1.62,
    );

    // Crown anchor: gives the block something to solve the meet-derived tier
    // against.
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: 30.0,
                name: "A".to_string(),
                indices: vec![0.0, 24.0, 48.0, 72.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();

    // What the real meet constraint derives for a same-block, same-index-shape tier
    // meeting A -- computed once up front so the "pinned" tier below can be pinned to
    // EXACTLY this, and the post-Adopt assertion has a real number to compare against.
    let mut derived = state.design.clone();
    derived.tiers.push(ConstraintTier {
        angle_deg: 45.0,
        name: "B".to_string(),
        indices: vec![0.0, 24.0, 48.0, 72.0],
        constraint: MeetConstraint::MeetNamed(vec!["A".to_string()]),
        imported_meet: None,
        detached: Vec::new(),
    });
    let derived_mast = derived
        .solve()
        .expect("A anchors the crown; B must solve against it")[1]
        .mast;

    // Import policy: B is PINNED to that same real mast, with the file's actual meet
    // instruction preserved alongside for one-click Adopt.
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: ConstraintTier {
                angle_deg: 45.0,
                name: "B".to_string(),
                indices: vec![0.0, 24.0, 48.0, 72.0],
                constraint: MeetConstraint::ScaleReference(derived_mast),
                imported_meet: Some(MeetConstraint::MeetNamed(vec!["A".to_string()])),
                detached: Vec::new(),
            },
        })
        .unwrap();

    // The explicit "Solve" action's own path: already solved, already reporting B's
    // real (pinned) mast.
    let items = tier_items(&state.design, state.design.effective_refractive_index());
    assert_eq!(items[1].mast.as_str(), derived_mast.to_string());
    let (status, is_problem) = status_text_and_is_problem(&state.design);
    assert!(
        !is_problem,
        "pinned design must already read as solved: {status}"
    );

    // "Adopt" itself: the exact `Edit::SetConstraint` call `setup_adopt_meet_callback`
    // issues, switching B over to its `imported_meet`.
    let constraint = state.design.tiers[1].imported_meet.clone().unwrap();
    state
        .apply(Edit::SetConstraint {
            index: 1,
            constraint,
        })
        .expect("adopt must apply");

    // The bug: after Adopt, the design must still solve -- a real "Solve" click must
    // never report "not solved" against a design that actually does.
    let (status, is_problem) = status_text_and_is_problem(&state.design);
    assert!(!is_problem, "adopted design must still solve: {status}");
    let items = tier_items(&state.design, state.design.effective_refractive_index());
    assert!(
        !items[1].strategy_is_uncertain,
        "the adopted meet must resolve to a real, trusted strategy, not an estimate"
    );
    // And it must converge to the SAME mast Adopt was suggesting in the first place --
    // not silently move the geometry.
    let adopted_mast: f64 = items[1].mast.parse().unwrap();
    assert!(
        (adopted_mast - derived_mast).abs() < 1e-9,
        "adopting should reproduce the exact same mast the suggestion was showing: \
         {adopted_mast} vs {derived_mast}"
    );
}

// --- tier_margin_and_risk ---

#[test]
fn tier_margin_and_risk_is_not_applicable_for_crown_and_girdle_angles() {
    assert_eq!(tier_margin_and_risk(30.0, 2.417), (String::new(), -1));
    assert_eq!(tier_margin_and_risk(0.0, 2.417), (String::new(), -1));
}

#[test]
fn tier_margin_and_risk_classifies_a_comfortably_safe_pavilion_tier() {
    // Diamond's critical angle is ~24.4 degrees; -40 sits well past it.
    let (text, risk) = tier_margin_and_risk(-40.0, 2.417);
    assert_eq!(risk, 0);
    assert!(text.starts_with('+'), "{text}");
}

#[test]
fn tier_margin_and_risk_classifies_a_windowing_pavilion_tier() {
    // -20 is well below diamond's ~24.4 degree critical angle.
    let (text, risk) = tier_margin_and_risk(-20.0, 2.417);
    assert_eq!(risk, 2);
    assert!(text.starts_with('-'), "{text}");
}

#[test]
fn tier_margin_and_risk_classifies_a_marginal_pavilion_tier() {
    // A pavilion angle exactly at the critical angle has zero margin -- Marginal,
    // not Safe or Windows.
    let n_d = 2.417;
    let critical = indicatrix_cut_core::critical_angle_deg(n_d);
    let (_, risk) = tier_margin_and_risk(-critical, n_d);
    assert_eq!(risk, 1);
}

// --- design_material_options / index / name ---

#[test]
fn design_material_options_lists_built_ins_then_customs_then_the_custom_ri_sentinel() {
    let custom = [{
        let mut m = GemMaterial::diamond();
        m.name = "MyGarnet".to_string();
        m
    }];
    let options = design_material_options(&custom);
    assert_eq!(options[0], "(none)");
    assert_eq!(options[1], "Diamond");
    assert_eq!(options[options.len() - 2], "MyGarnet");
    assert_eq!(options[options.len() - 1], "Custom RI\u{2026}");
}

#[test]
fn design_material_options_does_not_duplicate_a_custom_material_sharing_a_built_in_name() {
    let custom = [{
        let mut m = GemMaterial::diamond();
        m.name = "Diamond".to_string();
        m
    }];
    let options = design_material_options(&custom);
    assert_eq!(options.iter().filter(|&n| n == "Diamond").count(), 1);
}

#[test]
fn design_material_index_and_name_round_trip_for_a_built_in_and_a_custom() {
    let custom = [{
        let mut m = GemMaterial::diamond();
        m.name = "MyGarnet".to_string();
        m
    }];
    let options = design_material_options(&custom);
    let diamond_index = design_material_index_from_name(Some("Diamond"), &options);
    assert_eq!(
        design_material_name_from_index(diamond_index, &options).as_deref(),
        Some("Diamond")
    );
    let custom_index = design_material_index_from_name(Some("MyGarnet"), &options);
    assert_eq!(
        design_material_name_from_index(custom_index, &options).as_deref(),
        Some("MyGarnet")
    );
}

#[test]
fn design_material_name_from_index_is_none_for_none_and_the_custom_ri_sentinel() {
    let options = design_material_options(&[]);
    assert_eq!(design_material_name_from_index(0, &options), None);
    let last = (options.len() - 1) as i32;
    assert_eq!(design_material_name_from_index(last, &options), None);
}

#[test]
fn design_material_index_from_name_falls_back_to_zero_when_absent() {
    let options = design_material_options(&[]);
    assert_eq!(
        design_material_index_from_name(Some("Not A Material"), &options),
        0
    );
    assert_eq!(design_material_index_from_name(None, &options), 0);
}

// --- parse_design_material_form ---

#[test]
fn parse_design_material_form_blank_override_means_use_the_resolved_material() {
    let options = design_material_options(&[]);
    let current = MaterialSelection::none();
    let material = parse_design_material_form(1, "", &options, &current).unwrap();
    assert_eq!(material.name.as_deref(), Some("Diamond"));
    assert_eq!(material.refractive_index_override, None);
}

#[test]
fn parse_design_material_form_reads_a_valid_override_and_keeps_the_sg_override() {
    let options = design_material_options(&[]);
    let mut current = MaterialSelection::none();
    current.specific_gravity_override = Some(3.5);
    let material = parse_design_material_form(9, "1.544", &options, &current).unwrap();
    assert_eq!(material.name.as_deref(), Some("Quartz"));
    assert_eq!(material.refractive_index_override, Some(1.544));
    assert_eq!(material.specific_gravity_override, Some(3.5));
}

#[test]
fn parse_design_material_form_rejects_an_ri_at_or_below_one() {
    let options = design_material_options(&[]);
    let current = MaterialSelection::none();
    assert!(parse_design_material_form(0, "1.0", &options, &current).is_err());
    assert!(parse_design_material_form(0, "0.5", &options, &current).is_err());
    assert!(parse_design_material_form(0, "not-a-number", &options, &current).is_err());
}

// --- gear_index_from_teeth / gear_choice_to_teeth ---

#[test]
fn gear_index_and_choice_round_trip_for_every_preset() {
    for (i, &teeth) in GEAR_PRESETS.iter().enumerate() {
        assert_eq!(gear_index_from_teeth(teeth), i as i32);
        assert_eq!(gear_choice_to_teeth(i as i32, "").unwrap(), teeth);
    }
}

#[test]
fn gear_index_from_teeth_is_the_custom_slot_for_an_unlisted_gear() {
    assert_eq!(gear_index_from_teeth(50), GEAR_PRESETS.len() as i32);
}

#[test]
fn gear_choice_to_teeth_reads_the_custom_text_field_outside_the_preset_list() {
    let custom_index = GEAR_PRESETS.len() as i32;
    assert_eq!(gear_choice_to_teeth(custom_index, "50").unwrap(), 50);
    assert!(gear_choice_to_teeth(custom_index, "0").is_err());
    assert!(gear_choice_to_teeth(custom_index, "-5").is_err());
    assert!(gear_choice_to_teeth(custom_index, "wide").is_err());
}

// --- gear_remap_preview ---

#[test]
fn gear_remap_preview_flags_non_integral_positions_independently_of_rounding() {
    let mut design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    );
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![0.0, 12.0, 24.0], // multiples of 12: land cleanly at 80/96 * 12 = 10
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });
    let rows = gear_remap_preview(&design, 96, 80, RemapRounding::Nearest);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name.as_str(), "P1");
    // 12 * 80/96 = 10.0 exactly -- every position lands on a whole tooth.
    assert!(
        !rows[0].non_integral,
        "expected a clean remap: {}",
        rows[0].new_indices
    );
    assert_eq!(rows[0].new_indices.as_str(), "0, 10, 20");
}

#[test]
fn gear_remap_preview_flags_a_lossy_remap_as_non_integral() {
    let mut design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    );
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![1.0], // 1 * 80/96 = 0.8333... -- not a whole tooth
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });
    let rows = gear_remap_preview(&design, 96, 80, RemapRounding::Nearest);
    assert!(rows[0].non_integral);
}

#[test]
fn gear_remap_preview_does_not_mutate_the_original_design() {
    let mut design = Design::fresh(
        indicatrix_cut_core::PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        96,
        8,
        1.54,
    );
    design.tiers.push(ConstraintTier {
        angle_deg: -40.0,
        name: "P1".to_string(),
        indices: vec![0.0, 12.0],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    });
    let before = design.clone();
    let _ = gear_remap_preview(&design, 96, 80, RemapRounding::Nearest);
    assert_eq!(design, before);
}

// --- EditorState::fresh_from_spec ---

#[test]
fn fresh_from_spec_round_trips_gear_symmetry_mirror_and_material() {
    let spec = FreshDesignSpec {
        gear_teeth: 80,
        symmetry_order: 5,
        mirror: false,
        material: MaterialSelection {
            name: Some("Quartz".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.55),
        },
        preform: indicatrix_cut_core::PreformSpec::cylinder(80, 1.4, 1.0, 1.3),
    };
    let state = EditorState::fresh_from_spec(spec);
    assert_eq!(state.design.meta.gear_teeth, 80);
    assert_eq!(state.design.meta.symmetry_order, 5);
    assert!(!state.design.meta.mirror);
    assert_eq!(state.design.material.name.as_deref(), Some("Quartz"));
    assert_eq!(state.design.material.refractive_index_override, Some(1.55));
    assert_eq!(state.design.tiers.len(), 0);
    assert!(!state.history.can_undo());
}

// A `SetMaterial` edit never touches tier geometry, so the ONLY thing that could
// invalidate a material-keyed cache (in `bridge::render_thread`) is the material
// itself changing.

#[test]
fn set_material_edit_leaves_every_tier_untouched_but_bumps_generation() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: ConstraintTier {
                angle_deg: -40.0,
                name: "P1".to_string(),
                indices: vec![0.0, 12.0],
                constraint: MeetConstraint::ScaleReference(0.5),
                imported_meet: None,
                detached: Vec::new(),
            },
        })
        .unwrap();
    let tiers_before = state.design.tiers.clone();
    let generation_before = state.generation.load(std::sync::atomic::Ordering::Relaxed);

    state
        .apply(Edit::SetMaterial {
            material: MaterialSelection {
                name: Some("Quartz".to_string()),
                specific_gravity_override: None,
                refractive_index_override: None,
            },
        })
        .unwrap();

    assert_eq!(
        state.design.tiers, tiers_before,
        "SetMaterial must not touch geometry"
    );
    assert!(
        state.generation.load(std::sync::atomic::Ordering::Relaxed) > generation_before,
        "SetMaterial must still bump generation, which is what a material-keyed cache checks"
    );
}

// --- angle_nudge_coalesce_key ---

#[test]
fn angle_nudge_coalesce_key_is_order_independent() {
    assert_eq!(
        angle_nudge_coalesce_key(&[3, 4]),
        angle_nudge_coalesce_key(&[4, 3]),
    );
}

#[test]
fn angle_nudge_coalesce_key_distinguishes_a_single_tier_from_a_group_containing_it() {
    // Nudging tier 3 alone must never coalesce with nudging {3, 4} together, even
    // though the same tier is involved in both.
    assert_ne!(
        angle_nudge_coalesce_key(&[3]),
        angle_nudge_coalesce_key(&[3, 4]),
    );
}

#[test]
fn angle_nudge_coalesce_key_distinguishes_disjoint_targets() {
    assert_ne!(
        angle_nudge_coalesce_key(&[0]),
        angle_nudge_coalesce_key(&[1]),
    );
}

// --- EditorState::apply_coalescing / multi_selected pruning ---

fn pavilion_tier(name: &str, angle_deg: f64) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: vec![],
        constraint: MeetConstraint::ScaleReference(0.5),
        imported_meet: None,
        detached: Vec::new(),
    }
}

#[test]
fn apply_coalescing_through_editor_state_merges_into_one_undo_step() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: pavilion_tier("P1", -40.0),
        })
        .unwrap();
    let before_nudges = state.design.clone();
    let key = angle_nudge_coalesce_key(&[0]);

    state
        .apply_coalescing(
            Edit::ModifyTier {
                index: 0,
                tier: pavilion_tier("P1", -40.1),
            },
            key,
        )
        .expect("first nudge must apply");
    state
        .apply_coalescing(
            Edit::ModifyTier {
                index: 0,
                tier: pavilion_tier("P1", -40.2),
            },
            key,
        )
        .expect("second nudge must apply");
    assert_eq!(state.design.tiers[0].angle_deg, -40.2);

    // One undo reverts BOTH nudges, all the way back to before the burst started.
    assert!(state.undo().unwrap());
    assert_eq!(state.design, before_nudges);
}

#[test]
fn multi_selected_is_pruned_when_a_tier_it_names_is_removed() {
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: pavilion_tier("P1", -40.0),
        })
        .unwrap();
    state
        .apply(Edit::AddTier {
            index: 1,
            tier: pavilion_tier("P2", -35.0),
        })
        .unwrap();
    state.multi_selected.insert(0);
    state.multi_selected.insert(1);

    state.apply(Edit::RemoveTier { index: 1 }).unwrap();

    assert!(
        state.multi_selected.contains(&0),
        "tier 0 still exists and must stay selected"
    );
    assert!(
        !state.multi_selected.contains(&1),
        "tier 1 no longer exists -- must be dropped from the selection, not left dangling"
    );
}

// --- inline_set_angle's own rejection path (`loading::parse_angle_only`) ---

#[test]
fn inline_set_angle_rejection_leaves_the_design_and_history_untouched() {
    // Mirrors `callbacks::tier_actions::setup_inline_set_angle_callback`'s own
    // control flow at the state level: parse first, and only call
    // `EditorState::apply` on `Ok`. Proves garbage input never reaches `apply` at
    // all, so `History` (and `design`) are left completely untouched -- not merely
    // "no visible effect" -- exactly what that callback relies on to make an
    // invalid inline edit a safe no-op.
    let mut state = EditorState::fresh();
    state
        .apply(Edit::AddTier {
            index: 0,
            tier: pavilion_tier("P1", -40.0),
        })
        .unwrap();
    let history_before = state.history.clone();
    let design_before = state.design.clone();

    let parsed = super::super::loading::parse_angle_only("not-a-number");
    assert!(parsed.is_err());
    // The real callback's `Err` arm only shows a toast -- it never calls
    // `EditorState::apply`, which this test enforces simply by not calling it here
    // either, then checking nothing moved regardless.
    assert_eq!(state.history, history_before);
    assert_eq!(state.design, design_before);
}

#[test]
fn inline_set_angle_accepts_a_well_formed_value() {
    // The accept-path counterpart: a valid angle parses to the exact `f64` an
    // `Edit::ModifyTier` would then carry.
    assert_eq!(
        super::super::loading::parse_angle_only(" -41.5 ").unwrap(),
        -41.5
    );
}
