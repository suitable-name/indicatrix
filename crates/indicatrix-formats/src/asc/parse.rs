//! [`parse_asc`]: splits `.asc` text into physical lines, reassembles
//! wrapped `a` records, and builds the resulting [`AscSchedule`].

use super::{
    error::AscParseError,
    schedule::{AscLineEnding, AscSchedule},
    tier::AscTier,
};
use std::{num::ParseFloatError, str::SplitWhitespace};

/// Splits `content` into physical lines and reassembles `a` records that `GemCAD`
/// wrapped across multiple lines, returning the parsed schedule.
///
/// # Errors
///
/// Returns `Err` (as a human-readable message, matching the other lenient parsers in
/// this crate) if:
/// - `content` is empty or whitespace-only;
/// - a required header field (`g`/gear-teeth, `y`/symmetry, `I`/refractive-index) is
///   missing or has a non-numeric value where a number is required -- these are the
///   fields the rest of the crate depends on, so a file missing them is treated as
///   corrupt rather than silently defaulted;
/// - an `a` record's angle or mast field is missing or non-numeric (the two fields
///   this parser exists to extract reliably);
/// - an angle, mast, index position, refractive index, or gear reference angle parses
///   to `NaN` or an infinity (see [`AscParseError::NonFiniteValue`]) -- these values
///   feed the meet solver and stone measurement directly, where a non-finite value
///   would otherwise poison a result silently instead of failing loudly;
/// - the symmetry order is exactly zero, or the refractive index is not greater than
///   `1.0` (see [`AscParseError::SymmetryOrderZero`]/[`AscParseError::RefractiveIndexOutOfRange`]);
/// - the gear tooth count is zero or larger than [`AscParseError::MAX_GEAR_TEETH`] in
///   magnitude (see [`AscParseError::GearTeethZero`]/[`AscParseError::GearTeethTooLarge`]);
/// - the file contains no valid `a` records at all.
///
/// Everything else is handled leniently, but not always silently: a duplicate `g`
/// line (last value wins), free text following the last facet tier that is not a
/// real wrapped continuation, and a stray token inside an `a` record that is
/// neither a number nor a name/notes marker are all collected into
/// [`AscSchedule::warnings`] rather than rejected outright or silently folded into
/// tier data. A facet name with no indices is kept as a single azimuth-0 tier
/// (matching how table/culet rows are commonly written); unrecognized lines outside
/// any tier are skipped with a warning (decades of hand-edited files carry stray
/// header-area annotations, and undocumented preform records would land here); the
/// well-documented but rare `g` line missing its leading keyword (seen once in the
/// real corpus, as a bare `"96 0.0"`) is tolerated as an implicit gear line; and a
/// `g`/`y`/`I` keyword glued to its value (`g96 0.0`, `y8y`, `I1.54`, written by
/// some third-party exporters and printed in the German manual) is split apart with
/// a warning.
///
/// A leading UTF-8 byte-order mark is ignored, and the line terminator (`\n` or
/// `\r\n`) is recorded in [`AscSchedule::line_ending`]. This takes text; for raw
/// file bytes (which may be Windows-1252) use [`super::parse_asc_bytes`].
pub fn parse_asc(content: &str) -> Result<AscSchedule, AscParseError> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    if content.trim().is_empty() {
        return Err(AscParseError::EmptyInput);
    }

    let mut state = ParseState {
        schedule: AscSchedule {
            line_ending: AscLineEnding::detect(content),
            ..AscSchedule::default()
        },
        gear_teeth: None,
        symmetry_order: None,
        refractive_index: None,
        seen_any_tier: false,
        pending: None,
    };

    for (i, raw_line) in content.lines().enumerate() {
        state.parse_line(i + 1, raw_line)?;
    }
    state.finish()
}

/// Everything [`parse_asc`] accumulates while it walks the lines of a file.
struct ParseState {
    /// The schedule being built.
    schedule: AscSchedule,
    /// The `g` record's tooth count, once seen.
    gear_teeth: Option<i32>,
    /// The `y` record's symmetry order, once seen.
    symmetry_order: Option<u32>,
    /// The `I` record's refractive index, once seen.
    refractive_index: Option<f64>,
    /// Whether an `a` record has started yet.
    seen_any_tier: bool,
    /// Tokens accumulated for the 'a' record currently being assembled (possibly
    /// across several continuation lines), plus the physical line number it started
    /// on (for error messages).
    pending: Option<(usize, Vec<String>)>,
}

impl ParseState {
    /// Turns the open `a` record, if any, into a tier.
    fn flush_pending(&mut self) -> Result<(), AscParseError> {
        finalize_pending(
            &mut self.pending,
            &mut self.schedule.tiers,
            &mut self.schedule.warnings,
        )
    }

    /// Handles one physical line (`line_no` is 1-based).
    fn parse_line(&mut self, line_no: usize, raw_line: &str) -> Result<(), AscParseError> {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            return Ok(());
        }
        let split_line = split_glued_keyword(line);
        if let Some(split) = &split_line {
            self.schedule.warnings.push(format!(
                "line {line_no}: keyword glued to its value, read as {split:?}"
            ));
        }
        let line = split_line.as_deref().unwrap_or(line);

        let mut tokens = line.split_whitespace();
        let Some(first) = tokens.next() else {
            return Ok(());
        };

        match first {
            "GemCad" | "GemCAD" | "Gemcad" => {
                self.flush_pending()?;
                self.schedule.gemcad_version = tokens.collect::<Vec<_>>().join(" ");
            }
            "g" => {
                self.flush_pending()?;
                self.read_gear_line(line_no, line, tokens)?;
            }
            "y" => {
                self.flush_pending()?;
                self.read_symmetry_line(line_no, line, tokens)?;
            }
            "I" => {
                self.flush_pending()?;
                self.read_refractive_index_line(line_no, tokens)?;
            }
            "H" => {
                self.flush_pending()?;
                self.schedule.headers.push(line[1..].trim().to_string());
            }
            "F" => {
                self.flush_pending()?;
                self.schedule.footnotes.push(line[1..].trim().to_string());
            }
            "a" => {
                self.flush_pending()?;
                self.seen_any_tier = true;
                self.pending = Some((line_no, tokens.map(str::to_string).collect()));
            }
            _ => self.read_other_line(line_no, line),
        }
        Ok(())
    }

    /// The `g` record: gear tooth count and reference angle.
    fn read_gear_line(
        &mut self,
        line_no: usize,
        line: &str,
        tokens: SplitWhitespace<'_>,
    ) -> Result<(), AscParseError> {
        let rest: Vec<&str> = tokens.collect();
        if rest.len() < 2 {
            return Err(AscParseError::GearLineMissingFields {
                line: line_no,
                text: line.to_string(),
            });
        }
        let teeth: f64 = rest[0]
            .parse()
            .map_err(|_| AscParseError::GearTeethNotNumeric {
                line: line_no,
                token: rest[0].to_string(),
            })?;
        let teeth_i32 = require_nonzero_gear_teeth(teeth, line_no, rest[0])?;
        let ref_angle: f64 =
            rest[1]
                .parse()
                .map_err(|_| AscParseError::GearReferenceAngleNotNumeric {
                    line: line_no,
                    token: rest[1].to_string(),
                })?;
        if !ref_angle.is_finite() {
            return Err(AscParseError::NonFiniteValue {
                line: line_no,
                field: "gear reference angle",
                token: rest[1].to_string(),
                value: ref_angle,
            });
        }
        if self.gear_teeth.is_some() {
            self.schedule.warnings.push(format!(
                "line {line_no}: duplicate 'g' (gear) line overrides the previous value"
            ));
        }
        self.gear_teeth = Some(teeth_i32);
        self.schedule.gear_reference_angle = ref_angle;
        Ok(())
    }

    /// The `y` record: symmetry order and mirror flag.
    fn read_symmetry_line(
        &mut self,
        line_no: usize,
        line: &str,
        tokens: SplitWhitespace<'_>,
    ) -> Result<(), AscParseError> {
        let rest: Vec<&str> = tokens.collect();
        if rest.len() < 2 {
            return Err(AscParseError::SymmetryLineMissingFields {
                line: line_no,
                text: line.to_string(),
            });
        }
        let order: f64 = rest[0]
            .parse()
            .map_err(|_| AscParseError::SymmetryOrderNotNumeric {
                line: line_no,
                token: rest[0].to_string(),
            })?;
        let order_u32 = require_integral_u32(order, line_no, "symmetry order", rest[0])?;
        if order_u32 == 0 {
            return Err(AscParseError::SymmetryOrderZero { line: line_no });
        }
        self.symmetry_order = Some(order_u32);
        self.schedule.mirror = if rest[1].eq_ignore_ascii_case("y") {
            true
        } else if rest[1].eq_ignore_ascii_case("n") {
            false
        } else {
            return Err(AscParseError::MirrorFlagInvalid {
                line: line_no,
                token: rest[1].to_string(),
            });
        };
        Ok(())
    }

    /// The `I` record: the refractive index.
    fn read_refractive_index_line(
        &mut self,
        line_no: usize,
        mut tokens: SplitWhitespace<'_>,
    ) -> Result<(), AscParseError> {
        let val = tokens
            .next()
            .ok_or(AscParseError::RefractiveIndexMissing { line: line_no })?;
        let ri: f64 = val
            .parse()
            .map_err(|_| AscParseError::RefractiveIndexNotNumeric {
                line: line_no,
                token: val.to_string(),
            })?;
        if !ri.is_finite() {
            return Err(AscParseError::NonFiniteValue {
                line: line_no,
                field: "refractive index",
                token: val.to_string(),
                value: ri,
            });
        }
        if ri <= 1.0 {
            return Err(AscParseError::RefractiveIndexOutOfRange {
                line: line_no,
                value: ri,
            });
        }
        self.refractive_index = Some(ri);
        Ok(())
    }

    /// A line whose first token is no record keyword: a wrapped continuation of the open
    /// `a` record, a bare gear line, or stray text.
    fn read_other_line(&mut self, line_no: usize, line: &str) {
        if let Some((_, buf)) = self.pending.as_mut() {
            // Real wrapped continuations verified in the corpus all start with a
            // bare index number, an "n" name marker, or a "G" notes marker (see
            // the module doc comment) -- a line that starts with anything else
            // (e.g. a trailing "Designed 1999 by X" annotation left after the
            // last tier) is not a continuation at all, just stray free text that
            // happens to follow an still-open tier record. Folding it in used to
            // fabricate bogus indices/names; now it is a warning instead.
            let first_tok = line.split_whitespace().next();
            let looks_like_continuation =
                first_tok.is_some_and(|t| t.parse::<f64>().is_ok() || t == "n" || t == "G");
            if looks_like_continuation {
                buf.extend(line.split_whitespace().map(str::to_string));
            } else {
                self.schedule.warnings.push(format!(
                    "line {line_no}: ignored free text after the last facet tier: {line:?}"
                ));
            }
        } else if self.gear_teeth.is_none() && !self.seen_any_tier {
            // Tolerate the one real-world quirk seen in the corpus: a 'g' line
            // that lost its leading keyword, e.g. "96 0.0" instead of
            // "g 96 0.0". Only attempted before the first tier, and only for a
            // line that looks exactly like a bare gear record.
            let bare: Vec<&str> = line.split_whitespace().collect();
            if bare.len() == 2
                && let (Ok(teeth), Ok(refang)) = (bare[0].parse::<f64>(), bare[1].parse::<f64>())
                && refang.is_finite()
                && let Ok(teeth_i32) = require_nonzero_gear_teeth(teeth, line_no, bare[0])
            {
                self.gear_teeth = Some(teeth_i32);
                self.schedule.gear_reference_angle = refang;
            } else {
                // An unrecognized header-area line: skipped leniently (decades
                // of hand-edited files carry stray annotations), but visible.
                push_unrecognized_line_warning(&mut self.schedule.warnings, line_no, line);
            }
        } else {
            // Unrecognized line with no open tier and the gear already known
            // (or after the H/F block): skipped leniently, but visible --
            // an undocumented preform record would land here.
            push_unrecognized_line_warning(&mut self.schedule.warnings, line_no, line);
        }
    }

    /// Closes the last open record and checks that every required header field and at
    /// least one tier were seen.
    fn finish(mut self) -> Result<AscSchedule, AscParseError> {
        self.flush_pending()?;

        self.schedule.gear_teeth = self.gear_teeth.ok_or(AscParseError::MissingGearLine)?;
        self.schedule.symmetry_order = self
            .symmetry_order
            .ok_or(AscParseError::MissingSymmetryLine)?;
        self.schedule.refractive_index = self
            .refractive_index
            .ok_or(AscParseError::MissingRefractiveIndexLine)?;

        if self.schedule.tiers.is_empty() {
            return Err(AscParseError::NoTierRecords);
        }

        Ok(self.schedule)
    }
}

/// Records a skipped, unrecognised line in [`AscSchedule::warnings`].
fn push_unrecognized_line_warning(warnings: &mut Vec<String>, line_no: usize, line: &str) {
    warnings.push(format!(
        "line {line_no}: ignored unrecognized line: {line:?}"
    ));
}

/// Splits a `g`, `y` or `I` keyword glued to its value (`g96 0.0`, `y8y`,
/// `I1.54`) into the spaced form the record match below expects (`g 96 0.0`,
/// `y 8 y`, `I 1.54`). `None` when `line` does not start with such a token (a
/// standalone keyword, or a word such as `gear` whose tail is not numeric).
fn split_glued_keyword(line: &str) -> Option<String> {
    let first = line.split_whitespace().next()?;
    let mut chars = first.chars();
    let keyword = chars.next()?;
    if !matches!(keyword, 'g' | 'y' | 'I') {
        return None;
    }
    let value = chars.as_str();
    if value.is_empty() {
        return None;
    }
    let value = if keyword == 'y' {
        split_symmetry_value(value)?
    } else {
        parse_number(value).ok()?;
        value.to_string()
    };
    let rest = line[first.len()..].trim();
    Some(if rest.is_empty() {
        format!("{keyword} {value}")
    } else {
        format!("{keyword} {value} {rest}")
    })
}

/// The value part of a glued `y` token: `"8y"` becomes `"8 y"`, `"8"` stays
/// `"8"`. `None` unless what precedes an optional trailing `y`/`n` flag is a
/// number.
fn split_symmetry_value(value: &str) -> Option<String> {
    let (order, flag) = match value.char_indices().last() {
        Some((i, flag)) if matches!(flag, 'y' | 'Y' | 'n' | 'N') => (&value[..i], Some(flag)),
        _ => (value, None),
    };
    parse_number(order).ok()?;
    Some(flag.map_or_else(|| order.to_string(), |flag| format!("{order} {flag}")))
}

/// Parses a numeric token, accepting the Unicode minus sign (U+2212) the
/// `GemCAD` manuals' typeset examples use, so a line copied out of a PDF reads
/// the same as one `GemCAD` wrote.
fn parse_number(token: &str) -> Result<f64, ParseFloatError> {
    if token.contains('\u{2212}') {
        token.replace('\u{2212}', "-").parse()
    } else {
        token.parse()
    }
}

/// How far `value` may sit from its own nearest whole number and still count as
/// "integral" -- generous enough to absorb ordinary `f64` parse/arithmetic noise,
/// tight enough that anything a real `.asc` file would write on purpose (e.g. `96.5`)
/// still fails it.
const INTEGRAL_TOLERANCE: f64 = 1e-9;

/// Rejects `value` unless it is both integral (within [`INTEGRAL_TOLERANCE`]) and
/// small enough to fit an `i32` -- used for the `g` (gear teeth) header field, which
/// can legitimately be negative (an internal handedness convention), so this checks
/// magnitude, not sign. A plain `as i32` would silently truncate or saturate; this
/// instead reports the exact line and offending token.
fn require_integral_i32(
    value: f64,
    line_no: usize,
    field: &'static str,
    token: &str,
) -> Result<i32, AscParseError> {
    if !value.is_finite() {
        return Err(AscParseError::NonFiniteValue {
            line: line_no,
            field,
            token: token.to_string(),
            value,
        });
    }
    if (value - value.round()).abs() > INTEGRAL_TOLERANCE || value.abs() > f64::from(i32::MAX) {
        return Err(AscParseError::NotWholeNumber {
            line: line_no,
            field,
            token: token.to_string(),
            value,
        });
    }
    Ok(value.round() as i32)
}

/// Parses and validates a gear-tooth-count token exactly like [`require_integral_i32`],
/// additionally rejecting an all-zero tooth count: `phi = 2*pi*index/gear_teeth_abs()`
/// (see [`super::AscSchedule::gear_teeth_abs`]) divides by this value downstream, so a
/// zero-tooth gear is nonsensical. A count beyond [`AscParseError::MAX_GEAR_TEETH`] in
/// magnitude is rejected too: the index wheel draws and hit-tests once per tooth, so an
/// `i32`-sized count would stall every consumer. Shared by both places a gear tooth
/// count is parsed -- the `g` line itself, and the bare-gear-line fallback that
/// tolerates one real corpus file's missing `g` keyword.
fn require_nonzero_gear_teeth(
    value: f64,
    line_no: usize,
    token: &str,
) -> Result<i32, AscParseError> {
    let teeth = require_integral_i32(value, line_no, "gear tooth count", token)?;
    if teeth == 0 {
        return Err(AscParseError::GearTeethZero { line: line_no });
    }
    if teeth.unsigned_abs() > AscParseError::MAX_GEAR_TEETH {
        return Err(AscParseError::GearTeethTooLarge {
            line: line_no,
            teeth,
        });
    }
    Ok(teeth)
}

/// Same contract as [`require_integral_i32`], narrowed to `u32` instead -- used for
/// the `y` (symmetry order) header field, which (unlike gear teeth) has no negative
/// convention, so this also rejects negative values outright.
fn require_integral_u32(
    value: f64,
    line_no: usize,
    field: &'static str,
    token: &str,
) -> Result<u32, AscParseError> {
    if !value.is_finite() {
        return Err(AscParseError::NonFiniteValue {
            line: line_no,
            field,
            token: token.to_string(),
            value,
        });
    }
    if (value - value.round()).abs() > INTEGRAL_TOLERANCE
        || !(0.0..=f64::from(u32::MAX)).contains(&value)
    {
        return Err(AscParseError::NotWholeNumber {
            line: line_no,
            field,
            token: token.to_string(),
            value,
        });
    }
    Ok(value.round() as u32)
}

fn finalize_pending(
    pending: &mut Option<(usize, Vec<String>)>,
    tiers: &mut Vec<AscTier>,
    warnings: &mut Vec<String>,
) -> Result<(), AscParseError> {
    let Some((line_no, tokens)) = pending.take() else {
        return Ok(());
    };
    tiers.push(parse_tier(line_no, &tokens, warnings)?);
    Ok(())
}

/// Parses one accumulated `a` record's token buffer (everything after the leading
/// `a`, with continuation-line tokens already appended) into an [`AscTier`].
fn parse_tier(
    line_no: usize,
    tokens: &[String],
    warnings: &mut Vec<String>,
) -> Result<AscTier, AscParseError> {
    if tokens.len() < 2 {
        return Err(AscParseError::TierRecordTooShort {
            line: line_no,
            field_count: tokens.len(),
        });
    }
    let (angle_deg, mast) = parse_angle_and_mast(line_no, &tokens[0], &tokens[1], warnings)?;

    let mut indices = Vec::new();
    let mut names: Vec<String> = Vec::new();
    let mut index_names: Vec<(usize, String)> = Vec::new();
    let mut note_tokens: Vec<&str> = Vec::new();
    let mut in_notes = false;
    // Set right after consuming an "n" marker: GemCAD facet names can themselves be
    // plain numbers (e.g. "n 1", "n 4" -- see sample_4208.asc), so a token cannot be
    // classified as an index vs. a name by whether it parses as a number alone. The
    // token immediately following "n" is unconditionally the name; everything else
    // that parses as a number is an index.
    let mut expect_name = false;

    for tok in &tokens[2..] {
        if in_notes {
            note_tokens.push(tok);
            continue;
        }
        // Must run before the "n"/"G" marker checks below: GemCAD facet names can
        // themselves be exactly "G" (e.g. a girdle tier literally named "G" -- see
        // sample corpus files where "n G" is followed by indices or by another "n G"
        // group), so the token right after "n" is unconditionally consumed as the
        // name even when it reads like the notes marker.
        if expect_name {
            expect_name = false;
            // GemCAD binds a name to the index written just before it; a name
            // before any index binds to position 0.
            index_names.push((indices.len().saturating_sub(1), tok.clone()));
            if names.last().map(String::as_str) != Some(tok.as_str()) {
                // Repeated occurrences of the same name (common when a tier's indices
                // are split across more than one `n <name>` group) are folded
                // together rather than duplicated.
                names.push(tok.clone());
            }
            continue;
        }
        if tok == "n" {
            expect_name = true; // marker: the next token is a facet name, not an index
            continue;
        }
        if tok == "G" {
            in_notes = true;
            continue;
        }
        if let Ok(v) = parse_number(tok) {
            if !v.is_finite() {
                return Err(AscParseError::NonFiniteValue {
                    line: line_no,
                    field: "index",
                    token: tok.clone(),
                    value: v,
                });
            }
            indices.push(v);
        } else {
            // A token that is neither a number nor an "n"/"G" marker, and not
            // immediately after an "n" marker either: not seen in the sampled corpus
            // as a genuine facet name, and indistinguishable from stray free text
            // (e.g. a garbled index token, or a trailing annotation folded in as a
            // continuation) -- collected as a warning and dropped rather than
            // silently becoming part of the tier's name.
            warnings.push(format!(
                "line {line_no}: ignored token that is neither a number nor a name/notes marker: {tok:?}"
            ));
        }
    }

    Ok(AscTier {
        angle_deg,
        mast,
        name: names.join("/"),
        indices,
        index_names,
        notes: note_tokens.join(" "),
    })
}

/// Parses and validates an `a` record's angle and mast tokens, then applies the
/// culet convention (see [`AscTier::angle_deg`]): a zero angle with a negative
/// distance is `GemCAD`'s documented culet, stored as a sign-negative zero angle
/// with a positive mast. Every consumer then reads the side from the angle's sign
/// alone.
fn parse_angle_and_mast(
    line_no: usize,
    angle_token: &str,
    mast_token: &str,
    warnings: &mut Vec<String>,
) -> Result<(f64, f64), AscParseError> {
    let angle_deg = parse_number(angle_token).map_err(|_| AscParseError::AngleNotNumeric {
        line: line_no,
        token: angle_token.to_string(),
    })?;
    if !angle_deg.is_finite() {
        return Err(AscParseError::NonFiniteValue {
            line: line_no,
            field: "angle",
            token: angle_token.to_string(),
            value: angle_deg,
        });
    }
    let mast = parse_number(mast_token).map_err(|_| AscParseError::MastNotNumeric {
        line: line_no,
        token: mast_token.to_string(),
    })?;
    if !mast.is_finite() {
        return Err(AscParseError::NonFiniteValue {
            line: line_no,
            field: "mast distance",
            token: mast_token.to_string(),
            value: mast,
        });
    }
    if mast < 0.0 {
        if angle_deg == 0.0 {
            // The culet: "positive unless the facet is a culet (0° pavilion) facet".
            return Ok((-0.0, -mast));
        }
        // Undocumented: the manual only ever makes a culet's distance negative (no
        // real catalogue file does this either). Kept as written; geometry uses the
        // magnitude on the angle's own side.
        warnings.push(format!(
            "line {line_no}: negative mast {mast} on a nonzero-angle tier ({angle_deg} deg); \
             its magnitude is used"
        ));
    }
    Ok((angle_deg, mast))
}
