//! Yield/weight and proportion (table %, crown/pavilion depth, girdle thickness,
//! length-to-width) view-model text, the proportion-guidance verdict chips, and
//! the Edit tab's own cut-order schedule rows -- everything the Preform/Yield
//! tabs read once a design solves.

use super::row_format::{
    format_angle_cell, format_index_value, representative_crown_and_pavilion_angles_deg,
};
use crate::{AngleItem, gui::editor::material_lookup::EditorMaterialLookup};
use indicatrix::geometry::meet_solver::{Block, SolvedTier, classify_blocks};
use indicatrix_cut_core::Design;

/// `design`'s current yield/weight figures, formatted for `EditorView`'s read-only
/// display fields -- rides along with the ordinary Solve action (like
/// `super::rows::manufacturability_warnings_by_tier`) since `Design::yield_report`
/// takes an already-solved mast list.
///
/// Returns `(volumetric_yield_text, carat_weight_text, specific_gravity_used_text,
/// preform_fit_warning_text)` -- all four empty when the design does not currently
/// solve ([`indicatrix_cut_core::MissingAnchor`], already surfaced by the banner).
///
/// `custom_sg` is the catalogue's custom-material specific-gravity table --
/// see [`yield_report_texts_from_solved`]'s own doc comment for where a
/// caller sources it from.
pub(in crate::gui::editor) fn yield_report_texts(
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
/// own doc comment for why this exists. Never re-solves, and
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
pub(in crate::gui::editor) fn yield_report_texts_from_solved(
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
/// the figures a cutter actually quotes. Built from
/// [`Design::stone_proportions`] and converted to millimetres via
/// [`Design::yield_report`]'s own scale factor whenever a trusted one exists
/// (a girdle diameter is set and the design measures); otherwise shown in the
/// design's own mast units with no unit suffix, rather than guessing a scale.
///
/// Returns `(table_percent_text, crown_height_text, pavilion_depth_text,
/// total_depth_text, length_to_width_text)`, every one of them `"-"` when the
/// design has no tiers at all, does not solve, isn't currently a closed solid,
/// or (the two depth fields specifically) has no vertical girdle plane with a
/// live facet to measure from -- see
/// [`indicatrix::geometry::stone_metrics::StoneProportions`]'s own doc comment
/// for why those two are `Option`. Rides along with the explicit "Solve"
/// action (like [`yield_report_texts`]), not every edit.
///
/// A tierless design still solves (an empty
/// mast list is a valid, closed, zero-plane solve -- `Design::solve` never
/// rejects it), and `stone_proportions` then measures the bare preform block:
/// table 100%, crown/pavilion 0%, `total_depth` the preform's own depth. Those
/// are honest numbers about the preform and fabricated ones about a stone that
/// does not exist yet, so the `tiers.is_empty()` guard below runs BEFORE the
/// solve -- including for `total_depth`, which (unlike the other four fields)
/// is not an `Option` on [`indicatrix::geometry::stone_metrics::StoneProportions`]
/// and so has no other way to read as "-". [`girdle_and_ratio_texts`] carries
/// the identical guard for the same reason; the `view` module's own
/// `proportions_texts_from_solved`/`girdle_and_ratio_texts_from_solved`
/// mirror it too, since a caller with an already-`solved` list came from a
/// design that solved -- which, per the above, a tierless one does.
pub(in crate::gui::editor) fn proportions_texts(
    design: &Design,
) -> (String, String, String, String, String) {
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
    let total_depth_text = format!("{:.3}{unit}", proportions.total_depth);
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

/// The three proportion readouts
/// [`proportions_texts`] does not expose -- girdle thickness (a figure with
/// no existing text home at all) and the printed `C/W%`/`P/W%` ratios every
/// faceting diagram actually prints, as opposed to the absolute crown/pavilion
/// depths [`proportions_texts`] already returns. `Design::stone_proportions` has
/// already computed all four (`StoneProportions::girdle_thickness`/`crown_to_width_percent`/
/// `pavilion_to_width_percent`/`girdle_to_width_percent`, see
/// [`indicatrix::geometry::stone_metrics::StoneProportions`]);
/// nothing under `gui::editor` reads any of them (grepped `state/mod.rs`/
/// `view.rs` for all four field names).
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
///
/// # Handoff
/// `state/*.rs` only computes; `view.rs` owns
/// `refresh_editor_panel`'s call to [`proportions_texts`]/`proportions_texts_from_solved`
/// and would need a matching call to this function (and a small
/// local `_from_solved` mirror of it, exactly like `view.rs`'s own doc comment
/// already does for `proportions_texts_from_solved`) to reach the UI; the
/// four new strings would need a `EditorModel` property each (`ui/models/editor.slint`)
/// and a display row in `editor_inspector.slint`'s
/// "Proportions" section -- there is no girdle-thickness readout yet, and
/// crown/pavilion depth are still shown as absolute values, not the printed
/// C/W%/P/W% ratios.
#[must_use]
pub(in crate::gui::editor) fn girdle_and_ratio_texts(
    design: &Design,
) -> (String, String, String, String) {
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

/// One proportion metric's verdict against
/// [`indicatrix_cut_core::proportions_windows`]'s reference table -- `level`
/// is `0` (`Verdict::Within`), `1` (`Verdict::Near`), `2`
/// (`Verdict::Outside`), or `-1` ("nothing to judge yet": the design does
/// not currently solve/close, or this particular metric has no value to
/// judge -- e.g. no crown tier at all). `reason` is the matched window's own
/// one-line explanation, `""` at level `-1`.
pub(in crate::gui::editor) struct ProportionVerdict {
    pub(in crate::gui::editor) level: i32,
    pub(in crate::gui::editor) reason: String,
}

/// The five [`ProportionVerdict`]s the Preform tab's "Proportion guidance"
/// section shows, one per metric
/// [`indicatrix_cut_core::proportions_windows::Metric`] lists.
pub(in crate::gui::editor) struct ProportionVerdicts {
    pub(in crate::gui::editor) table_pct: ProportionVerdict,
    pub(in crate::gui::editor) crown_angle: ProportionVerdict,
    pub(in crate::gui::editor) pavilion_angle: ProportionVerdict,
    pub(in crate::gui::editor) total_depth_pct: ProportionVerdict,
    pub(in crate::gui::editor) girdle_pct: ProportionVerdict,
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

/// The Preform tab's five proportion-verdict chips, judged against `design`'s already-solved
/// `proportions` -- the SAME [`indicatrix::geometry::stone_metrics::
/// StoneProportions`] the plain-number readouts above already read, so the
/// chip and the number next to it can never disagree about the underlying
/// measurement. `n_d` is the design's effective refractive index (the same
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
pub(in crate::gui::editor) fn proportion_verdicts(
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

/// The Preform tab's Half-Width/Depth fields, converted to
/// millimetres via the same [`Design::yield_report`] scale factor
/// [`proportions_texts`] uses -- those two fields are typed and stored in the
/// design's own mast-unit scale (girdle half-width = 1), which reads as
/// ambiguous next to the Yield section's "Girdle Diameter (mm)" a few fields
/// down. Returned ALONGSIDE the model-unit value, never in place of it (the
/// fields stay editable in model units -- `PreformSpec` itself has no mm
/// concept); each `""` when the design does not currently solve, or no girdle
/// diameter is set to anchor `mm_per_unit` at all.
#[must_use]
pub(in crate::gui::editor) fn preform_mm_texts(design: &Design) -> (String, String) {
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

/// `EditorModel.preform_y_offset_mm`'s seed
/// value -- `design.preform_y_offset` (model/mast units) converted to real
/// millimetres via `mm_per_unit`, the SAME anchor [`preform_mm_texts`] converts
/// Half-Width/Depth with. Unlike that function, this is pure (no internal
/// `Design::solve`): the caller already has `mm_per_unit` on hand from a
/// SOLVED mast list, or passes `None` when it deliberately never solves (`view::
/// push_stale_content`) -- `""` in that case, the same "cleared, not left
/// showing a superseded value" treatment `preform_mm_texts`' own two fields get
/// there. Formatted as a bare decimal (not `preform_mm_texts`' "\u{2248} ... mm"
/// style) because, unlike those two read-only displays, this field is the
/// editable value `apply_preform_y_offset`'s `Edit::SetPreformYOffset` round-
/// trips through -- an approximation glyph or unit suffix would not re-parse.
#[must_use]
pub(in crate::gui::editor) fn preform_y_offset_mm_text(
    preform_y_offset: f64,
    mm_per_unit: Option<f64>,
) -> String {
    mm_per_unit.map_or_else(String::new, |mm_per_unit| {
        format!("{:.2}", preform_y_offset * mm_per_unit)
    })
}

/// A short label naming the design currently under edit -- the paired `.asc`'s
/// bare file name when this design was loaded from (or saved to) one, else
/// `"Untitled design"`. This is the editor-side half,
/// shown in the status strip; the viewport/render-side half would need a
/// separate `RenderContext` change outside this file.
pub(in crate::gui::editor) fn design_label_text(asc_filename: Option<&str>) -> String {
    asc_filename.map_or_else(|| "Untitled design".to_string(), str::to_string)
}

/// `design`'s current cut order as the Edit tab's own schedule rows -- angle,
/// facet name, index positions and notes, in cut order -- so the cutter can
/// see the design actually being edited rather than only ever the catalogue's
/// original schedule. Built from
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
pub(in crate::gui::editor) fn cutting_schedule_rows(
    design: &Design,
    solved: &[SolvedTier],
) -> Vec<AngleItem> {
    // `AngleItem::side` comes from the solver's own block classification, not from
    // the sign of the angle: that is the same source the tier table's own block
    // column uses, and it distinguishes a girdle facet (neither side) from a crown
    // one, which a sign test cannot.
    let tier_blocks = classify_blocks(&design.meet_tier_inputs());
    let schedule = match design.try_to_asc_schedule_from_solved(solved) {
        Ok(schedule) => schedule,
        Err(mismatch) => {
            tracing::warn!(%mismatch, "cutting_schedule_rows: solved masts do not match this design's tier count");
            return Vec::new();
        }
    };
    schedule
        .tiers
        .into_iter()
        .enumerate()
        .map(|(order_idx, tier)| AngleItem {
            order_idx: order_idx as i32,
            side: match tier_blocks.get(order_idx) {
                Some(Block::Crown) => 1,
                Some(Block::Pavilion) => -1,
                _ => 0,
            },
            facet: if tier.name.is_empty() {
                format!("#{}", order_idx + 1)
            } else {
                tier.name
            }
            .into(),
            angle: format_angle_cell(tier.angle_deg).into(),
            index_val: tier
                .indices
                .into_iter()
                .map(format_index_value)
                .collect::<Vec<_>>()
                .join(", ")
                .into(),
            notes: tier.notes.into(),
        })
        .collect()
}
