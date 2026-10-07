//! Text formatting shared by the commands: fixed-precision numbers, aligned tables, CSV and
//! JSON.
//!
//! Everything here is deterministic. Numbers are printed with a fixed number of decimals (never
//! the shortest round-trip form), a table's column widths come from its own cells, CSV fields
//! are quoted by one rule, and JSON objects come out with sorted keys (`serde_json`'s default
//! map), so the same design always gives the same bytes.

use serde_json::Value;

/// Decimals of an angle in degrees.
pub const ANGLE_DECIMALS: usize = 4;

/// Decimals of a percentage or a ratio.
pub const PERCENT_DECIMALS: usize = 2;

/// Decimals of a refractive index.
pub const INDEX_DECIMALS: usize = 4;

/// `value` with exactly `decimals` digits after the point. A negative zero keeps its sign
/// (the culet is stored at `-0.0` on purpose), so a facet angle meant for a person is written
/// as its magnitude, `fixed(angle.abs(), ...)`, which also turns `-0.0` into `0.0000`.
#[must_use]
pub fn fixed(value: f64, decimals: usize) -> String {
    format!("{value:.decimals$}")
}

/// `value` with `decimals` digits, or `n/a` when it is absent.
#[must_use]
pub fn fixed_or_na(value: Option<f64>, decimals: usize) -> String {
    value.map_or_else(|| "n/a".to_string(), |v| fixed(v, decimals))
}

/// `value` rounded to `decimals` places, as the nearest `f64` to that decimal. Printed by
/// `serde_json` it shows at most `decimals` digits.
#[must_use]
pub fn round_to(value: f64, decimals: i32) -> f64 {
    let scale = 10_f64.powi(decimals);
    (value * scale).round() / scale
}

/// A JSON number rounded to `decimals` places; `null` for a value that is not finite.
#[must_use]
pub fn json_number(value: f64, decimals: i32) -> Value {
    if value.is_finite() {
        Value::from(round_to(value, decimals))
    } else {
        Value::Null
    }
}

/// [`json_number`] for a single-precision measurement.
#[must_use]
pub fn json_number32(value: f32, decimals: i32) -> Value {
    json_number(f64::from(value), decimals)
}

/// [`json_number`], or `null` for an absent value.
#[must_use]
pub fn json_optional(value: Option<f64>, decimals: i32) -> Value {
    value.map_or(Value::Null, |v| json_number(v, decimals))
}

/// The JSON text of `value`, indented, ending in a newline.
#[must_use]
pub fn json_text(value: &Value) -> String {
    let mut text =
        serde_json::to_string_pretty(value).expect("a serde_json::Value always serializes to text");
    text.push('\n');
    text
}

/// One CSV field: quoted, with doubled quotes, when it holds a comma, a quote or a line break.
#[must_use]
pub fn csv_field(text: &str) -> String {
    if text.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", text.replace('"', "\"\""))
    } else {
        text.to_string()
    }
}

/// One CSV line (fields joined by commas, ending in a line feed).
#[must_use]
pub fn csv_line<S: AsRef<str>>(fields: &[S]) -> String {
    let mut line = fields
        .iter()
        .map(|field| csv_field(field.as_ref()))
        .collect::<Vec<_>>()
        .join(",");
    line.push('\n');
    line
}

/// How a table column is aligned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    /// Text starts at the column's left edge.
    Left,
    /// Text ends at the column's right edge (numbers).
    Right,
}

/// Rows laid out in columns of the width of their widest cell, two spaces apart, with a rule
/// under the header. Lines carry no trailing spaces.
#[must_use]
pub fn table(headers: &[&str], aligns: &[Align], rows: &[Vec<String>]) -> String {
    let columns = headers.len();
    let mut widths: Vec<usize> = headers.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (width, cell) in widths.iter_mut().zip(row) {
            *width = (*width).max(cell.chars().count());
        }
    }
    let line = |cells: Vec<&str>| -> String {
        let mut text = String::new();
        for (column, cell) in cells.iter().enumerate().take(columns) {
            if column > 0 {
                text.push_str("  ");
            }
            let pad = widths[column].saturating_sub(cell.chars().count());
            match aligns.get(column).copied().unwrap_or(Align::Left) {
                Align::Left => {
                    text.push_str(cell);
                    text.push_str(&" ".repeat(pad));
                }
                Align::Right => {
                    text.push_str(&" ".repeat(pad));
                    text.push_str(cell);
                }
            }
        }
        let mut trimmed = text.trim_end().to_string();
        trimmed.push('\n');
        trimmed
    };
    let mut out = line(headers.to_vec());
    let rule: Vec<String> = widths.iter().map(|w| "-".repeat(*w)).collect();
    out.push_str(&line(rule.iter().map(String::as_str).collect()));
    for row in rows {
        out.push_str(&line(row.iter().map(String::as_str).collect()));
    }
    out
}

/// `1 tier` / `3 tiers`.
#[must_use]
pub fn count_noun(count: usize, one: &str, many: &str) -> String {
    format!("{count} {}", if count == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_pads_and_keeps_a_negative_zero() {
        assert_eq!(fixed(41.1, 4), "41.1000");
        assert_eq!(fixed(-0.0, 2), "-0.00");
        assert_eq!(fixed_or_na(None, 2), "n/a");
        assert_eq!(fixed_or_na(Some(1.0), 2), "1.00");
    }

    #[test]
    fn json_numbers_are_rounded_and_never_nan() {
        let text = json_text(&Value::Array(vec![
            json_number(41.123_456_789, 3),
            json_number(f64::NAN, 3),
            json_optional(None, 3),
        ]));
        assert_eq!(text, "[\n  41.123,\n  null,\n  null\n]\n");
    }

    #[test]
    fn json_objects_have_sorted_keys() {
        let value = serde_json::json!({ "zebra": 1, "apple": 2, "mango": 3 });
        assert_eq!(
            json_text(&value),
            "{\n  \"apple\": 2,\n  \"mango\": 3,\n  \"zebra\": 1\n}\n"
        );
    }

    #[test]
    fn csv_quotes_only_when_it_must() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_field("two\nlines"), "\"two\nlines\"");
        assert_eq!(csv_line(&["a", "b,c", "d"]), "a,\"b,c\",d\n");
    }

    #[test]
    fn a_table_aligns_columns_and_trims_line_ends() {
        let text = table(
            &["name", "angle"],
            &[Align::Left, Align::Right],
            &[
                vec!["P1".to_string(), "41.1000".to_string()],
                vec!["Girdle".to_string(), "90.0000".to_string()],
            ],
        );
        let expected = "name      angle\n------  -------\nP1      41.1000\nGirdle  90.0000\n";
        assert_eq!(text, expected);
        assert!(text.lines().all(|line| line == line.trim_end()));
    }

    #[test]
    fn counts_use_the_right_noun() {
        assert_eq!(count_noun(1, "tier", "tiers"), "1 tier");
        assert_eq!(count_noun(0, "tier", "tiers"), "0 tiers");
        assert_eq!(count_noun(2, "tier", "tiers"), "2 tiers");
    }
}
