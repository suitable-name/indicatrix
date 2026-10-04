//! Small per-tier text/formatting helpers.
//!
//! The pieces [`super::rows`]'s row builders share, plus the handful other editor actions
//! reach directly (multi-select, the symmetry-preview and meet-name-resolution checks).
//!
//! The
//! desktop's Slint model push helpers stay with the desktop.

use super::{IndexChip, TierRow};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, MeetNameResolver, TokenResolution, classify_blocks,
};
use indicatrix_cut_core::{Design, OrbitUnit, TierTarget};

/// Splits a tier's [`MeetConstraint`] AND its [`TierTarget`] (if any) into the
/// `(constraint_kind, constraint_text)` pair [`TierRow`] carries and the
/// tier-edit form round-trips through its `LineEdit` -- the inverse of
/// `super::loading::parse_tier_form`'s constraint parsing and
/// `super::loading::parse_tier_target`'s target parsing, combined.
///
/// `target` wins whenever it is `Some`: `Design::resolved_meet_tier_inputs`
/// always leaves `constraint` as a `ScaleReference(0.0)` PLACEHOLDER on a
/// target-bearing tier (`indicatrix_cut_core::design::targets`'s module docs),
/// so reading `constraint` directly would show kind `2` with a meaningless
/// "0" rather than the target the cutter actually authored.
pub(super) fn constraint_kind_and_text(
    constraint: &MeetConstraint,
    target: Option<TierTarget>,
) -> (i32, String) {
    if let Some(target) = target {
        return match target {
            TierTarget::DepthMm(mm) => (3, mm.to_string()),
            TierTarget::GirdleThicknessMm(mm) => (4, mm.to_string()),
            TierTarget::TableWidthMm(mm) => (5, mm.to_string()),
        };
    }
    match constraint {
        MeetConstraint::MeetExisting => (0, String::new()),
        MeetConstraint::MeetNamed(names) => (1, names.join(", ")),
        // Rust's shortest round-trippable `f64` Display, not a fixed decimal count --
        // this feeds back into the form's `LineEdit`, so it must read back exactly
        // what `solve` (or the user) produced.
        MeetConstraint::ScaleReference(value) => (2, value.to_string()),
    }
}

/// The tier table's ANGLE cell text.
///
/// Always two decimals: a wheel/keyboard nudge accumulates plain `f64` noise (e.g.
/// `-40.300000000000004`), and showing that raw `Display` output read as a cut-off,
/// broken number rather than a rounding artifact.
///
/// A cutter compares
/// this against a 2-decimal gauge anyway, so truncating the DISPLAY (never the
/// stored value) is honest and matches every other angle readout in this app
/// (`view::refresh_design_settings`'s critical-angle text, the margin column,
/// ...).
///
/// This returned string IS rounded -- the inline editor and the tier-edit form
/// do NOT re-seed from it: both read `EditorTierItem::angle_full` instead (the
/// full, unrounded `Display`, see that field's own doc comment in
/// `ui/types.slint`), precisely so a re-save can never silently round a value
/// the design itself still holds exactly (the form does not
/// read the full, unrounded value back from this string).
#[must_use]
pub fn format_angle_cell(angle_deg: f64) -> String {
    format!("{angle_deg:.2}")
}

/// One index-wheel position's display text -- an integer when it lands exactly
/// on a whole tooth (the overwhelming common case), else two decimals.
///
/// Mirrors
/// [`format_angle_cell`]'s reasoning for the same nudge-noise problem, but
/// integral-aware: `"24"` reads better than `"24.00"` for the ordinary case,
/// while a genuinely fractional index (rare, but real -- see
/// `ManufacturabilityWarning::FractionalIndex`) still shows its non-integral part.
#[must_use]
pub fn format_index_value(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Builds one [`IndexChip`] per entry in `indices`, flagging exactly the occurrences also
/// present in `detached`.
///
/// The inspector's per-facet chip row, kept separate from [`TierRow`] (the desktop builds
/// tier rows on a background thread for a large design and ships them across threads; its
/// Slint chip model is `Rc`-backed and must not ride along).
///
/// A plain epsilon-equality scan against `detached` (not `crate::orbit::edit`'s
/// private, gear-wraparound-aware `same_index`, unreachable from here) is correct
/// for this: both `Vec`s are the SAME tier's own values, `detached` entries are
/// always pushed from `indices` verbatim (`Design::detach_orbit_member`/
/// `detach_all_in_tier`), never independently authored or wrapped, so no
/// wraparound normalization is ever needed to tell them apart -- only real edits
/// (`facet_toggle_detach`'s Rust handler) need the wraparound-aware comparison,
/// and those already go through core's own helpers.
#[must_use]
pub fn index_chip_items(indices: &[f64], detached: &[f64]) -> Vec<IndexChip> {
    indices
        .iter()
        .map(|&position| IndexChip {
            position: position as f32,
            label: format_index_value(position),
            detached: detached.iter().any(|&d| (d - position).abs() < 1e-9),
        })
        .collect()
}

/// Display text for a tier's `imported_meet` -- what the source `.asc` file's `G`
/// field claimed this facet meets, kept alongside the pinned `constraint` import now
/// writes instead. `""` when there's nothing to adopt, which `EditorView` also uses
/// as its "show the Adopt button" condition.
pub(super) fn imported_meet_text(imported_meet: Option<&MeetConstraint>) -> String {
    match imported_meet {
        Some(MeetConstraint::MeetExisting) => "meets an unspecified vertex".to_string(),
        Some(MeetConstraint::MeetNamed(names)) => format!("meets {}", names.join(", ")),
        // `Design::from_asc_schedule` never actually stores this variant here, but a
        // future non-exhaustive addition should degrade to "nothing to show," not panic.
        None | Some(MeetConstraint::ScaleReference(_)) => String::new(),
    }
}

/// A short label for `units` (`orbit::orbit_units`'s decomposition of one tier's
/// `indices`) plus whether it should be flagged amber. Empty/`false` for a tier with
/// nothing to link (0 or 1 occurrence).
pub(super) fn orbit_status_text(units: &[OrbitUnit]) -> (String, bool) {
    if units.iter().all(|u| u.members.len() <= 1) {
        return (String::new(), false);
    }
    match units {
        [one] => {
            if one.is_complete() {
                (format!("orbit x{}", one.members.len()), false)
            } else {
                (
                    format!("{}/{} orbit", one.members.len(), one.expected_len),
                    true,
                )
            }
        }
        // Reports which of the two cases this actually is -- a clean multi-facet
        // fold with one unit short a member, vs a `mixed_fold` where NOTHING
        // resembles a complete orbit -- rather than folding both into one "N
        // orbits" label indistinguishable except by the amber tint.
        // `orbit::mod`'s own corpus-measurement doc comment treats those
        // as different findings (a `partial` occurrence is common and benign;
        // `mixed_fold` -- every unit incomplete -- is "real incoherence").
        many => {
            let incomplete = many.iter().filter(|u| !u.is_complete()).count();
            if incomplete == 0 {
                (format!("{} orbits", many.len()), false)
            } else if incomplete == many.len() {
                ("not symmetric".to_string(), true)
            } else {
                (
                    format!("{} orbits ({incomplete} incomplete)", many.len()),
                    true,
                )
            }
        }
    }
}

/// Previews a proposed Symmetry Order/Mirror change's effect on every tier's orbit BEFORE
/// `setup_apply_symmetry_callback` actually applies it.
///
/// Mirroring the gear-remap path's own dry-run preview (`gear_remap_preview`), which
/// likewise never mutates `design` to compute its summary.
///
/// Clones
/// `design.meta` and swaps in only the two fields Apply Symmetry can change --
/// never `design` itself -- then counts a tier the same way its own "orbit"
/// table badge would ([`orbit_status_text`]'s second return value), so "N tiers
/// would become incomplete" always agrees with what those rows will show once
/// applied.
#[must_use]
pub fn tiers_incomplete_under_proposed_symmetry(
    design: &Design,
    symmetry_order: u32,
    mirror: bool,
) -> usize {
    let mut proposed_meta = design.meta.clone();
    proposed_meta.symmetry_order = symmetry_order;
    proposed_meta.mirror = mirror;
    design
        .tiers
        .iter()
        .filter(|tier| {
            let units = indicatrix_cut_core::orbit_units(&tier.indices, &proposed_meta);
            orbit_status_text(&units).1
        })
        .count()
}

/// One tier's display label for a validation-banner mention.
///
/// `"tier 5 (Girdle)"` when named, else `"tier 5"`. 1-based to match the tier table's own
/// `#` column, which is what a cutter actually reads off screen.
#[must_use]
pub fn tier_label(design: &Design, tier_index: usize) -> String {
    let Some(tier) = design.tiers.get(tier_index) else {
        return format!("tier {}", tier_index + 1);
    };
    if tier.name.is_empty() || indicatrix_cut_core::is_legacy_123_abc(&tier.name) {
        let labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
        labels.get(tier_index).map_or_else(
            || format!("tier {}", tier_index + 1),
            |l| format!("tier {} ({})", tier_index + 1, l.code),
        )
    } else {
        format!("tier {} ({})", tier_index + 1, tier.name)
    }
}

/// Patches [`TierRow::multi_selected`] onto every row in `rows` from `multi_selected`.
///
/// The post-pass a full tier-list rebuild ([`super::rows::
/// tier_items`]/[`super::rows::tier_items_stale`]) needs to survive with the live
/// multi-select highlight intact, since neither builder itself knows about
/// `EditorState::multi_selected` (both always set the flag to `false`; see their own doc
/// comments).
///
/// Every call site that replaces the WHOLE `editor_tiers` model
/// applies this immediately afterward: `view::refresh_editor_panel`/
/// `push_stale_content`, and `auto_solve`'s background-solve completion.
/// `setup_toggle_multi_select_callback` is the one exception -- it patches the
/// flag onto an ALREADY-pushed model in place instead, using this same function,
/// since toggling a selection changes nothing about `Design` and must never
/// re-run a full tier-list rebuild.
pub fn apply_multi_selection(
    rows: &mut [TierRow],
    multi_selected: &std::collections::BTreeSet<usize>,
) {
    for row in rows {
        row.multi_selected =
            usize::try_from(row.index).is_ok_and(|index| multi_selected.contains(&index));
    }
}

/// The first name in `names` (a `MeetConstraint::MeetNamed` constraint's typed list) that
/// does not resolve against `design`'s current tiers.
///
/// Built from the SAME [`MeetNameResolver`] `indicatrix::geometry::meet_solver::solve`
/// itself uses, so a name that would otherwise silently degrade to a dropped token inside
/// the solver (see that module's own doc comment) is instead caught at Save Tier time
/// with a specific, actionable message.
///
/// `None` when every name resolves to a
/// real tier or is a recognized meet-point word/connective prose
/// (`TokenResolution::Ignorable`).
#[must_use]
pub fn first_unresolved_meet_name(design: &Design, names: &[String]) -> Option<String> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    names
        .iter()
        .find(|name| matches!(resolver.resolve_token(name), TokenResolution::Unresolved))
        .cloned()
}

/// The design's own representative crown/pavilion facet angles, in degrees (magnitudes).
///
/// For the crown-window-estimate margin ([`tier_margin_and_risk`]) and the
/// proportion-verdict angle metrics (`view::push_yield_and_proportions`): the tier whose
/// name contains "main" (case-insensitive) on each side, or -- when no tier is named that
/// way -- the tier with the largest magnitude on that side.
///
/// "Crown Main"/"Pavilion
/// Main" is this crate's own template naming convention
/// ([`indicatrix_cut_core::ConstraintTier::standard_round_brilliant`] and every
/// [`indicatrix_cut_core::templates::TEMPLATES`] entry), so this reads the
/// real main facet for every template-derived design and falls back to a
/// reasonable guess for a hand-authored one using different names. `None` on
/// either side when the design has no tier on that side at all.
///
/// Classifies each tier by [`classify_blocks`] (the SAME classification
/// [`indicatrix_cut_core::design::export::planes`]/`super::rows::
/// cutting_instructions_rows`'s own "side" column already use), not by the sign of
/// `angle_deg` alone -- a girdle tier's own angle sits at magnitude 90 degrees,
/// not `0.0`, and is a real, signed, nonzero value that a bare `angle > 0.0`/
/// `angle < 0.0` test would otherwise fold into "crown"/"pavilion" as though it
/// were a real crown/pavilion facet. A design whose only "crown" tier is a
/// +90-degree girdle used to judge 90 degrees as the crown's representative
/// angle for the proportion-verdict chips.
#[must_use]
pub fn representative_crown_and_pavilion_angles_deg(design: &Design) -> (Option<f64>, Option<f64>) {
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let mut crown_main: Option<f64> = None;
    let mut crown_largest: Option<f64> = None;
    let mut pavilion_main: Option<f64> = None;
    let mut pavilion_largest: Option<f64> = None;
    for (tier, block) in design.tiers.iter().zip(&blocks) {
        let is_main = tier.name.to_lowercase().contains("main");
        match block {
            Block::Crown => {
                let angle = tier.angle_deg;
                crown_largest = Some(crown_largest.map_or(angle, |m| angle.max(m)));
                if is_main {
                    crown_main = Some(crown_main.map_or(angle, |m| angle.max(m)));
                }
            }
            Block::Pavilion => {
                let magnitude = tier.angle_deg.abs();
                pavilion_largest = Some(pavilion_largest.map_or(magnitude, |m| magnitude.max(m)));
                if is_main {
                    pavilion_main = Some(pavilion_main.map_or(magnitude, |m| magnitude.max(m)));
                }
            }
            Block::Girdle => {}
        }
    }
    (
        crown_main.or(crown_largest),
        pavilion_main.or(pavilion_largest),
    )
}

/// [`EditorTierItem::margin_text`]/`risk_level` for one tier, classified by
/// `block` -- pavilion tiers ([`Block::Pavilion`]) read the plain table-only
/// critical-angle margin ([`indicatrix_cut_core::tier_margin_deg`]/
/// [`indicatrix_cut_core::windowing_risk`]); crown tiers ([`Block::Crown`])
/// read the crown-window ESTIMATE ([`indicatrix_cut_core::
/// crown_window_margin_deg`]/[`indicatrix_cut_core::crown_windowing_risk`])
/// against `pavilion_partner_deg`, suffixed `" (est.)"` so it is
/// never mistaken for the same table-only certainty a pavilion row's margin
/// carries -- see that function's own doc comment for exactly what the
/// estimate does and does not model. A girdle tier ([`Block::Girdle`]), or a
/// crown tier when no pavilion angle could be found at all, always reads
/// `("", -1)`, "nothing to show," not a wrong badge. `n_d` is the design's
/// effective refractive index, the same value the design settings panel's
/// RI/critical-angle readouts show.
///
/// `block` is the caller's own [`classify_blocks`] result for this tier
/// (`super::rows::RowContext::tier_blocks`, computed once per design refresh),
/// not inferred from `tier_angle_deg`'s own sign/magnitude here -- a girdle
/// tier's angle sits at magnitude 90 degrees, not `0.0`. The previous
/// `angle == 0.0` check both mis-classified an exact table/culet tier (a real
/// crown/pavilion facet, angle `0.0`) as girdle, AND mis-classified a real
/// +/-90-degree girdle tier as crown/pavilion, giving it a meaningless
/// crown-window badge.
pub(super) fn tier_margin_and_risk(
    tier_angle_deg: f64,
    block: Block,
    n_d: f64,
    pavilion_partner_deg: Option<f64>,
) -> (String, i32) {
    let risk_level_of = |risk: indicatrix_cut_core::Risk| match risk {
        indicatrix_cut_core::Risk::Safe => 0,
        indicatrix_cut_core::Risk::Marginal => 1,
        indicatrix_cut_core::Risk::Windows => 2,
    };
    match block {
        Block::Pavilion => {
            let margin = indicatrix_cut_core::tier_margin_deg(tier_angle_deg, n_d);
            let risk_level =
                risk_level_of(indicatrix_cut_core::windowing_risk(tier_angle_deg, n_d));
            (format!("{margin:+.1}\u{b0}"), risk_level)
        }
        Block::Crown => {
            let Some(pavilion_deg) = pavilion_partner_deg else {
                return (String::new(), -1);
            };
            let margin =
                indicatrix_cut_core::crown_window_margin_deg(pavilion_deg, tier_angle_deg, n_d);
            let risk_level = risk_level_of(indicatrix_cut_core::crown_windowing_risk(
                pavilion_deg,
                tier_angle_deg,
                n_d,
            ));
            (format!("{margin:+.1}\u{b0} (est.)"), risk_level)
        }
        Block::Girdle => (String::new(), -1),
    }
}
