//! Yield/weight and proportion (table %, crown/pavilion depth, girdle thickness,
//! length-to-width) view-model text, the proportion-guidance verdict chips.
//!
//! And the Edit tab's own cut-order schedule rows -- everything the Preform/Yield tabs
//! read once a design solves.

use super::{
    CuttingRow,
    row_format::{
        format_angle_cell, format_index_value, representative_crown_and_pavilion_angles_deg,
    },
};
use crate::material_lookup::EditorMaterialLookup;
use indicatrix::geometry::meet_solver::{Block, SolvedTier, classify_blocks};
use indicatrix_cut_core::{Design, design::TierRef};

/// `design`'s current yield/weight figures, formatted for `EditorView`'s read-only
/// display fields.
///
/// Pushed by the explicit "Solve" action, a debounced background solve, and a completed
/// solid-preview replan alike (`view::panel::refresh_editor_panel_from_solve`,
/// `auto_solve::apply:: push_solve_dependent_background_fields`, `view::viewport::
/// push_solved_preview`), not just the synchronous path -- an ordinary edit with no solve
/// at all instead clears these to `""` via `view::panel_stale::
/// push_stale_preform_and_yield`, since `Design::yield_report` needs an already-solved
/// mast list this function does not have without one.
///
/// Returns `(volumetric_yield_text, carat_weight_text, specific_gravity_used_text,
/// preform_fit_warning_text)` -- all four empty when the design does not currently
/// solve ([`indicatrix_cut_core::MissingAnchor`], already surfaced by the banner).
///
/// `custom_sg` is the catalogue's custom-material specific-gravity table --
/// see [`yield_report_texts_from_solved`]'s own doc comment for where a
/// caller sources it from.
#[must_use]
pub fn yield_report_texts(
    design: &Design,
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    let Ok(solved) = design.solve() else {
        return (String::new(), String::new(), String::new(), String::new());
    };
    yield_report_texts_from_solved(design, &solved, custom_sg)
}

/// [`yield_report_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see `super::rows::tier_items_from_solved`'s
/// own doc comment for why this exists.
///
/// Never re-solves, and
/// never returns the all-empty tuple [`yield_report_texts`] falls back to on a
/// `MissingAnchor`: a caller holding a real `solved` slice already knows the
/// design solves.
///
/// Resolves `design.material`'s specific gravity through
/// [`EditorMaterialLookup`] instead of only [`Design::yield_report`]'s built-in
/// table, so a CUSTOM catalogue material's own recorded SG reaches the
/// carat-weight estimate too -- not just a built-in preset or a per-design
/// override. This module has no live `RenderContext` of its own (a pure
/// `Design`-only view-model helper), so `custom_sg` is threaded in by the caller
/// instead -- `view.rs`/`auto_solve.rs` each read it straight off their own
/// `RenderContext::custom_material_specific_gravity` (cloned before crossing onto
/// a background thread, same as `RenderContext::custom_materials` already is), so
/// this stays a single source of truth with no thread-local mirror to drift out
/// of sync.
pub fn yield_report_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    custom_sg: &[(String, f64)],
) -> (String, String, String, String) {
    let catalogue = EditorMaterialLookup::new(&[]).with_specific_gravity(custom_sg);
    let report = design.yield_report_with(solved, &catalogue);

    let volumetric_yield_text = report
        .volumetric_yield
        .map(|y| format!("{:.2}%", y * 100.0))
        .unwrap_or_default();
    let carat_weight_text = report
        .carat_weight
        .map(|c| format!("{c:.4} ct (est.)"))
        .unwrap_or_default();
    // "(override)" whenever the figure came from the user's typed number rather than
    // the selected preset's table figure.
    let specific_gravity_used_text = report.specific_gravity_used.map_or_else(String::new, |sg| {
        if design.material.specific_gravity_override.is_some() {
            format!("{sg:.3} (override)")
        } else {
            format!("{sg:.3}")
        }
    });
    let preform_fit_warning_text = report
        .preform_fit
        .map(|fit| fit.to_string())
        .unwrap_or_default();

    (
        volumetric_yield_text,
        carat_weight_text,
        specific_gravity_used_text,
        preform_fit_warning_text,
    )
}

/// `design`'s proportion readouts for the Preform tab's "Proportions" section
/// -- table %, crown height, pavilion depth, total depth, and length-to-width,
/// the figures a cutter actually quotes.
///
/// Built from
/// [`Design::stone_proportions`] and converted to millimetres via
/// [`Design::yield_report`]'s own scale factor whenever a trusted one exists
/// (a girdle diameter is set and the design measures); otherwise shown in the
/// design's own mast units with no unit suffix, rather than guessing a scale.
///
/// Returns `(table_percent_text, crown_height_text, pavilion_depth_text,
/// total_depth_text, length_to_width_text)`, every one of them `"-"` when the
/// design has no tiers at all, does not solve, isn't currently a closed solid,
/// or (the two depth fields, and `total_depth` too, since it is
/// only meaningful once a live girdle band exists to separate crown from
/// pavilion) has no vertical girdle plane with a live facet to measure from --
/// see [`indicatrix::geometry::stone_metrics::StoneProportions`]'s own doc
/// comment for why the two depth fields are `Option`. Pushed by the explicit
/// "Solve" action, a debounced background solve, and a completed solid-preview
/// replan alike -- see [`yield_report_texts`]'s own doc comment for the three
/// call sites, none of which are "every edit."
///
/// A tierless design still solves (an empty
/// mast list is a valid, closed, zero-plane solve -- `Design::solve` never
/// rejects it), and `stone_proportions` then measures the bare preform block:
/// table 100%, crown/pavilion 0%, `total_depth` the preform's own depth. Those
/// are honest numbers about the preform and fabricated ones about a stone that
/// does not exist yet, so the `tiers.is_empty()` guard below runs BEFORE the
/// solve -- including for `total_depth`, which is not an `Option` on
/// [`indicatrix::geometry::stone_metrics::StoneProportions`] and so has no
/// OTHER way to read as "-" once this guard is past (its `girdle_thickness.
/// is_some()` gate just below only ever hides it for a design that DOES have
/// tiers but no live girdle band -- this tierless guard is what stops the
/// bare preform's own numbers from reaching that gate at all).
/// [`girdle_and_ratio_texts`] carries the identical guard for the same reason;
/// the `view` module's own `proportions_texts_from_solved`/
/// `girdle_and_ratio_texts_from_solved` mirror it too, since a caller with an
/// already-`solved` list came from a design that solved -- which, per the
/// above, a tierless one does.
#[must_use]
pub fn proportions_texts(design: &Design) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    let Ok(solved) = design.solve() else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let Some(proportions) = design.stone_proportions(&solved) else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(&solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let table_percent_text = proportions
        .table_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let crown_height_text = proportions
        .crown_height
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let pavilion_depth_text = proportions
        .pavilion_depth
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    // `total_depth` has no `Option`-driven "-" fallback of its own (see
    // `StoneProportions`'s own doc comment) -- printed only when
    // `girdle_thickness` (which DOES have one) is `Some`, the same "is there a
    // live vertical girdle plane to measure a real depth from" signal the
    // other four fields already carry, since `total_depth` otherwise falls
    // back to the preform-clipped block height for a design with tiers but no
    // vertical girdle facet.
    let total_depth_text = if proportions.girdle_thickness.is_some() {
        format!("{:.3}{unit}", proportions.total_depth)
    } else {
        dash()
    };
    let length_to_width_text = proportions
        .length_to_width
        .map_or_else(dash, |v| format!("{v:.3}"));
    (
        table_percent_text,
        crown_height_text,
        pavilion_depth_text,
        total_depth_text,
        length_to_width_text,
    )
}

/// The three proportion readouts [`proportions_texts`] does not expose.
///
/// Girdle thickness (a figure with no existing text home at all) and the printed
/// `C/W%`/`P/W%` ratios every faceting diagram actually prints, as opposed to the
/// absolute crown/pavilion depths [`proportions_texts`] already returns.
///
/// `Design::stone_proportions` has
/// already computed all four (`StoneProportions::girdle_thickness`/`crown_to_width_percent`/
/// `pavilion_to_width_percent`/`girdle_to_width_percent`, see
/// [`indicatrix::geometry::stone_metrics::StoneProportions`]); `view::panel::
/// push_yield_and_proportions` reads all four and pushes them to
/// `EditorModel.girdle_thickness_text`/`crown_to_width_text`/
/// `pavilion_to_width_text`/`girdle_to_width_text` (the editor panel
/// reads them).
///
/// Mirrors [`proportions_texts`]'s own solve-then-measure shape, `"-"` fallback and
/// millimetre-vs-model-unit handling exactly (girdle thickness only -- the three
/// `_to_width_percent` fields are already scale-invariant percentages, exactly like
/// `table_percent`, so they never take the `unit` suffix), so the two families can
/// never disagree about which unit a given figure is shown in.
///
/// Returns `(girdle_thickness_text, crown_to_width_percent_text,
/// pavilion_to_width_percent_text, girdle_to_width_percent_text)`, every one of
/// them `"-"` under the exact same conditions [`proportions_texts`] already
/// documents (does not solve, isn't currently a closed solid, or -- all four here
/// specifically -- no vertical girdle plane with a live facet to measure the
/// girdle band from at all).
#[must_use]
pub fn girdle_and_ratio_texts(design: &Design) -> (String, String, String, String) {
    let dash = || "-".to_string();
    // A tierless design still solves, and `stone_proportions` then measures the bare
    // preform block: girdle 50% of a cube, crown and pavilion 0%. Those are honest
    // numbers about the preform and fabricated ones about the stone, so they must
    // never reach a proportions readout. The same
    // `tiers.is_empty()` guard protects `proportions_texts` (including its
    // `total_depth`) and both
    // `_from_solved` mirrors in `view.rs`, so every reader of these figures agrees.
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash());
    }
    let Ok(solved) = design.solve() else {
        return (dash(), dash(), dash(), dash());
    };
    let Some(proportions) = design.stone_proportions(&solved) else {
        return (dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(&solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let girdle_thickness_text = proportions
        .girdle_thickness
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let crown_to_width_percent_text = proportions
        .crown_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let pavilion_to_width_percent_text = proportions
        .pavilion_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let girdle_to_width_percent_text = proportions
        .girdle_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    (
        girdle_thickness_text,
        crown_to_width_percent_text,
        pavilion_to_width_percent_text,
        girdle_to_width_percent_text,
    )
}

/// One proportion metric's verdict against [`indicatrix_cut_core::proportions_windows`]'s
/// reference table.
///
/// `level` is `0` (`Verdict::Within`), `1` (`Verdict::Near`), `2` (`Verdict::Outside`),
/// or `-1` ("nothing to judge yet": the design does not currently solve/close, or this
/// particular metric has no value to judge -- e.g. no crown tier at all).
///
/// `reason` is the matched window's own
/// one-line explanation, `""` at level `-1`.
pub struct ProportionVerdict {
    /// Verdict level; `-1` means nothing to judge yet.
    pub level: i32,
    /// One-line explanation of the matched window.
    pub reason: String,
}

/// The five [`ProportionVerdict`]s the Preform tab's "Proportion guidance"
/// section shows, one per metric
/// [`indicatrix_cut_core::proportions_windows::Metric`] lists.
pub struct ProportionVerdicts {
    /// Verdict for the table width percentage.
    pub table_pct: ProportionVerdict,
    /// Verdict for the crown angle.
    pub crown_angle: ProportionVerdict,
    /// Verdict for the pavilion angle.
    pub pavilion_angle: ProportionVerdict,
    /// Verdict for the total depth percentage.
    pub total_depth_pct: ProportionVerdict,
    /// Verdict for the girdle percentage.
    pub girdle_pct: ProportionVerdict,
}

/// The "nothing to judge yet" verdict -- see [`ProportionVerdict`]'s own doc
/// comment for what level `-1` means.
const fn verdict_none() -> ProportionVerdict {
    ProportionVerdict {
        level: -1,
        reason: String::new(),
    }
}

/// Looks `value` up against `shape`/`material`/`metric`'s reference window
/// (via [`indicatrix_cut_core::proportions_windows::verdict_for`]) and turns
/// the result into a [`ProportionVerdict`] -- [`verdict_none`] when `value` is
/// `None` (there was nothing to measure) or the lookup itself found no window
/// at all (never happens for a real metric today -- see that function's own
/// doc comment on its Round/Mid fallback).
fn verdict_from(
    value: Option<f64>,
    shape: indicatrix_cut_core::ShapeClass,
    material: indicatrix_cut_core::MaterialClass,
    metric: indicatrix_cut_core::ProportionMetric,
) -> ProportionVerdict {
    let Some(v) = value else {
        return verdict_none();
    };
    let Some((verdict, window)) =
        indicatrix_cut_core::proportions_windows::verdict_for(shape, material, metric, v)
    else {
        return verdict_none();
    };
    let level = match verdict {
        indicatrix_cut_core::Verdict::Within => 0,
        indicatrix_cut_core::Verdict::Near => 1,
        indicatrix_cut_core::Verdict::Outside => 2,
    };
    ProportionVerdict {
        level,
        reason: window.reason.to_string(),
    }
}

/// This design's own [`indicatrix_cut_core::ShapeClass`], for the proportion
/// verdicts above. Only [`indicatrix_cut_core::ShapeClass::Round`] is ever
/// returned (a symmetry order of 6 or more with mirroring on -- the round
/// brilliant family, the one shape this app ships verified, closing templates
/// for, see [`indicatrix_cut_core::templates`]'s own doc comment); every other
/// schedule reads as [`indicatrix_cut_core::ShapeClass::Other`], the honest
/// default this crate has no real cushion/oval/step/trillion classifier for
/// (see `proportions_windows`'s own top doc comment -- `Other` still judges
/// against the same generic lapidary window `Round`'s own Mid/Low band uses).
const fn shape_class_for(
    meta: &indicatrix_cut_core::ScheduleMeta,
) -> indicatrix_cut_core::ShapeClass {
    if meta.symmetry_order >= 6 && meta.mirror {
        indicatrix_cut_core::ShapeClass::Round
    } else {
        indicatrix_cut_core::ShapeClass::Other
    }
}

/// The Preform tab's five proportion-verdict chips, judged against `design`'s
/// already-solved `proportions`.
///
/// The SAME [`indicatrix::geometry::stone_metrics:: StoneProportions`] the plain-number
/// readouts above already read, so the chip and the number next to it can never disagree
/// about the underlying measurement.
///
/// `n_d` is the design's effective refractive index (the same
/// value `view::refresh_design_settings` pushes as `effective_ri_text`).
///
/// Total depth is judged as the SUM of `crown_to_width_percent` +
/// `pavilion_to_width_percent` + `girdle_to_width_percent` -- `StoneProportions`
/// has no standalone "total depth as a percentage of width" field of its own
/// (only the absolute `total_depth`, in model units/mm), and total depth is,
/// by construction, crown height plus pavilion depth plus girdle thickness, so
/// this sum is the honest percentage-of-width equivalent, not an approximation
/// invented for this function.
#[must_use]
pub fn proportion_verdicts(
    design: &Design,
    proportions: &indicatrix::geometry::stone_metrics::StoneProportions,
    n_d: f64,
) -> ProportionVerdicts {
    let shape = shape_class_for(&design.meta);
    let material = indicatrix_cut_core::MaterialClass::from_ri(n_d);
    let (crown_angle_deg, pavilion_angle_deg) =
        representative_crown_and_pavilion_angles_deg(design);
    let total_depth_pct = match (
        proportions.crown_to_width_percent,
        proportions.pavilion_to_width_percent,
        proportions.girdle_to_width_percent,
    ) {
        (Some(c), Some(p), Some(g)) => Some(c + p + g),
        _ => None,
    };
    ProportionVerdicts {
        table_pct: verdict_from(
            proportions.table_percent,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::TablePercent,
        ),
        crown_angle: verdict_from(
            crown_angle_deg,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::CrownAngle,
        ),
        pavilion_angle: verdict_from(
            pavilion_angle_deg,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::PavilionAngle,
        ),
        total_depth_pct: verdict_from(
            total_depth_pct,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::TotalDepthPercent,
        ),
        girdle_pct: verdict_from(
            proportions.girdle_to_width_percent,
            shape,
            material,
            indicatrix_cut_core::ProportionMetric::GirdleThicknessPercent,
        ),
    }
}

/// The Preform tab's Half-Width/Depth fields, converted to millimetres via the same
/// [`Design::yield_report`] scale factor [`proportions_texts`] uses.
///
/// Those two fields are typed and stored in the design's own mast-unit scale (girdle
/// half-width = 1), which reads as ambiguous next to the Yield section's "Girdle Diameter
/// (mm)" a few fields down.
///
/// Returned ALONGSIDE the model-unit value, never in place of it (the
/// fields stay editable in model units -- `PreformSpec` itself has no mm
/// concept); each `""` when the design does not currently solve, or no girdle
/// diameter is set to anchor `mm_per_unit` at all.
#[must_use]
pub fn preform_mm_texts(design: &Design) -> (String, String) {
    let Ok(solved) = design.solve() else {
        return (String::new(), String::new());
    };
    let Some(mm_per_unit) = design.yield_report(&solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
}

/// `EditorModel.preform_y_offset_mm`'s seed value.
///
/// `design.preform_y_offset` (model/mast units) converted to real millimetres via
/// `mm_per_unit`, the SAME anchor [`preform_mm_texts`] converts Half-Width/Depth with.
///
/// Unlike that function, this is pure (no internal
/// `Design::solve`): the caller already has `mm_per_unit` on hand from a
/// SOLVED mast list, or passes `None` when it deliberately never solves (`view::
/// push_stale_content`) -- `""` in that case, the same "cleared, not left
/// showing a superseded value" treatment `preform_mm_texts`' own two fields get
/// there. Formatted as a bare decimal (not `preform_mm_texts`' "\u{2248} ... mm"
/// style) because, unlike those two read-only displays, this field is the
/// editable value `apply_preform_y_offset`'s `Edit::SetPreformYOffset` round-
/// trips through -- an approximation glyph or unit suffix would not re-parse.
#[must_use]
pub fn preform_y_offset_mm_text(preform_y_offset: f64, mm_per_unit: Option<f64>) -> String {
    mm_per_unit.map_or_else(String::new, |mm_per_unit| {
        format!("{:.2}", preform_y_offset * mm_per_unit)
    })
}

/// A short label naming the design currently under edit -- the paired `.asc`'s
/// bare file name when this design was loaded from (or saved to) one, else
/// `"Untitled design"`.
///
/// This is the editor-side half,
/// shown in the status strip; the viewport/render-side half would need a
/// separate `RenderContext` change outside this file.
pub fn design_label_text(asc_filename: Option<&str>) -> String {
    asc_filename.map_or_else(|| "Untitled design".to_string(), str::to_string)
}

/// `design`'s current cut order as the Edit tab's own schedule rows.
///
/// Angle, facet name, index positions and notes, in cut order -- so the cutter can see
/// the design actually being edited rather than only ever the catalogue's original
/// schedule.
///
/// Built from
/// [`Design::try_to_asc_schedule_from_solved`], the same conversion "Export Edited
/// .asc" already uses, so this can never disagree with what an export writes.
///
/// Uses `try_to_asc_schedule_from_solved`
/// (not the panicking `to_asc_schedule_from_solved`) -- every real caller already
/// passes a `solved` it just derived from THIS SAME `design`, so a
/// `SolveMismatch` should not be reachable in practice, but this is a
/// `pub(in crate::gui::editor)` helper with several callers across this crate,
/// none of which should be able to crash the editor over a design/solve pairing
/// that slipped out of sync. Logs and returns an empty schedule rather than
/// panicking; every caller already treats an empty `cutting_rows`/schedule as
/// the ordinary "nothing to show yet" state.
pub fn cutting_instructions_rows(design: &Design, solved: &[SolvedTier]) -> Vec<CuttingRow> {
    // `CuttingRow::side` comes from the solver's own block classification, not from
    // the sign of the angle: that is the same source the tier table's own block
    // column uses, and it distinguishes a girdle facet (neither side) from a crown
    // one, which a sign test cannot.
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    let canonical_labels = indicatrix_cut_core::compute_tier_labels(&design.tiers);
    let schedule = match design.try_to_asc_schedule_from_solved(solved) {
        Ok(schedule) => schedule,
        Err(mismatch) => {
            tracing::warn!(%mismatch, "cutting_instructions_rows: solved masts do not match this design's tier count");
            return Vec::new();
        }
    };
    // `tier_index` is the tier's stored position (what `tier_blocks` is indexed by),
    // `order_idx` its position in the cutting sequence (what the row and an unnamed
    // tier's placeholder label show). A planar design's two are the same number.
    let flat_row =
        |tier_index: usize, order_idx: usize, tier: indicatrix_formats::asc::AscTier| CuttingRow {
            order_idx: order_idx as i32,
            side: match tier_blocks.get(tier_index) {
                Some(Block::Crown) => 1,
                Some(Block::Pavilion) => -1,
                _ => 0,
            },
            facet: if tier.name.is_empty() {
                format!("#{}", order_idx + 1)
            } else if indicatrix_cut_core::is_legacy_123_abc(&tier.name) {
                canonical_labels
                    .get(tier_index)
                    .map_or_else(|| tier.name.clone(), |l| l.display_name.clone())
            } else {
                tier.name
            },
            angle: format_angle_cell(tier.angle_deg.abs()),
            index_val: tier
                .indices
                .into_iter()
                .map(format_index_value)
                .collect::<Vec<_>>()
                .join(", "),
            notes: tier.notes,
            second_line: None,
        };
    if design.concave_tiers.is_empty() {
        // Planar designs keep the schedule's own (stored) order, byte for byte.
        return schedule
            .tiers
            .into_iter()
            .enumerate()
            .map(|(order_idx, tier)| flat_row(order_idx, order_idx, tier))
            .collect();
    }
    // With concave tiers the schedule is read in `cutting_order()`: each tool line
    // sits at the end of its section, the crown's directly above the table. A flat
    // row's `side` still comes from its own stored position, so the lookup keeps
    // `flat_row`'s index (the tier's position in `design.tiers`) apart from the
    // row's `order_idx` (its position in the cutting sequence, which also numbers an
    // unnamed tier's placeholder label).
    let mut flat_tiers: Vec<Option<indicatrix_formats::asc::AscTier>> =
        schedule.tiers.into_iter().map(Some).collect();
    design
        .cutting_order()
        .into_iter()
        .enumerate()
        .filter_map(|(order_idx, tier_ref)| match tier_ref {
            TierRef::Flat(i) => {
                let tier = flat_tiers.get_mut(i)?.take()?;
                Some(flat_row(i, order_idx, tier))
            }
            TierRef::Concave(i) => {
                let tier = design.concave_tiers.get(i)?;
                Some(CuttingRow {
                    order_idx: order_idx as i32,
                    side: if tier.is_crown_side() { 1 } else { -1 },
                    facet: if tier.name.is_empty() {
                        format!("#{}", order_idx + 1)
                    } else {
                        tier.name.clone()
                    },
                    angle: format_angle_cell(tier.angle_deg.abs()),
                    index_val: tier
                        .indices
                        .iter()
                        .copied()
                        .map(format_index_value)
                        .collect::<Vec<_>>()
                        .join(", "),
                    notes: tier.instructions.clone(),
                    second_line: Some(tier.second_line_fields()),
                })
            }
        })
        .collect()
}

/// [`proportions_texts`]'s counterpart for a caller that already has an up-to-date
/// `solved` mast list on hand (a completed background solve or preview replan).
///
/// Mirrors that function's body exactly, minus the internal `design.solve()` it
/// exists to avoid repeating -- INCLUDING its `tiers.is_empty()` guard: a tierless
/// design still solves (an empty mast list is a valid, closed, zero-plane solve), so
/// a caller here can perfectly well be holding exactly that solved-empty list, and
/// without this guard `stone_proportions` would measure the bare preform block as
/// if it were the stone.
#[must_use]
pub fn proportions_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash(), dash());
    }
    let Some(proportions) = design.stone_proportions(solved) else {
        return (dash(), dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let table_percent_text = proportions
        .table_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let crown_height_text = proportions
        .crown_height
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let pavilion_depth_text = proportions
        .pavilion_depth
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    // See `state::yield_report::proportions_texts`'s identical gate --
    // `total_depth` has no `Option`-driven "-" fallback of its own, so this
    // mirrors that function's own `girdle_thickness.is_some()` guard.
    let total_depth_text = if proportions.girdle_thickness.is_some() {
        format!("{:.3}{unit}", proportions.total_depth)
    } else {
        dash()
    };
    let length_to_width_text = proportions
        .length_to_width
        .map_or_else(dash, |v| format!("{v:.3}"));
    (
        table_percent_text,
        crown_height_text,
        pavilion_depth_text,
        total_depth_text,
        length_to_width_text,
    )
}

/// [`girdle_and_ratio_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`proportions_texts_from_solved`].
///
/// Mirrors that function's body exactly, minus the internal `design.solve()` it
/// exists to avoid repeating.
///
/// A tierless design does NOT skip `Design::solve` -- an empty mast list is a
/// valid, closed, zero-plane solve -- so a caller here can perfectly well be
/// holding exactly that solved-empty list. This mirror carries the SAME
/// `tiers.is_empty()` guard [`girdle_and_ratio_texts`] documents, not a weaker
/// one, since without it `stone_proportions` would measure the bare preform
/// block (girdle 50% of a cube, crown/pavilion 0%) as if it were the stone --
/// and a UI prefers this `_from_solved` mirror over the guarded plain function
/// on its hot path once a design solves.
#[must_use]
pub fn girdle_and_ratio_texts_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, String, String, String) {
    let dash = || "-".to_string();
    if design.tiers.is_empty() {
        return (dash(), dash(), dash(), dash());
    }
    let Some(proportions) = design.stone_proportions(solved) else {
        return (dash(), dash(), dash(), dash());
    };
    let mm_per_unit = design.yield_report(solved).mm_per_unit;
    let proportions = mm_per_unit.map_or(proportions, |mm| proportions.to_mm(mm));
    let unit = if mm_per_unit.is_some() { " mm" } else { "" };
    let girdle_thickness_text = proportions
        .girdle_thickness
        .map_or_else(dash, |v| format!("{v:.3}{unit}"));
    let crown_to_width_percent_text = proportions
        .crown_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let pavilion_to_width_percent_text = proportions
        .pavilion_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    let girdle_to_width_percent_text = proportions
        .girdle_to_width_percent
        .map_or_else(dash, |v| format!("{v:.1}%"));
    (
        girdle_thickness_text,
        crown_to_width_percent_text,
        pavilion_to_width_percent_text,
        girdle_to_width_percent_text,
    )
}

/// [`preform_mm_texts`]'s counterpart for a caller that already has an
/// up-to-date `solved` mast list on hand -- see [`proportions_texts_from_solved`].
#[must_use]
pub fn preform_mm_texts_from_solved(design: &Design, solved: &[SolvedTier]) -> (String, String) {
    let Some(mm_per_unit) = design.yield_report(solved).mm_per_unit else {
        return (String::new(), String::new());
    };
    let preform = &design.preform;
    (
        format!("\u{2248} {:.3} mm", preform.half_width * mm_per_unit),
        format!("\u{2248} {:.3} mm", preform.depth * mm_per_unit),
    )
}
