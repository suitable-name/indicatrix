//! Arithmetic in the editor's number fields: `41 + 0.5`, `(90 - 41) / 2`, `-2 * 3`.
//!
//! [`eval_number`] is the one entry point every numeric form parser calls instead of
//! `str::parse::<f64>`. The rule that keeps every existing behaviour unchanged: text
//! that `str::parse::<f64>` accepts is returned exactly as that parse gives it (even
//! `NaN` or `inf`, which the callers' own finite checks then refuse as before), and
//! text without an operator or parenthesis is [`NumberExprError::NotANumber`], which
//! [`NumberExprError::message`] words exactly like the old "... is not a number."
//! error. Only text that really contains `+ - * / ( )` is calculated, with the same
//! parser a tier relation uses (see `indicatrix_cut_core::design::Expr`).

use indicatrix_cut_core::design::{Expr, ExprError, MAX_RELATION_CHARS, RawName, snap_noise};
use std::fmt;

/// Why [`eval_number`] could not give a number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NumberExprError {
    /// The text is not a number and holds no calculation either; the caller words
    /// this as its old "... is not a number." message (see [`Self::message`]).
    NotANumber,
    /// The text looks like a calculation but cannot be done; the reason is a plain
    /// English phrase without a final full stop ("there is no tier called 'X'").
    Invalid(String),
}

impl NumberExprError {
    /// The message for a field called `label` that held `text`: the historical
    /// "`label` 'text' is not a number." for [`Self::NotANumber`], and
    /// "`label` 'text' cannot be calculated: reason." otherwise. Both start with
    /// `label`, which `tier_form_error_field` relies on to find the offending field.
    #[must_use]
    pub fn message(&self, label: &str, text: &str) -> String {
        let text = text.trim();
        match self {
            Self::NotANumber => format!("{label} '{text}' is not a number."),
            Self::Invalid(reason) => format!("{label} '{text}' cannot be calculated: {reason}."),
        }
    }
}

impl fmt::Display for NumberExprError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotANumber => f.write_str("that is not a number"),
            Self::Invalid(reason) => f.write_str(reason),
        }
    }
}

impl std::error::Error for NumberExprError {}

/// What a tier name stands for in a number field: the value of the tier called
/// that, or `None` when no single tier has the name.
pub type NameValues<'a> = &'a dyn Fn(&str) -> Option<f64>;

/// Reads `text` as a number or as arithmetic over numbers.
///
/// `names`, when given, lets the text refer to values by name (`P1 - 2` with the
/// tier's angle magnitude); without it a name is an error.
///
/// # Errors
///
/// [`NumberExprError::NotANumber`] for text that is neither a number nor a
/// calculation; [`NumberExprError::Invalid`] for a calculation that cannot be read or
/// done (a missing bracket, an unknown name, a division by zero, a result that is not
/// a finite number).
pub fn eval_number(text: &str, names: Option<NameValues<'_>>) -> Result<f64, NumberExprError> {
    let trimmed = text.trim();
    if let Ok(value) = trimmed.parse::<f64>() {
        return Ok(value);
    }
    if trimmed.starts_with('=') {
        return Err(NumberExprError::Invalid(
            "a leading '=' makes a relation, which only a tier's angle can use".to_owned(),
        ));
    }
    if !has_operator(trimmed) {
        return Err(NumberExprError::NotANumber);
    }
    let syntax_tree = Expr::<RawName>::parse_syntax(trimmed, MAX_RELATION_CHARS)
        .map_err(|error| NumberExprError::Invalid(error.to_string()))?;
    let mut lookup = |name: &RawName| -> Result<f64, ExprError> {
        match (name, names) {
            (RawName::Bare(word) | RawName::Bracketed(word), Some(names)) => names(word)
                .ok_or_else(|| ExprError::UnknownName(format!("there is no tier called '{word}'"))),
            (other, _) => Err(ExprError::UnknownName(format!(
                "'{}' is not a number",
                other.text()
            ))),
        }
    };
    syntax_tree
        .eval(&mut lookup)
        .map(snap_noise)
        .map_err(|error| NumberExprError::Invalid(error.to_string()))
}

/// Whether `text` holds an operator or bracket beyond one leading sign.
fn has_operator(text: &str) -> bool {
    let body = text.strip_prefix(['+', '-']).unwrap_or(text);
    body.contains(['+', '-', '*', '/', '(', ')'])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str) -> f64 {
        eval_number(text, None).expect("a number")
    }

    #[test]
    fn plain_numbers_come_back_exactly_as_parse_gives_them() {
        assert_eq!(value("-41.0").to_bits(), (-41.0_f64).to_bits());
        assert_eq!(value(" 12 ").to_bits(), 12.0_f64.to_bits());
        assert_eq!(value("90.01").to_bits(), 90.01_f64.to_bits());
        assert_eq!(value("-90").to_bits(), (-90.0_f64).to_bits());
        assert_eq!(value("1e-3").to_bits(), 1e-3_f64.to_bits());
        assert_eq!(value("+5").to_bits(), 5.0_f64.to_bits());
        // Not finite, but exactly what `parse` says: the callers' finite checks
        // refuse these with their historical messages.
        assert!(value("NaN").is_nan());
        assert_eq!(value("inf"), f64::INFINITY);
    }

    #[test]
    fn text_without_an_operator_keeps_the_historical_wording() {
        for text in ["x", "", "  ", "12abc", "1.2.3", "- 41", "1e", "[P1]"] {
            let error = eval_number(text, None).expect_err("not a number");
            assert_eq!(error, NumberExprError::NotANumber, "{text:?}");
        }
        assert_eq!(
            NumberExprError::NotANumber.message("Angle", "  x "),
            "Angle 'x' is not a number."
        );
    }

    #[test]
    fn arithmetic_follows_the_usual_precedence() {
        assert_eq!(value("41 + 0.5"), 41.5);
        assert_eq!(value("2 + 3 * 4"), 14.0);
        assert_eq!(value("(2 + 3) * 4"), 20.0);
        assert_eq!(value("10 - 4 - 3"), 3.0);
        assert_eq!(value("100 / 4 / 5"), 5.0);
        assert_eq!(value("-2 * 3"), -6.0);
        assert_eq!(value("-(2 + 3)"), -5.0);
        assert_eq!(value("2 * -3"), -6.0);
        assert_eq!(value("(90 - 41) / 2"), 24.5);
    }

    #[test]
    fn float_noise_is_snapped_away() {
        assert_eq!(value("0.1 + 0.2").to_bits(), 0.3_f64.to_bits());
        assert_eq!(value("40 + 3 * 0.1").to_bits(), 40.3_f64.to_bits());
    }

    #[test]
    fn a_broken_calculation_says_why() {
        for (text, needle) in [
            ("1 / 0", "divides by zero"),
            ("(1 + 2", "("),
            ("1 +", "missing"),
            ("2 3 + 1", "put +"),
            ("1 + x", "is not a number"),
        ] {
            match eval_number(text, None) {
                Err(NumberExprError::Invalid(reason)) => {
                    assert!(reason.contains(needle), "{text:?}: {reason}");
                }
                other => panic!("{text:?}: expected Invalid, got {other:?}"),
            }
        }
        let error = eval_number("1 / 0", None).expect_err("divides by zero");
        assert_eq!(
            error.message("Step", " 1 / 0 "),
            "Step '1 / 0' cannot be calculated: it divides by zero."
        );
    }

    #[test]
    fn a_result_that_is_not_finite_is_refused() {
        assert!(matches!(
            eval_number("1e308 * 10", None),
            Err(NumberExprError::Invalid(_))
        ));
    }

    #[test]
    fn names_resolve_only_when_a_lookup_is_given() {
        let lookup = |name: &str| (name == "P1").then_some(41.0);
        assert_eq!(eval_number("P1 - 2", Some(&lookup)), Ok(39.0));
        assert_eq!(eval_number("[P1] + 0.5", Some(&lookup)), Ok(41.5));
        match eval_number("P2 - 2", Some(&lookup)) {
            Err(NumberExprError::Invalid(reason)) => {
                assert_eq!(reason, "there is no tier called 'P2'");
            }
            other => panic!("expected Invalid, got {other:?}"),
        }
        assert!(matches!(
            eval_number("P1 - 2", None),
            Err(NumberExprError::Invalid(_))
        ));
    }

    #[test]
    fn the_yield_and_material_fields_take_arithmetic() {
        use crate::material::{
            design_material_options, parse_design_material_form, parse_yield_form,
        };
        use indicatrix_cut_core::MaterialSelection;
        let current = MaterialSelection {
            name: None,
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
        };
        let (girdle, selection) =
            parse_yield_form("6 + 0.5", 0, "3.1 + 0.1", &current).expect("parses");
        assert_eq!(girdle, Some(6.5));
        assert_eq!(selection.specific_gravity_override, Some(3.2));
        assert_eq!(
            parse_yield_form("6 / 0", 0, "", &current).unwrap_err(),
            "Girdle diameter '6 / 0' cannot be calculated: it divides by zero."
        );
        assert_eq!(
            parse_yield_form("x", 0, "", &current).unwrap_err(),
            "Girdle diameter 'x' is not a number."
        );

        let options = design_material_options(&[]);
        let selection =
            parse_design_material_form(0, "1.5 + 0.04", &options, &current).expect("parses");
        assert_eq!(selection.refractive_index_override, Some(1.54));
        let error = parse_design_material_form(0, "1.5 +", &options, &current).unwrap_err();
        assert!(
            error.starts_with("Refractive index '1.5 +' cannot be calculated:"),
            "{error}"
        );
    }

    #[test]
    fn overlong_text_is_refused() {
        let long = format!("1{}", " + 1".repeat(200));
        assert!(matches!(
            eval_number(&long, None),
            Err(NumberExprError::Invalid(_))
        ));
    }
}
