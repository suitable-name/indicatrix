//! The console: a ring of log lines, a severity filter and the clipboard copy.

use std::{collections::VecDeque, rc::Rc, time::SystemTime};

use slint::{Model, ModelRc, VecModel};

use crate::LogEntry;

/// Console lines kept.
pub const MAX_LOG_ENTRIES: usize = 1000;

/// Severity numbers of the Slint contract.
pub const INFO: i32 = 0;
pub const SUCCESS: i32 = 1;
pub const WARNING: i32 = 2;
pub const ERROR: i32 = 3;
pub const VALIDATION: i32 = 4;

/// One console line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogRecord {
    pub timestamp: String,
    pub level: String,
    pub message: String,
    pub severity: i32,
}

impl LogRecord {
    fn to_entry(&self) -> LogEntry {
        LogEntry {
            timestamp: self.timestamp.as_str().into(),
            level: self.level.as_str().into(),
            message: self.message.as_str().into(),
            severity: self.severity,
        }
    }
}

/// The severity of a log level tag (case-insensitive).
pub fn severity_of(level: &str) -> i32 {
    match level.to_ascii_uppercase().as_str() {
        "BEST" | "CHECKPOINT" | "EXPORT" | "IMPORT" => SUCCESS,
        "WARN" | "WARNING" => WARNING,
        "ERROR" => ERROR,
        "VAL" => VALIDATION,
        _ => INFO,
    }
}

/// Whether a line of `severity` is shown by `filter` (0 everything, 1 warnings and errors,
/// 2 errors).
pub const fn passes_filter(severity: i32, filter: i32) -> bool {
    match filter {
        1 => severity == WARNING || severity == ERROR,
        2 => severity == ERROR,
        _ => true,
    }
}

/// `HH:MM:SS` of a Unix time in seconds (UTC).
pub fn format_time_of_day(secs: u64) -> String {
    format!(
        "{:02}:{:02}:{:02}",
        (secs / 3600) % 24,
        (secs / 60) % 60,
        secs % 60
    )
}

/// The current UTC time of day.
pub fn utc_time_of_day() -> String {
    let secs = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_time_of_day(secs)
}

/// The lines as plain text for the clipboard, one per line.
pub fn clipboard_text<'a>(records: impl Iterator<Item = &'a LogRecord>) -> String {
    records
        .map(|r| format!("{} [{}] {}", r.timestamp, r.level, r.message))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Puts `text` on the system clipboard.
///
/// # Errors
///
/// The clipboard's own message when it is unavailable.
pub fn copy_to_clipboard(text: &str) -> Result<(), String> {
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text.to_owned()))
        .map_err(|error| error.to_string())
}

/// The console ring plus the Slint model of the lines the filter lets through.
pub struct LogStore {
    records: VecDeque<LogRecord>,
    filter: i32,
    model: Rc<VecModel<LogEntry>>,
    warnings: usize,
    errors: usize,
}

impl LogStore {
    pub fn new(filter: i32) -> Self {
        Self {
            records: VecDeque::new(),
            filter: filter.clamp(0, 2),
            model: Rc::new(VecModel::default()),
            warnings: 0,
            errors: 0,
        }
    }

    pub fn model(&self) -> ModelRc<LogEntry> {
        ModelRc::new(self.model.clone())
    }

    pub const fn filter(&self) -> i32 {
        self.filter
    }

    /// Warnings and errors in the ring (not only the visible ones).
    pub const fn counts(&self) -> (usize, usize) {
        (self.warnings, self.errors)
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.records.len()
    }

    const fn adjust_counts(&mut self, severity: i32, added: bool) {
        let counter = match severity {
            WARNING => &mut self.warnings,
            ERROR => &mut self.errors,
            _ => return,
        };
        *counter = if added {
            *counter + 1
        } else {
            counter.saturating_sub(1)
        };
    }

    /// Appends a line and trims the ring; returns the line's severity.
    pub fn push(&mut self, timestamp: &str, level: &str, message: String) -> i32 {
        let level = level.to_ascii_uppercase();
        let severity = severity_of(&level);
        let record = LogRecord {
            timestamp: timestamp.to_owned(),
            level,
            message,
            severity,
        };
        if passes_filter(severity, self.filter) {
            self.model.push(record.to_entry());
        }
        self.adjust_counts(severity, true);
        self.records.push_back(record);
        while self.records.len() > MAX_LOG_ENTRIES {
            let Some(old) = self.records.pop_front() else {
                break;
            };
            self.adjust_counts(old.severity, false);
            if passes_filter(old.severity, self.filter) && self.model.row_count() > 0 {
                self.model.remove(0);
            }
        }
        severity
    }

    /// Changes the filter and rebuilds the visible lines.
    pub fn set_filter(&mut self, filter: i32) {
        self.filter = filter.clamp(0, 2);
        let visible: Vec<LogEntry> = self.visible().map(LogRecord::to_entry).collect();
        self.model.set_vec(visible);
    }

    /// Empties the console.
    pub fn clear(&mut self) {
        self.records.clear();
        self.model.set_vec(Vec::new());
        self.warnings = 0;
        self.errors = 0;
    }

    /// The lines the filter lets through, oldest first.
    pub fn visible(&self) -> impl Iterator<Item = &LogRecord> {
        let filter = self.filter;
        self.records
            .iter()
            .filter(move |r| passes_filter(r.severity, filter))
    }

    /// The visible lines as clipboard text.
    pub fn visible_text(&self) -> String {
        clipboard_text(self.visible())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_map_to_the_contract_severities() {
        assert_eq!(severity_of("INFO"), INFO);
        assert_eq!(severity_of("best"), SUCCESS);
        assert_eq!(severity_of("CHECKPOINT"), SUCCESS);
        assert_eq!(severity_of("EXPORT"), SUCCESS);
        assert_eq!(severity_of("IMPORT"), SUCCESS);
        assert_eq!(severity_of("WARN"), WARNING);
        assert_eq!(severity_of("ERROR"), ERROR);
        assert_eq!(severity_of("VAL"), VALIDATION);
        assert_eq!(severity_of("DEVICE"), INFO);
    }

    #[test]
    fn filters_pick_the_right_severities() {
        for severity in [INFO, SUCCESS, WARNING, ERROR, VALIDATION] {
            assert!(passes_filter(severity, 0));
        }
        assert!(passes_filter(WARNING, 1) && passes_filter(ERROR, 1));
        assert!(!passes_filter(INFO, 1) && !passes_filter(VALIDATION, 1));
        assert!(passes_filter(ERROR, 2) && !passes_filter(WARNING, 2));
    }

    #[test]
    fn time_of_day_wraps_at_midnight() {
        assert_eq!(format_time_of_day(0), "00:00:00");
        assert_eq!(format_time_of_day(3_661), "01:01:01");
        assert_eq!(format_time_of_day(86_400 + 59), "00:00:59");
    }

    #[test]
    fn clipboard_text_has_one_line_per_record() {
        let records = [
            LogRecord {
                timestamp: "10:00:00".into(),
                level: "INFO".into(),
                message: "a".into(),
                severity: INFO,
            },
            LogRecord {
                timestamp: "10:00:01".into(),
                level: "ERROR".into(),
                message: "b".into(),
                severity: ERROR,
            },
        ];
        assert_eq!(
            clipboard_text(records.iter()),
            "10:00:00 [INFO] a\n10:00:01 [ERROR] b"
        );
        assert_eq!(clipboard_text(std::iter::empty()), "");
    }

    #[test]
    fn the_ring_is_trimmed_and_counts_follow() {
        let mut store = LogStore::new(0);
        store.push("t", "ERROR", "first".into());
        for i in 0..MAX_LOG_ENTRIES {
            store.push("t", "WARN", format!("line {i}"));
        }
        assert_eq!(store.len(), MAX_LOG_ENTRIES);
        // The first line (the only error) fell out of the ring.
        assert_eq!(store.counts(), (MAX_LOG_ENTRIES, 0));
        assert_eq!(store.model.row_count(), MAX_LOG_ENTRIES);
    }

    #[test]
    fn the_filter_rebuilds_the_visible_lines() {
        let mut store = LogStore::new(0);
        store.push("t", "INFO", "i".into());
        store.push("t", "WARN", "w".into());
        store.push("t", "ERROR", "e".into());
        assert_eq!(store.model.row_count(), 3);
        store.set_filter(1);
        assert_eq!(store.model.row_count(), 2);
        store.push("t", "INFO", "ignored".into());
        assert_eq!(store.model.row_count(), 2);
        store.set_filter(2);
        assert_eq!(store.model.row_count(), 1);
        assert_eq!(store.visible_text(), "t [ERROR] e");
        store.clear();
        assert_eq!(store.model.row_count(), 0);
        assert_eq!(store.counts(), (0, 0));
    }

    #[test]
    fn filter_values_are_clamped() {
        assert_eq!(LogStore::new(9).filter(), 2);
        assert_eq!(LogStore::new(-3).filter(), 0);
    }
}
