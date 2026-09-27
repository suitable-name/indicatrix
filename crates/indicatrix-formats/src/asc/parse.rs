//! [`parse_asc`]: splits `.asc` text into physical lines, reassembles
//! wrapped `a` records, and builds the resulting [`AscSchedule`].

use super::{error::AscParseError, schedule::AscSchedule, tier::AscTier};

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
/// - the file contains no valid `a` records at all.
///
/// Everything else is handled leniently: unrecognized lines are ignored, a facet name
/// with no indices is kept as a single azimuth-0 tier (matching how table/culet rows
/// are commonly written), and the well-documented but rare `g` line missing its
/// leading keyword (seen once in the real corpus, as a bare `"96 0.0"`) is tolerated
/// as an implicit gear line.
#[expect(
    clippy::too_many_lines,
    reason = "one state machine over every record keyword; splitting it would scatter the parsing logic"
)]
pub fn parse_asc(content: &str) -> Result<AscSchedule, AscParseError> {
    if content.trim().is_empty() {
        return Err(AscParseError::EmptyInput);
    }

    let mut schedule = AscSchedule::default();
    let mut gear_teeth: Option<i32> = None;
    let mut symmetry_order: Option<u32> = None;
    let mut refractive_index: Option<f64> = None;
    let mut seen_any_tier = false;

    // Tokens accumulated for the 'a' record currently being assembled (possibly
    // across several continuation lines), plus the physical line number it started
    // on (for error messages).
    let mut pending: Option<(usize, Vec<String>)> = None;

    for (i, raw_line) in content.lines().enumerate() {
        let line_no = i + 1;
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with(';') || line.starts_with('#') {
            continue;
        }

        let mut tokens = line.split_whitespace();
        let Some(first) = tokens.next() else { continue };

        match first {
            "GemCad" | "GemCAD" | "Gemcad" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                schedule.gemcad_version = tokens.collect::<Vec<_>>().join(" ");
            }
            "g" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                let rest: Vec<&str> = tokens.collect();
                if rest.len() < 2 {
                    return Err(AscParseError::GearLineMissingFields {
                        line: line_no,
                        text: line.to_string(),
                    });
                }
                let teeth: f64 =
                    rest[0]
                        .parse()
                        .map_err(|_| AscParseError::GearTeethNotNumeric {
                            line: line_no,
                            token: rest[0].to_string(),
                        })?;
                gear_teeth = Some(require_nonzero_gear_teeth(teeth, line_no, rest[0])?);
                schedule.gear_reference_angle =
                    rest[1]
                        .parse()
                        .map_err(|_| AscParseError::GearReferenceAngleNotNumeric {
                            line: line_no,
                            token: rest[1].to_string(),
                        })?;
            }
            "y" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                let rest: Vec<&str> = tokens.collect();
                if rest.len() < 2 {
                    return Err(AscParseError::SymmetryLineMissingFields {
                        line: line_no,
                        text: line.to_string(),
                    });
                }
                let order: f64 =
                    rest[0]
                        .parse()
                        .map_err(|_| AscParseError::SymmetryOrderNotNumeric {
                            line: line_no,
                            token: rest[0].to_string(),
                        })?;
                symmetry_order = Some(require_integral_u32(
                    order,
                    line_no,
                    "symmetry order",
                    rest[0],
                )?);
                schedule.mirror = if rest[1].eq_ignore_ascii_case("y") {
                    true
                } else if rest[1].eq_ignore_ascii_case("n") {
                    false
                } else {
                    return Err(AscParseError::MirrorFlagInvalid {
                        line: line_no,
                        token: rest[1].to_string(),
                    });
                };
            }
            "I" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                let val = tokens
                    .next()
                    .ok_or(AscParseError::RefractiveIndexMissing { line: line_no })?;
                refractive_index =
                    Some(
                        val.parse()
                            .map_err(|_| AscParseError::RefractiveIndexNotNumeric {
                                line: line_no,
                                token: val.to_string(),
                            })?,
                    );
            }
            "H" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                schedule.headers.push(line[1..].trim().to_string());
            }
            "F" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                schedule.footnotes.push(line[1..].trim().to_string());
            }
            "a" => {
                finalize_pending(&mut pending, &mut schedule.tiers)?;
                seen_any_tier = true;
                pending = Some((line_no, tokens.map(str::to_string).collect()));
            }
            _ => {
                if let Some((_, buf)) = pending.as_mut() {
                    // Continuation of the currently-open 'a' record.
                    buf.extend(line.split_whitespace().map(str::to_string));
                } else if gear_teeth.is_none() && !seen_any_tier {
                    // Tolerate the one real-world quirk seen in the corpus: a 'g' line
                    // that lost its leading keyword, e.g. "96 0.0" instead of
                    // "g 96 0.0". Only attempted before the first tier, and only for a
                    // line that looks exactly like a bare gear record.
                    let bare: Vec<&str> = line.split_whitespace().collect();
                    if bare.len() == 2
                        && let (Ok(teeth), Ok(refang)) =
                            (bare[0].parse::<f64>(), bare[1].parse::<f64>())
                        && let Ok(teeth_i32) = require_nonzero_gear_teeth(teeth, line_no, bare[0])
                    {
                        gear_teeth = Some(teeth_i32);
                        schedule.gear_reference_angle = refang;
                    }
                    // Otherwise: an unrecognized header-area line. Ignore leniently --
                    // decades of hand-edited files carry stray annotations.
                }
                // else: unrecognized line with no open tier and gear already known;
                // ignore leniently.
            }
        }
    }
    finalize_pending(&mut pending, &mut schedule.tiers)?;

    schedule.gear_teeth = gear_teeth.ok_or(AscParseError::MissingGearLine)?;
    schedule.symmetry_order = symmetry_order.ok_or(AscParseError::MissingSymmetryLine)?;
    schedule.refractive_index =
        refractive_index.ok_or(AscParseError::MissingRefractiveIndexLine)?;

    if schedule.tiers.is_empty() {
        return Err(AscParseError::NoTierRecords);
    }

    Ok(schedule)
}

/// Parses one accumulated `a` record's token buffer (everything after the leading
/// `a`, with continuation-line tokens already appended) into an [`AscTier`], and
/// pushes it onto `tiers`.
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
/// zero-tooth gear is nonsensical. Shared by both places a gear tooth count is parsed
/// -- the `g` line itself, and the bare-gear-line fallback that tolerates one real
/// corpus file's missing `g` keyword.
fn require_nonzero_gear_teeth(
    value: f64,
    line_no: usize,
    token: &str,
) -> Result<i32, AscParseError> {
    let teeth = require_integral_i32(value, line_no, "gear tooth count", token)?;
    if teeth == 0 {
        return Err(AscParseError::GearTeethZero { line: line_no });
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
) -> Result<(), AscParseError> {
    let Some((line_no, tokens)) = pending.take() else {
        return Ok(());
    };
    tiers.push(parse_tier(line_no, &tokens)?);
    Ok(())
}

fn parse_tier(line_no: usize, tokens: &[String]) -> Result<AscTier, AscParseError> {
    if tokens.len() < 2 {
        return Err(AscParseError::TierRecordTooShort {
            line: line_no,
            field_count: tokens.len(),
        });
    }

    let angle_deg: f64 = tokens[0]
        .parse()
        .map_err(|_| AscParseError::AngleNotNumeric {
            line: line_no,
            token: tokens[0].clone(),
        })?;
    let mast: f64 = tokens[1]
        .parse()
        .map_err(|_| AscParseError::MastNotNumeric {
            line: line_no,
            token: tokens[1].clone(),
        })?;

    let mut indices = Vec::new();
    let mut names: Vec<String> = Vec::new();
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
        if let Ok(v) = tok.parse::<f64>() {
            indices.push(v);
        } else if names.last().map(String::as_str) != Some(tok.as_str()) {
            // A facet-name token that showed up without an "n" marker ahead of it.
            // Not seen in the sampled corpus, but tolerated for robustness.
            names.push(tok.clone());
        }
    }

    Ok(AscTier {
        angle_deg,
        mast,
        name: names.join("/"),
        indices,
        notes: note_tokens.join(" "),
    })
}
