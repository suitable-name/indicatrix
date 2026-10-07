//! The `push_*` renderers that turn the Slint-free retarget proposal view model
//! (`indicatrix_editor::retarget::view`, shared with the web app and re-exported
//! here at its old paths) into `MainWindow`'s `editor_retarget_*`/`RetargetModel`
//! properties -- see this group's own `mod.rs` doc comment ("Slint-free view-model
//! split").

use crate::{MainWindow, RetargetCandidateItem, RetargetModel, RetargetRowItem};
use indicatrix_cut_core::{MaterialSelection, ResolvedMaterial};
use indicatrix_editor::retarget::{
    search::{CandidateKind, SearchCandidate, SearchReport},
    validity::ValidityStatus,
};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

#[cfg(test)]
pub(super) use indicatrix_editor::retarget::view::retarget_view;
pub(super) use indicatrix_editor::retarget::view::{CheckView, RetargetView, plan_view};

/// One option of a finished Optimize search as the candidate list shows it: every number
/// already formatted, so the Slint side only places text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CandidateRowView {
    /// 1 for the best option.
    pub(super) rank: i32,
    /// Where the option came from.
    pub(super) label: String,
    /// The combined score; lower is better.
    pub(super) score: String,
    /// Light leaking straight out of the pavilion.
    pub(super) windowing: String,
    /// Light returned to the eye.
    pub(super) brilliance: String,
    /// Light trapped or lost.
    pub(super) extinction: String,
    /// The share of the rough the finished stone gives up.
    pub(super) yield_loss: String,
    /// `Valid` for an option that passed the gate.
    pub(super) verdict: String,
}

/// The list rows for `candidates` (best first, as the report orders them).
pub(super) fn candidate_row_views(candidates: &[SearchCandidate]) -> Vec<CandidateRowView> {
    candidates
        .iter()
        .enumerate()
        .map(|(index, candidate)| {
            let label = match candidate.kind {
                CandidateKind::Shift => "Shift result".to_string(),
                CandidateKind::Partial { percent } => {
                    format!("Part of the Shift change ({percent} %)")
                }
                CandidateKind::Optimized => "Searched angles".to_string(),
            };
            let verdict = if candidate.validity.status == ValidityStatus::Valid {
                "Valid"
            } else {
                "Not valid"
            };
            CandidateRowView {
                rank: i32::try_from(index + 1).unwrap_or(i32::MAX),
                label,
                score: format!("{:.1}", candidate.numbers.score),
                windowing: format!("{:.1} %", candidate.numbers.windowing_pct),
                brilliance: format!("{:.1} %", candidate.numbers.brilliance_pct),
                extinction: format!("{:.1} %", candidate.numbers.extinction_pct),
                yield_loss: format!("{:.1} %", candidate.numbers.yield_loss_pct),
                verdict: verdict.to_string(),
            }
        })
        .collect()
}

/// One sentence on what a finished search found.
pub(super) fn search_summary_line(report: &SearchReport) -> String {
    let count = report.candidates.len();
    if count == 0 {
        return "No valid option was found.".to_string();
    }
    let options = if count == 1 {
        "1 valid option".to_string()
    } else {
        format!("{count} valid options")
    };
    format!(
        "{options}, best score first (lower is better). The search used {} steps.",
        report.evaluations
    )
}

/// Pushes the candidate list into `RetargetModel.candidates`.
pub(super) fn push_candidates(ui: &MainWindow, rows: Vec<CandidateRowView>) {
    let items: Vec<RetargetCandidateItem> = rows
        .into_iter()
        .map(|row| RetargetCandidateItem {
            rank: row.rank,
            label: row.label.into(),
            score: row.score.into(),
            windowing: row.windowing.into(),
            brilliance: row.brilliance.into(),
            extinction: row.extinction.into(),
            yield_loss: row.yield_loss.into(),
            verdict: row.verdict.into(),
        })
        .collect();
    ui.global::<RetargetModel>()
        .set_candidates(ModelRc::new(VecModel::from(items)));
}

/// A model of strings from any list of text.
pub(super) fn string_model<I, S>(items: I) -> ModelRc<SharedString>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(|item| SharedString::from(item.as_ref()))
            .collect::<Vec<_>>(),
    ))
}

/// Pushes [`CheckView`] into `RetargetModel`'s `validity_*`/`strategy_text`/
/// `metrics_cells` -- the validity line, its further reasons, how the masts were chosen
/// and the nine cells of the optical comparison table (empty when there is nothing to
/// show). Called with [`CheckView::default`] to hide the whole block.
pub(super) fn push_check_view(ui: &MainWindow, view: &CheckView) {
    let strings = |lines: &[String]| {
        ModelRc::new(VecModel::from(
            lines
                .iter()
                .map(|line| SharedString::from(line.as_str()))
                .collect::<Vec<_>>(),
        ))
    };
    let model = ui.global::<RetargetModel>();
    model.set_validity_state(view.state.code());
    model.set_validity_text(view.headline.as_str().into());
    model.set_validity_details(strings(view.details.as_slice()));
    model.set_strategy_text(view.strategy.as_str().into());
    model.set_metrics_cells(strings(view.metrics.as_slice()));
}

/// Pushes [`RetargetView`] into `MainWindow`'s `editor_retarget_rows`/`_notes`/
/// `_anchored_errors`/`_solve_error` -- the only place a `RetargetRowView` becomes
/// a real `RetargetRowItem`.
///
/// A row the retarget leaves alone arrives with `risk_label` `"Not changed"` and the dialog
/// greys it; so does a row with no risk (`"\u{2014}"`). Their `risk_color` is unused.
pub(super) fn push_retarget_view(ui: &MainWindow, view: RetargetView) {
    let rows: Vec<RetargetRowItem> = view
        .rows
        .into_iter()
        .map(|r| RetargetRowItem {
            tier_index: r.tier_index as i32,
            block: r.block.into(),
            name: r.name.into(),
            old_angle: r.old_angle.into(),
            new_angle: r.new_angle.into(),
            margin: r.margin.into(),
            risk_label: r.risk_label.into(),
            risk_color: Color::from_rgb_u8(r.risk_rgb.0, r.risk_rgb.1, r.risk_rgb.2),
        })
        .collect();
    ui.global::<RetargetModel>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>()
        .set_notes(ModelRc::new(VecModel::from(
            view.notes
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_anchored_tier_errors(ModelRc::new(VecModel::from(
            view.anchored_errors
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_solve_error(view.solve_error.into());
}

/// Pushes the target material readout -- fixed for the lifetime of one dialog
/// session, but re-pushed on every rebuild anyway since it costs nothing. Also clears
/// `RetargetModel.target_material_error` (see [`push_target_error`]): reaching this
/// function at all means `material::resolve_target_selection` just succeeded, so any
/// earlier parse error no longer applies.
pub(super) fn push_target_readout(
    ui: &MainWindow,
    selection: &MaterialSelection,
    target: &ResolvedMaterial,
) {
    ui.global::<RetargetModel>()
        .set_target_material_name(super::material::target_display_name(selection).into());
    ui.global::<RetargetModel>()
        .set_target_material_ri(format!("{:.4}", target.n_d).into());
    ui.global::<RetargetModel>()
        .set_target_critical_angle(format!("{:.2}\u{b0}", target.critical_angle_deg).into());
    ui.global::<RetargetModel>()
        .set_target_material_error("".into());
}

/// `ri_override_text` failed to parse -- pushes `message` into
/// `RetargetModel.target_material_error` (shown above the proposal table,
/// `retarget_dialog.slint`) and clears `rows`/`notes` so nothing on screen implies a
/// proposal was actually built against this text. The readout above the error
/// (`target_material_name`/`_ri`/`_critical_angle`) is deliberately left as-is --
/// whatever the last valid target was -- rather than reset, matching the error
/// banner's own "showing the last valid target" wording.
pub(super) fn push_target_error(ui: &MainWindow, message: &str) {
    ui.global::<RetargetModel>()
        .set_target_material_error(message.into());
    // Nothing on screen describes a proposal any more, so no verdict applies either.
    super::check_run::reset_check(ui);
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: Vec::new(),
            anchored_errors: Vec::new(),
            solve_error: String::new(),
        },
    );
}
