//! Resolving a [`Design`](indicatrix_cut_core::Design) from external sources -- a catalogue entry's
//! full record (`super::callbacks::setup_load_selected_callback`) -- and parsing the
//! tier/preform edit forms into `indicatrix-cut-core` edit payloads. See this group's own `mod.rs`
//! doc comment.

use super::state::material_name_from_index;
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions};
use indicatrix_cut_core::{
    ConstraintTier, Design, FreshDesignSpec, MaterialSelection, PreformSpec, TierTarget,
    load_paired,
    native::{LEGACY_NATIVE_EXTENSION_SUFFIX, NATIVE_EXTENSION_SUFFIX},
};
use indicatrix_vault::model::file::AttachedFile;
use tracing::warn;

/// A preform sized to bound a design that may already fully specify its own closed
/// shape (a real `.asc` file's tiers, or the placeholder reconstruction in
/// [`design_from_full_record`]): generously large relative to the "mast order 1" scale
/// every facet offset in this codebase uses (see `indicatrix_cut_core::preform`'s module doc
/// comment), so this backstop rough does not itself clip a single facet of an
/// already-complete design -- only a schedule that's missing a closing facet in some
/// direction would ever actually touch this preform's own walls.
///
/// `length_over_width` is read from the catalogue's own recorded `lw_ratio` when it
/// parses as a positive finite number, so an oval design's preform isn't needlessly
/// round; `1.0` (round/square) otherwise, matching `indicatrix_cut_core::PreformSpec`'s own default
/// shape assumption.
fn default_preform_for_schedule(
    schedule: &indicatrix_formats::asc::AscSchedule,
    lw_ratio: Option<&str>,
) -> PreformSpec {
    let length_over_width = lw_ratio
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| v.is_finite() && *v > 0.0)
        .unwrap_or(1.0);
    PreformSpec::cylinder_for_schedule(schedule, 3.0, length_over_width, 3.0)
}

/// [`design_from_full_record`]'s full result -- see that function's own doc comment.
pub(super) struct LoadedDesign {
    pub(super) design: Design,
    /// `true` iff no real attached `.asc` was found and `design`'s schedule instead
    /// came from the angle-table placeholder reconstruction (every mast `0.0`) --
    /// see [`design_from_full_record`]'s doc comment.
    pub(super) used_placeholder: bool,
    /// The real attached `.asc`'s own bare file name, `None` on the placeholder
    /// path. Reconstructed text was never a real `.asc` this catalogue entry
    /// ships, so there is nothing there worth [`indicatrix_cut_core::save_paired`]
    /// preserving verbatim. Fed to `super::state::EditorState::asc_filename` by
    /// `super::callbacks::setup_load_selected_callback`.
    pub(super) asc_filename: Option<String>,
    /// The real attached `.asc`'s own exact original text, `None` on the
    /// placeholder path for the identical reason. Fed to
    /// `super::state::EditorState::original_asc_text`.
    pub(super) original_asc_text: Option<String>,
}

/// Builds a [`LoadedDesign`] from a real `.asc` file's own text, always along the
/// "real schedule" outcome (`used_placeholder: false` -- there is no angle-table
/// fallback here, since that reconstruction needs a full catalogue record's columns
/// this function never sees).
///
/// Factored out of [`design_from_full_record`]'s own real-attachment branch so
/// `gui::editor::callbacks::tier_actions::setup_load_selected_callback`'s remote
/// branch can build the identical [`Design`] from
/// `gui::library::remote::RemoteDesignSource::asc_text` -- the exact bytes/text a
/// locally-attached `.asc` file would carry (see that struct's own doc comment) --
/// without needing a `FullDiagramRecord` at all (a remote fetch never has one; only
/// the bare file name and text cross the wire).
///
/// # Errors
///
/// Returns `Err` (the parse failure's own message) when `text` does not parse as a
/// `.asc` cutting schedule at all.
pub(super) fn design_from_asc_text(
    file_name: &str,
    text: &str,
    lw_ratio: Option<&str>,
) -> Result<LoadedDesign, String> {
    let schedule = indicatrix_formats::asc::parse_asc(text).map_err(|e| e.to_string())?;
    let preform = default_preform_for_schedule(&schedule, lw_ratio);
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: false,
        asc_filename: Some(file_name.to_string()),
        original_asc_text: Some(text.to_string()),
    })
}

/// A sibling native sidecar (current `.indicatrix.toml`, or the legacy
/// `.gemcut.toml`) among `files`, if the catalogue entry has one attached alongside
/// its `.asc` -- see [`design_from_asc_and_native`]'s own doc comment for why that
/// combination is worth preferring over the bare `.asc` alone.
///
/// Nothing in this app's importer attaches a second file today (`gui::library::
/// local::import` collects only `.asc`), so this currently only ever matches when a
/// FUTURE import (or a remote source) starts doing so -- see this crate's own
/// handoff notes for that other half.
fn native_sidecar_attachment(files: &[AttachedFile]) -> Option<&AttachedFile> {
    files.iter().find(|f| {
        let lower = f.name.to_lowercase();
        lower.ends_with(NATIVE_EXTENSION_SUFFIX) || lower.ends_with(LEGACY_NATIVE_EXTENSION_SUFFIX)
    })
}

/// [`design_from_full_record`]'s preferred path when a catalogue entry carries BOTH
/// a real `.asc` attachment and a native sidecar paired with it: [`load_paired`]
/// restores the sidecar's own preform/material/girdle-diameter unconditionally, and
/// (when the fingerprint still matches) every tier's authored meet constraint and
/// detached-facet set too -- all of which a bare `.asc` re-import otherwise
/// reconstructs as fresh `ScaleReference` anchors with no detached facets at all
/// (see `indicatrix_cut_core::native`'s own module doc comment for the full list of
/// what only the sidecar carries).
///
/// Falls back to [`design_from_asc_text`] (logging a warning) when the sidecar text
/// itself fails to parse -- a real `.asc` schedule is still strictly better than
/// refusing to load the design at all.
fn design_from_asc_and_native(
    asc_name: &str,
    asc_text: &str,
    native_text: &str,
    lw_ratio: Option<&str>,
    entry_id: i64,
) -> Result<LoadedDesign, String> {
    match load_paired(asc_text, native_text, false) {
        Ok(result) => Ok(LoadedDesign {
            design: result.design,
            used_placeholder: false,
            asc_filename: Some(asc_name.to_string()),
            original_asc_text: Some(asc_text.to_string()),
        }),
        Err(e) => {
            warn!(
                "Native sidecar paired with '{asc_name}' on diagram #{entry_id} failed to \
                 load ({e}); falling back to the plain .asc schedule."
            );
            design_from_asc_text(asc_name, asc_text, lw_ratio)
        }
    }
}

/// Builds the design that should be loaded for one catalogue entry's full record.
///
/// Prefers a real attached `.asc` file's own schedule -- its mast values are the
/// file's actual recorded depths -- over `indicatrix_vault::local::reconstruct_asc_schedule`'s
/// placeholder reconstruction from the angle/index table alone, which (per that
/// function's own doc comment) has no depth data to work with at all and fills every
/// tier's `mast` with `0.0`. That fallback is still offered rather than refused
/// outright -- it is the SAME reconstruction `gui::library::local::export::setup_export_asc_callback`
/// already exports today with the same caveat -- but [`LoadedDesign::used_placeholder`]
/// tells the caller which path was taken, so it can warn exactly the way that existing
/// export path already does, rather than silently handing the user a schedule whose
/// masts are all zero.
///
/// When a native sidecar is attached alongside the `.asc` (see
/// [`native_sidecar_attachment`]), [`design_from_asc_and_native`] is preferred over
/// the plain `.asc` path so an imported design does not lose the sidecar-only fields
/// a re-import would otherwise silently discard.
///
/// # Errors
///
/// Returns `Err` (a human-readable message) only when there is no attached `.asc` AND
/// no angle-settings table to reconstruct from at all -- nothing in this diagram's
/// record describes a cutting schedule.
pub(super) fn design_from_full_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Result<LoadedDesign, String> {
    if let Some(attached) = full
        .attached_files
        .iter()
        .find(|f| f.name.to_lowercase().ends_with(".asc"))
    {
        let text = String::from_utf8_lossy(&attached.content);
        let loaded = native_sidecar_attachment(&full.attached_files).map_or_else(
            || design_from_asc_text(&attached.name, &text, full.lw_ratio.as_deref()),
            |sidecar| {
                let native_text = String::from_utf8_lossy(&sidecar.content);
                design_from_asc_and_native(
                    &attached.name,
                    &text,
                    &native_text,
                    full.lw_ratio.as_deref(),
                    full.entry_id,
                )
            },
        );
        match loaded {
            Ok(loaded) => return Ok(loaded),
            Err(e) => warn!(
                "Attached .asc '{}' on diagram #{} failed to parse ({e}); falling back to the \
                 angle-table reconstruction.",
                attached.name, full.entry_id
            ),
        }
    }

    let schedule = indicatrix_vault::local::reconstruct_asc_schedule(
        &full.title,
        full.refractive_index.as_deref(),
        full.index_gear.as_deref(),
        &full.angle_settings,
    )
    .ok_or_else(|| "This diagram has no cutting-schedule data to load.".to_string())?;
    let preform = default_preform_for_schedule(&schedule, full.lw_ratio.as_deref());
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: true,
        asc_filename: None,
        original_asc_text: None,
    })
}

/// Builds Deep Solve's external verification targets from a catalogue entry's own
/// printed proportion columns -- see `deep_solve`'s module doc comment ("External
/// verification: printed proportions" in `solve_meet_points_verified`'s own doc
/// comment is the underlying reasoning). `volume`/`lw_ratio`/`cw_ratio`/`pw_ratio`/
/// `hw_ratio` map straight onto `ExternalProportions`' `vol_w3`/`lw`/`cw`/`pw`/`hw`
/// -- the exact same column-to-field mapping
/// `crates/indicatrix/examples/meet_solver_validation.rs` uses when it builds the same
/// struct from a `diagram_details` row.
///
/// Returns `None` when none of the five columns hold a usable positive, finite
/// number: a design with nothing printed on it at all gives the search no external
/// signal to score against at all, so Deep Solve must be disabled rather than run
/// against a target that can never accept or reject anything (see
/// `super::view::deep_solve_hint`).
pub(super) fn external_proportions_from_full_record(
    full: &indicatrix_vault::model::entry::FullDiagramRecord,
) -> Option<ExternalProportions> {
    fn parse(value: Option<&String>) -> Option<f64> {
        value
            .map(String::as_str)?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v > 0.0)
    }
    let props = ExternalProportions {
        vol_w3: parse(full.volume.as_ref()),
        lw: parse(full.lw_ratio.as_ref()),
        cw: parse(full.cw_ratio.as_ref()),
        pw: parse(full.pw_ratio.as_ref()),
        hw: parse(full.hw_ratio.as_ref()),
    };
    (props.vol_w3.is_some()
        || props.lw.is_some()
        || props.cw.is_some()
        || props.pw.is_some()
        || props.hw.is_some())
    .then_some(props)
}

/// Parses just the angle text an inline tier-list cell commits (`inline_set_angle`)
/// -- the same validation [`parse_tier_form`]'s own angle field applies (must parse
/// as a finite `f64`), pulled out so the inline cell doesn't need a whole tier form's
/// worth of other fields just to validate one number.
pub(super) fn parse_angle_only(angle: &str) -> Result<f64, String> {
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
pub(super) struct TierFormFields<'a> {
    /// The angle text field; must parse as a finite `f64`.
    pub(super) angle: &'a str,
    /// `EditorTierItem`'s three-way constraint-kind encoding -- see
    /// [`parse_tier_form`]'s own doc comment for the `0`/`1`/`2` mapping.
    pub(super) constraint_kind: i32,
    /// The constraint text field, meaning depending on `constraint_kind`.
    pub(super) constraint_text: &'a str,
    /// The tier's name field.
    pub(super) name: &'a str,
    /// The comma/space/semicolon-separated index list text field.
    pub(super) indices: &'a str,
    /// The design's own index-wheel tooth count, used to validate `indices`.
    pub(super) gear_teeth_abs: u32,
    /// The tier's current `imported_meet`, threaded straight through unchanged.
    pub(super) imported_meet: Option<MeetConstraint>,
    /// The tier's current `original_notes`, threaded straight through unchanged.
    pub(super) original_notes: Option<String>,
    /// Every OTHER tier's own name token (`ConstraintTier::names()`, i.e. already
    /// split on `/` -- see that method's own doc comment for the multi-name join
    /// convention), gathered by the caller with the tier being saved itself
    /// excluded. Checked case-insensitively against this form's own name so two
    /// tiers can never silently share a name --
    /// `MeetNameResolver::name_match` (`indicatrix::geometry::meet_solver::names`)
    /// binds a `MeetNamed` reference to the FIRST tier bearing a given name, so an
    /// undetected collision does not error, it just silently redirects some OTHER
    /// tier's constraint to the wrong facet.
    pub(super) other_tier_names: Vec<String>,
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
pub(super) fn parse_index_list(indices: &str, gear_teeth_abs: u32) -> Result<Vec<f64>, String> {
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
/// module docs: a mast is [`Design::solve`]'s output, never authored state.
/// The form does not offer a text field for it at all. `constraint_kind` is
/// `EditorTierItem`'s own three-way encoding (`app.slint`'s `editor_save_tier`
/// callback), the inverse of `super::state`'s `constraint_kind_and_text`:
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
pub(super) fn parse_tier_form(form: TierFormFields<'_>) -> Result<ConstraintTier, String> {
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
/// ([`parse_preform_form`]): a depth, girdle thickness, or table width of
/// zero or less is never a real target.
pub(super) fn parse_tier_target(
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

/// Parses the preform form's shape choice and three dimension fields into a
/// [`PreformSpec`]. Pure and unit tested directly, same reasoning as
/// [`parse_tier_form`]. `cylinder_sides` is passed in (rather than read from a
/// `Design` inside this function) so it stays decoupled from `indicatrix-cut-core` state and easy
/// to test in isolation. Every caller (`callbacks::tier_actions`) always passes
/// a FIXED side count, independent of the active design's index gear. Unlike
/// [`indicatrix_cut_core::PreformSpec::cylinder_for_schedule`]'s "fold count
/// matches the index gear" reasoning (correct only for a preform reconstructed
/// ONCE from an already-authored schedule), a reapply after gear change can
/// leave a stale side count behind if the design is queried for it.
pub(super) fn parse_preform_form(
    shape_index: i32,
    half_width: &str,
    length_over_width: &str,
    depth: &str,
    cylinder_sides: usize,
) -> Result<PreformSpec, String> {
    let half_width: f64 = half_width
        .trim()
        .parse()
        .map_err(|_| format!("Half-width '{}' is not a number.", half_width.trim()))?;
    let length_over_width: f64 = length_over_width.trim().parse().map_err(|_| {
        format!(
            "Length/width '{}' is not a number.",
            length_over_width.trim()
        )
    })?;
    let depth: f64 = depth
        .trim()
        .parse()
        .map_err(|_| format!("Depth '{}' is not a number.", depth.trim()))?;
    if !(half_width.is_finite() && length_over_width.is_finite() && depth.is_finite())
        || half_width <= 0.0
        || length_over_width <= 0.0
        || depth <= 0.0
    {
        return Err("Preform dimensions must be positive, finite numbers.".to_string());
    }

    Ok(if shape_index == 0 {
        PreformSpec::block(half_width, length_over_width, depth)
    } else {
        PreformSpec::cylinder(cylinder_sides, half_width, length_over_width, depth)
    })
}

/// The New Design dialog's own symmetry-order/mirror/material fields,
/// combined with an already-resolved gear tooth count and [`PreformSpec`]
/// (the caller -- `super::callbacks::setup_new_design_create_callback` --
/// resolves those two first, via [`gear_choice_to_teeth`] then
/// [`parse_preform_form`], since a cylinder preform's fold count needs the
/// gear already resolved; kept as separate calls rather than folded into one
/// wide function here to stay under clippy's `too_many_arguments`, matching
/// `gui::tilt_hover_preview`'s own documented preference for a handful of
/// small calls over one wide parameter list), into a [`FreshDesignSpec`] for
/// `indicatrix_cut_core::Design::fresh_from_spec`. `symmetry_order_text` must be a
/// positive whole number; `material_index` reuses
/// [`material_name_from_index`] (the fixed built-in list -- see that
/// function's own doc comment; a brand-new design's starting material is
/// just that, a starting point the design settings panel can change to a
/// custom catalogue material afterward).
pub(super) fn parse_new_design_form(
    gear_teeth: i32,
    preform: PreformSpec,
    symmetry_order_text: &str,
    mirror: bool,
    material_index: i32,
) -> Result<FreshDesignSpec, String> {
    let symmetry_order: u32 = symmetry_order_text.trim().parse().map_err(|_| {
        format!(
            "Symmetry order '{}' is not a whole number.",
            symmetry_order_text.trim()
        )
    })?;
    if symmetry_order < 1 {
        return Err("Symmetry order must be a positive whole number.".to_string());
    }
    Ok(FreshDesignSpec {
        gear_teeth,
        symmetry_order,
        mirror,
        material: MaterialSelection {
            name: material_name_from_index(material_index),
            specific_gravity_override: None,
            refractive_index_override: None,
        },
        preform,
    })
}

/// How far a built-in material's own `n_D` may drift from the RI a design's
/// exported `.asc` currently reports before [`ri_override_to_preserve`] steps in
/// to hold the export steady -- see that function's own doc comment.
pub(in crate::gui::editor) const RI_PRESERVE_TOLERANCE: f64 = 0.01;

/// `Design::effective_refractive_index` derives the EXPORTED RI from the selected
/// material's own built-in `n_D` whenever no override is set -- see that method's
/// own doc comment. That is exactly right for a design that never had a
/// recorded RI of its own, but wrong for one that did: picking "Diamond" (`n_D`
/// 1.5442) for a design whose exported schedule currently reads `I 1.54` would
/// otherwise silently rewrite that line the moment a material is applied, even
/// though nothing about the facet geometry or the cutter's typed figure changed.
///
/// Returns `Some(original_ri)` -- to be pinned into
/// [`MaterialSelection::refractive_index_override`] -- when `name` resolves to a
/// built-in whose own `n_D` differs from `original_ri` by more than
/// [`RI_PRESERVE_TOLERANCE`], so the export keeps reading `original_ri` exactly
/// as before. Returns `None` (no override needed) when `name` is not a built-in
/// at all, or when the two already agree closely enough that pinning would be
/// pure noise.
///
/// Shared by [`material_selection_for_accepted_suggestion`] (the catalogue-load
/// suggestion, which already applied this reasoning under a different name) and
/// `callbacks::tier_actions::setup_apply_design_material_callback` (for a plain
/// in-editor material pick with no typed RI override of its own).
#[must_use]
pub(in crate::gui::editor) fn ri_override_to_preserve(name: &str, original_ri: f64) -> Option<f64> {
    let built_in_ri = indicatrix_cut_core::built_in_refractive_index(name)?;
    ((built_in_ri - original_ri).abs() > RI_PRESERVE_TOLERANCE).then_some(original_ri)
}

/// What accepting the catalogue-load material suggestion applies -- see
/// `super::callbacks::setup_load_selected_callback`'s own doc comment for when
/// the suggestion (`super::material_lookup::nearest_built_in_material`) is
/// offered in the first place. `current`'s `specific_gravity_override` is
/// carried through unchanged -- this only ever touches
/// `name`/`refractive_index_override`; see [`ri_override_to_preserve`] for why
/// the override is pinned at all.
pub(super) fn material_selection_for_accepted_suggestion(
    name: &str,
    schedule_ri: f64,
    current: &MaterialSelection,
) -> MaterialSelection {
    MaterialSelection {
        name: Some(name.to_string()),
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override: ri_override_to_preserve(name, schedule_ri),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::PreformShape;
    use indicatrix_vault::model::entry::FullDiagramRecord;

    /// Builds a [`TierFormFields`] for the tests below that only care about the
    /// five text/enum fields -- `gear_teeth_abs` fixed at 96 (every test index
    /// here is well within that range) and no imported meet/original notes to
    /// carry through, keeping each call site down to the fields it actually
    /// varies.
    fn tier_form<'a>(
        angle: &'a str,
        constraint_kind: i32,
        constraint_text: &'a str,
        name: &'a str,
        indices: &'a str,
    ) -> TierFormFields<'a> {
        TierFormFields {
            angle,
            constraint_kind,
            constraint_text,
            name,
            indices,
            gear_teeth_abs: 96,
            imported_meet: None,
            original_notes: None,
            other_tier_names: Vec::new(),
        }
    }

    #[test]
    fn parse_angle_only_accepts_a_well_formed_number() {
        assert_eq!(parse_angle_only(" -41.0 ").unwrap(), -41.0);
        assert_eq!(parse_angle_only("0").unwrap(), 0.0);
    }

    #[test]
    fn parse_angle_only_rejects_non_numeric_and_non_finite_text() {
        assert!(parse_angle_only("not-a-number").is_err());
        assert!(parse_angle_only("NaN").is_err());
        assert!(parse_angle_only("inf").is_err());
        assert!(parse_angle_only("").is_err());
    }

    #[test]
    fn parse_angle_only_accepts_exactly_90_either_sign() {
        assert_eq!(parse_angle_only("90").unwrap(), 90.0);
        assert_eq!(parse_angle_only("-90").unwrap(), -90.0);
    }

    #[test]
    fn parse_angle_only_rejects_a_magnitude_over_90() {
        let err = parse_angle_only("90.01").unwrap_err();
        assert!(err.contains("90"));
        let err = parse_angle_only("-410").unwrap_err();
        assert!(err.contains("90"));
    }

    #[test]
    fn parse_tier_form_accepts_a_well_formed_scale_reference_row() {
        let tier = parse_tier_form(TierFormFields {
            angle: "-41.0",
            constraint_kind: 2,
            constraint_text: "0.65",
            name: " P1 ",
            indices: "0, 24, 48, 72",
            gear_teeth_abs: 96,
            imported_meet: None,
            original_notes: None,
            other_tier_names: Vec::new(),
        })
        .unwrap();
        assert_eq!(tier.angle_deg, -41.0);
        assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.65));
        assert_eq!(tier.name, "P1");
        assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
    }

    #[test]
    fn parse_tier_form_carries_the_files_original_note_through_an_edit() {
        let tier = parse_tier_form(TierFormFields {
            angle: "-41.0",
            constraint_kind: 2,
            constraint_text: "0.65",
            name: "P1",
            indices: "0, 24",
            gear_teeth_abs: 96,
            imported_meet: None,
            original_notes: Some("Cut to TCP".to_string()),
            other_tier_names: Vec::new(),
        })
        .unwrap();
        assert_eq!(tier.original_notes.as_deref(), Some("Cut to TCP"));
    }

    #[test]
    fn parse_tier_form_accepts_an_empty_index_list() {
        let tier = parse_tier_form(tier_form("0.0", 2, "0.32", "T", "")).unwrap();
        assert_eq!(tier.indices, [] as [f64; 0]);
    }

    #[test]
    fn parse_tier_form_splits_on_comma_space_and_semicolon() {
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "1, 2 3;4")).unwrap();
        assert_eq!(tier.indices, vec![1.0, 2.0, 3.0, 4.0]);
    }

    // --- Shorthand index entry ---

    #[test]
    fn parse_tier_form_expands_a_colon_sequence() {
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0:12:96")).unwrap();
        assert_eq!(
            tier.indices,
            vec![0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0]
        );
    }

    #[test]
    fn parse_tier_form_colon_sequence_can_be_combined_with_other_tokens() {
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0:24:96, 6")).unwrap();
        assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0, 6.0]);
    }

    #[test]
    fn parse_tier_form_colon_sequence_out_of_range_is_still_reported() {
        // Step 200 on a 96-tooth gear wraps every generated value modulo 96,
        // so this never actually goes out of range -- confirm the wrap lands
        // on real teeth rather than raw (unwrapped) 200/400/etc.
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0:200:500")).unwrap();
        assert!(tier.indices.iter().all(|&v| (0.0..96.0).contains(&v)));
    }

    #[test]
    fn parse_tier_form_expands_the_orbit_shorthand() {
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "12 x8")).unwrap();
        assert_eq!(
            tier.indices,
            vec![12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0, 0.0]
        );
    }

    #[test]
    fn parse_tier_form_orbit_shorthand_accepts_uppercase_x() {
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "0 X4")).unwrap();
        assert_eq!(tier.indices, vec![0.0, 24.0, 48.0, 72.0]);
    }

    #[test]
    fn parse_tier_form_orbit_shorthand_can_be_combined_with_other_tokens() {
        let tier = parse_tier_form(tier_form("10", 2, "0.5", "G", "6, 0 x4")).unwrap();
        assert_eq!(tier.indices, vec![6.0, 0.0, 24.0, 48.0, 72.0]);
    }

    #[test]
    fn parse_tier_form_orbit_shorthand_duplicate_with_a_prior_token_is_rejected() {
        let err = parse_tier_form(tier_form("10", 2, "0.5", "G", "0, 0 x4")).unwrap_err();
        assert!(err.contains("more than once"));
    }

    #[test]
    fn parse_tier_form_a_lone_x_token_is_not_mistaken_for_shorthand() {
        // No preceding numeric token -- "x8" alone must fall through to the
        // ordinary "not a number" error, not panic or silently vanish.
        let err = parse_tier_form(tier_form("10", 2, "0.5", "G", "x8")).unwrap_err();
        assert!(err.contains("Index 'x8' is not a number."));
    }

    #[test]
    fn parse_tier_form_negative_index_is_not_mistaken_for_a_colon_sequence() {
        let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "-1")).unwrap_err();
        assert!(err.contains("-1"));
    }

    #[test]
    fn parse_tier_form_rejects_a_non_numeric_angle() {
        let err = parse_tier_form(tier_form("not-a-number", 2, "0.5", "T", "")).unwrap_err();
        assert!(err.contains("Angle"));
    }

    #[test]
    fn parse_tier_form_rejects_a_magnitude_over_90() {
        let err = parse_tier_form(tier_form("95.0", 2, "0.5", "T", "")).unwrap_err();
        assert!(err.contains("exceeds 90"));
    }

    #[test]
    fn parse_tier_form_accepts_exactly_90() {
        assert!(parse_tier_form(tier_form("90.0", 2, "0.5", "T", "")).is_ok());
        assert!(parse_tier_form(tier_form("-90.0", 2, "0.5", "T", "")).is_ok());
    }

    #[test]
    fn parse_tier_form_rejects_a_name_another_tier_already_holds() {
        let mut form = tier_form("30.0", 2, "0.5", "P1", "");
        form.other_tier_names = vec!["P1".to_string()];
        let err = parse_tier_form(form).unwrap_err();
        assert!(err.contains("P1"));
    }

    #[test]
    fn parse_tier_form_name_collision_check_is_case_insensitive() {
        let mut form = tier_form("30.0", 2, "0.5", "p1", "");
        form.other_tier_names = vec!["P1".to_string()];
        assert!(parse_tier_form(form).is_err());
    }

    #[test]
    fn parse_tier_form_checks_each_slash_joined_name_for_a_collision() {
        let mut form = tier_form("30.0", 2, "0.5", "P1/P2", "");
        form.other_tier_names = vec!["P2".to_string()];
        let err = parse_tier_form(form).unwrap_err();
        assert!(err.contains("P2"));
    }

    #[test]
    fn parse_tier_form_allows_a_name_no_other_tier_holds() {
        let mut form = tier_form("30.0", 2, "0.5", "P1", "");
        form.other_tier_names = vec!["G1".to_string(), "C1".to_string()];
        assert!(parse_tier_form(form).is_ok());
    }

    #[test]
    fn parse_tier_form_rejects_a_non_numeric_index() {
        let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, x, 3")).unwrap_err();
        assert!(err.contains("Index"));
    }

    #[test]
    fn parse_tier_form_rejects_non_finite_values() {
        assert!(parse_tier_form(tier_form("NaN", 2, "0.5", "T", "")).is_err());
        assert!(parse_tier_form(tier_form("0.0", 2, "inf", "T", "")).is_err());
    }

    #[test]
    fn parse_tier_form_rejects_a_non_finite_index() {
        let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, NaN, 3")).unwrap_err();
        assert!(err.contains("Index"));
        assert!(err.contains("finite"));
    }

    #[test]
    fn parse_tier_form_rejects_an_index_outside_the_gears_tooth_count() {
        let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "1, 96")).unwrap_err();
        assert!(err.contains("96"));
    }

    #[test]
    fn parse_tier_form_rejects_a_negative_index() {
        let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "-1")).unwrap_err();
        assert!(err.contains("-1"));
    }

    #[test]
    fn parse_tier_form_rejects_a_duplicate_index() {
        let err = parse_tier_form(tier_form("0.0", 2, "0.5", "T", "24, 48, 24")).unwrap_err();
        assert!(err.contains("24"));
        assert!(err.contains("more than once"));
    }

    #[test]
    fn parse_tier_form_kind_zero_is_meet_existing_and_ignores_the_text_field() {
        let tier = parse_tier_form(tier_form("30.0", 0, "this text is ignored", "C1", "")).unwrap();
        assert_eq!(tier.constraint, MeetConstraint::MeetExisting);
    }

    #[test]
    fn parse_tier_form_kind_one_splits_named_facets_on_comma() {
        let tier = parse_tier_form(tier_form("30.0", 1, "P1, P2 , G1", "C1", "")).unwrap();
        assert_eq!(
            tier.constraint,
            MeetConstraint::MeetNamed(vec!["P1".to_string(), "P2".to_string(), "G1".to_string()])
        );
    }

    #[test]
    fn parse_tier_form_kind_one_rejects_an_empty_name_list() {
        let err = parse_tier_form(tier_form("30.0", 1, "  , ", "C1", "")).unwrap_err();
        assert!(err.contains("Meet named"));
    }

    #[test]
    fn parse_tier_form_rejects_an_unknown_constraint_kind() {
        // `3`/`4`/`5` are now the three target kinds (see
        // `parse_tier_form_kinds_three_to_five_use_a_scale_reference_placeholder`
        // below) -- `6` is the first kind nothing recognizes.
        let err = parse_tier_form(tier_form("30.0", 6, "", "C1", "")).unwrap_err();
        assert!(err.contains('6'));
    }

    #[test]
    fn parse_tier_form_kinds_three_to_five_use_a_scale_reference_placeholder() {
        // The real millimetre value lives in the `TierTarget` `parse_tier_target`
        // returns, never in the `ConstraintTier` itself -- see both functions' own
        // doc comments for why `Design::resolved_meet_tier_inputs` always
        // overwrites this placeholder before solving.
        for kind in 3..=5 {
            let tier = parse_tier_form(tier_form("30.0", kind, "3.2", "T", "")).unwrap();
            assert_eq!(tier.constraint, MeetConstraint::ScaleReference(0.0));
        }
    }

    // --- parse_tier_target (depth/girdle-thickness/table-width targets) ---

    #[test]
    fn parse_tier_target_is_none_for_the_three_plain_constraint_kinds() {
        for kind in 0..=2 {
            assert_eq!(parse_tier_target(kind, "0.5").unwrap(), None);
        }
    }

    #[test]
    fn parse_tier_target_parses_a_depth_target_in_mm() {
        assert_eq!(
            parse_tier_target(3, " 3.20 ").unwrap(),
            Some(TierTarget::DepthMm(3.20))
        );
    }

    #[test]
    fn parse_tier_target_parses_a_girdle_thickness_target_in_mm() {
        assert_eq!(
            parse_tier_target(4, "0.25").unwrap(),
            Some(TierTarget::GirdleThicknessMm(0.25))
        );
    }

    #[test]
    fn parse_tier_target_parses_a_table_width_target_in_mm() {
        assert_eq!(
            parse_tier_target(5, "4.10").unwrap(),
            Some(TierTarget::TableWidthMm(4.10))
        );
    }

    #[test]
    fn parse_tier_target_rejects_a_non_numeric_value_with_the_scale_reference_wording_style() {
        let err = parse_tier_target(3, "not-a-number").unwrap_err();
        assert_eq!(err, "Depth 'not-a-number' is not a number.");
        let err = parse_tier_target(4, "wide").unwrap_err();
        assert_eq!(err, "Girdle thickness 'wide' is not a number.");
        let err = parse_tier_target(5, "wide").unwrap_err();
        assert_eq!(err, "Table width 'wide' is not a number.");
    }

    #[test]
    fn parse_tier_target_rejects_a_non_finite_value() {
        let err = parse_tier_target(3, "NaN").unwrap_err();
        assert!(err.contains("finite"));
        let err = parse_tier_target(3, "inf").unwrap_err();
        assert!(err.contains("finite"));
    }

    #[test]
    fn parse_tier_target_rejects_a_non_positive_value() {
        let err = parse_tier_target(4, "0.0").unwrap_err();
        assert!(err.contains("positive"));
        let err = parse_tier_target(5, "-1.0").unwrap_err();
        assert!(err.contains("positive"));
    }

    #[test]
    fn parse_preform_form_builds_a_block_at_index_zero() {
        let preform = parse_preform_form(0, "1.2", "1.5", "0.8", 96).unwrap();
        assert_eq!(preform.shape, PreformShape::Block);
        assert_eq!(preform.half_width, 1.2);
        assert_eq!(preform.length_over_width, 1.5);
        assert_eq!(preform.depth, 0.8);
    }

    #[test]
    fn parse_preform_form_builds_a_cylinder_with_the_given_side_count() {
        let preform = parse_preform_form(1, "1.0", "1.0", "0.8", 64).unwrap();
        assert_eq!(preform.shape, PreformShape::Cylinder { sides: 64 });
    }

    #[test]
    fn parse_preform_form_rejects_zero_or_negative_dimensions() {
        assert!(parse_preform_form(0, "0.0", "1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "-1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "1.0", "0.0", 96).is_err());
    }

    #[test]
    fn parse_preform_form_rejects_unparseable_text() {
        assert!(parse_preform_form(0, "wide", "1.0", "0.8", 96).is_err());
    }

    fn empty_full_record() -> FullDiagramRecord {
        FullDiagramRecord {
            entry_id: 1,
            title: "Test Design".to_string(),
            url: "local://test.asc".to_string(),
            design_id: None,
            page_url: String::new(),
            diagram_image_name: None,
            diagram_image_data: None,
            competition_diagram: None,
            lw_ratio: None,
            refractive_index: None,
            index_gear: None,
            volume: None,
            facets_count: None,
            shape: None,
            designer_info: None,
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: None,
            designer: None,
            source_citation: None,
            pdf_file: None,
            gem_file: None,
            shape_category: None,
            angle_settings: Vec::new(),
            attached_files: Vec::new(),
        }
    }

    #[test]
    fn design_from_asc_text_parses_a_real_schedule() {
        let text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let loaded = design_from_asc_text("design.asc", text, None).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert_eq!(loaded.original_asc_text.as_deref(), Some(text));
        assert_eq!(loaded.design.tiers.len(), 1);
    }

    #[test]
    fn design_from_asc_text_rejects_unparseable_text() {
        assert!(design_from_asc_text("bad.asc", "not a real .asc schedule", None).is_err());
    }

    #[test]
    fn design_from_full_record_prefers_a_real_attached_asc_file() {
        let mut full = empty_full_record();
        full.attached_files
            .push(indicatrix_vault::model::file::AttachedFile {
                name: "design.asc".to_string(),
                url: String::new(),
                content:
                    b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                        .to_vec(),
            });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert!(loaded.original_asc_text.is_some());
        assert_eq!(loaded.design.tiers.len(), 1);
        // A lone tier with no explicit anchor gets its own real recorded mast
        // borrowed as a `ScaleReference` anchor by `Design::from_asc_schedule`
        // (see that function's doc comment) -- not a placeholder zero.
        match loaded.design.tiers[0].constraint {
            MeetConstraint::ScaleReference(v) => assert!((v - 0.649_912_34).abs() < 1e-9),
            ref other => panic!("expected a borrowed ScaleReference anchor, got {other:?}"),
        }
    }

    // --- native_sidecar_attachment / design_from_asc_and_native ---

    #[test]
    fn native_sidecar_attachment_finds_the_indicatrix_toml_sibling() {
        let files = vec![
            AttachedFile {
                name: "design.asc".to_string(),
                url: String::new(),
                content: Vec::new(),
            },
            AttachedFile {
                name: "design.indicatrix.toml".to_string(),
                url: String::new(),
                content: b"native".to_vec(),
            },
        ];
        let sidecar = native_sidecar_attachment(&files).expect("sidecar must be found");
        assert_eq!(sidecar.name, "design.indicatrix.toml");
    }

    #[test]
    fn native_sidecar_attachment_also_finds_the_legacy_gemcut_toml_sibling() {
        let files = vec![AttachedFile {
            name: "design.gemcut.toml".to_string(),
            url: String::new(),
            content: b"native".to_vec(),
        }];
        assert!(native_sidecar_attachment(&files).is_some());
    }

    #[test]
    fn native_sidecar_attachment_is_none_without_a_sidecar() {
        let files = vec![AttachedFile {
            name: "design.asc".to_string(),
            url: String::new(),
            content: Vec::new(),
        }];
        assert!(native_sidecar_attachment(&files).is_none());
    }

    #[test]
    fn design_from_asc_and_native_falls_back_to_the_plain_asc_when_the_sidecar_does_not_parse() {
        let asc_text = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let loaded =
            design_from_asc_and_native("design.asc", asc_text, "not valid toml [[[", None, 1)
                .expect("must fall back to the plain .asc path rather than erroring");
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert_eq!(loaded.design.tiers.len(), 1);
    }

    #[test]
    fn design_from_full_record_falls_back_to_the_angle_table_when_no_asc_is_attached() {
        let mut full = empty_full_record();
        full.angle_settings
            .push(indicatrix_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
            });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(loaded.used_placeholder);
        assert_eq!(loaded.asc_filename, None);
        assert_eq!(loaded.original_asc_text, None);
        assert_eq!(loaded.design.tiers.len(), 1);
        // The reconstruction's own documented placeholder mast (0.0) is what gets
        // borrowed as this lone tier's `ScaleReference` anchor.
        assert_eq!(
            loaded.design.tiers[0].constraint,
            MeetConstraint::ScaleReference(0.0)
        );
    }

    #[test]
    fn design_from_full_record_errors_with_no_schedule_data_at_all() {
        let full = empty_full_record();
        assert!(design_from_full_record(&full).is_err());
    }

    // --- parse_new_design_form ---

    #[test]
    fn parse_new_design_form_round_trips_gear_symmetry_mirror_and_material() {
        let preform = PreformSpec::cylinder(80, 1.5, 1.0, 1.5);
        let spec = parse_new_design_form(80, preform, "6", false, 9).unwrap();
        assert_eq!(spec.gear_teeth, 80);
        assert_eq!(spec.symmetry_order, 6);
        assert!(!spec.mirror);
        assert_eq!(spec.material.name.as_deref(), Some("Quartz")); // builtin_preset_names()[9]
        assert_eq!(spec.preform.half_width, 1.5);
    }

    #[test]
    fn parse_new_design_form_none_material_index_means_no_material_selected() {
        let preform = PreformSpec::cylinder(50, 1.0, 1.0, 1.0);
        let spec = parse_new_design_form(50, preform, "8", true, 0).unwrap();
        assert_eq!(spec.gear_teeth, 50);
        assert_eq!(spec.material.name, None);
    }

    #[test]
    fn parse_new_design_form_rejects_a_non_positive_symmetry_order() {
        let preform = PreformSpec::cylinder(96, 1.0, 1.0, 1.0);
        assert!(parse_new_design_form(96, preform, "0", true, 0).is_err());
        assert!(parse_new_design_form(96, preform, "wide", true, 0).is_err());
    }

    // --- material_selection_for_accepted_suggestion ---

    #[test]
    fn accepted_suggestion_sets_no_override_when_the_built_in_ri_is_within_tolerance() {
        let quartz_ri = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        let current = MaterialSelection::none();
        let selection = material_selection_for_accepted_suggestion("Quartz", quartz_ri, &current);
        assert_eq!(selection.name.as_deref(), Some("Quartz"));
        assert_eq!(selection.refractive_index_override, None);
    }

    #[test]
    fn accepted_suggestion_pins_the_schedule_ri_when_the_built_in_ri_differs_by_more_than_the_tolerance()
     {
        let quartz_ri = indicatrix_cut_core::built_in_refractive_index("Quartz").unwrap();
        // A schedule RI 0.02 away from Quartz's own real n_D -- still within the
        // suggestion's own 0.01 SEARCH tolerance is not guaranteed here (this
        // test picks the schedule RI directly, not via `nearest_built_in_material`),
        // but exercises exactly the "accepting would otherwise silently move the
        // exported RI" case this function exists to prevent.
        let schedule_ri = quartz_ri + 0.02;
        let current = MaterialSelection::none();
        let selection = material_selection_for_accepted_suggestion("Quartz", schedule_ri, &current);
        assert_eq!(selection.refractive_index_override, Some(schedule_ri));
    }

    #[test]
    fn accepted_suggestion_keeps_the_current_specific_gravity_override() {
        let mut current = MaterialSelection::none();
        current.specific_gravity_override = Some(3.9);
        let selection = material_selection_for_accepted_suggestion("Diamond", 2.417, &current);
        assert_eq!(selection.specific_gravity_override, Some(3.9));
    }

    // --- ri_override_to_preserve ---

    #[test]
    fn ri_override_to_preserve_pins_the_original_ri_when_the_built_in_drifts() {
        let diamond_ri = indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap();
        // The design's exported schedule currently reads "I 1.54" -- far enough
        // from Diamond's real n_D (~1.5442) that applying Diamond outright would
        // silently rewrite it.
        let original_ri = 1.54;
        assert!((diamond_ri - original_ri).abs() > RI_PRESERVE_TOLERANCE);
        assert_eq!(
            ri_override_to_preserve("Diamond", original_ri),
            Some(original_ri)
        );
    }

    #[test]
    fn ri_override_to_preserve_does_nothing_when_already_close_enough() {
        let diamond_ri = indicatrix_cut_core::built_in_refractive_index("Diamond").unwrap();
        assert_eq!(ri_override_to_preserve("Diamond", diamond_ri), None);
    }

    #[test]
    fn ri_override_to_preserve_does_nothing_for_a_non_built_in_name() {
        assert_eq!(ri_override_to_preserve("Not A Real Material", 1.54), None);
    }
}
