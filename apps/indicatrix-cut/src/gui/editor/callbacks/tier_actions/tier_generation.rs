//! Generated tier series ("Generate steps"), mirroring a tier to the other block,
//! and the name-generation helpers both those actions (and Duplicate/Save Tier)
//! share.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Edit};
use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            loading,
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

/// [`setup_generate_step_series_callback`]'s form-parsing half -- everything the
/// "Generate steps" form needs turned into real values BEFORE
/// [`ConstraintTier::step_series`] can be called, except the indices list, which
/// the caller parses separately via [`loading::parse_index_list`] (that parse
/// also needs `state`'s own `gear_teeth_abs`, not available here). An empty
/// anchor field means [`MeetConstraint::MeetExisting`] (the same "blank means use
/// whatever's already anchored" convention -- the caller is relying on an anchor
/// elsewhere in the design); a
/// non-empty one is parsed and pinned via [`MeetConstraint::ScaleReference`].
///
/// # Errors
///
/// A message naming the offending field, ready to show the cutter.
fn parse_step_series_form(
    start_angle_text: &str,
    angle_step_text: &str,
    count: i32,
    anchor_text: &str,
) -> Result<(f64, f64, usize, MeetConstraint), String> {
    let start_angle: f64 = start_angle_text
        .trim()
        .parse()
        .map_err(|_| format!("Start angle '{start_angle_text}' is not a number."))?;
    let angle_step: f64 = angle_step_text
        .trim()
        .parse()
        .map_err(|_| format!("Angle step '{angle_step_text}' is not a number."))?;
    if !start_angle.is_finite() || !angle_step.is_finite() {
        return Err("Start angle and angle step must be finite numbers.".to_string());
    }
    let count = usize::try_from(count)
        .ok()
        .filter(|&count| count >= 1)
        .ok_or_else(|| "Tier count must be a positive whole number.".to_string())?;
    let anchor_text = anchor_text.trim();
    let first_constraint = if anchor_text.is_empty() {
        MeetConstraint::MeetExisting
    } else {
        let anchor: f64 = anchor_text
            .parse()
            .map_err(|_| format!("Anchor '{anchor_text}' is not a number."))?;
        if !anchor.is_finite() {
            return Err("Anchor must be a finite number.".to_string());
        }
        MeetConstraint::ScaleReference(anchor)
    };
    Ok((start_angle, angle_step, count, first_constraint))
}

/// "Generate steps": builds `count`
/// tiers via [`ConstraintTier::step_series`] and applies them as one
/// [`Edit::Batch`] of [`Edit::AddTier`]s, appended after the design's current
/// last tier. Mirrors `editor_tier_table.slint`'s own call site's argument
/// order: name prefix, start-angle text, angle-step text, tier count, a
/// comma-separated index list shared by every generated tier (parsed the same
/// way [`setup_save_tier_callback`]'s own indices field is, via
/// [`loading::parse_index_list`]), and an optional anchor -- see
/// [`parse_step_series_form`]'s own doc comment for what an empty one means.
/// Previously this callback did not exist at all (the Slint side called
/// straight into nothing); see [`setup_toggle_detach_callback`]'s own doc
/// comment for why it is registered from there rather than from
/// `gui::editor::mod::setup_editor_callbacks`.
///
/// `pub(super)` since [`super::tier_crud::setup_toggle_detach_callback`] is the
/// one call site that registers it.
pub(super) fn setup_generate_step_series_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_generate_step_series(
        move |name_prefix: SharedString,
              start_angle_text: SharedString,
              angle_step_text: SharedString,
              count: i32,
              indices_text: SharedString,
              anchor_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let (start_angle, angle_step, count, first_constraint) = match parse_step_series_form(
                &start_angle_text,
                &angle_step_text,
                count,
                &anchor_text,
            ) {
                Ok(parsed) => parsed,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let mut st = state.borrow_mut();
            let gear_teeth_abs = st.design.meta.gear_teeth_abs();
            let indices = match loading::parse_index_list(&indices_text, gear_teeth_abs) {
                Ok(indices) => indices,
                Err(e) => {
                    show_toast(&ui, &e, "error");
                    return;
                }
            };
            let tiers = ConstraintTier::step_series(
                &name_prefix,
                start_angle,
                angle_step,
                count,
                &indices,
                &first_constraint,
            );
            let start_index = st.design.tiers.len();
            let edits: Vec<Edit> = tiers
                .into_iter()
                .enumerate()
                .map(|(offset, tier)| Edit::AddTier {
                    index: start_index + offset,
                    tier,
                })
                .collect();
            let added_count = edits.len();
            match st.apply(Edit::Batch(edits)) {
                Ok(()) => {
                    let dirty: BTreeSet<usize> = (start_index..start_index + added_count).collect();
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        dirty,
                        false,
                    );
                    drop(st);
                    show_toast(&ui, &format!("Generated {added_count} tier(s)."), "info");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );
}

/// "Mirror tier to other block" :
/// duplicates the tier at `tier_index` to the opposite block via
/// [`ConstraintTier::mirrored_to_other_block`] (angle negated, same
/// indices/constraint/detached set, name suffixed by `name_suffix`) and applies
/// it as one [`Edit::AddTier`], appended after the design's current last tier.
/// A silent no-op for an out-of-range `tier_index`, the same guard
/// [`setup_duplicate_tier_callback`] uses for its own source lookup.
///
/// `pub(super)` for the same reason as [`setup_generate_step_series_callback`]
/// above.
pub(super) fn setup_mirror_tier_to_other_block_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_mirror_tier_to_other_block(
        move |tier_index: i32, name_suffix: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            let Some(source) = st.design.tiers.get(tier_index) else {
                return;
            };
            let mirrored = source.mirrored_to_other_block(&name_suffix);
            let mirrored_label = mirrored.name.clone();
            let new_index = st.design.tiers.len();
            match st.apply(Edit::AddTier {
                index: new_index,
                tier: mirrored,
            }) {
                Ok(()) => {
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([new_index]));
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([new_index]),
                        false,
                    );
                    drop(st);
                    ui.global::<EditorModel>()
                        .set_selected_tier_index(new_index as i32);
                    show_toast(&ui, &format!("Mirrored to {mirrored_label}"), "info");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );
}

/// Generates a name for [`setup_duplicate_tier_callback`] that is guaranteed not
/// to collide with any name in `existing_names`, by counting up a `" (N)"` suffix
/// (`"P1 (2)"`, `"P1 (3)"`, ...) rather than appending an apostrophe: the
/// apostrophe scheme produced an unreadable "P1''''" pile on a second or third
/// duplicate of the same tier and, worse, silently created a duplicate name that
/// `MeetNameResolver::name_match` (`indicatrix::geometry::meet_solver::names`)
/// resolves by binding to whichever tier holds it FIRST -- so a duplicate's stale
/// copy of a popular name could silently steal every future `MeetNamed` reference
/// meant for the original.
///
/// [`split_duplicate_suffix`] first removes a trailing `" (N)"` a PREVIOUS call to
/// this same function already appended, so duplicating "P1 (2)" produces
/// "P1 (3)" rather than nesting into "P1 (2) (2)". An empty source name (an
/// unnamed tier) falls back to the base "Tier" rather than producing a bare
/// "(2)" -- giving the duplicate a real name is also what lets it become a
/// `MeetNamed` target, since `ConstraintTier::names` never resolves a name from an
/// empty string.
///
/// `pub(super)` since [`super::tier_crud::setup_duplicate_tier_callback`] uses it.
pub(super) fn unique_duplicate_name(source_name: &str, existing_names: &[String]) -> String {
    let (base, source_number) = split_duplicate_suffix(source_name.trim());
    let base = if base.is_empty() { "Tier" } else { base };
    // One past the source's OWN number when it already carries one, so this
    // holds to its documented contract on its own terms rather than relying on
    // the caller's list happening to contain the source tier: duplicating
    // "P1 (2)" gives "P1 (3)" even against an empty list. `checked_add` falls
    // back to 2 for the (unreachable by duplicating, but typeable by hand)
    // number that cannot be counted past.
    let mut n: u32 = source_number
        .and_then(|number| number.checked_add(1))
        .unwrap_or(2);
    loop {
        let candidate = format!("{base} ({n})");
        if !existing_names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n += 1;
    }
}

/// [`setup_save_tier_callback`]'s auto-name for a brand-new tier saved with a
/// blank Name field ([`unique_duplicate_name`] above does not cover this case --
/// that one only ever runs against an already-named
/// source). An empty name can never become a `MeetNamed` target
/// (`ConstraintTier::names()` returns nothing for it), so leaving a fresh
/// `AddTier` unnamed silently makes it un-meetable until the cutter notices.
///
/// The block letter (`C`rown/`P`avilion) follows the same sign the tier's own
/// angle will be classified by (`meet_solver::blocks`), simplified: this only
/// has to pick a reasonable DEFAULT name the cutter can always retype, so it
/// does not reproduce that module's unsigned-zero "inherits the previous
/// tier's side" rule just to name a single new tier. Case-insensitive
/// collision against `existing_names` counts up past it, matching
/// `unique_duplicate_name`'s own convention.
///
/// `pub(super)` since [`super::tier_form::setup_save_tier_callback`] uses it.
pub(super) fn next_free_block_name(angle_deg: f64, existing_names: &[String]) -> String {
    let letter = if angle_deg.is_sign_negative() {
        'P'
    } else {
        'C'
    };
    let mut n: u32 = 1;
    loop {
        let candidate = format!("{letter}{n}");
        if !existing_names
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(&candidate))
        {
            return candidate;
        }
        n += 1;
    }
}

/// Splits `name` into its base portion and the number of a trailing `" (N)"` (a
/// whole, non-negative number in parentheses, preceded by exactly one space) --
/// see [`unique_duplicate_name`]'s own doc comment for why. A name with no such
/// suffix comes back whole with `None`, and so does one whose digits do not fit
/// a `u32`, for the same reason `"P1 (a)"` does: a suffix this cannot read is
/// part of the name the cutter typed, not a counter to continue.
///
/// `pub(super)` since [`super::tests`] exercises this directly.
pub(super) fn split_duplicate_suffix(name: &str) -> (&str, Option<u32>) {
    let Some((base, rest)) = name.rsplit_once(" (") else {
        return (name, None);
    };
    let Some(digits) = rest.strip_suffix(')') else {
        return (name, None);
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return (name, None);
    }
    digits
        .parse()
        .map_or((name, None), |number| (base, Some(number)))
}
