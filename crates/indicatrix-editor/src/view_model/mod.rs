//! Plain-Rust view models: the tier table's rows, the inspector's index chips, the
//! cut-order schedule rows, the yield/proportion texts and verdicts, and the
//! validation banner.
//!
//! Each UI maps these structs to its own toolkit's row types
//! (the desktop in a thin adapter over its generated Slint structs).
//!
//! Split by responsibility: [`curve_path`] (tilt curves as SVG path data),
//! [`live_margin`] (the Angle field's live critical-angle bar),
//! [`row_format`] (per-tier text/formatting helpers),
//! [`rows`] (the tier-row builders), [`yield_report`] (yield/proportion texts,
//! proportion verdicts, the cut-order rows) and [`solid_status`] (the
//! validation-banner text and the viewport-plane conversion).

pub mod curve_path;
pub mod live_margin;
pub mod row_format;
pub mod rows;
pub mod solid_status;
pub mod yield_report;

#[cfg(test)]
mod tests;

/// Whether a [`TierRow`] stands for a flat tier or a concave one.
///
/// A concave tier lives in `design.concave_tiers`, a separate list from
/// `design.tiers`, so a row's [`TierRow::index`] is only meaningful together with
/// its kind: it addresses the list the kind names.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TierRowKind {
    /// A tier of `design.tiers`.
    #[default]
    Flat,
    /// A tier of `design.concave_tiers`.
    Concave,
}

/// One tier-table row. Field for field the desktop's `EditorTierItem` (see
/// `ui/types.slint` for how each is displayed); the integer fields keep that type's
/// `i32` so an adapter is a plain copy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TierRow {
    /// The tier's position in `design.tiers`, or in `design.concave_tiers` for a
    /// [`TierRowKind::Concave`] row.
    pub index: i32,
    /// The angle, two decimals (see [`row_format::format_angle_cell`]).
    pub angle_deg: String,
    /// The angle, full precision -- what an inline editor or form re-seeds from, so a
    /// re-save never rounds a value the design still holds exactly.
    pub angle_full: String,
    /// The tier's name.
    pub name: String,
    /// The tier's code in cutting order (`P1`, `G1`, `C1`, `T`, `Culet`, a concave tier's
    /// `P4`; see `indicatrix_cut_core::Design::tier_codes`) -- what the table's code column
    /// and the cutting sheet's label column show. Empty only for a row built by hand.
    pub code: String,
    /// The index list, display-rounded (see [`row_format::format_index_value`]).
    pub indices: String,
    /// The index list, full precision (see [`Self::angle_full`]).
    pub indices_full: String,
    /// `0` meet existing, `1` meet named, `2` scale reference, `3`/`4`/`5` a
    /// depth/girdle-thickness/table-width target.
    pub constraint_kind: i32,
    /// The constraint's text (names, or a number).
    pub constraint_text: String,
    /// The solved mast, four decimals, or `"-"`/`"?"`.
    pub mast: String,
    /// The mast in millimetres, empty when nothing anchors a real scale.
    pub mast_mm: String,
    /// The mast unrounded.
    pub mast_full: String,
    /// The solve strategy's label (or `"not solved"`/`"blocked"`/...).
    pub strategy: String,
    /// Whether the strategy is an estimate or untrusted.
    pub strategy_is_uncertain: bool,
    /// The solver's per-tier detail sentence.
    pub strategy_detail: String,
    /// Whether this tier is one a missing anchor names.
    pub needs_anchor: bool,
    /// What the source `.asc` said this facet meets, `""` when nothing to adopt.
    pub imported_meet_text: String,
    /// The orbit summary (`"orbit x8"`, `"6/8 orbit"`, ...).
    pub orbit_status: String,
    /// Whether the orbit summary should be flagged.
    pub orbit_incomplete: bool,
    /// Whether any occurrence is detached from its orbit.
    pub is_detached: bool,
    /// `"Crown"`/`"Pavilion"`/`"Girdle"`.
    pub block: String,
    /// The critical-angle margin text, `""` when not applicable.
    pub margin_text: String,
    /// `0` safe, `1` marginal, `2` windows, `-1` nothing to show.
    pub risk_level: i32,
    /// The tiers this tier's meet constraint resolved against.
    pub meet_partners_text: String,
    /// Manufacturability findings for this tier, `"; "`-joined.
    pub warning_text: String,
    /// Whether the row is in the multi-selection (patched afterwards, see
    /// [`row_format::apply_multi_selection`]).
    pub multi_selected: bool,
    /// A pending proposal's ghost angle, `""` when none (see
    /// [`rows::apply_proposed_angles`]).
    pub proposed_angle: String,
    /// Flat or concave; decides which design list [`Self::index`] addresses.
    pub kind: TierRowKind,
    /// A concave row's tool line, the four
    /// [`indicatrix_cut_core::design::ConcaveTier::second_line_fields`] columns
    /// joined with two spaces (`"CYL  +10.00°  X = 0.000, ...  D/W = 0.400, plunge"`);
    /// empty for a flat row.
    pub tool_line: String,
    /// The relation that drives this tier's angle, as a cutter reads it (`"C1 - 4"`),
    /// or empty when the angle is free. A non-empty text is what the tier table's link
    /// badge and the inspector's "Follows a relation" note are built from.
    pub relation_text: String,
}

/// One per-facet chip in the inspector's Tier tab -- see
/// [`row_format::index_chip_items`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct IndexChip {
    /// The index-wheel position.
    pub position: f32,
    /// The position's display text.
    pub label: String,
    /// Whether this occurrence is detached from its orbit.
    pub detached: bool,
}

/// One cut-order schedule row -- see [`yield_report::cutting_instructions_rows`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CuttingRow {
    /// The 0-based cutting order.
    pub order_idx: i32,
    /// `1` crown, `-1` pavilion, `0` girdle.
    pub side: i32,
    /// The tier's code in cutting order (`P1`, `G1`, `C1`, `T`), as the printed cutting sheet
    /// writes it; the name (or `"#N"`) only if no code exists.
    pub facet: String,
    /// The angle, two decimals.
    pub angle: String,
    /// The index list as the cutting sheet prints it: dash-joined, zero-padded (`96-08-16`),
    /// `"-"` when empty.
    pub index_val: String,
    /// The tier's notes.
    pub notes: String,
    /// A concave tier's tool line, one string per column (tool, theta, displacement,
    /// details) from
    /// [`indicatrix_cut_core::design::ConcaveTier::second_line_fields`], shown under
    /// the facet line; `None` for a flat tier.
    pub second_line: Option<[String; 4]>,
}
