//! Small per-tier text/formatting helpers and the `EditorModel.tiers`/
//! `multi_selected_count` push helpers -- the pieces [`super::rows`]'s row builders
//! share, plus the handful other editor callbacks reach directly (multi-select,
//! the tier-list push helpers, the symmetry-preview and meet-name-resolution
//! checks).

use crate::{EditorTierItem, IndexChipItem, MainWindow};
use indicatrix::geometry::meet_solver::{MeetConstraint, MeetNameResolver, TokenResolution};
use indicatrix_cut_core::{Design, OrbitUnit, TierTarget};
use slint::{ComponentHandle, Model, ModelRc, VecModel};

/// Splits a tier's [`MeetConstraint`] AND its [`TierTarget`] (if any) into the
/// `(constraint_kind, constraint_text)` pair [`EditorTierItem`] carries and the
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

/// The tier table's ANGLE cell text -- always two decimals:
/// a wheel/keyboard nudge accumulates plain `f64` noise (e.g.
/// `-40.300000000000004`), and showing that raw `Display` output read as a
/// cut-off, broken number rather than a rounding artifact. A cutter compares
/// this against a 2-decimal gauge anyway, so truncating the DISPLAY (never the
/// stored value -- the inline editor and the tier-edit form both still read the
/// full, unrounded value) is honest and matches every other angle readout in
/// this app (`view::refresh_design_settings`'s critical-angle text, the margin
/// column, ...).
pub(super) fn format_angle_cell(angle_deg: f64) -> String {
    format!("{angle_deg:.2}")
}

/// One index-wheel position's display text -- an integer when it lands exactly
/// on a whole tooth (the overwhelming common case), else two decimals. Mirrors
/// [`format_angle_cell`]'s reasoning for the same nudge-noise problem, but
/// integral-aware: `"24"` reads better than `"24.00"` for the ordinary case,
/// while a genuinely fractional index (rare, but real -- see
/// `ManufacturabilityWarning::FractionalIndex`) still shows its non-integral part.
pub(super) fn format_index_value(value: f64) -> String {
    if value.fract().abs() < 1e-9 {
        format!("{value:.0}")
    } else {
        format!("{value:.2}")
    }
}

/// Builds one [`IndexChipItem`] per entry in `indices`, flagging exactly the
/// occurrences also present in `detached` -- the inspector's per-facet chip row,
/// pushed by `gui::editor::view::selected_tier_chips` as its
/// own `EditorModel.selected_tier_chips`, NOT a field on [`EditorTierItem`] --
/// that struct's `Vec` is built on a background thread for a large design
/// (`gui::editor::auto_solve`) and shipped to the UI thread inside
/// `BackgroundSolveResult`, which must stay `Send`; a `[IndexChipItem]` field
/// there is `ModelRc`-backed (`Rc`, not `Send`) and does not compile.
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
pub(in crate::gui::editor) fn index_chip_items(
    indices: &[f64],
    detached: &[f64],
) -> Vec<IndexChipItem> {
    indices
        .iter()
        .map(|&position| IndexChipItem {
            position: position as f32,
            label: format_index_value(position).into(),
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

/// Previews a proposed Symmetry Order/Mirror change's effect
/// on every tier's orbit BEFORE `setup_apply_symmetry_callback` actually applies
/// it, mirroring the gear-remap path's own dry-run preview (`gear_remap_preview`),
/// which likewise never mutates `design` to compute its summary. Clones
/// `design.meta` and swaps in only the two fields Apply Symmetry can change --
/// never `design` itself -- then counts a tier the same way its own "orbit"
/// table badge would ([`orbit_status_text`]'s second return value), so "N tiers
/// would become incomplete" always agrees with what those rows will show once
/// applied.
#[must_use]
pub(in crate::gui::editor) fn tiers_incomplete_under_proposed_symmetry(
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

/// One tier's display label for a validation-banner mention -- `"tier 5
/// (Girdle)"` when named, else `"tier 5"`. 1-based to match the tier table's
/// own `#` column, which is what a cutter actually reads off screen.
pub(super) fn tier_label(design: &Design, tier_index: usize) -> String {
    design.tiers.get(tier_index).map_or_else(
        || format!("tier {}", tier_index + 1),
        |tier| {
            if tier.name.is_empty() {
                format!("tier {}", tier_index + 1)
            } else {
                format!("tier {} ({})", tier_index + 1, tier.name)
            }
        },
    )
}

/// Patches [`EditorTierItem::multi_selected`] onto every row in `rows` from
/// `multi_selected` -- the post-pass a full tier-list rebuild ([`super::rows::
/// tier_items`]/[`super::rows::tier_items_stale`]) needs to survive with the live
/// multi-select highlight intact, since neither builder itself knows about
/// `EditorState::multi_selected` (both always set the flag to `false`; see their
/// own doc comments). Every call site that replaces the WHOLE `editor_tiers` model
/// applies this immediately afterward: `view::refresh_editor_panel`/
/// `push_stale_content`, and `auto_solve`'s background-solve completion.
/// `setup_toggle_multi_select_callback` is the one exception -- it patches the
/// flag onto an ALREADY-pushed model in place instead, using this same function,
/// since toggling a selection changes nothing about `Design` and must never
/// re-run a full tier-list rebuild.
pub(in crate::gui::editor) fn apply_multi_selection(
    rows: &mut [EditorTierItem],
    multi_selected: &std::collections::BTreeSet<usize>,
) {
    for row in rows {
        row.multi_selected =
            usize::try_from(row.index).is_ok_and(|index| multi_selected.contains(&index));
    }
}

/// Pushes `EditorModel.multi_selected_count` -- the tier table's "N selected"
/// header indicator (`editor_tier_table.slint`) -- kept a SEPARATE call from
/// [`apply_multi_selection`] rather than folded into it, since one of that
/// function's call sites (`auto_solve`'s background-solve worker thread) runs off
/// the UI thread and must never touch a Slint global; every caller of THIS
/// function, by contrast, already runs on the UI thread (the two `view::` refresh
/// paths, the toggle/selection-changed callbacks in `callbacks::tier_actions`, and
/// `auto_solve`'s UI-thread completion handler).
pub(in crate::gui::editor) fn push_multi_selected_count(ui: &MainWindow, count: usize) {
    ui.global::<crate::EditorModel>()
        .set_multi_selected_count(i32::try_from(count).unwrap_or(i32::MAX));
}

/// Pushes `rows` into `EditorModel.tiers`, reusing the existing model via
/// [`slint::Model::set_row_data`] when the row count is unchanged instead of
/// replacing the whole `ModelRc` -- an ordinary edit (`ModifyTier`), an undo/redo
/// that doesn't change the tier count, or a background-solve completion never
/// resizes the list, and Slint only recreates a `for` loop's per-row component
/// tree when the MODEL ITSELF changes identity, not when one row's data does. A
/// wholesale replacement tears down and rebuilds every row's component tree on every
/// refresh, including one mid-inline-edit -- dropping keyboard focus
/// out of an open inline angle edit (`editor_tier_table.slint`'s `TierAngleCell`)
/// on every refresh, which is exactly what the same-length reuse path above avoids.
/// A structural edit that actually changes the tier count
/// (`AddTier`/`RemoveTier`, or undoing/redoing one) still needs a real replacement
/// -- `set_row_data` cannot resize a model -- so that case still replaces the model
/// wholesale.
///
/// Explicitly invokes `EditorModel.recompute_dirty` afterward rather than relying
/// on `editor.slint`'s own `changed tiers => { recompute_dirty(); }` watcher to
/// catch it: that watcher only fires when the `tiers` PROPERTY itself is
/// reassigned (the `set_tiers` branch below), never when `set_row_data` merely
/// mutates the SAME `ModelRc`'s contents in place -- without this explicit call,
/// the dirty/"Unsaved" indicator would stop updating for the common case (an
/// edit that doesn't change the tier count) the moment that branch is taken.
pub(in crate::gui::editor) fn push_tiers(ui: &MainWindow, rows: Vec<EditorTierItem>) {
    push_rows(
        &ui.global::<crate::EditorModel>().get_tiers(),
        rows,
        |model| {
            ui.global::<crate::EditorModel>().set_tiers(model);
        },
    );
    ui.global::<crate::EditorModel>().invoke_recompute_dirty();
}

/// The general form of [`push_tiers`]'s own in-place-update trick (see that
/// function's own doc comment for the full "why" -- rebuilding a Slint `for`
/// loop's whole component tree on every refresh dropped keyboard focus out of an
/// open inline edit): reuses `current` via [`slint::Model::set_row_data`] when its
/// row count already matches `rows`, calling `set` with a fresh `ModelRc` only
/// when the length actually changed (an add/remove, not an ordinary edit).
///
/// `set` is called ONLY on that replace path, never on the reuse path -- exactly
/// matching [`push_tiers`]'s own original behaviour (see its doc comment on why
/// `EditorModel.tiers`'s reassignment is what fires `editor.slint`'s `changed
/// tiers` watcher, and why reusing `current` in place must not also trigger it
/// again for nothing). A caller pushing a property with no such watcher (every
/// other use below) still benefits: skipping the property write when nothing
/// structural changed is itself the point, whether or not Slint's own property
/// setter would already have elided a same-model reassignment.
///
/// `view::push_stale_content`/
/// `push_manufacturability_and_preform_scratch`/`push_selected_tier_chips`
/// (`view.rs`, not this file) route `EditorModel.cutting_rows`/
/// `manufacturability_warnings`/`manufacturability_warning_tiers`/
/// `selected_tier_chips` through this same generic helper instead of each
/// replacing the model with a brand-new `ModelRc<VecModel<_>>` on every single
/// refresh, even a same-length one -- this is what lets each of
/// those call sites share the identical incremental-update behaviour `push_tiers`
/// already had, without four near-duplicate copies of the same length-check.
///
/// Callers still own deciding WHAT to push (the `Vec<T>` computation itself is
/// unchanged); this only changes HOW it reaches `EditorModel`.
pub(in crate::gui::editor) fn push_rows<T: Clone + 'static>(
    current: &ModelRc<T>,
    rows: Vec<T>,
    set: impl FnOnce(ModelRc<T>),
) {
    if current.row_count() == rows.len() {
        for (index, row) in rows.into_iter().enumerate() {
            current.set_row_data(index, row);
        }
    } else {
        set(ModelRc::new(VecModel::from(rows)));
    }
}

/// The first name in `names` (a `MeetConstraint::MeetNamed` constraint's typed
/// list) that does not resolve against `design`'s current tiers -- built from the
/// SAME [`MeetNameResolver`] `indicatrix::geometry::meet_solver::solve` itself
/// uses, so a name that would otherwise silently degrade to a dropped token inside
/// the solver (see that module's own doc comment) is instead caught at Save Tier
/// time with a specific, actionable message. `None` when every name resolves to a
/// real tier or is a recognized meet-point word/connective prose
/// (`TokenResolution::Ignorable`).
pub(in crate::gui::editor) fn first_unresolved_meet_name(
    design: &Design,
    names: &[String],
) -> Option<String> {
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    names
        .iter()
        .find(|name| matches!(resolver.resolve_token(name), TokenResolution::Unresolved))
        .cloned()
}

/// The design's own representative crown/pavilion facet angles, in degrees
/// (magnitudes), for the crown-window-estimate margin
/// ([`tier_margin_and_risk`]) and the proportion-verdict angle metrics
/// (`view::push_yield_and_proportions`): the tier whose name contains "main"
/// (case-insensitive) on each side, or -- when no tier is named that way --
/// the tier with the largest magnitude on that side. "Crown Main"/"Pavilion
/// Main" is this crate's own template naming convention
/// ([`indicatrix_cut_core::ConstraintTier::standard_round_brilliant`] and every
/// [`indicatrix_cut_core::templates::TEMPLATES`] entry), so this reads the
/// real main facet for every template-derived design and falls back to a
/// reasonable guess for a hand-authored one using different names. `None` on
/// either side when the design has no tier on that side at all.
pub(in crate::gui::editor) fn representative_crown_and_pavilion_angles_deg(
    design: &Design,
) -> (Option<f64>, Option<f64>) {
    let mut crown_main: Option<f64> = None;
    let mut crown_largest: Option<f64> = None;
    let mut pavilion_main: Option<f64> = None;
    let mut pavilion_largest: Option<f64> = None;
    for tier in &design.tiers {
        let angle = tier.angle_deg;
        let is_main = tier.name.to_lowercase().contains("main");
        if angle > 0.0 {
            crown_largest = Some(crown_largest.map_or(angle, |m| angle.max(m)));
            if is_main {
                crown_main = Some(crown_main.map_or(angle, |m| angle.max(m)));
            }
        } else if angle < 0.0 {
            let magnitude = angle.abs();
            pavilion_largest = Some(pavilion_largest.map_or(magnitude, |m| magnitude.max(m)));
            if is_main {
                pavilion_main = Some(pavilion_main.map_or(magnitude, |m| magnitude.max(m)));
            }
        }
    }
    (
        crown_main.or(crown_largest),
        pavilion_main.or(pavilion_largest),
    )
}

/// [`EditorTierItem::margin_text`]/`risk_level` for one tier -- pavilion tiers
/// (`tier_angle_deg < 0.0`) read the plain table-only critical-angle margin
/// ([`indicatrix_cut_core::tier_margin_deg`]/[`indicatrix_cut_core::windowing_risk`]);
/// crown tiers (`tier_angle_deg >
/// 0.0`) read the crown-window ESTIMATE ([`indicatrix_cut_core::
/// crown_window_margin_deg`]/[`indicatrix_cut_core::crown_windowing_risk`])
/// against `pavilion_partner_deg`, suffixed `" (est.)"` so it is
/// never mistaken for the same table-only certainty a pavilion row's margin
/// carries -- see that function's own doc comment for exactly what the
/// estimate does and does not model. A girdle tier (`tier_angle_deg == 0.0`,
/// or a crown tier when no pavilion angle could be found at all) always reads
/// `("", -1)`, "nothing to show," not a wrong badge. `n_d` is the design's
/// effective refractive index, the same value the design settings panel's
/// RI/critical-angle readouts show.
pub(super) fn tier_margin_and_risk(
    tier_angle_deg: f64,
    n_d: f64,
    pavilion_partner_deg: Option<f64>,
) -> (String, i32) {
    let risk_level_of = |risk: indicatrix_cut_core::Risk| match risk {
        indicatrix_cut_core::Risk::Safe => 0,
        indicatrix_cut_core::Risk::Marginal => 1,
        indicatrix_cut_core::Risk::Windows => 2,
    };
    if tier_angle_deg < 0.0 {
        let margin = indicatrix_cut_core::tier_margin_deg(tier_angle_deg, n_d);
        let risk_level = risk_level_of(indicatrix_cut_core::windowing_risk(tier_angle_deg, n_d));
        (format!("{margin:+.1}\u{b0}"), risk_level)
    } else if tier_angle_deg > 0.0 {
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
    } else {
        (String::new(), -1)
    }
}
