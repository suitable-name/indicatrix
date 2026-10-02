//! The descriptive `[meta]` table of a `.indicatrix` design file as the browser app
//! handles it: kept from the opened file, stamped on save, written back unchanged.
//!
//! The file codec reads no clock and generates no randomness, so the page supplies both:
//! [`iso8601_utc_from_epoch_ms`] turns `Date.now()` into the stamp the file stores and
//! [`uuid_v4_from_bytes`] turns 16 random bytes into a design id. [`stamped_for_save`]
//! applies them to the metadata a design was opened with, leaving every other field
//! (including keys this build does not know) exactly as read.

use indicatrix_formats::native::design::DesignMetadata;
use std::fmt::Write;

/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 86_400;

/// The ISO-8601 UTC text (`2026-10-02T09:30:00Z`) for a time given as milliseconds since
/// the Unix epoch (what `Date.now()` returns). A non-finite value reads as the epoch.
#[must_use]
pub fn iso8601_utc_from_epoch_ms(epoch_ms: f64) -> String {
    let seconds = if epoch_ms.is_finite() {
        // Saturating float-to-int conversion; dates outside the representable range are
        // not a case a browser clock produces.
        (epoch_ms / 1000.0).floor() as i64
    } else {
        0
    };
    let days = seconds.div_euclid(SECONDS_PER_DAY);
    let of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60
    )
}

/// Year, month (1-12) and day (1-31) of the proleptic Gregorian date `days` after
/// 1970-01-01.
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// A version-4 UUID string (`8-4-4-4-12` lowercase hexadecimal) from 16 random bytes: the
/// version and variant bits are set, every other bit is taken from `bytes`.
#[must_use]
pub fn uuid_v4_from_bytes(mut bytes: [u8; 16]) -> String {
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let mut text = String::with_capacity(36);
    for (i, byte) in bytes.iter().enumerate() {
        if matches!(i, 4 | 6 | 8 | 10) {
            text.push('-');
        }
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The metadata to write for a design opened with `opened`.
///
/// It is the same table with `modified_at` set to `now`, `created_at` set to `now` when the
/// file had none, an `id` from `fresh_id_bytes` when the file had none, and the tags sorted.
/// Everything else, including unknown keys, is carried over unchanged. `fresh_id_bytes`
/// runs only when an id is needed.
#[must_use]
pub fn stamped_for_save(
    opened: &DesignMetadata,
    now: &str,
    fresh_id_bytes: impl FnOnce() -> [u8; 16],
) -> DesignMetadata {
    let mut meta = opened.clone();
    if meta.id.is_empty() {
        meta.id = uuid_v4_from_bytes(fresh_id_bytes());
    }
    if meta.created_at.is_empty() {
        meta.created_at = now.to_string();
    }
    meta.modified_at = now.to_string();
    meta.tags.sort();
    meta
}

#[cfg(test)]
mod tests;
