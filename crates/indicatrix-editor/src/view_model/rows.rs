//! The tier-list row builders.
//!
//! [`tier_items`]/[`tier_items_stale`]/
//! [`tier_items_stale_with_last_solved`]/[`tier_items_from_solved`] -- and the smaller
//! per-tier helpers ([`build_tier_row`], the manufacturability-warning grouping, the
//! `MissingAnchor` block-to-tier mapping) they share.

use super::row_format::{
    constraint_kind_and_text, format_angle_cell, format_index_value, imported_meet_text,
    orbit_status_text, representative_crown_and_pavilion_angles_deg, tier_label,
    tier_margin_and_risk,
};

/// Full-precision companion to [`format_angle_cell`]/[`format_index_value`] --
/// see [`TierRow::angle_full`]/[`TierRow::indices_full`]'s own doc
/// comments for why a form/inline-cell re-save needs the unrounded value rather
/// than either display string. Plain `{}` `Display`, the same convention
/// [`TierSolveInfo::mast_full`] already uses for the same reason.
fn full_precision(value: f64) -> String {
    format!("{value}")
}
use super::{TierRow, TierRowKind};
use indicatrix::geometry::meet_solver::{Block, SolveStrategy, SolvedTier, classify_blocks};
use indicatrix_cut_core::{
    ConstraintTier, Design, DesignSolveError, ManufacturabilityWarning, MissingAnchor,
    design::ConcaveTier,
};
use std::collections::BTreeMap;

mod warnings;

use warnings::TierWarnings;
pub use warnings::{
    manufacturability_warnings_tagged, manufacturability_warnings_tagged_with,
    solved_manufacturability_warnings,
};

/// The mast/strategy-derived half of one tier row -- [`build_tier_row`]'s only
/// per-tier-varying input beyond the tier itself, bundled into one struct so that
/// function stays under clippy's argument-count lint, instead of
/// [`tier_items`]/[`tier_items_stale`]/[`tier_items_from_solved`] each building
/// the whole [`TierRow`] literal inline, three times over.
struct TierSolveInfo {
    mast: String,
    /// The mast in millimetres, empty when nothing anchors a real scale -- see
    /// [`TierRow::mast_mm`].
    mast_mm: String,
    /// The same mast unrounded -- see [`TierRow::mast_full`].
    mast_full: String,
    strategy: String,
    strategy_is_uncertain: bool,
    strategy_detail: String,
    needs_anchor: bool,
}

/// [`build_tier_row`]'s per-DESIGN (not per-tier) context, bundled into one
/// struct purely to keep that function under clippy's argument-count lint --
/// the same reasoning [`TierSolveInfo`] (per-tier) is already split out for.
/// Computed once by each of [`tier_items`]/[`tier_items_stale`]/
/// [`tier_items_stale_with_last_solved`]/[`tier_items_from_solved`] before
/// their own per-tier `.map(...)`, not per row.
struct RowContext<'a> {
    tier_blocks: &'a [Block],
    warnings: &'a BTreeMap<usize, String>,
    /// See [`tier_margin_and_risk`]'s own doc comment.
    pavilion_partner_deg: Option<f64>,
    tier_labels: &'a [indicatrix_cut_core::TierLabelInfo],
}

/// Builds one [`TierRow`] row from `tier`'s own fields plus its
/// already-resolved [`TierSolveInfo`] -- the part [`tier_items`],
/// [`tier_items_stale`] and [`tier_items_from_solved`] all do identically once
/// they've each worked out mast/strategy/detail their own way (a real solve, no
/// solve at all, or an externally-supplied `solved` list, respectively).
/// `multi_selected` is always `false` here: none of the three callers above has
/// access to `EditorState::multi_selected` (deliberately not a parameter, even
/// indirectly -- see [`super::row_format::apply_multi_selection`]'s own doc
/// comment for why that's a post-pass instead); every real push site calls it on
/// the built `Vec` immediately afterward.
fn build_tier_row(
    design: &Design,
    index: usize,
    tier: &ConstraintTier,
    n_d: f64,
    info: TierSolveInfo,
    row_context: &RowContext<'_>,
) -> TierRow {
    let tier_blocks = row_context.tier_blocks;
    let warnings = row_context.warnings;
    let pavilion_partner_deg = row_context.pavilion_partner_deg;
    let (constraint_kind, constraint_text) =
        constraint_kind_and_text(&tier.constraint, design.tier_target(index));
    let units = indicatrix_cut_core::orbit_units(&tier.indices, &design.meta);
    let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
    let (margin_text, risk_level) = tier_margin_and_risk(
        tier.angle_deg,
        tier_blocks[index],
        n_d,
        pavilion_partner_deg,
    );
    let code = row_context
        .tier_labels
        .get(index)
        .map(|label| label.code.clone());
    let name = if tier.name.trim().is_empty() || indicatrix_cut_core::is_legacy_123_abc(&tier.name)
    {
        code.clone().unwrap_or_else(|| tier.name.clone())
    } else {
        tier.name.clone()
    };
    TierRow {
        index: index as i32,
        angle_deg: format_angle_cell(tier.angle_deg.abs()),
        angle_full: full_precision(tier.angle_deg.abs()),
        name,
        code: code.unwrap_or_default(),
        indices: tier
            .indices
            .iter()
            .copied()
            .map(format_index_value)
            .collect::<Vec<_>>()
            .join(", "),
        indices_full: tier
            .indices
            .iter()
            .copied()
            .map(full_precision)
            .collect::<Vec<_>>()
            .join(", "),
        constraint_kind,
        constraint_text,
        mast: info.mast,
        mast_mm: info.mast_mm,
        mast_full: info.mast_full,
        strategy: info.strategy,
        strategy_is_uncertain: info.strategy_is_uncertain,
        strategy_detail: info.strategy_detail,
        needs_anchor: info.needs_anchor,
        imported_meet_text: imported_meet_text(tier.imported_meet.as_ref()),
        orbit_status,
        orbit_incomplete,
        is_detached: !tier.detached.is_empty(),
        block: block_label(tier_blocks[index]).to_string(),
        margin_text,
        risk_level,
        meet_partners_text: meet_partners_text(design, index),
        warning_text: warnings.get(&index).cloned().unwrap_or_default(),
        multi_selected: false,
        // Patched in place afterward, once there is a whole row list to patch --
        // see `apply_proposed_angles`'s own doc comment.
        proposed_angle: String::new(),
        kind: TierRowKind::Flat,
        tool_line: String::new(),
        relation_text: design.relation_text(index).unwrap_or_default(),
    }
}

/// The tool line a concave row carries: the four
/// [`ConcaveTier::second_line_fields`] columns, joined with two spaces.
#[must_use]
pub fn concave_tool_line(tier: &ConcaveTier) -> String {
    tier.second_line_fields().join("  ")
}

/// One [`TierRow`] per concave tier, in storage order, appended after the flat rows.
///
/// The tier table is an editing view, so (like the flat rows, which keep their
/// stored order rather than cutting order) it lists concave tiers as authored; the
/// cutting schedule ([`super::yield_report::cutting_instructions_rows`]) is the one
/// view in `cutting_order()`. Each row's `code` is the one the cutting order gives it
/// ([`Design::tier_codes`]: `P4` continues the flat pavilion tiers' count). A concave tier has
/// no mast, strategy, meet constraint or critical-angle margin, so those cells read as "not
/// applicable" (`"-"`, an empty string, risk `-1`). `warnings` is [`TierWarnings::concave`].
fn concave_tier_rows(design: &Design, warnings: &BTreeMap<usize, String>) -> Vec<TierRow> {
    let codes = design.tier_codes().concave;
    design
        .concave_tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let units = indicatrix_cut_core::orbit_units(&tier.indices, &design.meta);
            let (orbit_status, orbit_incomplete) = orbit_status_text(&units);
            TierRow {
                index: index as i32,
                angle_deg: format_angle_cell(tier.angle_deg.abs()),
                angle_full: full_precision(tier.angle_deg.abs()),
                name: tier.name.clone(),
                code: codes
                    .get(index)
                    .map(|label| label.code.clone())
                    .unwrap_or_default(),
                indices: tier
                    .indices
                    .iter()
                    .copied()
                    .map(format_index_value)
                    .collect::<Vec<_>>()
                    .join(", "),
                indices_full: tier
                    .indices
                    .iter()
                    .copied()
                    .map(full_precision)
                    .collect::<Vec<_>>()
                    .join(", "),
                mast: "-".to_string(),
                strategy: "Tool cut".to_string(),
                strategy_detail: tier.instructions.clone(),
                orbit_status,
                orbit_incomplete,
                block: if tier.is_crown_side() {
                    "Crown"
                } else {
                    "Pavilion"
                }
                .to_string(),
                risk_level: -1,
                warning_text: warnings.get(&index).cloned().unwrap_or_default(),
                kind: TierRowKind::Concave,
                tool_line: concave_tool_line(tier),
                ..TierRow::default()
            }
        })
        .collect()
}

/// Builds the tier list's rows WITHOUT calling [`Design::solve`] at all -- every
/// mast/strategy cell reads `"-"`/`"not solved"` (flagged uncertain) regardless of
/// what the design actually is.
///
/// Used by `refresh_editor_panel_stale` after every edit
/// that isn't the explicit "Solve" action -- see this group's `mod.rs` doc comment for
/// why: a real design can take multiple seconds to solve, and showing a PREVIOUS
/// solve's masts would be actively wrong the moment the edit changed tier count/order.
#[must_use]
pub fn tier_items_stale(design: &Design, n_d: f64) -> Vec<TierRow> {
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // Never solved here (see this function's own doc comment), so only the two
    // mast-free manufacturability checks can run -- tagged "(pre-solve)" since
    // that is the whole design's state right now, not a completed pass.
    let warnings = TierWarnings::of(design, None);
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let tier_labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings.flat,
        pavilion_partner_deg,
        tier_labels: &tier_labels,
    };
    let mut rows: Vec<TierRow> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let info = TierSolveInfo {
                mast: "-".to_string(),
                mast_mm: String::new(),
                mast_full: String::new(),
                strategy: "not solved".to_string(),
                strategy_is_uncertain: true,
                // Never solved here (see this function's own doc comment), so
                // there is no per-tier detail or missing-anchor block to report
                // yet -- both come back once the explicit "Solve" action calls
                // `tier_items`.
                strategy_detail: String::new(),
                needs_anchor: false,
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect();
    rows.extend(concave_tier_rows(design, &warnings.concave));
    rows
}

/// [`tier_items_stale`]'s counterpart for a caller that still has the LAST real solve's
/// mast list on hand.
///
/// [`tier_items_stale`] blanks every mast/strategy/ warning to `"-"`/`"not solved"` until
/// the next solve completes, even for an edit like a rename that cannot possibly move a
/// mast.
///
/// On a slow design (auto-solve
/// off, or over its own measured budget -- see `should_schedule_auto_solve`) that
/// wipes 100+ mast readings the cutter may be comparing against a gauge, for no
/// reason: a rename never invalidates anyone's mast. This function instead keeps
/// the previous solve's readings for every tier the edit didn't touch.
///
/// Rows named in `dirty` (the edit's own affected tier indices -- e.g. the single
/// index a Save Tier/inline-angle/wheel-nudge edit touched, or every index for a
/// structural edit like Undo/Redo/remove/duplicate/detach/gear-remap that can move
/// masts application-wide) get [`tier_items_stale`]'s own `"-"`/`"not solved"`
/// treatment -- no PREVIOUS mast can be trusted for a tier that itself just changed.
/// Every other row keeps `last_solved`'s own mast/strategy/detail, with strategy
/// prefixed `"stale"` so it never reads as a fresh, just-completed solve.
///
/// `last_solved` (and therefore every row) falls back to [`tier_items_stale`]'s
/// blank treatment whenever `None`, OR whenever its length no longer matches
/// `design.tiers.len()` -- a tier add/remove/reorder invalidates every index in an
/// old solve's list, so trusting it positionally would show tier 5's old mast on
/// today's tier 6.
///
/// # Handoff
/// `state/rows.rs` only computes; the caller needs a cached `Vec<SolvedTier>` from
/// this design's last real solve (`auto_solve::solid_last_solved`'s shared handle
/// already holds exactly this, refreshed by every completed background solve and
/// every solid-preview replan) and the dirty tier index set each edit computes
/// per callback -- `callbacks::tier_actions` is a directory now, not the single
/// file this once pointed at; `tier_actions::nudge`/`tier_actions::tier_form`
/// are two callers that build one -- both read from `view.rs`'s
/// `push_stale_content`/`refresh_editor_panel_stale` (not this file), which
/// would call this in place of [`tier_items_stale`].
#[must_use]
pub fn tier_items_stale_with_last_solved(
    design: &Design,
    n_d: f64,
    last_solved: Option<&[SolvedTier]>,
    dirty: &std::collections::BTreeSet<usize>,
) -> Vec<TierRow> {
    let last_solved = last_solved.filter(|solved| solved.len() == design.tiers.len());
    let Some(last_solved) = last_solved else {
        return tier_items_stale(design, n_d);
    };
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // A cached solve is still real evidence for the mast-free checks even though
    // it may now be one edit old -- tagged "(pre-solve)" regardless, same as
    // `tier_items_stale`, since a fresh edit landed since it ran and nothing here
    // re-verifies it still holds.
    let warnings = TierWarnings::of(design, None);
    let mm_per_unit = design.yield_report(last_solved).mm_per_unit;
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let tier_labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings.flat,
        pavilion_partner_deg,
        tier_labels: &tier_labels,
    };
    let mut rows: Vec<TierRow> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let info = if dirty.contains(&index) {
                TierSolveInfo {
                    mast: "-".to_string(),
                    mast_mm: String::new(),
                    mast_full: String::new(),
                    strategy: "not solved".to_string(),
                    strategy_is_uncertain: true,
                    strategy_detail: String::new(),
                    needs_anchor: false,
                }
            } else {
                let (label, _) = strategy_label(last_solved[index].strategy);
                TierSolveInfo {
                    mast: format!("{:.4}", last_solved[index].mast),
                    mast_mm: mm_per_unit.map_or_else(String::new, |mm| {
                        format!("{:.3} mm", last_solved[index].mast * mm)
                    }),
                    mast_full: format!("{}", last_solved[index].mast),
                    strategy: format!("stale ({label})"),
                    strategy_is_uncertain: true,
                    strategy_detail: last_solved[index].detail.clone(),
                    needs_anchor: false,
                }
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect();
    rows.extend(concave_tier_rows(design, &warnings.concave));
    rows
}

/// Patches `TierRow::proposed_angle` (`ui/types.slint`) onto each row one of `changes`'
/// own [`indicatrix_cut_core::AngleChange`]s targets.
///
/// Called AFTER the row list is already built, the same "patch the already-pushed row
/// list in place" shape [`super::row_format::apply_multi_selection`] already uses for the
/// multi-select highlight, rather than threading an
/// [`indicatrix_cut_core::OptimizeOutcome`] through
/// [`tier_items`]/[`tier_items_stale`]/[`tier_items_from_solved`]/
/// [`tier_items_stale_with_last_solved`]'s four independent call sites.
///
/// Leaves `proposed_angle` at its default (`""`) for every row `changes` does
/// not mention. Formatted with [`format_angle_cell`]'s own two-decimal
/// convention, so a ghost value reads exactly like the real `angle_deg` cell
/// beside it.
///
/// # Handoff
/// `ui/components/editor_tier_table.slint` still
/// needs the ANGLE column's own rendering of this field -- e.g. a small
/// "\u{2192} 41.20\u{b0}" ghost beside the real value.
pub fn apply_proposed_angles(tiers: &mut [TierRow], changes: &[indicatrix_cut_core::AngleChange]) {
    for change in changes {
        if let Some(row) = tiers.get_mut(change.index) {
            row.proposed_angle = format_angle_cell(change.to_deg.abs());
        }
    }
}

/// Maps every tier index [`MissingAnchor::block_details`] names to the exact
/// [`Block`] it belongs to -- lets [`tier_items`] mark exactly the tiers
/// responsible for a [`MissingAnchor`] failure (and give each one its own
/// one-block remedy sentence) rather than flagging every tier in the design,
/// which is the bug this exists to fix (a pavilion-only anchor failure used to
/// paint the crown's tiers "?" too).
fn missing_anchor_tier_blocks(missing: &MissingAnchor, design: &Design) -> BTreeMap<usize, Block> {
    missing
        .block_details(design)
        .into_iter()
        .flat_map(|(block, indices)| indices.into_iter().map(move |index| (index, block)))
        .collect()
}

/// The tier(s) [`Design::facet_meets`] actually resolved tier `index`'s meet
/// constraint against, as a display string (e.g. `"meets tier 3 (C1)"`,
/// `"meets tier 2 (C1), tier 4 (C2)"`), or `""` when it resolves to nothing
/// (a `ScaleReference`/`MeetExisting` tier, or a `MeetNamed` tier every one of
/// whose names is unresolved). Uses the same solver-grade
/// [`indicatrix::geometry::meet_solver::MeetNameResolver`] the solver itself
/// runs, unlike guessing from the typed `constraint_text` alone, so this shows
/// the meet partners it actually resolved. Never needs a solve (`facet_meets`
/// only resolves names against `meet_tier_inputs`), so this is populated
/// identically by [`tier_items`] and [`tier_items_stale`].
fn meet_partners_text(design: &Design, index: usize) -> String {
    let Ok(targets) = design.facet_meets(index) else {
        return String::new();
    };
    if targets.is_empty() {
        return String::new();
    }
    format!(
        "meets {}",
        targets
            .into_iter()
            .map(|target| tier_label(design, target))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// [`SolveStrategy`]'s human-readable label for the tier list's "SOLVE" column,
/// plus whether it should be flagged uncertain: `LeastSquaresFallback` is an
/// estimate, not vertex-derived, and `Failed` is documented as "should not be
/// trusted" -- both get flagged, `ScaleReference`/`DependencyOrder`/`JointGroup`
/// do not.
const fn strategy_label(strategy: SolveStrategy) -> (&'static str, bool) {
    match strategy {
        SolveStrategy::ScaleReference => ("Scale reference", false),
        SolveStrategy::DependencyOrder => ("Dependency order", false),
        SolveStrategy::JointGroup => ("Joint group", false),
        SolveStrategy::LeastSquaresFallback => ("Least-squares est.", true),
        SolveStrategy::Failed => ("FAILED (untrusted)", true),
    }
}

/// Converts `design`'s current tier list into the rows `EditorView`'s list renders,
/// including the solved mast and [`SolveStrategy`] label for each.
///
/// `index` is the
/// tier's position in `design.tiers` -- round-tripped back by
/// `EditorView.save_tier`/`remove_tier`, a list index rather than a stable id.
///
/// Solves `design` exactly once up front. When [`Design::solve`] returns
/// [`MissingAnchor`], only the tiers that error actually
/// names (via [`missing_anchor_tier_blocks`]) get the "no anchor yet" `"?"`
/// treatment; every other tier -- blocked only because some OTHER block has no
/// anchor, not because it lacks one itself -- reads `"-"`/`"blocked"` instead,
/// so a pavilion-only failure no longer paints the crown's tiers "?" too. See
/// `status_text_and_is_problem`, which surfaces which block(s) are missing an
/// anchor in the validation banner.
///
/// Only called from `refresh_all` (New/Load/the explicit "Solve" action) -- see
/// [`tier_items_stale`] for the no-solve version every other edit callback uses.
#[must_use]
pub fn tier_items(design: &Design, n_d: f64) -> Vec<TierRow> {
    let solved = design.solve();
    match &solved {
        Ok(rows) => tier_items_from_solved(design, rows, n_d),
        Err(err) => {
            // Only a real `MissingAnchor` can name individual tiers -- a
            // `TierTarget`/mismatch/plane-cap failure blocks the whole design at
            // once, so every tier falls to the generic "blocked" arm below with
            // that error's own status-strip sentence rather than a bogus "no
            // anchor yet" on tiers that were never the problem.
            let missing_tier_blocks = match err {
                DesignSolveError::MissingAnchor(missing) => {
                    missing_anchor_tier_blocks(missing, design)
                }
                DesignSolveError::Mismatch(_)
                | DesignSolveError::Solve(_)
                | DesignSolveError::Target(_) => BTreeMap::new(),
            };
            let blocked_detail = if matches!(err, DesignSolveError::MissingAnchor(_)) {
                "Another block in this design has no anchor -- see the validation banner above."
                    .to_string()
            } else {
                err.to_string()
            };
            let tier_blocks = classify_blocks(&design.meet_tier_inputs());
            // Tagged "(pre-solve)" -- see `manufacturability_warnings_tagged`'s
            // own doc comment; there is no real solve to show warnings from yet.
            let warnings = TierWarnings::of(design, None);
            let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
            let tier_labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
            let row_context = RowContext {
                tier_blocks: &tier_blocks,
                warnings: &warnings.flat,
                pavilion_partner_deg,
                tier_labels: &tier_labels,
            };
            let mut rows: Vec<TierRow> = design
                .tiers
                .iter()
                .enumerate()
                .map(|(index, tier)| {
                    let info = missing_tier_blocks.get(&index).map_or_else(
                        || TierSolveInfo {
                            mast: "-".to_string(),
                            mast_mm: String::new(),
                            mast_full: String::new(),
                            strategy: "blocked".to_string(),
                            strategy_is_uncertain: true,
                            strategy_detail: blocked_detail.clone(),
                            needs_anchor: false,
                        },
                        |&block| TierSolveInfo {
                            mast: "?".to_string(),
                            mast_mm: String::new(),
                            mast_full: String::new(),
                            strategy: "no anchor yet".to_string(),
                            strategy_is_uncertain: true,
                            strategy_detail: MissingAnchor::block_sentence(block),
                            needs_anchor: true,
                        },
                    );
                    build_tier_row(design, index, tier, n_d, info, &row_context)
                })
                .collect();
            rows.extend(concave_tier_rows(design, &warnings.concave));
            rows
        }
    }
}

/// [`tier_items`]'s counterpart for a caller that already has an up-to-date `solved`
/// mast list on hand.
///
/// Builds every row's mast/strategy/detail straight from it instead of calling
/// [`Design::solve`] again. Exists so the
/// solid-preview worker's own replan solve (`solid_preview::live_update::
/// plan_preview`, run off the UI thread) can feed the SAME masts into the tier
/// table instead of a second, separately dispatched full solve recomputing them.
/// Not a single caller: `view::viewport::push_solved_preview`, `view::panel`'s
/// own stale-refresh success arm, and `auto_solve::dispatch`'s background-solve
/// completion each call this directly.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same
/// order -- the same alignment contract [`Design::to_asc_schedule_from_solved`]
/// documents; indexing out of that range panics.
#[must_use]
pub fn tier_items_from_solved(design: &Design, solved: &[SolvedTier], n_d: f64) -> Vec<TierRow> {
    tier_items_from_solved_and(design, solved, n_d, &TierWarnings::of(design, Some(solved)))
}

/// [`tier_items_from_solved`] for a caller on a UI thread, which must not run the pass itself.
///
/// The manufacturability pass builds meshes (milliseconds for a planar design, far more for
/// one with concave tools), so the findings come from `warnings`, the pass a worker ran
/// against this same `design` and `solved` ([`solved_manufacturability_warnings`]).
/// `None` -- the worker has no pass for this solve -- behaves as [`tier_items_from_solved`]
/// for a design without concave tools, and for one with tools shows only the mast-free
/// findings, tagged `(pre-solve)`, never building a mesh.
///
/// # Panics
///
/// As [`tier_items_from_solved`]: `solved` must have one entry per tier of `design`.
#[must_use]
pub fn tier_items_from_solved_with_warnings(
    design: &Design,
    solved: &[SolvedTier],
    n_d: f64,
    warnings: Option<&[ManufacturabilityWarning]>,
) -> Vec<TierRow> {
    tier_items_from_solved_and(
        design,
        solved,
        n_d,
        &TierWarnings::from_precomputed(design, solved, warnings),
    )
}

/// What the inline pass may be given when no worker pass exists: the masts for a design
/// without concave tools (the pass is then cheap and complete), `None` -- the mast-free
/// checks, which build no mesh -- for one with tools, whose check 6 builds a solid.
fn without_tool_checks<'a>(design: &Design, solved: &'a [SolvedTier]) -> Option<&'a [SolvedTier]> {
    design.concave_tiers.is_empty().then_some(solved)
}

/// The rows of [`tier_items_from_solved`] with `warnings` already split by row.
fn tier_items_from_solved_and(
    design: &Design,
    solved: &[SolvedTier],
    n_d: f64,
    warnings: &TierWarnings,
) -> Vec<TierRow> {
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    // `None` whenever the design carries no girdle diameter, which is
    // the only thing that anchors model units to a real size. Computed once here
    // rather than per row -- `yield_report` measures the whole solid.
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let (_, pavilion_partner_deg) = representative_crown_and_pavilion_angles_deg(design);
    let tier_labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let row_context = RowContext {
        tier_blocks: &tier_blocks,
        warnings: &warnings.flat,
        pavilion_partner_deg,
        tier_labels: &tier_labels,
    };
    let mut rows: Vec<TierRow> = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let (label, uncertain) = strategy_label(solved[index].strategy);
            let info = TierSolveInfo {
                mast: format!("{:.4}", solved[index].mast),
                mast_mm: mm_per_unit.map_or_else(String::new, |mm| {
                    format!("{:.3} mm", solved[index].mast * mm)
                }),
                mast_full: format!("{}", solved[index].mast),
                strategy: label.to_string(),
                strategy_is_uncertain: uncertain,
                strategy_detail: solved[index].detail.clone(),
                needs_anchor: false,
            };
            build_tier_row(design, index, tier, n_d, info, &row_context)
        })
        .collect();
    rows.extend(concave_tier_rows(design, &warnings.concave));
    rows
}

/// [`TierRow::block`]'s one-word label for a [`Block`] -- shared by
/// [`tier_items`]/[`tier_items_stale`] so a table row's crown/pavilion/girdle side
/// (a zero angle's side is the sign of the zero, `tier.rs`'s own doc comment) is
/// never left to be guessed from the formatted angle text.
const fn block_label(block: Block) -> &'static str {
    match block {
        Block::Crown => "Crown",
        Block::Pavilion => "Pavilion",
        Block::Girdle => "Girdle",
    }
}
