//! [`to_asc_string`]: serializes an [`AscSchedule`] back to `.asc` text, and
//! [`mark_reconstructed`]: marks a schedule as derived rather than authored.

use super::{schedule::AscSchedule, tier::AscTier};
use std::{borrow::Cow, fmt};

/// Everything that can go wrong serializing an [`AscSchedule`] to `.asc` text with
/// [`to_asc_string`].
///
/// A tier's name is never rejected here -- an unsafe one is silently sanitised by
/// [`asc_safe_tier_name`] instead (see [`to_asc_string`]'s own doc comment). Every
/// error left in this enum is a value the written bytes could not carry back
/// unchanged. A line break (a newline or a carriage return) in a header, footnote, or
/// tier's notes makes [`super::parse_asc`] read *more* physical lines than were meant,
/// silently merging or dropping content rather than failing to parse at all. A `NaN`
/// or infinite number is written by `Display` as text [`super::parse_asc`] then
/// rejects. Rejecting the write is the only way to keep [`to_asc_string`]'s
/// round-trip promise honest for these fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AscWriteError {
    /// An `H` (header) line's text contains a newline or carriage return. Each
    /// header is written on its own physical line (`H {text}`); a line break would
    /// split it into two physical lines, the second of which [`super::parse_asc`]
    /// does not recognize as a continuation of anything -- it would be misread as a
    /// stray line (silently dropped before the first tier, or worse, folded into
    /// whatever tier happens to be open at that point in the file).
    HeaderContainsNewline {
        /// 0-based position of the offending header in [`AscSchedule::headers`].
        header_index: usize,
        /// The offending header text.
        text: String,
    },
    /// An `F` (footnote) line's text contains a newline or carriage return -- same
    /// hazard as [`Self::HeaderContainsNewline`], for [`AscSchedule::footnotes`].
    FootnoteContainsNewline {
        /// 0-based position of the offending footnote in [`AscSchedule::footnotes`].
        footnote_index: usize,
        /// The offending footnote text.
        text: String,
    },
    /// A tier's `G`-field notes text contains a newline or carriage return. Notes are
    /// written as the tail of that tier's own `a` record line (`... G {notes}`); a
    /// line break would end the physical line early, truncating the note, and the
    /// remaining text would either be dropped as a warning or misread as a
    /// continuation of the (already-closed, as far as the note is concerned) tier.
    NotesContainNewline {
        /// 0-based position of the offending tier in [`AscSchedule::tiers`].
        tier_index: usize,
        /// The offending notes text.
        text: String,
    },
    /// A number is `NaN` or infinite. `Display` writes it as `NaN` or `inf`, which
    /// [`super::parse_asc`] rejects as a non-finite value, so the file would not
    /// read back at all.
    NonFiniteValue {
        /// 0-based position of the offending tier in [`AscSchedule::tiers`], or
        /// `None` for a header-level number.
        tier_index: Option<usize>,
        /// Which number: `"angle"`, `"mast"`, `"index"`, `"gear reference angle"` or
        /// `"refractive index"`.
        field: &'static str,
        /// The value as `Display` writes it (`NaN`, `inf` or `-inf`).
        value: String,
    },
}

impl fmt::Display for AscWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::HeaderContainsNewline { header_index, text } => write!(
                f,
                "header #{header_index} {text:?} contains a line break, which would split it \
                 across physical lines"
            ),
            Self::FootnoteContainsNewline {
                footnote_index,
                text,
            } => write!(
                f,
                "footnote #{footnote_index} {text:?} contains a line break, which would split \
                 it across physical lines"
            ),
            Self::NotesContainNewline { tier_index, text } => write!(
                f,
                "tier #{tier_index}: notes {text:?} contain a line break, which would truncate \
                 them on re-parse"
            ),
            Self::NonFiniteValue {
                tier_index: Some(tier_index),
                field,
                value,
            } => write!(
                f,
                "tier #{tier_index}: {field} is {value}, which cannot be read back"
            ),
            Self::NonFiniteValue {
                tier_index: None,
                field,
                value,
            } => write!(f, "{field} is {value}, which cannot be read back"),
        }
    }
}

impl std::error::Error for AscWriteError {}

/// `true` iff `name` would survive a plain `.asc` write/re-parse round trip as this
/// exact string -- equivalently, iff [`asc_safe_tier_name`] returns `name` unchanged
/// (borrowed, allocating nothing).
///
/// [`to_asc_string`] writes a tier's name as a single ` n <name>` token;
/// [`super::parse_asc`]'s tokenizer splits every record on ASCII whitespace, so a
/// name with an embedded space, tab, or newline reads back as *several* tokens
/// instead of one: only the first becomes the name (silently truncating it), and
/// each token after it is either dropped (a warning, if it parses as neither a
/// number nor an `n`/`G` marker) or -- worse -- misread as a fresh `n`/`G` marker,
/// corrupting the rest of the record (e.g. a name like `"Upper G Girdle"` reads
/// back as name `"Upper"` with everything from the embedded `"G"` onward folded
/// into notes instead of the name). An empty name is trivially safe -- see
/// [`super::AscTier::name`]'s own doc comment for why an empty name is common and
/// expected. Names joined with `/` (more than one facet name sharing a single tier)
/// are still one whitespace-free token, so that convention is unaffected.
///
/// [`to_asc_string`] never rejects an unsafe name -- it sanitises it via
/// [`asc_safe_tier_name`] before writing it. This predicate remains useful on its
/// own to check, without allocating, whether a name will show up in the written
/// `.asc` exactly as authored or in its sanitised form.
#[must_use]
pub fn is_asc_safe_tier_name(name: &str) -> bool {
    !name.chars().any(char::is_whitespace)
}

/// Sanitises `name` into a value [`to_asc_string`] can safely write as a tier's
/// single ` n <name>` token.
///
/// Every run of Unicode whitespace (spaces, tabs, newlines, ...) becomes one `_`,
/// and leading/trailing whitespace is trimmed away entirely rather than becoming a
/// leading/trailing `_`. `"Crown Main"` becomes `"Crown_Main"`; `"  Star  "` becomes
/// `"Star"`; `""` stays `""`. Returns `name` itself, borrowed, whenever it is
/// already [`is_asc_safe_tier_name`] -- the common case allocates nothing.
///
/// This is the sanitising counterpart of the rejection [`is_asc_safe_tier_name`]
/// merely detects: the true, human-typed name (e.g. `"Crown Main"`) still lives in
/// the native `.indicatrix.toml` sidecar, which restores it on load -- a plain
/// `.asc` export is the only place this sanitised, single-token form is ever seen.
#[must_use]
pub fn asc_safe_tier_name(name: &str) -> Cow<'_, str> {
    if is_asc_safe_tier_name(name) {
        return Cow::Borrowed(name);
    }
    let mut out = String::with_capacity(name.len());
    let mut last_was_space = true; // leading whitespace is trimmed, not turned into `_`
    for ch in name.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                out.push('_');
            }
            last_was_space = true;
        } else {
            out.push(ch);
            last_was_space = false;
        }
    }
    if out.ends_with('_') {
        out.pop();
    }
    Cow::Owned(out)
}

/// Serializes an [`AscSchedule`] back to `.asc` text.
///
/// Not byte-identical to hand-authored `GemCAD` output -- whitespace and numeric
/// formatting are normalized, a wrapped tier is written on one line, and a culet is
/// always written as `a -0 -<mast> <indices>` (see [`AscTier::angle_deg`]) -- but
/// it round-trips semantically: `parse_asc(&to_asc_string(s)?)` reproduces a
/// schedule equal to `s` for any schedule [`super::parse_asc`] produced, with one
/// deliberate exception: a culet with no index comes back with the gear tooth
/// count as its one index (the same azimuth-independent plane). Facet
/// names are written at the index each one followed in the source
/// ([`AscTier::index_names`]), and every line ends with the schedule's own
/// [`AscSchedule::line_ending`] (CRLF for a schedule built by hand). Every numeric
/// field is written with `f64`'s/`i32`'s default `Display` formatting, which Rust
/// guarantees produces the shortest decimal string that reads back to the exact same
/// value, so no precision is lost across the round trip.
///
/// A tier name is sanitised via [`asc_safe_tier_name`] before being written as the
/// `n` marker's token -- e.g. `"Crown Main"` is written as `Crown_Main` -- so a
/// plain `.asc` export never carries the true, human-typed name for a tier whose
/// name has embedded whitespace; only the native `.indicatrix.toml` sidecar does,
/// and restores it on load. A name that is already [`is_asc_safe_tier_name`] is
/// written unchanged. A name that sanitises to nothing (only whitespace) would leave
/// the `n` marker bare, so the next index would be read back as the name; it is
/// written as an automatic label instead -- the tier's block letter (`C` crown, `G`
/// girdle, `P` pavilion or culet, `T` table) and its 1-based position in the
/// schedule, e.g. `C3`.
///
/// # Errors
///
/// [`AscWriteError`] when a header, footnote, or tier notes string contains a
/// newline or carriage return, or when a tier angle, mast or index, the gear
/// reference angle, or the refractive index is `NaN` or infinite -- writing any of
/// these as-is would produce `.asc` text that [`super::parse_asc`] reads back as a
/// *different* schedule, silently, or rejects. See [`AscWriteError`]'s own doc
/// comment.
pub fn to_asc_string(schedule: &AscSchedule) -> Result<String, AscWriteError> {
    use fmt::Write as _;

    validate_writable(schedule)?;

    let eol = schedule.line_ending.as_str();
    let mut out = String::new();

    let _ = write!(out, "GemCad {}{eol}", schedule.gemcad_version);
    let _ = write!(
        out,
        "g {} {}{eol}",
        schedule.gear_teeth, schedule.gear_reference_angle
    );
    let _ = write!(
        out,
        "y {} {}{eol}",
        schedule.symmetry_order,
        if schedule.mirror { "y" } else { "n" }
    );
    let _ = write!(out, "I {}{eol}", schedule.refractive_index);
    for header in &schedule.headers {
        let _ = write!(out, "H {header}{eol}");
    }

    for (tier_index, tier) in schedule.tiers.iter().enumerate() {
        write_tier(&mut out, tier, tier_index, schedule.gear_teeth_abs());
        out.push_str(eol);
    }

    for footnote in &schedule.footnotes {
        let _ = write!(out, "F {footnote}{eol}");
    }

    Ok(out)
}

/// `true` when `text` holds a newline or a carriage return. The reader turns either
/// into a line break, so neither may sit inside a single-line field.
fn has_line_break(text: &str) -> bool {
    text.contains(['\n', '\r'])
}

/// An [`AscWriteError::NonFiniteValue`] unless `value` is finite.
fn require_finite(
    tier_index: Option<usize>,
    field: &'static str,
    value: f64,
) -> Result<(), AscWriteError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(AscWriteError::NonFiniteValue {
            tier_index,
            field,
            value: value.to_string(),
        })
    }
}

/// Checks everything [`to_asc_string`] cannot write faithfully: a line break in a
/// header, footnote or tier note, and a `NaN` or infinite number anywhere.
fn validate_writable(schedule: &AscSchedule) -> Result<(), AscWriteError> {
    for (header_index, text) in schedule.headers.iter().enumerate() {
        if has_line_break(text) {
            return Err(AscWriteError::HeaderContainsNewline {
                header_index,
                text: text.clone(),
            });
        }
    }
    for (footnote_index, text) in schedule.footnotes.iter().enumerate() {
        if has_line_break(text) {
            return Err(AscWriteError::FootnoteContainsNewline {
                footnote_index,
                text: text.clone(),
            });
        }
    }
    require_finite(None, "gear reference angle", schedule.gear_reference_angle)?;
    require_finite(None, "refractive index", schedule.refractive_index)?;
    for (tier_index, tier) in schedule.tiers.iter().enumerate() {
        if has_line_break(&tier.notes) {
            return Err(AscWriteError::NotesContainNewline {
                tier_index,
                text: tier.notes.clone(),
            });
        }
        let at = Some(tier_index);
        require_finite(at, "angle", tier.angle_deg)?;
        require_finite(at, "mast", tier.mast)?;
        for &index in &tier.indices {
            require_finite(at, "index", index)?;
        }
    }
    Ok(())
}

/// The label written for a facet name that sanitises to nothing: the tier's block
/// letter and `ordinal + 1`. `G` for a girdle (`|angle| = 90`), `C` for a crown
/// tier, `P` for a pavilion tier or the culet, `T` for the table.
fn auto_tier_name(tier: &AscTier, ordinal: usize) -> String {
    let letter = if tier.angle_deg.abs() == 90.0 {
        'G'
    } else if tier.angle_deg > 0.0 {
        'C'
    } else if tier.angle_deg < 0.0 || tier.is_culet() {
        'P'
    } else {
        'T'
    };
    format!("{letter}{}", ordinal + 1)
}

/// Writes one `a` record (without its line terminator).
///
/// A culet ([`AscTier::is_culet`]) is written in `GemCAD`'s own documented form,
/// `a -0 -<mast> <indices>`: a zero angle with a negative distance, which every
/// reader understands, and at least one index (the gear tooth count when the tier
/// has none -- a culet is azimuth-independent, and readers that need an index to
/// create a facet would otherwise drop it). [`super::parse_asc`] reads that line
/// back as the same sign-negative zero angle with a positive mast.
///
/// Names go where [`AscTier::index_names`] recorded them (`idx n name`); a tier with
/// a [`AscTier::name`] but no recorded positions (anything authored in the editor)
/// puts it after the first index, `GemCAD`'s default labelling. A name that sanitises
/// to nothing is written as [`auto_tier_name`] of `ordinal` (the tier's 0-based
/// position in the schedule), so the `n` marker is never left bare.
fn write_tier(out: &mut String, tier: &AscTier, ordinal: usize, gear_teeth_abs: u32) {
    use fmt::Write as _;

    let culet = tier.is_culet();
    if culet {
        let _ = write!(out, "a -0 {}", -tier.mast.abs());
    } else {
        let _ = write!(out, "a {} {}", tier.angle_deg, tier.mast);
    }
    let gear_index = [f64::from(gear_teeth_abs)];
    let indices: &[f64] = if culet && tier.indices.is_empty() {
        &gear_index
    } else {
        &tier.indices
    };
    let folded_name;
    let names: &[(usize, String)] = if tier.index_names.is_empty() && !tier.name.is_empty() {
        folded_name = [(0, tier.name.clone())];
        &folded_name
    } else {
        &tier.index_names
    };
    let write_name = |out: &mut String, name: &str| {
        let safe = asc_safe_tier_name(name);
        if safe.is_empty() {
            let _ = write!(out, " n {}", auto_tier_name(tier, ordinal));
        } else {
            let _ = write!(out, " n {safe}");
        }
    };
    if indices.is_empty() {
        for (_, name) in names {
            write_name(out, name);
        }
    } else {
        let last = indices.len() - 1;
        for (position, idx) in indices.iter().enumerate() {
            let _ = write!(out, " {idx}");
            // A recorded position past the end (the indices were edited since)
            // stays on the last index rather than being dropped.
            for (_, name) in names
                .iter()
                .filter(|(at, _)| *at == position || (position == last && *at > last))
            {
                write_name(out, name);
            }
        }
    }
    if !tier.notes.is_empty() {
        let _ = write!(out, " G {}", tier.notes);
    }
}

/// Prepends a clear "this is derived, not authored" marker to `schedule`'s headers.
///
/// A reconstructed `.asc` (mast distances solved from angles and meet constraints,
/// not the original design's own measured masts) must never be mistaken for a
/// hand-authored, verified cutting instructions -- a user must be able to tell the two
/// apart before cutting a stone from either. Idempotent: calling it again on a
/// schedule that already carries a `RECONSTRUCTED` marker as its first header does
/// not add a second one.
///
/// `note` should say how the schedule was derived (e.g. which solver, and any caveat
/// about accuracy); it is appended after the marker on the same header line.
pub fn mark_reconstructed(schedule: &mut AscSchedule, note: &str) {
    let already_marked = schedule
        .headers
        .first()
        .is_some_and(|h| h.starts_with("RECONSTRUCTED"));
    if already_marked {
        return;
    }
    schedule.headers.insert(
        0,
        format!("RECONSTRUCTED -- mast distances are solved, not original -- {note}"),
    );
}
