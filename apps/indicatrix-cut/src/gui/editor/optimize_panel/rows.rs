//! The Optimize tab's plain row structs ([`indicatrix_editor::optimize_view`]) as the Slint
//! row types of `ui/models/optimize.slint`.

use crate::{OptimizeCandidateRow, OptimizeRangeRow};
use indicatrix_editor::optimize_view::{CandidateLine, RangeInput, RangeRow, format_range_value};

/// One candidate-list row; the cells are formatted already.
pub(super) fn candidate_row(line: &CandidateLine) -> OptimizeCandidateRow {
    OptimizeCandidateRow {
        rank: line.rank.as_str().into(),
        score: line.score.as_str().into(),
        windowing: line.windowing.as_str().into(),
        brilliance: line.brilliance.as_str().into(),
        extinction: line.extinction.as_str().into(),
        yield_loss: line.yield_loss.as_str().into(),
        changed: line.changed.as_str().into(),
        score_dir: line.score_direction,
        windowing_dir: line.windowing_direction,
        brilliance_dir: line.brilliance_direction,
        extinction_dir: line.extinction_direction,
        yield_dir: line.yield_direction,
        tone: line.tone.as_str().into(),
        tone_dir: line.tone_direction,
        tone_swatch: line
            .tone_srgb
            .map_or(slint::Color::from_argb_u8(0, 0, 0, 0), srgb_color),
        has_tone: line.tone_srgb.is_some(),
    }
}

/// An sRGB triple as the opaque Slint colour the swatches draw.
pub(super) const fn srgb_color([red, green, blue]: [u8; 3]) -> slint::Color {
    slint::Color::from_rgb_u8(red, green, blue)
}

/// The text a tier's angle shows in the ranges table: its magnitude with two decimals and a
/// degree sign. The side of the girdle is the tier's, not a sign.
pub(super) fn angle_text(angle_deg: f64) -> String {
    format!("{:.2}\u{b0}", angle_deg.abs())
}

/// The default range of `row` as the two fields show it: magnitudes, flattest first (a
/// pavilion tier stored at -41 shows `36` and `46`). What is typed back may carry a sign or
/// not: [`indicatrix_editor::optimize_view::parse_range`] reads the magnitude.
fn default_range_texts(row: &RangeRow) -> (String, String) {
    let (first, second) = (row.min_deg.abs(), row.max_deg.abs());
    (
        format_range_value(first.min(second)),
        format_range_value(first.max(second)),
    )
}

/// One ranges-table row. A range the cutter typed (`typed`) is shown as typed; otherwise
/// the tier's default range. `name` is what the tier table calls the tier: its own name, or
/// its standard code when the name is empty or old-style
/// ([`indicatrix_editor::retarget::plan::tier_display_names`]).
pub(super) fn range_row(
    row: &RangeRow,
    typed: Option<&RangeInput>,
    name: &str,
) -> OptimizeRangeRow {
    let (min_text, max_text) = typed.map_or_else(
        || default_range_texts(row),
        |input| (input.min_text.clone(), input.max_text.clone()),
    );
    OptimizeRangeRow {
        tier_index: i32::try_from(row.tier).unwrap_or(i32::MAX),
        tier_number: format!("#{}", row.tier + 1).into(),
        name: name.into(),
        angle: angle_text(row.angle_deg).into(),
        min_text: min_text.into(),
        max_text: max_text.into(),
        driven: row.driven,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(driven: bool) -> RangeRow {
        RangeRow {
            tier: 4,
            name: "P2".to_string(),
            angle_deg: -41.0,
            driven,
            min_deg: -46.0,
            max_deg: -36.0,
        }
    }

    #[test]
    fn a_candidate_line_keeps_every_cell_and_direction() {
        let line = CandidateLine {
            rank: "2".to_string(),
            score: "9.50".to_string(),
            windowing: "9.3%".to_string(),
            brilliance: "65.0%".to_string(),
            extinction: "11.0%".to_string(),
            yield_loss: "25.0%".to_string(),
            changed: "3".to_string(),
            score_direction: 1,
            windowing_direction: 1,
            brilliance_direction: 0,
            extinction_direction: -1,
            yield_direction: 1,
            tone: "L* 62.3".to_string(),
            tone_direction: 1,
            tone_srgb: Some([10, 20, 30]),
        };
        let row = candidate_row(&line);
        assert_eq!(row.tone.as_str(), "L* 62.3");
        assert_eq!(row.tone_dir, 1);
        assert!(row.has_tone);
        assert_eq!(row.tone_swatch, slint::Color::from_rgb_u8(10, 20, 30));
        let plain = candidate_row(&CandidateLine::default());
        assert!(!plain.has_tone);
        assert_eq!(row.rank.as_str(), "2");
        assert_eq!(row.score.as_str(), "9.50");
        assert_eq!(row.windowing.as_str(), "9.3%");
        assert_eq!(row.brilliance.as_str(), "65.0%");
        assert_eq!(row.extinction.as_str(), "11.0%");
        assert_eq!(row.yield_loss.as_str(), "25.0%");
        assert_eq!(row.changed.as_str(), "3");
        assert_eq!(
            (
                row.score_dir,
                row.windowing_dir,
                row.brilliance_dir,
                row.extinction_dir,
                row.yield_dir
            ),
            (1, 1, 0, -1, 1)
        );
    }

    #[test]
    fn a_range_row_shows_the_default_range_in_plain_numbers() {
        let shown = range_row(&row(false), None, "P2");
        assert_eq!(shown.tier_index, 4);
        assert_eq!(shown.tier_number.as_str(), "#5");
        assert_eq!(shown.name.as_str(), "P2");
        // The tier is stored at -41 with a range of -46 to -36; a person reads 41 and 36 to 46.
        assert_eq!(shown.angle.as_str(), "41.00\u{b0}");
        assert_eq!(shown.min_text.as_str(), "36");
        assert_eq!(shown.max_text.as_str(), "46");
        assert!(!shown.driven);
    }

    #[test]
    fn a_crown_range_row_reads_the_same_way() {
        let crown = RangeRow {
            tier: 1,
            name: "C1".to_string(),
            angle_deg: 34.5,
            driven: false,
            min_deg: 29.5,
            max_deg: 39.5,
        };
        let shown = range_row(&crown, None, "C1");
        assert_eq!(shown.angle.as_str(), "34.50\u{b0}");
        assert_eq!(shown.min_text.as_str(), "29.5");
        assert_eq!(shown.max_text.as_str(), "39.5");
    }

    #[test]
    fn the_range_fields_read_back_whether_or_not_they_carry_a_sign() {
        // The default texts are positive; typing the old signed form still means the same
        // range, so nothing a cutter had written down breaks.
        let shown = range_row(&row(false), None, "P2");
        let positive = indicatrix_editor::optimize_view::parse_range(
            -41.0,
            shown.min_text.as_str(),
            shown.max_text.as_str(),
        );
        let signed = indicatrix_editor::optimize_view::parse_range(-41.0, "-46", "-36");
        assert_eq!(positive, Ok((-46.0, -36.0)));
        assert_eq!(positive, signed);
    }

    #[test]
    fn a_range_row_shows_what_the_cutter_typed() {
        let typed = RangeInput {
            tier: 4,
            min_text: "40 + 2".to_string(),
            max_text: "44".to_string(),
        };
        let shown = range_row(&row(false), Some(&typed), "P2");
        assert_eq!(shown.min_text.as_str(), "40 + 2");
        assert_eq!(shown.max_text.as_str(), "44");
    }

    #[test]
    fn a_driven_row_is_flagged() {
        assert!(range_row(&row(true), None, "P2").driven);
    }

    #[test]
    fn a_range_row_shows_the_name_it_is_given() {
        // The tier is stored under an old-style name; the caller passes the code.
        let mut old_style = row(false);
        old_style.name = "3".to_string();
        assert_eq!(range_row(&old_style, None, "P2").name.as_str(), "P2");
    }
}
