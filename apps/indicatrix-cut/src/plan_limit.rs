//! The Rough planner's "Scan plan time limit".
//!
//! A plan of a mesh rough (a scan) can run for minutes. The limit is a deadline the plan's
//! progress callback checks, in seconds, `0` for none. When it is reached the plan stops
//! and shows the layouts found so far. This module holds the window-free parts: the
//! deadline, the text of the field, the words of the notes and the stored value, so their
//! tests run outside `gui::rough_plan`.

use crate::settings::SettingsPersister;
use std::time::{Duration, Instant};

/// The limit of a fresh settings file, in seconds.
pub const DEFAULT_LIMIT_SECS: u32 = 120;

/// The largest limit the field takes, in seconds (one day). Anything above is a typing slip.
pub const MAX_LIMIT_SECS: u32 = 86_400;

/// Shown when the field does not hold whole seconds.
pub const BAD_LIMIT_MESSAGE: &str =
    "Scan plan time limit must be a whole number of seconds (0 = no limit).";

/// A running plan's deadline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlanDeadline {
    start: Instant,
    limit: Duration,
}

impl PlanDeadline {
    /// The deadline `limit_secs` after `start`; `None` for `0` (no limit). A limit above
    /// [`MAX_LIMIT_SECS`] counts as that.
    #[must_use]
    pub fn new(start: Instant, limit_secs: u32) -> Option<Self> {
        (limit_secs > 0).then(|| Self {
            start,
            limit: Duration::from_secs(u64::from(limit_secs.min(MAX_LIMIT_SECS))),
        })
    }

    /// Whether `now` is at or past the deadline.
    #[must_use]
    pub fn expired(&self, now: Instant) -> bool {
        now.saturating_duration_since(self.start) >= self.limit
    }

    /// The time left at `now`, zero once expired.
    #[must_use]
    pub fn remaining(&self, now: Instant) -> Duration {
        self.limit
            .saturating_sub(now.saturating_duration_since(self.start))
    }
}

/// Parses the field: whole seconds, `0` for no limit. Surrounding blanks are ignored.
///
/// # Errors
///
/// Returns the message for the window's `error_text` for text that is not a whole number
/// of seconds, or one above [`MAX_LIMIT_SECS`].
pub fn parse_limit(text: &str) -> Result<u32, String> {
    let secs: u32 = text
        .trim()
        .parse()
        .map_err(|_| BAD_LIMIT_MESSAGE.to_string())?;
    if secs > MAX_LIMIT_SECS {
        return Err(format!(
            "Scan plan time limit must be at most {MAX_LIMIT_SECS} s (0 = no limit)."
        ));
    }
    Ok(secs)
}

/// The time left as shown next to the progress: `45 s` under a minute, else `m:ss`. Rounded
/// up, so it never reads `0 s` while the plan still runs.
#[must_use]
pub fn remaining_text(remaining: Duration) -> String {
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{}:{:02}", secs / 60, secs % 60)
    }
}

/// The note on a plan that was stopped by the limit but has layouts.
#[must_use]
pub fn stop_note(limit_secs: u64) -> String {
    format!(
        "Stopped at the time limit ({limit_secs} s): partial search; a rerun may differ. \
         Raise the Scan plan time limit under Plan for a full plan."
    )
}

/// The message for a plan that was stopped by the limit before any layout existed.
#[must_use]
pub fn nothing_found_message(limit_secs: u64) -> String {
    format!(
        "The time limit ({limit_secs} s) was reached before the plan had found any layout. \
         Raise the Scan plan time limit under Plan (0 = no limit) and plan again."
    )
}

/// The stored limit in seconds; the default when there is no settings persister.
#[must_use]
pub fn stored_limit_secs() -> u32 {
    SettingsPersister::installed_for_this_thread().map_or(DEFAULT_LIMIT_SECS, |persister| {
        persister.snapshot().settings.plan_time_limit_secs
    })
}

/// Stores `secs` as the limit through the settings persister. Returns whether there was
/// one to write through.
pub fn store_limit_secs(secs: u32) -> bool {
    SettingsPersister::installed_for_this_thread().is_some_and(|persister| {
        persister.update(|file| file.settings.plan_time_limit_secs = secs);
        true
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::SettingsFile;

    fn at(start: Instant, secs: u64) -> Instant {
        start + Duration::from_secs(secs)
    }

    #[test]
    fn zero_is_no_limit() {
        assert_eq!(PlanDeadline::new(Instant::now(), 0), None);
    }

    #[test]
    fn the_deadline_expires_exactly_at_the_limit() {
        let start = Instant::now();
        let deadline = PlanDeadline::new(start, 5).expect("a limit");
        assert!(!deadline.expired(start));
        assert!(!deadline.expired(at(start, 4)));
        assert!(deadline.expired(at(start, 5)));
        assert!(deadline.expired(at(start, 500)));
        assert_eq!(deadline.remaining(start), Duration::from_secs(5));
    }

    #[test]
    fn the_time_left_counts_down_and_stops_at_zero() {
        let start = Instant::now();
        let deadline = PlanDeadline::new(start, 120).expect("a limit");
        assert_eq!(deadline.remaining(start), Duration::from_secs(120));
        assert_eq!(deadline.remaining(at(start, 20)), Duration::from_secs(100));
        assert_eq!(deadline.remaining(at(start, 999)), Duration::ZERO);
        // A clock reading before the start is no time spent.
        assert_eq!(
            deadline.remaining(start.checked_sub(Duration::from_secs(1)).unwrap_or(start)),
            Duration::from_secs(120)
        );
    }

    #[test]
    fn a_huge_limit_counts_as_the_largest() {
        let start = Instant::now();
        let deadline = PlanDeadline::new(start, u32::MAX).expect("a limit");
        assert_eq!(
            deadline.remaining(start),
            Duration::from_secs(u64::from(MAX_LIMIT_SECS))
        );
    }

    #[test]
    fn the_field_takes_whole_seconds_and_zero() {
        assert_eq!(parse_limit("120"), Ok(120));
        assert_eq!(parse_limit("  5 "), Ok(5));
        assert_eq!(parse_limit("0"), Ok(0));
        assert_eq!(parse_limit("86400"), Ok(86_400));
        for bad in ["", " ", "abc", "-1", "2.5", "1e3", "86401", "99999999999"] {
            assert!(parse_limit(bad).is_err(), "{bad:?}");
        }
        assert_eq!(parse_limit("x"), Err(BAD_LIMIT_MESSAGE.to_string()));
        assert!(parse_limit("86401").unwrap_err().contains("86400"));
    }

    #[test]
    fn the_remaining_time_is_rounded_up_and_never_reads_zero_while_running() {
        assert_eq!(remaining_text(Duration::from_secs(45)), "45 s");
        assert_eq!(remaining_text(Duration::from_millis(44_100)), "45 s");
        assert_eq!(remaining_text(Duration::from_millis(100)), "1 s");
        assert_eq!(remaining_text(Duration::ZERO), "0 s");
        assert_eq!(remaining_text(Duration::from_secs(60)), "1:00");
        assert_eq!(remaining_text(Duration::from_secs(100)), "1:40");
        assert_eq!(remaining_text(Duration::from_secs(7_325)), "122:05");
    }

    #[test]
    fn the_notes_name_the_limit_and_the_field() {
        let note = stop_note(5);
        assert!(note.starts_with("Stopped at the time limit (5 s): partial search;"));
        assert!(note.contains("a rerun may differ"));
        assert!(note.contains("Scan plan time limit"));
        let none = nothing_found_message(5);
        assert!(none.contains("(5 s)"));
        assert!(none.contains("before the plan had found any layout"));
        assert!(none.contains("Scan plan time limit"));
    }

    #[test]
    fn an_old_settings_file_loads_the_default_limit() {
        let parsed: SettingsFile =
            toml::from_str("[settings]\nexposure = 1.1\n").expect("an old file");
        assert_eq!(parsed.settings.plan_time_limit_secs, DEFAULT_LIMIT_SECS);
        let set: SettingsFile =
            toml::from_str("[settings]\nplan_time_limit_secs = 0\n").expect("a limit of zero");
        assert_eq!(set.settings.plan_time_limit_secs, 0);
    }
}
