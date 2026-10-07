//! Arithmetic in the tier form's number fields (`40 + 1.5`), the names variant of the
//! inline angle parser, and the `=` relation form of the angle field.

use super::*;

fn form<'a>(angle: &'a str, constraint_text: &'a str) -> TierFormFields<'a> {
    TierFormFields {
        angle,
        constraint_kind: 2,
        constraint_text,
        name: "P1",
        indices: "0, 12",
        gear_teeth_abs: 96,
        imported_meet: None,
        original_notes: None,
        other_tier_names: Vec::new(),
    }
}

#[test]
fn the_angle_and_the_scale_reference_take_arithmetic() {
    let tier = parse_tier_form(form("-40 - 1.5", "0.5 + 0.25")).expect("parses");
    assert_eq!(tier.angle_deg, -41.5);
    assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.75));
    // Plain numbers come out exactly as before.
    let tier = parse_tier_form(form(" -41.0 ", "0.65")).expect("parses");
    assert_eq!(tier.angle_deg.to_bits(), (-41.0_f64).to_bits());
    assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.65));
}

#[test]
fn a_calculation_that_fails_keeps_the_field_prefix_the_ui_reads() {
    let message = parse_tier_form(form("abc", "1")).unwrap_err();
    assert_eq!(message, "Angle 'abc' is not a number.");
    assert_eq!(tier_form_error_field(&message), "angle");

    let message = parse_tier_form(form("40 +", "1")).unwrap_err();
    assert!(
        message.starts_with("Angle '40 +' cannot be calculated:"),
        "{message}"
    );
    assert_eq!(tier_form_error_field(&message), "angle");

    let message = parse_tier_form(form("40", "1 / 0")).unwrap_err();
    assert_eq!(
        message,
        "Scale reference '1 / 0' cannot be calculated: it divides by zero."
    );
    assert_eq!(tier_form_error_field(&message), "constraint");

    // The old range and finiteness checks still apply to a calculated value.
    let message = parse_tier_form(form("100 - 5", "1")).unwrap_err();
    assert!(message.contains("exceeds 90"), "{message}");
}

#[test]
fn a_target_takes_arithmetic_and_keeps_its_wording() {
    assert_eq!(
        parse_tier_target(3, "2 + 0.5"),
        Ok(Some(TierTarget::DepthMm(2.5)))
    );
    assert_eq!(
        parse_tier_target(5, "3 * 1.5"),
        Ok(Some(TierTarget::TableWidthMm(4.5)))
    );
    assert_eq!(
        parse_tier_target(4, "1 - 2"),
        Err("Girdle thickness must be a positive number.".to_string())
    );
    assert_eq!(
        parse_tier_target(3, "x"),
        Err("Depth 'x' is not a number.".to_string())
    );
    let message = parse_tier_target(3, "1 / 0").unwrap_err();
    assert_eq!(
        message,
        "Depth '1 / 0' cannot be calculated: it divides by zero."
    );
    assert_eq!(tier_form_error_field(&message), "constraint");
    // A kind that is not a target still reads nothing.
    assert_eq!(parse_tier_target(2, "1 / 0"), Ok(None));
}

#[test]
fn the_inline_angle_takes_arithmetic_and_optionally_names() {
    assert_eq!(parse_angle_only("90 - 0.5"), Ok(89.5));
    assert_eq!(parse_angle_only("-(40 + 1)"), Ok(-41.0));
    assert!(
        parse_angle_only("100 - 5")
            .unwrap_err()
            .contains("exceeds 90")
    );
    assert_eq!(
        parse_angle_only("x"),
        Err("Angle 'x' is not a number.".to_string())
    );

    let names = |name: &str| (name == "C1").then_some(40.0);
    assert_eq!(
        parse_angle_only_with_names("C1 - 4", Some(&names)),
        Ok(36.0)
    );
    assert_eq!(parse_angle_only_with_names("41.5", Some(&names)), Ok(41.5));
    assert_eq!(
        parse_angle_only_with_names("C2 - 4", Some(&names)),
        Err("Angle 'C2 - 4' cannot be calculated: there is no tier called 'C2'.".to_string())
    );
    // Without a lookup, a name is not a number.
    assert!(parse_angle_only("C1 - 4").is_err());
    // A leading `=` is a relation, which only the angle field of a form can carry.
    let message = parse_angle_only("=C1").unwrap_err();
    assert!(
        message.starts_with("Angle '=C1' cannot be calculated:"),
        "{message}"
    );
}

#[test]
fn an_equals_sign_makes_the_angle_a_relation() {
    let (tier, relation) =
        parse_tier_form_with_relation(form("=C1-4", "0.5"), 12.5).expect("parses");
    assert_eq!(relation.as_deref(), Some("C1-4"));
    // The relation, not the form, decides the angle; the tier carries a placeholder.
    assert_eq!(tier.angle_deg, 12.5);
    assert_eq!(tier.name, "P1");
    assert_eq!(tier.indices, vec![0.0, 12.0]);

    let (_, relation) =
        parse_tier_form_with_relation(form("= (C1 + C3) / 2 ", "0.5"), 12.5).expect("parses");
    assert_eq!(relation.as_deref(), Some("(C1 + C3) / 2"));

    // No `=`: exactly `parse_tier_form`, with no relation.
    let (tier, relation) = parse_tier_form_with_relation(form("41", "0.5"), 12.5).expect("parses");
    assert_eq!((tier.angle_deg, relation), (41.0, None));
}

#[test]
fn a_relation_form_is_still_checked_field_by_field() {
    let message = parse_tier_form_with_relation(form("=", "0.5"), 12.5).unwrap_err();
    assert!(message.starts_with("Angle relation is empty"), "{message}");
    assert_eq!(tier_form_error_field(&message), "angle");

    let mut bad_indices = form("=C1-4", "0.5");
    bad_indices.indices = "x";
    let message = parse_tier_form_with_relation(bad_indices, 12.5).unwrap_err();
    assert_eq!(tier_form_error_field(&message), "indices", "{message}");

    let message = parse_tier_form_with_relation(form("=C1-4", "1 / 0"), 12.5).unwrap_err();
    assert!(message.starts_with("Scale reference"), "{message}");
}
