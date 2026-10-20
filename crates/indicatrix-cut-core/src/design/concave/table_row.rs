//! Reading a [`ConcaveTier`] back from the cells of a library angle-table row.
//!
//! A library record keeps each concave tier of its design file as one row of its angle
//! table: the facet line in the flat rows' own format (name, `angle°`, the indices joined
//! by commas, the instructions as notes) and the tool as two texts, the tool code and the
//! formatted second line of the cutting sheet ([`ConcaveTier::second_line_fields`] joined
//! by two spaces). A record that has no usable design file (or only an `.asc`, whose
//! concave tiers are footnotes) still carries those rows, and this is the inverse of the
//! writer, so the stone can be drawn and measured with its tools.
//!
//! **The cells are rounded.** The tool line prints the angles with two decimals and the
//! offsets and the diameter ratio with three (the cutting sheet's own precision), so a tier
//! read back from a row equals the original only to that precision. A tier typed at the
//! sheet's precision (the usual case) comes back exactly.

use super::{ConcaveTier, ConcaveTool, ToolMotion};

/// The cells of one angle-table row that describes a concave tier.
#[derive(Debug, Clone, Copy)]
pub struct ConcaveTableRow<'a> {
    /// The facet name.
    pub facet: &'a str,
    /// The angle cell, `-42°` or a plain number.
    pub angle: &'a str,
    /// The indices, separated by commas or spaces.
    pub index: &'a str,
    /// The notes cell, which holds the tier's instructions.
    pub notes: &'a str,
    /// The tool code (`CYL`), or empty to read it off the tool line.
    pub tool: &'a str,
    /// The formatted tool line: the four columns joined by two spaces.
    pub tool_line: &'a str,
}

/// `text` without a trailing degree sign (`°` or `deg`) and surrounding blanks.
fn without_degrees(text: &str) -> &str {
    let trimmed = text.trim();
    let trimmed = trimmed.strip_suffix('\u{b0}').unwrap_or(trimmed);
    trimmed.strip_suffix("deg").unwrap_or(trimmed).trim()
}

/// `text` as a finite number (a leading `+` is allowed), or a sentence naming `what`.
fn number(text: &str, what: &str) -> Result<f64, String> {
    let value = without_degrees(text);
    match value.parse::<f64>() {
        Ok(parsed) if parsed.is_finite() => Ok(parsed),
        _ => Err(format!("{what} is not a number ({value:?})")),
    }
}

/// The number after the `=` of `part` (`X = 0.150`), checking the label before it.
fn labelled(part: &str, label: &str) -> Result<f64, String> {
    let (key, value) = part
        .split_once('=')
        .ok_or_else(|| format!("{label} is missing in {part:?}"))?;
    if key.trim() != label {
        return Err(format!("expected {label} in {part:?}"));
    }
    number(value, label)
}

impl ConcaveTier {
    /// Rebuilds a tier from an angle-table row (see the module comment for what is
    /// rounded). The result is not validated: the caller's resolver does that
    /// ([`crate::design::resolve_tiers_on_planes`]).
    ///
    /// # Errors
    ///
    /// A sentence saying which cell could not be read.
    pub fn from_table_row(row: &ConcaveTableRow<'_>) -> Result<Self, String> {
        let angle_deg = number(row.angle, "the angle")?;
        let indices = row
            .index
            .split([',', ' ', ';'])
            .filter(|piece| !piece.trim().is_empty())
            .map(|piece| number(piece, "an index"))
            .collect::<Result<Vec<f64>, String>>()?;

        let fields: Vec<&str> = row
            .tool_line
            .split("  ")
            .map(str::trim)
            .filter(|field| !field.is_empty())
            .collect();
        let &[code, theta, offsets, details] = fields.as_slice() else {
            return Err(format!(
                "the tool line has {} columns, not 4 ({:?})",
                fields.len(),
                row.tool_line
            ));
        };
        let tool_code = if row.tool.trim().is_empty() {
            code
        } else {
            row.tool
        };
        let tool = tool_code
            .parse::<ConcaveTool>()
            .map_err(|error| error.to_string())?;
        let tool_azimuth_deg = number(theta, "theta")?;

        let parts: Vec<&str> = offsets.split(',').collect();
        let &[x, y, z] = parts.as_slice() else {
            return Err(format!("the offsets are not X, Y and Z ({offsets:?})"));
        };
        let displacement = [labelled(x, "X")?, labelled(y, "Y")?, labelled(z, "Z")?];

        let mut diameter_ratio = None;
        let mut tool_angle_deg = None;
        let mut motion = None;
        for part in details.split(',').map(str::trim) {
            if let Some(value) = part.strip_prefix("D/W") {
                diameter_ratio = Some(number(value.trim_start().trim_start_matches('='), "D/W")?);
            } else if let Some(value) = part.strip_prefix("angle") {
                tool_angle_deg = Some(number(
                    value.trim_start().trim_start_matches('='),
                    "the tool angle",
                )?);
            } else if let Some(word) = ToolMotion::from_word(part) {
                motion = Some(word);
            } else {
                return Err(format!("unreadable tool detail {part:?}"));
            }
        }
        let diameter_ratio =
            diameter_ratio.ok_or_else(|| format!("no D/W in the tool details ({details:?})"))?;
        let motion =
            motion.ok_or_else(|| format!("no motion in the tool details ({details:?})"))?;

        Ok(Self {
            name: row.facet.trim().to_owned(),
            angle_deg,
            indices,
            instructions: row.notes.to_owned(),
            tool,
            tool_azimuth_deg,
            displacement,
            diameter_ratio,
            tool_angle_deg,
            motion,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The cells the library writes for `tier` (`apply_concave_rows` in the desktop app).
    fn cells(tier: &ConcaveTier) -> (String, String, String, String) {
        (
            format!("{}\u{b0}", tier.angle_deg),
            tier.indices
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            tier.tool.code().to_owned(),
            tier.second_line_fields().join("  "),
        )
    }

    fn read_back(tier: &ConcaveTier) -> Result<ConcaveTier, String> {
        let (angle, index, tool, tool_line) = cells(tier);
        ConcaveTier::from_table_row(&ConcaveTableRow {
            facet: &tier.name,
            angle: &angle,
            index: &index,
            notes: &tier.instructions,
            tool: &tool,
            tool_line: &tool_line,
        })
    }

    fn cone() -> ConcaveTier {
        ConcaveTier {
            name: "Funnel".to_owned(),
            angle_deg: 36.5,
            indices: vec![0.0, 12.5, 24.0],
            instructions: "cut to the meet".to_owned(),
            tool: ConcaveTool::Cone,
            tool_azimuth_deg: -15.25,
            displacement: [0.125, -0.05, 0.375],
            diameter_ratio: 0.5,
            tool_angle_deg: Some(60.0),
            motion: ToolMotion::Plunge,
        }
    }

    #[test]
    fn a_tier_at_the_sheets_precision_reads_back_exactly() {
        for tier in crate::design::Design::concave_fixture()
            .concave_tiers
            .iter()
            .chain([&cone()])
        {
            assert_eq!(read_back(tier).as_ref(), Ok(tier), "{}", tier.name);
        }
    }

    #[test]
    fn a_finer_value_comes_back_rounded_to_the_sheets_precision() {
        let mut tier = cone();
        tier.displacement = [0.123_456, 0.0, 0.0];
        tier.tool_azimuth_deg = 10.004;
        let back = read_back(&tier).expect("reads");
        assert!((back.displacement[0] - 0.123).abs() < 1e-12);
        assert!((back.tool_azimuth_deg - 10.0).abs() < 1e-12);
    }

    #[test]
    fn the_tool_code_cell_may_be_empty_and_the_ascii_degree_word_reads() {
        let tier = cone();
        let (angle, index, _, _) = cells(&tier);
        let ascii = tier.second_line_fields_ascii().join("  ");
        let back = ConcaveTier::from_table_row(&ConcaveTableRow {
            facet: &tier.name,
            angle: &angle,
            index: &index,
            notes: &tier.instructions,
            tool: "",
            tool_line: &ascii,
        })
        .expect("reads");
        assert_eq!(back, tier);
    }

    #[test]
    fn an_unreadable_row_says_which_cell() {
        let tier = cone();
        let (angle, index, tool, tool_line) = cells(&tier);
        let row = |angle: &str, tool: &str, tool_line: &str| {
            ConcaveTier::from_table_row(&ConcaveTableRow {
                facet: "x",
                angle,
                index: &index,
                notes: "",
                tool,
                tool_line,
            })
        };
        assert!(
            row("steep", &tool, &tool_line)
                .unwrap_err()
                .contains("angle")
        );
        assert!(row(&angle, "XYZ", &tool_line).is_err());
        assert!(
            row(&angle, &tool, "CON  +0.00\u{b0}")
                .unwrap_err()
                .contains("columns")
        );
        let no_motion = tool_line.replace(", plunge", "");
        assert!(
            row(&angle, &tool, &no_motion)
                .unwrap_err()
                .contains("motion")
        );
        let no_ratio = tool_line.replace("D/W = 0.500, ", "");
        assert!(row(&angle, &tool, &no_ratio).unwrap_err().contains("D/W"));
    }
}
