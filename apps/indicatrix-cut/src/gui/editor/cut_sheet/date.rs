//! The export date a printed cutting sheet carries: the month and year, in English, read from
//! the system clock here in the app layer. The shared sheet builders are pure and take the text
//! as given.

use crate::bridge::export_thread::filename_template::civil_from_unix_seconds;
use indicatrix_editor::cut_sheet::month_year_text;
use std::time::{SystemTime, UNIX_EPOCH};

/// `"October 2026"` for the instant `unix_seconds` seconds after the epoch (UTC).
pub fn date_text_at(unix_seconds: i64) -> String {
    let (year, month, ..) = civil_from_unix_seconds(unix_seconds);
    month_year_text(year, month)
}

/// The current month and year as the sheet prints them; empty when the clock is before the epoch.
pub fn current_date_text() -> String {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or_else(
        |_| String::new(),
        |elapsed| date_text_at(i64::try_from(elapsed.as_secs()).unwrap_or(0)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The instants the filename template's own tests pin: the epoch, a leap day and a day in
    /// March 2024.
    #[test]
    fn the_date_is_the_english_month_and_year() {
        assert_eq!(date_text_at(0), "January 1970");
        assert_eq!(date_text_at(1_709_208_000), "February 2024");
        assert_eq!(date_text_at(1_709_618_828), "March 2024");
    }

    #[test]
    fn the_current_date_names_a_month() {
        let text = current_date_text();
        let (month, year) = text.split_once(' ').expect("month and year");
        assert!(year.parse::<i64>().is_ok_and(|y| y >= 2024), "{text}");
        assert!(
            [
                "January",
                "February",
                "March",
                "April",
                "May",
                "June",
                "July",
                "August",
                "September",
                "October",
                "November",
                "December"
            ]
            .contains(&month),
            "{text}"
        );
    }
}
