//! The concave tier form: its string fields, parsing them into a [`ConcaveTier`] and
//! reading a tier back into fields.
//!
//! A concave tier is authored as two lines (a facet line and a tool line), so its form
//! carries both: name, angle, indices and instructions for the facet, then tool code,
//! θ, X/Y/Z, D/W, tool angle and motion. Every error message starts with the name of
//! the field it is about (`"angle: ..."`), which is what
//! [`super::tier_form::tier_form_error_field`] reads to mark the offending control.

use super::parse_index_list;
use indicatrix_cut_core::design::{ConcaveTier, ConcaveTierError, ConcaveTool, ToolMotion};

/// A concave tier form's fields, as typed.
///
/// `x`, `y`, `z` and `tool_azimuth_deg` may be left blank to mean `0` (a tool centred
/// on the facet, axis along the facet's own direction is the natural starting point);
/// `angle_deg` and `diameter_ratio` have no sensible default and must be given.
/// `tool_angle_deg` is blank for a tool without an angle and required for a cone or
/// disc. [`Self::reciprocating`] picks the motion: stroked back and forth, or plunged.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConcaveTierFormFields {
    /// Facet name, free text.
    pub name: String,
    /// φ, signed degrees.
    pub angle_deg: String,
    /// Index-wheel positions, in the flat form's own shorthand ([`parse_index_list`]).
    pub indices: String,
    /// Free-text cutting instructions, kept verbatim.
    pub instructions: String,
    /// Tool code (`CYL`, `CON`, `CIR`, `DSC`, `SPH`), case-insensitive.
    pub tool: String,
    /// θ, degrees.
    pub tool_azimuth_deg: String,
    /// X displacement, as a ratio of the stone width.
    pub x: String,
    /// Y displacement.
    pub y: String,
    /// Z displacement.
    pub z: String,
    /// D/W: tool diameter over stone width.
    pub diameter_ratio: String,
    /// Included angle of a cone or disc, degrees.
    pub tool_angle_deg: String,
    /// Whether the tool is stroked (`true`) or plunged (`false`).
    pub reciprocating: bool,
}

/// `text` as a finite number, or `"{field}: ..."`. A blank field is `default` when one
/// is given and an error otherwise.
fn parse_number(field: &str, label: &str, text: &str, default: Option<f64>) -> Result<f64, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return default.ok_or_else(|| format!("{field}: {label} is required."));
    }
    match trimmed.parse::<f64>() {
        Ok(value) if value.is_finite() => Ok(value),
        Ok(_) => Err(format!("{field}: {label} must be a finite number.")),
        Err(_) => Err(format!("{field}: '{trimmed}' is not a number.")),
    }
}

/// Parses a concave tier form into a validated [`ConcaveTier`].
///
/// Beyond reading each field, this runs [`ConcaveTier::validate`] against
/// `gear_teeth`, so a tier it returns is one an `Edit::AddConcaveTier` accepts (except
/// for a name clash with a flat tier, which needs the whole design and is reported by
/// the edit itself).
///
/// # Errors
///
/// A message ready to show the cutter, prefixed with the field it is about: `"name:"`,
/// `"angle:"`, `"indices:"`, `"tool:"`, `"theta:"`, `"x:"`, `"y:"`, `"z:"`,
/// `"diameter:"` or `"tool angle:"`.
pub fn parse_concave_tier_form(
    fields: &ConcaveTierFormFields,
    gear_teeth: i32,
) -> Result<ConcaveTier, String> {
    let tool = fields
        .tool
        .parse::<ConcaveTool>()
        .map_err(|error| format!("tool: {error}"))?;
    let angle_deg = parse_number("angle", "the angle", &fields.angle_deg, None)?;
    let indices = parse_index_list(&fields.indices, gear_teeth.unsigned_abs())
        .map_err(|message| format!("indices: {message}"))?;
    let tool_azimuth_deg = parse_number("theta", "theta", &fields.tool_azimuth_deg, Some(0.0))?;
    let displacement = [
        parse_number("x", "X", &fields.x, Some(0.0))?,
        parse_number("y", "Y", &fields.y, Some(0.0))?,
        parse_number("z", "Z", &fields.z, Some(0.0))?,
    ];
    let diameter_ratio = parse_number("diameter", "D/W", &fields.diameter_ratio, None)?;
    let tool_angle_deg = if fields.tool_angle_deg.trim().is_empty() {
        None
    } else {
        Some(parse_number(
            "tool angle",
            "the tool angle",
            &fields.tool_angle_deg,
            None,
        )?)
    };
    let tier = ConcaveTier {
        name: fields.name.trim().to_owned(),
        angle_deg,
        indices,
        instructions: fields.instructions.clone(),
        tool,
        tool_azimuth_deg,
        displacement,
        diameter_ratio,
        tool_angle_deg,
        motion: if fields.reciprocating {
            ToolMotion::Reciprocating
        } else {
            ToolMotion::Plunge
        },
    };
    tier.validate(gear_teeth)
        .map_err(|error| validation_message(&error))?;
    Ok(tier)
}

/// A [`ConcaveTierError`] as a form message with its field prefix.
fn validation_message(error: &ConcaveTierError) -> String {
    let field = match error {
        ConcaveTierError::NonFinite { field, .. } => match *field {
            "angle_deg" => "angle",
            "indices" => "indices",
            "tool_azimuth_deg" => "theta",
            "diameter_ratio" => "diameter",
            "tool_angle_deg" => "tool angle",
            _ => "x",
        },
        ConcaveTierError::AngleOutOfRange { .. } => "angle",
        ConcaveTierError::NoIndices | ConcaveTierError::IndexOutOfRange { .. } => "indices",
        ConcaveTierError::DiameterNotPositive { .. }
        | ConcaveTierError::DiameterTooLarge { .. } => "diameter",
        ConcaveTierError::DisplacementTooLarge { .. } => "x",
        ConcaveTierError::ToolAngleMissing { .. }
        | ConcaveTierError::ToolAngleUnexpected { .. }
        | ConcaveTierError::ToolAngleOutOfRange { .. } => "tool angle",
        // `NameClash`, and any variant added later: the tier as a whole.
        _ => "name",
    };
    format!("{field}: {error}")
}

/// A tier as form fields, at full precision.
///
/// Re-saving the fields never rounds a value the design still holds exactly:
/// [`parse_concave_tier_form`] of the result is the same tier (the `Display` of an
/// `f64` round-trips).
#[must_use]
pub fn concave_tier_form_fields(tier: &ConcaveTier) -> ConcaveTierFormFields {
    let [x, y, z] = tier.displacement;
    ConcaveTierFormFields {
        name: tier.name.clone(),
        angle_deg: format!("{}", tier.angle_deg),
        indices: tier
            .indices
            .iter()
            .map(|index| format!("{index}"))
            .collect::<Vec<_>>()
            .join(", "),
        instructions: tier.instructions.clone(),
        tool: tier.tool.code().to_owned(),
        tool_azimuth_deg: format!("{}", tier.tool_azimuth_deg),
        x: format!("{x}"),
        y: format!("{y}"),
        z: format!("{z}"),
        diameter_ratio: format!("{}", tier.diameter_ratio),
        tool_angle_deg: tier
            .tool_angle_deg
            .map_or_else(String::new, |angle| format!("{angle}")),
        reciprocating: tier.motion == ToolMotion::Reciprocating,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loading::tier_form_error_field;

    fn valid() -> ConcaveTierFormFields {
        ConcaveTierFormFields {
            name: "Groove".to_owned(),
            angle_deg: "-42".to_owned(),
            indices: "0 12 24 36".to_owned(),
            instructions: "meet: cut to depth".to_owned(),
            tool: "cyl".to_owned(),
            tool_azimuth_deg: "15".to_owned(),
            x: "0".to_owned(),
            y: "0.12".to_owned(),
            z: "0.05".to_owned(),
            diameter_ratio: "0.25".to_owned(),
            tool_angle_deg: String::new(),
            reciprocating: true,
        }
    }

    #[test]
    fn parse_concave_tier_form_maps_each_hostile_input_to_its_field() {
        let with = |edit: fn(&mut ConcaveTierFormFields)| {
            let mut fields = valid();
            edit(&mut fields);
            fields
        };
        let cases: [(ConcaveTierFormFields, &str); 10] = [
            (with(|f| f.angle_deg = "NaN".to_owned()), "angle:"),
            (with(|f| f.angle_deg.clear()), "angle:"),
            (with(|f| f.angle_deg = "90".to_owned()), "angle:"),
            (with(|f| f.tool = "CON".to_owned()), "tool angle:"),
            (with(|f| f.tool_angle_deg = "60".to_owned()), "tool angle:"),
            (with(|f| f.tool = "XYZ".to_owned()), "tool:"),
            (with(|f| f.indices.clear()), "indices:"),
            (with(|f| f.indices = "0 500".to_owned()), "indices:"),
            (with(|f| f.diameter_ratio = "-1".to_owned()), "diameter:"),
            (with(|f| f.y = "inf".to_owned()), "y:"),
        ];
        for (fields, prefix) in cases {
            let message = parse_concave_tier_form(&fields, 96)
                .expect_err(&format!("{fields:?} must be rejected"));
            assert!(message.starts_with(prefix), "{message:?} for {fields:?}");
            // The prefix is the field the UI marks; none of them may fall into the
            // "general" bucket.
            assert_ne!(tier_form_error_field(&message), "", "{message:?}");
        }
        assert!(parse_concave_tier_form(&valid(), 96).is_ok());
    }

    #[test]
    fn concave_form_fields_round_trip_through_parse() {
        let cone = ConcaveTier {
            name: "Bowl".to_owned(),
            angle_deg: 33.333_333_333_333_336,
            indices: vec![0.0, 24.5, 48.0],
            instructions: "line one".to_owned(),
            tool: ConcaveTool::Cone,
            tool_azimuth_deg: -12.345_678_901_234_567,
            displacement: [0.1, -0.2, 0.3],
            diameter_ratio: 0.456_789_012_345_678_9,
            tool_angle_deg: Some(61.5),
            motion: ToolMotion::Plunge,
        };
        let fields = concave_tier_form_fields(&cone);
        assert_eq!(parse_concave_tier_form(&fields, 96), Ok(cone));
        let sphere = ConcaveTier {
            tool: ConcaveTool::Sphere,
            tool_angle_deg: None,
            motion: ToolMotion::Reciprocating,
            ..parse_concave_tier_form(&valid(), 96).unwrap()
        };
        assert_eq!(
            parse_concave_tier_form(&concave_tier_form_fields(&sphere), 96),
            Ok(sphere)
        );
    }
}
