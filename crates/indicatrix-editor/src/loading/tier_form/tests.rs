//! Tests for [`super::parse_tier_form`]/[`super::parse_angle_only`]/[`super::parse_tier_target`].

use super::*;

/// Builds a [`TierFormFields`] for the tests below that only care about the
/// five text/enum fields -- `gear_teeth_abs` fixed at 96 (every test index
/// here is well within that range) and no imported meet/original notes to
/// carry through, keeping each call site down to the fields it actually
/// varies.
fn tier_form<'a>(
    angle: &'a str,
    constraint_kind: i32,
    constraint_text: &'a str,
    name: &'a str,
    indices: &'a str,
) -> TierFormFields<'a> {
    TierFormFields {
        angle,
        constraint_kind,
        constraint_text,
        name,
        indices,
        gear_teeth_abs: 96,
        imported_meet: None,
        original_notes: None,
        other_tier_names: Vec::new(),
    }
}

#[test]
fn parse_angle_only_accepts_a_well_formed_number() {
    assert_eq!(parse_angle_only(" -41.0 ").unwrap(), -41.0);
    assert_eq!(parse_angle_only("0").unwrap(), 0.0);
}

#[test]
fn parse_angle_only_rejects_non_numeric_and_non_finite_text() {
    assert!(parse_angle_only("not-a-number").is_err());
    assert!(parse_angle_only("NaN").is_err());
    assert!(parse_angle_only("inf").is_err());
    assert!(parse_angle_only("").is_err());
}

#[test]
fn parse_angle_only_accepts_exactly_90_either_sign() {
    assert_eq!(parse_angle_only("90").unwrap(), 90.0);
    assert_eq!(parse_angle_only("-90").unwrap(), -90.0);
}

#[test]
fn parse_angle_only_rejects_a_magnitude_over_90() {
    let err = parse_angle_only("90.01").unwrap_err();
    assert!(err.contains("90"));
    let err = parse_angle_only("-410").unwrap_err();
    assert!(err.contains("90"));
}

#[test]
fn parse_tier_form_accepts_a_well_formed_scale_reference_row() {
    let tier = parse_tier_form(TierFormFields {
        angle: "-41.0",
        constraint_kind: 2,
        constraint_text: "0.65",
        name: " P1 ",
        indices: "0, 24, 48, 72",
        gear_teeth_abs: 96,
        imported_meet: None,
        original_notes: None,
        other_tier_names: Vec::new(),
    })
    .unwrap();
    assert_eq!(tier.angle_deg, -41.0);
    assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.65));
    assert_eq!(tier.name, "P1");
    assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
}

#[test]
fn parse_tier_form_carries_the_files_original_note_through_an_edit() {
    let tier = parse_tier_form(TierFormFields {
        angle: "-41.0",
        constraint_kind: 2,
        constraint_text: "0.65",
        name: "P1",
        indices: "0, 24",
        gear_teeth_abs: 96,
        imported_meet: None,
        original_notes: Some("Cut to TCP".to_string()),
        other_tier_names: Vec::new(),
    })
    .unwrap();
    assert_eq!(tier.original_notes.as_deref(), Some("Cut to TCP"));
}

#[test]
fn parse_tier_form_accepts_an_empty_index_list() {
    let tier = parse_tier_form(tier_form("0.0", 2, "0.32", "T", "")).unwrap();
    assert_eq!(tier.indices, [] as [f64; 0]);
}

#[test]
fn parse_tier_form_splits_on_comma_space_and_semicolon() {
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "1, 2 3;4")).unwrap();
    assert_eq!(tier.indices, vec![1.0, 2.0, 3.0, 4.0]);
}

// --- Shorthand index entry ---

#[test]
fn parse_tier_form_expands_a_colon_sequence() {
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0:12:96")).unwrap();
    assert_eq!(
        tier.indices,
        vec![0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0]
    );
}

#[test]
fn parse_tier_form_colon_sequence_can_be_combined_with_other_tokens() {
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0:24:96, 6")).unwrap();
    assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0, 6.0]);
}

#[test]
fn parse_tier_form_colon_sequence_out_of_range_is_still_reported() {
    // Step 200 on a 96-tooth gear wraps every generated value modulo 96,
    // so this never actually goes out of range -- confirm the wrap lands
    // on real teeth rather than raw (unwrapped) 200/400/etc.
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0:200:500")).unwrap();
    assert!(tier.indices.iter().all(|&v| (0.0..96.0).contains(&v)));
}

#[test]
fn parse_tier_form_expands_the_orbit_shorthand() {
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "12 x8")).unwrap();
    assert_eq!(
        tier.indices,
        vec![12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0, 0.0]
    );
}

#[test]
fn parse_tier_form_orbit_shorthand_accepts_uppercase_x() {
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0 X4")).unwrap();
    assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
}

#[test]
fn parse_tier_form_orbit_shorthand_can_be_combined_with_other_tokens() {
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "6, 0 x4")).unwrap();
    assert_eq!(tier.indices, vec![6.0, 0.0, 24.0, 48.0, 72.0]);
}

#[test]
fn parse_tier_form_orbit_shorthand_duplicate_with_a_prior_token_is_rejected() {
    let err = parse_tier_form(tier_form("10", 2, "0.5", "G", "0, 0 x4")).unwrap_err();
    assert!(err.contains("more than once"));
}

#[test]
fn parse_tier_form_a_lone_x_token_is_not_mistaken_for_shorthand() {
    // No preceding numeric token -- "x8" alone must fall through to the
    // ordinary "not a number" error, not panic or silently vanish.
    let err = parse_tier_form(tier_form("10", 2, "0.5", "G", "x8")).unwrap_err();
    assert!(err.contains("Index 'x8' is not a number."));
}

#[test]
fn parse_tier_form_negative_index_is_not_mistaken_for_a_colon_sequence() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "-1")).unwrap_err();
    assert!(err.contains("-1"));
}

#[test]
fn parse_tier_form_rejects_a_non_numeric_angle() {
    let err = parse_tier_form(tier_form("not-a-number", 2, "0.5", "T", "")).unwrap_err();
    assert!(err.contains("Angle"));
}

#[test]
fn parse_tier_form_rejects_a_magnitude_over_90() {
    let err = parse_tier_form(tier_form("95.0", 2, "0.5", "T", "")).unwrap_err();
    assert!(err.contains("exceeds 90"));
}

#[test]
fn the_over_90_message_reads_a_signed_entry_as_a_positive_angle() {
    let pavilion = parse_tier_form(tier_form("-95.5", 2, "0.5", "T", "")).unwrap_err();
    let crown = parse_tier_form(tier_form("95.5", 2, "0.5", "T", "")).unwrap_err();
    assert!(
        pavilion.starts_with("Angle 95.50\u{b0} exceeds 90\u{b0}"),
        "{pavilion}"
    );
    assert_eq!(pavilion, crown);
}

#[test]
fn parse_tier_form_accepts_exactly_90() {
    assert!(parse_tier_form(tier_form("90.0", 2, "0.5", "T", "")).is_ok());
    assert!(parse_tier_form(tier_form("-90.0", 2, "0.5", "T", "")).is_ok());
}

#[test]
fn parse_tier_form_rejects_a_name_another_tier_already_holds() {
    let mut form = tier_form("30.0", 2, "0.5", "P1", "");
    form.other_tier_names = vec!["P1".to_string()];
    let err = parse_tier_form(form).unwrap_err();
    assert!(err.contains("P1"));
}

#[test]
fn parse_tier_form_name_collision_check_is_case_insensitive() {
    let mut form = tier_form("30.0", 2, "0.5", "p1", "");
    form.other_tier_names = vec!["P1".to_string()];
    assert!(parse_tier_form(form).is_err());
}

#[test]
fn parse_tier_form_checks_each_slash_joined_name_for_a_collision() {
    let mut form = tier_form("30.0", 2, "0.5", "P1/P2", "");
    form.other_tier_names = vec!["P2".to_string()];
    let err = parse_tier_form(form).unwrap_err();
    assert!(err.contains("P2"));
}

#[test]
fn parse_tier_form_allows_a_name_no_other_tier_holds() {
    let mut form = tier_form("30.0", 2, "0.5", "P1", "");
    form.other_tier_names = vec!["G1".to_string(), "C1".to_string()];
    assert!(parse_tier_form(form).is_ok());
}

#[test]
fn parse_tier_form_rejects_a_non_numeric_index() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, x, 3")).unwrap_err();
    assert!(err.contains("Index"));
}

#[test]
fn parse_tier_form_rejects_non_finite_values() {
    assert!(parse_tier_form(tier_form("NaN", 2, "0.5", "T", "")).is_err());
    assert!(parse_tier_form(tier_form("0.0", 2, "inf", "T", "")).is_err());
}

#[test]
fn parse_tier_form_rejects_a_non_finite_index() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, NaN, 3")).unwrap_err();
    assert!(err.contains("Index"));
    assert!(err.contains("finite"));
}

#[test]
fn parse_tier_form_accepts_the_gears_tooth_count_as_a_valid_index() {
    // 96 on a 96-tooth gear is the same position as 0: valid, and kept as typed.
    let tier = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "96, 16, 32")).unwrap();
    assert_eq!(tier.indices, vec![96.0, 16.0, 32.0]);
    let tier = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, 96")).unwrap();
    assert_eq!(tier.indices, vec![1.0, 96.0]);
}

#[test]
fn parse_tier_form_rejects_an_index_past_the_gears_tooth_count() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, 97")).unwrap_err();
    assert!(err.contains("'97'"));
    assert!(err.contains("Indices run from 0 to 96 on this gear."));
    assert_eq!(tier_form_error_field(&err), "indices");
    // Just past the closing value is off the gear as well.
    assert!(parse_tier_form(tier_form("0.0", 2, "0.5", "T", "96.5")).is_err());
}

#[test]
fn parse_tier_form_treats_zero_and_the_tooth_count_as_the_same_position() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "0, 96")).unwrap_err();
    assert!(err.contains("'96'"));
    assert!(err.contains("more than once"));
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "96, 0")).unwrap_err();
    assert!(err.contains("'0'"));
    assert!(err.contains("more than once"));
}

#[test]
fn parse_tier_form_shorthand_that_wraps_onto_zero_collides_with_a_typed_tooth_count() {
    // "96 x4" expands to 96 (stored as 0 after the wrap), 24, 48, 72, so a later "0" repeats it.
    let tier = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "96 x4")).unwrap();
    assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "96 x4, 0")).unwrap_err();
    assert!(err.contains("more than once"));
}

#[test]
fn parse_tier_form_rejects_a_negative_index() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "-1")).unwrap_err();
    assert!(err.contains("-1"));
    assert!(err.contains("Indices run from 0 to 96 on this gear."));
}

#[test]
fn parse_tier_form_shorthand_that_runs_past_the_gear_wraps_onto_real_teeth() {
    // 96 lands on the closing tooth and wraps to 0; the rest keep counting on from there.
    let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "84:6:120")).unwrap();
    assert_eq!(tier.indices, vec![84.0, 90.0, 0.0, 6.0, 12.0, 18.0]);
}

#[test]
fn parse_index_list_accepts_the_tooth_count_on_any_gear() {
    assert_eq!(parse_index_list("64, 8", 64).unwrap(), vec![64.0, 8.0]);
    assert!(parse_index_list("65", 64).is_err());
    // A gear of zero teeth has only position 0.
    assert_eq!(parse_index_list("0", 0).unwrap(), vec![0.0]);
    assert!(parse_index_list("1", 0).is_err());
}

#[test]
fn parse_tier_form_rejects_a_duplicate_index() {
    let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "24, 48, 24")).unwrap_err();
    assert!(err.contains("24"));
    assert!(err.contains("more than once"));
}

#[test]
fn parse_tier_form_kind_zero_is_meet_existing_and_ignores_the_text_field() {
    let tier = parse_tier_form(tier_form("30.0", 0, "this text is ignored", "C1", "")).unwrap();
    assert_eq!(tier.constraint, MeetConstraint::MeetExisting);
}

#[test]
fn parse_tier_form_kind_one_splits_named_facets_on_comma() {
    let tier = parse_tier_form(tier_form("30.0", 1, "P1, P2 , G1", "C1", "")).unwrap();
    assert_eq!(
        tier.constraint,
        MeetConstraint::MeetNamed(vec!["P1".to_string(), "P2".to_string(), "G1".to_string()])
    );
}

#[test]
fn parse_tier_form_kind_one_rejects_an_empty_name_list() {
    let err = parse_tier_form(tier_form("30.0", 1, "  , ", "C1", "")).unwrap_err();
    assert!(err.contains("Meet named"));
}

#[test]
fn parse_tier_form_rejects_an_unknown_constraint_kind() {
    // `3`/`4`/`5` are now the three target kinds (see
    // `parse_tier_form_kinds_three_to_five_use_a_scale_reference_placeholder`
    // below) -- `6` is the first kind nothing recognizes.
    let err = parse_tier_form(tier_form("30.0", 6, "", "C1", "")).unwrap_err();
    assert!(err.contains('6'));
}

#[test]
fn parse_tier_form_kinds_three_to_five_use_a_scale_reference_placeholder() {
    // The real millimetre value lives in the `TierTarget` `parse_tier_target`
    // returns, never in the `ConstraintTier` itself -- see both functions' own
    // doc comments for why `Design::resolved_meet_tier_inputs` always
    // overwrites this placeholder before solving.
    for kind in 3..=5 {
        let tier = parse_tier_form(tier_form("30.0", kind, "3.2", "T", "")).unwrap();
        assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.0));
    }
}

// --- parse_tier_target (depth/girdle-thickness/table-width targets) ---

#[test]
fn parse_tier_target_is_none_for_the_three_plain_constraint_kinds() {
    for kind in 0..=2 {
        assert_eq!(parse_tier_target(kind, "0.5").unwrap(), None);
    }
}

#[test]
fn parse_tier_target_parses_a_depth_target_in_mm() {
    assert_eq!(
        parse_tier_target(3, " 3.20 ").unwrap(),
        Some(TierTarget::DepthMm(3.20))
    );
}

#[test]
fn parse_tier_target_parses_a_girdle_thickness_target_in_mm() {
    assert_eq!(
        parse_tier_target(4, "0.25").unwrap(),
        Some(TierTarget::GirdleThicknessMm(0.25))
    );
}

#[test]
fn parse_tier_target_parses_a_table_width_target_in_mm() {
    assert_eq!(
        parse_tier_target(5, "4.10").unwrap(),
        Some(TierTarget::TableWidthMm(4.10))
    );
}

#[test]
fn parse_tier_target_rejects_a_non_numeric_value_with_the_scale_reference_wording_style() {
    // A plain word holds no operator, so it keeps the short historical wording ...
    let err = parse_tier_target(3, "abc").unwrap_err();
    assert_eq!(err, "Depth 'abc' is not a number.");
    // ... while text with '-' signs in it is read as arithmetic (number fields take
    // expressions, see `number_expr`), so it is worded as a calculation that failed, and
    // names the word it could not use.
    let err = parse_tier_target(3, "not-a-number").unwrap_err();
    assert_eq!(
        err,
        "Depth 'not-a-number' cannot be calculated: 'not' is not a number."
    );
    let err = parse_tier_target(4, "wide").unwrap_err();
    assert_eq!(err, "Girdle thickness 'wide' is not a number.");
    let err = parse_tier_target(5, "wide").unwrap_err();
    assert_eq!(err, "Table width 'wide' is not a number.");
}

#[test]
fn parse_tier_target_rejects_a_non_finite_value() {
    let err = parse_tier_target(3, "NaN").unwrap_err();
    assert!(err.contains("finite"));
    let err = parse_tier_target(3, "inf").unwrap_err();
    assert!(err.contains("finite"));
}

#[test]
fn parse_tier_target_rejects_a_non_positive_value() {
    let err = parse_tier_target(4, "0.0").unwrap_err();
    assert!(err.contains("positive"));
    let err = parse_tier_target(5, "-1.0").unwrap_err();
    assert!(err.contains("positive"));
}
