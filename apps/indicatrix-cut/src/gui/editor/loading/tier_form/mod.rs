//! Parses the tier-edit form's text/enum fields into a
//! [`indicatrix_cut_core::ConstraintTier`]/[`TierTarget`], including the shorthand
//! index-entry notations (colon-separated arithmetic sequences and the orbit "xN"
//! fold-count suffix).

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, TierTarget};

/// Parses just the angle text an inline tier-list cell commits (`inline_set_angle`)
/// -- the same validation [`parse_tier_form`]'s own angle field applies (must parse
/// as a finite `f64`), pulled out so the inline cell doesn't need a whole tier form's
/// worth of other fields just to validate one number.
pub(in crate::gui::editor) fn parse_angle_only(angle: &str) -> Result<f64, String> {
    let angle_deg: f64 = angle
        .trim()
        .parse()
        .map_err(|_| format!("Angle '{}' is not a number.", angle.trim()))?;
    if !angle_deg.is_finite() {
        return Err("Angle must be a finite number.".to_string());
    }
    reject_angle_over_90(angle_deg)?;
    Ok(angle_deg)
}

/// Parses `token` as the colon-separated arithmetic sequence shorthand
/// `start:step:stop` (e.g. `"0:12:96"`) for the Indices field -- generates every
/// `start + k*step` strictly before `stop` (exclusive, the same half-open
/// convention every other "count up to N" in
/// this codebase uses), wrapped modulo `gear_teeth_abs` so a sequence that
/// runs past the gear's own tooth count still lands on real positions
/// instead of failing outright. Each generated value still goes through the
/// caller's own finite/range/duplicate checks, so a wrap that lands out of
/// range or repeats an earlier value is still caught and reported.
///
/// Returns `None` when `token` does not split into exactly three colon-
/// separated parts or any of them fails to parse as a plain `f64` (or `step`
/// is exactly zero, which would loop forever) -- the caller then falls
/// through to treating `token` as an ordinary single index, which reports
/// its own "not a number" error the normal way.
fn parse_colon_sequence(token: &str, gear_teeth_abs_f64: f64) -> Option<Vec<f64>> {
    let parts: Vec<&str> = token.split(':').collect();
    let [start, step, stop] = parts.as_slice() else {
        return None;
    };
    let start: f64 = start.trim().parse().ok()?;
    let step: f64 = step.trim().parse().ok()?;
    let stop: f64 = stop.trim().parse().ok()?;
    if step == 0.0 || !(start.is_finite() && step.is_finite() && stop.is_finite()) {
        return None;
    }
    let mut values = Vec::new();
    let mut current = start;
    // Bounded rather than a `while` run to `stop`: a pathological step (say
    // `1e-9`) must still terminate rather than hang the UI thread. No real
    // index gear has anywhere near this many teeth, so the bound is never
    // reached by a legitimate sequence.
    for _ in 0..10_000 {
        if (step > 0.0 && current >= stop) || (step < 0.0 && current <= stop) {
            break;
        }
        values.push(if gear_teeth_abs_f64 > 0.0 {
            current.rem_euclid(gear_teeth_abs_f64)
        } else {
            current
        });
        current += step;
    }
    Some(values)
}

/// Parses `suffix` as the "xN" fold-count half of the "N xM" orbit shorthand
/// (e.g. `"12 x8"`) -- the same `x`-prefixed notation the ORBIT column already
/// displays for a complete family (see `docs/manual/03-loading-and-tier-list.md`'s
/// "Orbits" section: `orbit x8`).
/// Returns the fold count when `suffix` is exactly `x`/`X` followed by one or
/// more digits naming a positive count, `None` otherwise -- the caller then
/// treats the two tokens as two ordinary (and, for the second, almost
/// certainly invalid) indices, which still reports a normal "not a number"
/// error rather than silently doing nothing.
fn parse_orbit_suffix(suffix: &str) -> Option<u32> {
    let digits = suffix.strip_prefix(['x', 'X'])?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().filter(|&n| n > 0)
}

/// Expands `base` into `fold_count` evenly spaced index-wheel positions --
/// `base + k * (gear_teeth_abs / fold_count)` for `k` in `0..fold_count`, wrapped
/// modulo `gear_teeth_abs`. The other half of [`parse_orbit_suffix`]'s "12 x8"
/// shorthand. A `fold_count` that does not
/// evenly divide the gear still produces values (landing on fractional
/// teeth) rather than being rejected here -- the caller's own non-integral-
/// index warning is what flags that, exactly as it would for a hand-typed
/// fractional index.
fn expand_orbit_shorthand(base: f64, fold_count: u32, gear_teeth_abs_f64: f64) -> Vec<f64> {
    if gear_teeth_abs_f64 <= 0.0 {
        return vec![base];
    }
    let step = gear_teeth_abs_f64 / f64::from(fold_count);
    (0..fold_count)
        .map(|k| f64::mul_add(f64::from(k), step, base).rem_euclid(gear_teeth_abs_f64))
        .collect()
}

/// A magnitude beyond 90 degrees is never a real facet. Every angle in this app
/// is measured from the girdle plane or is the girdle itself (`0.0`), so nothing
/// legitimately authored ever exceeds a right angle either side. Left unchecked,
/// a mistyped extra digit (e.g. `410` for `41.0`) can sail through the inline
/// cell and tier form and only surface later as "Degenerate: only N vertex(es)",
/// with no hint the angle was the mistake.
fn reject_angle_over_90(angle_deg: f64) -> Result<(), String> {
    if angle_deg.abs() > 90.0 {
        return Err(format!(
            "Angle {angle_deg:.2}\u{b0} exceeds 90\u{b0} -- angles are measured from the girdle \
             plane, so no crown or pavilion facet can be steeper than that."
        ));
    }
    Ok(())
}

/// [`parse_tier_form`]'s bundled input -- the tier-edit form's five text/enum
/// fields (`angle`/`constraint_kind`/`constraint_text`/`name`/`indices`), the
/// design's own `gear_teeth_abs` needed to validate `indices` against it, and the
/// two carry-through values (`imported_meet`/`original_notes`) an existing row
/// keeps across a save (see [`parse_tier_form`]'s own doc comment for both).
/// Bundled into its own struct purely to keep [`parse_tier_form`] under clippy's
/// argument-count lint -- every one of these eight fields is still required, none
/// dropped.
pub(in crate::gui::editor) struct TierFormFields<'a> {
    /// The angle text field; must parse as a finite `f64`.
    pub(in crate::gui::editor) angle: &'a str,
    /// `EditorTierItem`'s three-way constraint-kind encoding -- see
    /// [`parse_tier_form`]'s own doc comment for the `0`/`1`/`2` mapping.
    pub(in crate::gui::editor) constraint_kind: i32,
    /// The constraint text field, meaning depending on `constraint_kind`.
    pub(in crate::gui::editor) constraint_text: &'a str,
    /// The tier's name field.
    pub(in crate::gui::editor) name: &'a str,
    /// The comma/space/semicolon-separated index list text field.
    pub(in crate::gui::editor) indices: &'a str,
    /// The design's own index-wheel tooth count, used to validate `indices`.
    pub(in crate::gui::editor) gear_teeth_abs: u32,
    /// The tier's current `imported_meet`, threaded straight through unchanged.
    pub(in crate::gui::editor) imported_meet: Option<MeetConstraint>,
    /// The tier's current `original_notes`, threaded straight through unchanged.
    pub(in crate::gui::editor) original_notes: Option<String>,
    /// Every OTHER tier's own name token (`ConstraintTier::names()`, i.e. already
    /// split on `/` -- see that method's own doc comment for the multi-name join
    /// convention), gathered by the caller with the tier being saved itself
    /// excluded. Checked case-insensitively against this form's own name so two
    /// tiers can never silently share a name --
    /// `MeetNameResolver::name_match` (`indicatrix::geometry::meet_solver::names`)
    /// binds a `MeetNamed` reference to the FIRST tier bearing a given name, so an
    /// undetected collision does not error, it just silently redirects some OTHER
    /// tier's constraint to the wrong facet.
    pub(in crate::gui::editor) other_tier_names: Vec<String>,
}

/// Parses a tier form's index field into validated index-wheel positions.
///
/// Accepts a comma/space/semicolon separated list whose tokens may each be a plain
/// number, a `start:step:stop` arithmetic sequence, or a `base xN` orbit shorthand
/// (see [`parse_colon_sequence`] and [`expand_orbit_shorthand`]
/// for each form's own rules). Every produced value is checked for finiteness, for
/// being inside the design's own gear, and for not repeating.
///
/// # Errors
///
/// A message naming the offending token, ready to show the cutter. A value produced
/// by a shorthand names the shorthand it came from as well as the value itself, so
/// a range or duplicate error is traceable back to what was actually typed.
///
/// `pub(super)` (rather than private to this module) so
/// `callbacks::tier_actions::setup_generate_step_series_callback` can parse its own
/// shared indices field with the exact same free-text convention
/// [`parse_tier_form`]'s own indices field uses, instead of a second, divergent
/// parser.
pub(in crate::gui::editor) fn parse_index_list(
    indices: &str,
    gear_teeth_abs: u32,
) -> Result<Vec<f64>, String> {
    let gear_teeth_abs_f64 = f64::from(gear_teeth_abs);
    let mut parsed_indices: Vec<f64> = Vec::new();
    let push_index =
        |value: f64, label: &str, parsed_indices: &mut Vec<f64>| -> Result<(), String> {
            if !value.is_finite() {
                return Err(format!("Index '{label}' must be a finite number."));
            }
            if value < 0.0 || value >= gear_teeth_abs_f64 {
                return Err(format!(
                    "Index '{label}' is outside this design's {gear_teeth_abs}-tooth gear."
                ));
            }
            if parsed_indices.contains(&value) {
                return Err(format!("Index '{label}' is listed more than once."));
            }
            parsed_indices.push(value);
            Ok(())
        };

    let raw_tokens: Vec<&str> = indices
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut i = 0;
    while i < raw_tokens.len() {
        let token = raw_tokens[i];
        // "0:12:96" -- a colon-separated arithmetic sequence.
        if let Some(values) = parse_colon_sequence(token, gear_teeth_abs_f64) {
            for value in values {
                push_index(
                    value,
                    &format!("{value} (from '{token}')"),
                    &mut parsed_indices,
                )?;
            }
            i += 1;
            continue;
        }
        // "12 x8" -- a base index followed by an "xN" fold-count token.
        if let Ok(base) = token.parse::<f64>()
            && let Some(next) = raw_tokens.get(i + 1)
            && let Some(fold_count) = parse_orbit_suffix(next)
        {
            let shorthand = format!("{token} {next}");
            for value in expand_orbit_shorthand(base, fold_count, gear_teeth_abs_f64) {
                push_index(
                    value,
                    &format!("{value} (from '{shorthand}')"),
                    &mut parsed_indices,
                )?;
            }
            i += 2;
            continue;
        }
        let value: f64 = token
            .parse()
            .map_err(|_| format!("Index '{token}' is not a number."))?;
        push_index(value, token, &mut parsed_indices)?;
        i += 1;
    }
    Ok(parsed_indices)
}

/// Parses the tier-edit form's five text/enum fields into a [`ConstraintTier`]. Pure
/// and unit tested directly (see this module's tests) rather than only exercised
/// through the Slint callback -- every validation an editor's numeric text field
/// needs (not a number, not finite) surfaces as a plain `Err` message shown via a
/// toast, never a panic on bad user input.
///
/// There is deliberately no `mast` field -- see `indicatrix_cut_core::design`'s
/// module docs: a mast is [`indicatrix_cut_core::Design::solve`]'s output, never
/// authored state. The form does not offer a text field for it at all.
/// `constraint_kind` is `EditorTierItem`'s own three-way encoding (`app.slint`'s
/// `editor_save_tier` callback), the inverse of `super::state`'s
/// `constraint_kind_and_text`:
/// - `0` -- [`MeetConstraint::MeetExisting`] (`constraint_text` ignored).
/// - `1` -- [`MeetConstraint::MeetNamed`], `constraint_text` a comma-separated name
///   list.
/// - `2` -- [`MeetConstraint::ScaleReference`], `constraint_text` a single real
///   number (the authored dimension itself).
/// - `3`/`4`/`5` -- an authored [`crate::gui::editor::state`] target ("cut to
///   depth"/"girdle thickness"/"table width" -- see [`parse_tier_target`], this
///   function's cutter-facing counterpart). The returned
///   [`ConstraintTier::constraint`] is a `ScaleReference(0.0)` PLACEHOLDER for
///   these three kinds -- `indicatrix_cut_core::design::targets`'s module docs
///   document that `Design::resolved_meet_tier_inputs` always overwrites it
///   with the real resolved mast before solving, so this function itself never
///   needs (and cannot, without a `Design` to bisect against) compute the real
///   value. The caller (`callbacks::tier_actions::setup_save_tier_callback`)
///   calls [`parse_tier_target`] separately, with the same `constraint_kind`/
///   `constraint_text`, to get the actual `TierTarget` for its own
///   `Edit::SetTierTarget`.
/// - anything else -- rejected as an `Err`, never silently coerced to a default
///   constraint kind.
///
/// Indices split on comma/space/semicolon, matching
/// `indicatrix_vault::local::reconstruct_asc_schedule`'s own splitting convention for
/// the same kind of field, so a value copied from that reconstruction's indices
/// column round-trips straight back in. Two shorthand forms are recognized before
/// that plain split-and-parse: `"start:step:stop"` (a colon-separated arithmetic
/// sequence, exclusive of `stop`, see [`parse_colon_sequence`]) and `"base xN"`
/// (a base index followed by an `xN` fold count, expanded the same
/// way the ORBIT column's own `orbit x8` label already describes a complete family,
/// see [`parse_orbit_suffix`]/[`expand_orbit_shorthand`]); either form's generated
/// values are wrapped modulo the gear and fed through the exact same per-value
/// validation below as a hand-typed index. Each parsed value is then validated
/// against `gear_teeth_abs` (the design's own index-wheel tooth count, from
/// [`indicatrix_cut_core::design::tier::ScheduleMeta::gear_teeth_abs`]): it must be
/// finite, within `0.0..gear_teeth_abs as f64` (an index the gear physically has no
/// tooth for is never silently accepted), and not a repeat of an earlier value in the
/// same list (a duplicate names the same facet occurrence twice, which is never a
/// meaningful tier). The first entry that fails any of these is reported by its own
/// text, matching every other field's own "name the offending value" convention here.
/// A non-integral value (from either a hand-typed fraction or a fold count that does
/// not evenly divide the gear) is still ACCEPTED here -- warning about it without
/// rejecting it is the caller's job (`callbacks::tier_actions::setup_save_tier_callback`),
/// since it is real GemCad-file behavior (`indicatrix_formats::asc`'s own module docs
/// measure roughly 0.2% of real index tokens as fractional), not necessarily a mistake.
///
/// `imported_meet` is threaded straight through to the built [`ConstraintTier`]
/// unchanged -- this function never inspects or clears it. The caller
/// (`super::callbacks::setup_save_tier_callback`) passes the CURRENT tier's own
/// `imported_meet` when editing an existing row (`index >= 0`) so that tweaking,
/// say, just this tier's name does not silently discard the file's stated meet
/// instruction, and `None` when adding a brand-new tier (`index < 0`), which
/// has no import history to preserve at all. The one-click "Adopt" action
/// (`super::callbacks::setup_adopt_meet_callback`) is the only thing that ever
/// actually CONSULTS this field to build a new constraint from it.
pub(in crate::gui::editor) fn parse_tier_form(
    form: TierFormFields<'_>,
) -> Result<ConstraintTier, String> {
    let TierFormFields {
        angle,
        constraint_kind,
        constraint_text,
        name,
        indices,
        gear_teeth_abs,
        imported_meet,
        original_notes,
        other_tier_names,
    } = form;
    let angle_deg: f64 = angle
        .trim()
        .parse()
        .map_err(|_| format!("Angle '{}' is not a number.", angle.trim()))?;
    if !angle_deg.is_finite() {
        return Err("Angle must be a finite number.".to_string());
    }
    reject_angle_over_90(angle_deg)?;

    let name = name.trim();
    // Reject a name (or, for a `/`-joined multi-name tier, any ONE of its names)
    // another tier already holds -- see `TierFormFields::other_tier_names`'s own
    // doc comment for why an undetected collision is worse than merely confusing.
    for token in name.split('/').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(existing) = other_tier_names
            .iter()
            .find(|other| other.eq_ignore_ascii_case(token))
        {
            return Err(format!(
                "Another tier is already named '{existing}' -- facet names must be unique so \
                 meet constraints resolve to the right tier."
            ));
        }
    }

    let parsed_indices = parse_index_list(indices, gear_teeth_abs)?;

    let constraint = match constraint_kind {
        0 => MeetConstraint::MeetExisting,
        1 => {
            let names: Vec<String> = constraint_text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if names.is_empty() {
                return Err("\"Meet named\" needs at least one facet name.".to_string());
            }
            MeetConstraint::MeetNamed(names)
        }
        2 => {
            let value: f64 = constraint_text.trim().parse().map_err(|_| {
                format!(
                    "Scale reference '{}' is not a number.",
                    constraint_text.trim()
                )
            })?;
            if !value.is_finite() {
                return Err("Scale reference must be a finite number.".to_string());
            }
            MeetConstraint::ScaleReference(value)
        }
        // 3-5: an authored `TierTarget` ("cut to depth"/"girdle thickness"/
        // "table width") -- see this function's own doc comment and
        // `parse_tier_target` below. The tier's own `constraint` is a
        // `ScaleReference` placeholder that `Design::resolved_meet_tier_inputs`
        // always overwrites before solving; the real millimetre value is
        // validated and returned separately, by `parse_tier_target`, since this
        // function only ever returns a `ConstraintTier`.
        3..=5 => MeetConstraint::ScaleReference(0.0),
        other => return Err(format!("Unknown constraint kind {other}.")),
    };

    Ok(ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: parsed_indices,
        constraint,
        imported_meet,
        original_notes,
        detached: Vec::new(),
    })
}

/// Parses the tier form's Meets combo into a [`TierTarget`] when
/// `constraint_kind` names one of the three target kinds -- `3`
/// [`TierTarget::DepthMm`], `4` [`TierTarget::GirdleThicknessMm`], `5`
/// [`TierTarget::TableWidthMm`] -- the
/// counterpart to [`parse_tier_form`]'s own `0`/`1`/`2` handling for a plain
/// [`MeetConstraint`]. `constraint_text` is the SAME single numeric field
/// [`parse_tier_form`] reads for kind `2` (`ScaleReference`); here it is
/// always a millimetre figure, matching the unit every [`TierTarget`] variant
/// itself declares.
///
/// `Ok(None)` for every other kind (`0`/`1`/`2`, or anything
/// [`parse_tier_form`] would itself reject) -- "no target," not an error, so a
/// plain meet/scale-reference save that never authored a target is a no-op
/// through `callbacks::tier_actions::setup_save_tier_callback`'s own
/// target-clearing logic, which calls this unconditionally alongside
/// [`parse_tier_form`].
///
/// # Errors
///
/// The same "name the offending value" wording [`parse_tier_form`]'s own
/// `ScaleReference` arm uses, prefixed with the target's own cutter-facing
/// label ("Depth"/"Girdle thickness"/"Table width") instead of "Scale
/// reference" -- `callbacks::tier_actions::tier_form_error_field` recognizes
/// all three prefixes and classifies them the same `"constraint"` field a
/// bad Meets value always has. A non-finite or non-positive value is
/// rejected the same way a non-positive preform dimension is
/// ([`super::preform_and_new_design::parse_preform_form`]): a depth, girdle
/// thickness, or table width of zero or less is never a real target.
pub(in crate::gui::editor) fn parse_tier_target(
    constraint_kind: i32,
    constraint_text: &str,
) -> Result<Option<TierTarget>, String> {
    let (label, build): (&str, fn(f64) -> TierTarget) = match constraint_kind {
        3 => ("Depth", TierTarget::DepthMm as fn(f64) -> TierTarget),
        4 => (
            "Girdle thickness",
            TierTarget::GirdleThicknessMm as fn(f64) -> TierTarget,
        ),
        5 => (
            "Table width",
            TierTarget::TableWidthMm as fn(f64) -> TierTarget,
        ),
        _ => return Ok(None),
    };
    let text = constraint_text.trim();
    let value: f64 = text
        .parse()
        .map_err(|_| format!("{label} '{text}' is not a number."))?;
    if !value.is_finite() {
        return Err(format!("{label} must be a finite number."));
    }
    if value <= 0.0 {
        return Err(format!("{label} must be a positive number."));
    }
    Ok(Some(build(value)))
}

#[cfg(test)]
mod tests;
