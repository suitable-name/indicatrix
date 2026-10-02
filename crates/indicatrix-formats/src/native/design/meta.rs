//! The `[meta]` table of a design file: descriptive facts about a design that no other
//! part of the file lets an importer recompute (who drew it, where it was published,
//! the owner's notes and tags, rights, timestamps, a stable id).
//!
//! Every text field is a plain `String` where the empty string means "not set": a
//! field that is unset is not written, so an old file and a freshly cleared field look
//! the same. Dates are ISO-8601 UTC text supplied by the caller -- the codec never reads
//! a clock and never generates an id.

/// Upper bound on one free-text field (`notes`, `license`, ...), in bytes of UTF-8.
pub const MAX_META_TEXT_BYTES: usize = 1024 * 1024;

/// Upper bound on the number of tags.
pub const MAX_META_TAGS: usize = 1_000;

/// Upper bound on one tag, in bytes of UTF-8.
pub const MAX_META_TAG_BYTES: usize = 200;

/// The `[meta]` table: per-design descriptive data. The design module's documentation
/// has the table of what is stored and what is deliberately recomputed instead.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct DesignMetadata {
    /// Stable design identity as a UUID string (`8-4-4-4-12` hexadecimal), supplied by
    /// the caller; empty when the design has none yet.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Display name of the design.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub title: String,
    /// The designer alone (e.g. `"Capps, Jerry"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub designer: String,
    /// The free-text `"Designer; Publication citation"` line as displayed. Kept next to
    /// the split halves because a hand-edited line need not equal their join.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub designer_info: String,
    /// The publication citation alone (e.g. `"Lapidary Journal, May 1994, p95"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_citation: String,
    /// The page the design was published on; empty for a locally made design (the
    /// vault's synthetic `local://` key is machine-local and never stored).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_url: String,
    /// The source catalogue's own id text for the design.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source_design_id: String,
    /// Shape label as recorded (free text, e.g. `"Pear"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shape: String,
    /// The numbered shape-category id kept as decimal text (e.g. `"5"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub shape_category: String,
    /// The competition-entry class/label of a competition design.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub competition: String,
    /// Name of the PDF the design is described in. A plain name, not a link: several
    /// designs may name one booklet, and the bytes (if kept) are an attachment.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub pdf_file: String,
    /// Name of the original `GemCad` `.gem` file; see [`Self::pdf_file`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub gem_file: String,
    /// Licence or usage terms of the design, free text (e.g. `"CC BY-NC 4.0"`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    /// Copyright / rights statement, free text.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub copyright: String,
    /// The owner's free-text notes about the design.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub notes: String,
    /// Creation time, ISO-8601 UTC (`2026-10-02T09:30:00Z`, optional fraction).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub created_at: String,
    /// Last-modification time, same format as [`Self::created_at`].
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub modified_at: String,
    /// The owner's tags, in the order kept (an unordered set is the caller's to sort).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// The design is excluded from the Rough Planner's candidate set.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub planner_excluded: bool,
    /// The owner marked the design ignored in the library.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ignored: bool,
    /// Keys a newer build wrote that this build does not claim; written back as read.
    #[serde(flatten, default)]
    pub unknown: toml::Table,
}

impl DesignMetadata {
    /// `true` when nothing would be written: every field unset and no unknown keys.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    fn text_fields(&self) -> [(&'static str, &str); 15] {
        [
            ("meta.title", &self.title),
            ("meta.designer", &self.designer),
            ("meta.designer_info", &self.designer_info),
            ("meta.source_citation", &self.source_citation),
            ("meta.source_url", &self.source_url),
            ("meta.source_design_id", &self.source_design_id),
            ("meta.shape", &self.shape),
            ("meta.shape_category", &self.shape_category),
            ("meta.competition", &self.competition),
            ("meta.pdf_file", &self.pdf_file),
            ("meta.gem_file", &self.gem_file),
            ("meta.license", &self.license),
            ("meta.copyright", &self.copyright),
            ("meta.notes", &self.notes),
            ("meta.id", &self.id),
        ]
    }

    /// Checks the sizes and formats no field type can express: each text at most
    /// [`MAX_META_TEXT_BYTES`], `id` empty or a UUID, both dates empty or
    /// [`is_iso8601_utc`], at most [`MAX_META_TAGS`] tags, each non-blank, at most
    /// [`MAX_META_TAG_BYTES`] and without control characters.
    ///
    /// # Errors
    ///
    /// `(field, reason)` naming the first offending key.
    pub fn validate(&self) -> Result<(), (&'static str, String)> {
        for (field, text) in self.text_fields() {
            if text.len() > MAX_META_TEXT_BYTES {
                return Err((
                    field,
                    format!(
                        "{} bytes; at most {MAX_META_TEXT_BYTES} are allowed",
                        text.len()
                    ),
                ));
            }
        }
        if !self.id.is_empty() && !is_uuid(&self.id) {
            return Err(("meta.id", "not a UUID (8-4-4-4-12 hexadecimal)".to_string()));
        }
        for (field, stamp) in [
            ("meta.created_at", &self.created_at),
            ("meta.modified_at", &self.modified_at),
        ] {
            if !stamp.is_empty() && !is_iso8601_utc(stamp) {
                return Err((
                    field,
                    format!("'{stamp}' is not ISO-8601 UTC (YYYY-MM-DDTHH:MM:SSZ)"),
                ));
            }
        }
        if self.tags.len() > MAX_META_TAGS {
            return Err((
                "meta.tags",
                format!(
                    "{} tags; at most {MAX_META_TAGS} are allowed",
                    self.tags.len()
                ),
            ));
        }
        for tag in &self.tags {
            if tag.trim().is_empty()
                || tag.len() > MAX_META_TAG_BYTES
                || tag.chars().any(char::is_control)
            {
                return Err((
                    "meta.tags",
                    format!("tag '{tag}' is blank, too long or holds a control character"),
                ));
            }
        }
        Ok(())
    }
}

/// `true` for `8-4-4-4-12` hexadecimal groups, either case.
#[must_use]
pub fn is_uuid(text: &str) -> bool {
    let groups: Vec<&str> = text.split('-').collect();
    groups.len() == 5
        && groups
            .iter()
            .zip([8usize, 4, 4, 4, 12])
            .all(|(g, len)| g.len() == len && g.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn digits(part: &str, len: usize) -> Option<u32> {
    if part.len() == len && part.bytes().all(|b| b.is_ascii_digit()) {
        part.parse().ok()
    } else {
        None
    }
}

/// `true` for an ISO-8601 UTC timestamp (`YYYY-MM-DDTHH:MM:SSZ`).
///
/// An optional `.` and 1 to 9 fraction digits may precede the `Z`; month 1-12, day 1-31, hour 0-23, minute 0-59 and second 0-59.
/// A shape check, not a calendar: `2026-02-31T00:00:00Z` passes.
#[must_use]
pub fn is_iso8601_utc(text: &str) -> bool {
    let Some(body) = text.strip_suffix('Z') else {
        return false;
    };
    let (main, fraction) = body
        .split_once('.')
        .map_or((body, None), |(m, f)| (m, Some(f)));
    if let Some(f) = fraction
        && (f.is_empty() || f.len() > 9 || !f.bytes().all(|b| b.is_ascii_digit()))
    {
        return false;
    }
    let Some((date, time)) = main.split_once('T') else {
        return false;
    };
    let date: Vec<&str> = date.split('-').collect();
    let time: Vec<&str> = time.split(':').collect();
    let (&[year, month, day], &[hour, minute, second]) = (date.as_slice(), time.as_slice()) else {
        return false;
    };
    digits(year, 4).is_some()
        && digits(month, 2).is_some_and(|m| (1..=12).contains(&m))
        && digits(day, 2).is_some_and(|d| (1..=31).contains(&d))
        && digits(hour, 2).is_some_and(|h| h <= 23)
        && digits(minute, 2).is_some_and(|m| m <= 59)
        && digits(second, 2).is_some_and(|s| s <= 59)
}
