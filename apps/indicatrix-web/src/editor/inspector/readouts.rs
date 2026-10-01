//! The inspector's read-only figures, computed from the last solve: the Tier tab's
//! "Solved" section, the Preform tab's proportions, verdicts and yield, and the Schedule
//! tab's cut order. Each is computed only while its tab is showing (a design of a hundred
//! tiers measures in tens of milliseconds, but nobody needs it hidden), and blank -- never
//! a stale figure -- when the design does not currently solve.

use super::PushCtx;
use crate::{InspectorModel, PreformReadout, ScheduleRow, SolvedInfo, Verdict, VerdictSet};
use indicatrix_editor::view_model::yield_report::{
    ProportionVerdict, cutting_instructions_rows, girdle_and_ratio_texts_from_solved,
    preform_mm_texts_from_solved, proportion_verdicts, proportions_texts_from_solved,
    yield_report_texts_from_solved,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

/// One refresh of the read-only figures.
pub(super) fn push(pcx: &PushCtx<'_>) {
    let model = pcx.ui.global::<InspectorModel>();
    if pcx.collapsed {
        return;
    }
    match pcx.tab {
        0 => model.set_solved(solved_info(pcx, &model)),
        1 => push_preform(pcx, &model),
        3 => push_schedule(pcx, &model),
        _ => {}
    }
}

/// The "Solved" section for the tier the form has loaded.
fn solved_info(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) -> SolvedInfo {
    let Some(row) = usize::try_from(model.get_loaded_tier_index())
        .ok()
        .and_then(|i| pcx.rows().get(i))
    else {
        return SolvedInfo::default();
    };
    SolvedInfo {
        visible: true,
        mast: row.mast.as_str().into(),
        block: row.block.as_str().into(),
        strategy: row.strategy.as_str().into(),
        strategy_detail: row.strategy_detail.as_str().into(),
        strategy_uncertain: row.strategy_is_uncertain,
        margin_text: row.margin_text.as_str().into(),
        risk_level: row.risk_level,
        orbit_status: row.orbit_status.as_str().into(),
        orbit_incomplete: row.orbit_incomplete,
        meet_partners: row.meet_partners_text.as_str().into(),
        imported_meet: row.imported_meet_text.as_str().into(),
        warning: row.warning_text.as_str().into(),
    }
}

fn verdict(v: &ProportionVerdict) -> Verdict {
    Verdict {
        level: v.level,
        reason: v.reason.as_str().into(),
    }
}

/// "Nothing to judge yet" for all five figures.
fn no_verdicts() -> VerdictSet {
    let none = || Verdict {
        level: -1,
        reason: SharedString::new(),
    };
    VerdictSet {
        table: none(),
        crown_angle: none(),
        pavilion_angle: none(),
        total_depth: none(),
        girdle: none(),
    }
}

/// The proportions, ratios, verdicts and yield figures.
fn push_preform(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) {
    let design = pcx.design;
    let Some(solved) = pcx.solved else {
        model.set_readout(PreformReadout::default());
        model.set_verdicts(no_verdicts());
        return;
    };
    let (half_width_mm, depth_mm) = preform_mm_texts_from_solved(design, solved);
    let (table_pct, crown_height, pavilion_depth, total_depth, length_to_width) =
        proportions_texts_from_solved(design, solved);
    let (girdle_thickness, crown_to_width, pavilion_to_width, girdle_to_width) =
        girdle_and_ratio_texts_from_solved(design, solved);
    let (volumetric_yield, carat_weight, specific_gravity_used, preform_fit_warning) =
        yield_report_texts_from_solved(design, solved, &[]);
    model.set_readout(PreformReadout {
        half_width_mm: half_width_mm.into(),
        depth_mm: depth_mm.into(),
        table_pct: table_pct.into(),
        crown_height: crown_height.into(),
        pavilion_depth: pavilion_depth.into(),
        total_depth: total_depth.into(),
        length_to_width: length_to_width.into(),
        girdle_thickness: girdle_thickness.into(),
        crown_to_width: crown_to_width.into(),
        pavilion_to_width: pavilion_to_width.into(),
        girdle_to_width: girdle_to_width.into(),
        volumetric_yield: volumetric_yield.into(),
        carat_weight: carat_weight.into(),
        specific_gravity_used: specific_gravity_used.into(),
        preform_fit_warning: preform_fit_warning.into(),
    });
    // A tierless design has no stone whose proportions mean anything to judge.
    let verdicts = (!design.tiers.is_empty())
        .then(|| design.stone_proportions(solved))
        .flatten()
        .map(|proportions| proportion_verdicts(design, &proportions, pcx.n_d));
    model.set_verdicts(verdicts.map_or_else(no_verdicts, |v| VerdictSet {
        table: verdict(&v.table_pct),
        crown_angle: verdict(&v.crown_angle),
        pavilion_angle: verdict(&v.pavilion_angle),
        total_depth: verdict(&v.total_depth_pct),
        girdle: verdict(&v.girdle_pct),
    }));
}

/// The design's own cut order.
fn push_schedule(pcx: &PushCtx<'_>, model: &InspectorModel<'_>) {
    let rows: Vec<ScheduleRow> = pcx.solved.map_or_else(Vec::new, |solved| {
        cutting_instructions_rows(pcx.design, solved)
            .into_iter()
            .map(|row| ScheduleRow {
                facet: row.facet.into(),
                angle: row.angle.into(),
                index_val: row.index_val.into(),
                notes: row.notes.into(),
            })
            .collect()
    });
    model.set_schedule_rows(ModelRc::new(VecModel::from(rows)));
}
