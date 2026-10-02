//! Names and dates of saved plans.

use super::dto::MAX_NAME_CHARS;
use crate::bridge::export_thread::filename_template::civil_from_unix_seconds;
use indicatrix_cut_core::rough_plan::{RoughBase, RoughModel};
use std::time::{SystemTime, UNIX_EPOCH};

/// The default file name suffix of an exported plan.
pub const EXPORT_SUFFIX: &str = ".indicatrix-rough.toml";

/// A Unix timestamp (seconds, UTC) as "YYYY-MM-DD".
#[must_use]
pub fn format_unix_date(unix_secs: i64) -> String {
    let (year, month, day, ..) = civil_from_unix_seconds(unix_secs);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The current time as Unix seconds (0 if the clock is before the epoch).
#[must_use]
pub fn current_unix_time() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| i64::try_from(elapsed.as_secs()).unwrap_or(0))
}

/// The suggested name of a plan saved at `unix_secs`: "Pebble 18x11x10 mm Aquamarine
/// 2026-09-30", or "Cylinder Ø12x30 mm Quartz 2026-09-30". Sizes are rounded to whole
/// millimetres.
#[must_use]
pub fn default_plan_name(model: &RoughModel, material_name: &str, unix_secs: i64) -> String {
    let date = format_unix_date(unix_secs);
    match model.base {
        RoughBase::Block { x_mm, y_mm, z_mm } => {
            format!("Block {x_mm:.0}x{y_mm:.0}x{z_mm:.0} mm {material_name} {date}")
        }
        RoughBase::Pebble { x_mm, y_mm, z_mm } => {
            format!("Pebble {x_mm:.0}x{y_mm:.0}x{z_mm:.0} mm {material_name} {date}")
        }
        RoughBase::Cylinder {
            diameter_mm,
            length_mm,
            ..
        } => format!("Cylinder \u{d8}{diameter_mm:.0}x{length_mm:.0} mm {material_name} {date}"),
        RoughBase::Hull {
            x_mm, y_mm, z_mm, ..
        } => format!("Mesh {x_mm:.0}x{y_mm:.0}x{z_mm:.0} mm {material_name} {date}"),
    }
}

/// Longest file name stem of an exported plan, in characters (the suffix comes on top).
const MAX_STEM_CHARS: usize = 120;

/// The device names Windows reserves: a file called one of these, with any extension, is
/// not a file.
fn is_reserved_stem(stem: &str) -> bool {
    let device = stem
        .split('.')
        .next()
        .unwrap_or_default()
        .trim_end()
        .to_ascii_uppercase();
    matches!(device.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            device
                .strip_prefix(prefix)
                .is_some_and(|digits| matches!(digits.as_bytes(), [b'1'..=b'9']))
        })
}

/// A plan name as it is stored: without outer spaces and at most
/// [`MAX_NAME_CHARS`] characters.
#[must_use]
pub fn clean_plan_name(name: &str) -> String {
    name.trim().chars().take(MAX_NAME_CHARS).collect()
}

/// A file name for exporting the plan called `name`: characters a file system rejects
/// become underscores, a stem Windows reserves for a device (`CON`, `NUL`, `COM1`, ...)
/// gets an underscore in front, the stem is cut to 120 characters, and the export suffix is
/// appended.
#[must_use]
pub fn export_file_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_control() || "\\/:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.');
    let capped: String = trimmed.chars().take(MAX_STEM_CHARS).collect();
    let stem = capped.trim_end().trim_end_matches('.');
    if stem.is_empty() {
        format!("rough-plan{EXPORT_SUFFIX}")
    } else if is_reserved_stem(stem) {
        format!("_{stem}{EXPORT_SUFFIX}")
    } else {
        format!("{stem}{EXPORT_SUFFIX}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::Axis;

    #[test]
    fn the_epoch_is_1970_01_01() {
        assert_eq!(format_unix_date(0), "1970-01-01");
    }

    #[test]
    fn a_leap_day_is_formatted_as_such() {
        // 2000 is a leap year (divisible by 400).
        assert_eq!(format_unix_date(951_782_400), "2000-02-29");
        assert_eq!(format_unix_date(951_782_400 + 86_399), "2000-02-29");
        assert_eq!(format_unix_date(951_782_400 + 86_400), "2000-03-01");
    }

    #[test]
    fn a_recent_date_and_the_day_boundaries_are_exact() {
        assert_eq!(format_unix_date(1_790_726_400), "2026-09-30");
        assert_eq!(format_unix_date(1_790_726_400 + 43_200), "2026-09-30");
        assert_eq!(format_unix_date(1_790_726_400 + 86_400), "2026-10-01");
        assert_eq!(format_unix_date(1_790_726_400 - 1), "2026-09-29");
    }

    #[test]
    fn days_on_a_year_boundary_do_not_spill_into_the_next_year() {
        // 1461 days after the epoch is 1974-01-01; the days around it sit on the
        // year-of-era rounding edge of the calendar arithmetic.
        assert_eq!(format_unix_date(1_460 * 86_400), "1973-12-31");
        assert_eq!(format_unix_date(1_461 * 86_400), "1974-01-01");
        assert_eq!(format_unix_date(11_016 * 86_400 - 1), "2000-02-28");
    }

    #[test]
    fn the_default_name_follows_the_base_and_rounds_to_whole_millimetres() {
        let today = 1_790_726_400;
        let pebble = RoughModel::new(
            RoughBase::Pebble {
                x_mm: 18.4,
                y_mm: 11.0,
                z_mm: 9.6,
            },
            Vec::new(),
        );
        assert_eq!(
            default_plan_name(&pebble, "Aquamarine", today),
            "Pebble 18x11x10 mm Aquamarine 2026-09-30"
        );
        let block = RoughModel::new(
            RoughBase::Block {
                x_mm: 20.0,
                y_mm: 12.4,
                z_mm: 8.0,
            },
            Vec::new(),
        );
        assert_eq!(
            default_plan_name(&block, "Quartz", today),
            "Block 20x12x8 mm Quartz 2026-09-30"
        );
        let cylinder = RoughModel::new(
            RoughBase::Cylinder {
                diameter_mm: 12.2,
                length_mm: 30.0,
                axis: Axis::Y,
            },
            Vec::new(),
        );
        assert_eq!(
            default_plan_name(&cylinder, "Quartz", today),
            "Cylinder \u{d8}12x30 mm Quartz 2026-09-30"
        );
    }

    #[test]
    fn export_names_lose_characters_a_file_system_rejects() {
        assert_eq!(
            export_file_name("Pebble 18x11x10 mm Aquamarine 2026-09-30"),
            "Pebble 18x11x10 mm Aquamarine 2026-09-30.indicatrix-rough.toml"
        );
        assert_eq!(
            export_file_name("a/b:c*d?\"e<f>g|h\\i"),
            "a_b_c_d__e_f_g_h_i.indicatrix-rough.toml"
        );
        assert_eq!(
            export_file_name("  ..  "),
            "rough-plan.indicatrix-rough.toml"
        );
    }

    #[test]
    fn windows_device_names_get_an_underscore_in_front() {
        assert_eq!(export_file_name("CON"), "_CON.indicatrix-rough.toml");
        assert_eq!(export_file_name("nul"), "_nul.indicatrix-rough.toml");
        assert_eq!(export_file_name("Com1"), "_Com1.indicatrix-rough.toml");
        assert_eq!(export_file_name("LPT9"), "_LPT9.indicatrix-rough.toml");
        // Windows reserves the device name whatever follows the first dot.
        assert_eq!(
            export_file_name("aux.backup"),
            "_aux.backup.indicatrix-rough.toml"
        );
        // Not devices: COM0, COM10, a longer word, a name that only contains one.
        for plain in ["COM0", "COM10", "CONSOLE", "my CON", "LPT"] {
            assert_eq!(
                export_file_name(plain),
                format!("{plain}.indicatrix-rough.toml")
            );
        }
    }

    #[test]
    fn a_long_name_is_cut_to_120_characters_before_the_suffix() {
        let long = "x".repeat(300);
        assert_eq!(
            export_file_name(&long),
            format!("{}.indicatrix-rough.toml", "x".repeat(120))
        );
        // The cut never leaves a trailing space or dot (Windows drops them).
        let spaced = format!("{} tail", "y".repeat(119));
        assert_eq!(
            export_file_name(&spaced),
            format!("{}.indicatrix-rough.toml", "y".repeat(119))
        );
    }

    #[test]
    fn a_stored_name_is_trimmed_and_capped_at_200_characters() {
        assert_eq!(clean_plan_name("  Aqua  "), "Aqua");
        let long = "n".repeat(250);
        assert_eq!(clean_plan_name(&long).chars().count(), MAX_NAME_CHARS);
    }
}
