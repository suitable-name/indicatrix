//! Optimize and Retarget in the solve Worker: the plain-data request and result types
//! that cross the Worker boundary, and the handlers that run the desktop's own search.
//!
//! # Same search as the desktop
//!
//! [`run_optimize`] is the body of the desktop's Optimize action
//! (`gui/editor/callbacks/solve_actions/optimize_run.rs` +
//! `optimize_solve.rs`): the RI defaulting for a design that names no material
//! (`optimize_view::default_optimize_material_ri`), the "only selected tiers" pinning
//! (`optimize_view::pin_non_selected_free_tiers`), then
//! `indicatrix_cut_core::optimize_design` against the resolved material. [`run_retarget`]
//! is the desktop's Retarget action (`retarget_actions/{proposal,optimize_run}.rs`):
//! `retarget::build_proposal` for Shift mode, and for Optimize mode the same
//! seed-then-search sequence the desktop's worker thread runs (`seed_shift_design`,
//! `optimize_design`, `rows_from_outcome`).
//!
//! # Progress and cancel
//!
//! `optimize_design` reports its running evaluation count and stage through
//! [`SolveHooks::on_progress`]; the Worker turns each report into a
//! `FromWorker::Progress`. A Worker cannot read a message while it computes, so the page
//! cancels a running search through a side channel: the Worker's progress hook (run
//! between tier decisions) looks at the job's cancel URL (`ToWorker::WatchCancel`) and
//! sets [`SolveHooks::cancel`], the flag the search polls once per tier decision. The
//! search then returns the best result it has, as the desktop's Cancel does; the host
//! terminates the Worker only if that takes longer than
//! `host::GRACEFUL_CANCEL_MS` (30 s: the search still scores the design before and
//! after at full fidelity, about 10 s each on one wasm thread, and cannot look at the
//! cancel flag while it is scoring the starting point).

use indicatrix::optics::raytracer::LightingPreset;
use std::{collections::BTreeSet, sync::atomic::AtomicBool};

use indicatrix::{
    color::metrics::SweepProgress, geometry::meet_solver::Block, optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    AngleChange, Design, DesignSolveError, MaterialSelection, ObjectiveComponents,
    ObjectiveWeights, OptimizeConfig, OptimizeOutcome, ResolvedMaterial, Risk, SearchHooks,
    free_tier_indices,
    optimize::{SearchStage, inclusive_max_evaluations},
    optimize_design,
};
use indicatrix_editor::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    optimize_view::{default_optimize_material_ri, pin_non_selected_free_tiers},
    retarget::{
        self, CrownShift, RetargetError, RetargetMode, RetargetProposal, RetargetRow,
        view::resolved_material_from_selection,
    },
};
use serde::{Deserialize, Serialize};

use super::SolveResponse;

/// What a running search reports: the stage, and the evaluations spent of the run's
/// inclusive budget (`inclusive_max_evaluations`, the coordinate cap plus the polish
/// stage's own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgressReport {
    /// The search stage.
    pub stage: SearchStage,
    /// Candidate evaluations so far.
    pub evaluations: usize,
    /// The run's inclusive evaluation budget.
    pub max_evaluations: usize,
}

/// What a handler is given besides the design and the request: the cancel flag the
/// search polls, and where to send progress.
pub struct SolveHooks<'a> {
    /// Polled once per tier decision; a set flag ends the search with its partial result.
    pub cancel: &'a AtomicBool,
    /// Called (on the Worker's thread) for every search progress report.
    pub on_progress: &'a dyn Fn(ProgressReport),
    /// Called before every raytrace evaluation of a tilt sweep. The Worker's hook looks
    /// at the job's cancel URL from here and sets [`Self::cancel`], which the sweep
    /// checks right after.
    pub on_sweep: &'a dyn Fn(SweepProgress),
}

/// Optimize's settings as plain data: the Optimize tab's weight, budget, seed, polish
/// and "only selected tiers" fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptimizeParams {
    /// Windowing weight.
    pub windowing: f32,
    /// Extinction weight.
    pub extinction: f32,
    /// Tilt-brilliance weight.
    pub tilt_brilliance: f32,
    /// Yield weight, `0..=1`.
    pub yield_weight: f32,
    /// The search seed.
    pub seed: u64,
    /// The coordinate stage's evaluation budget.
    pub max_evaluations: u32,
    /// Whether the Nelder-Mead polish stage runs.
    pub polish: bool,
    /// When `Some`, only these tiers stay free (the multi-selection); every other free
    /// tier is pinned at its solved mast first.
    pub only_tiers: Option<Vec<u32>>,
    /// Index of the `LightingPreset` the objective scores under (the preset the
    /// viewport shows); decoded with `LightingPreset::from_index`.
    pub lighting_preset_index: i32,
}

impl Default for OptimizeParams {
    /// `OptimizeConfig::default()`'s settings, no restriction.
    fn default() -> Self {
        let config = OptimizeConfig::default();
        Self {
            windowing: config.weights.windowing,
            extinction: config.weights.extinction,
            tilt_brilliance: config.weights.tilt_brilliance,
            yield_weight: config.weights.yield_weight,
            seed: config.seed,
            max_evaluations: u32::try_from(config.max_evaluations).unwrap_or(u32::MAX),
            polish: config.polish_start_step_deg.is_some(),
            only_tiers: None,
            lighting_preset_index: config.lighting.index(),
        }
    }
}

impl OptimizeParams {
    /// The desktop's `OptimizeConfig` for these settings: the given weights, seed and
    /// budget, polish on or off, everything else at its default.
    #[must_use]
    pub fn config(&self) -> OptimizeConfig {
        let mut config = OptimizeConfig {
            weights: ObjectiveWeights {
                windowing: self.windowing,
                extinction: self.extinction,
                tilt_brilliance: self.tilt_brilliance,
                yield_weight: self.yield_weight,
            },
            seed: self.seed,
            max_evaluations: self.max_evaluations as usize,
            lighting: LightingPreset::from_index(self.lighting_preset_index),
            ..OptimizeConfig::default()
        };
        if !self.polish {
            config.polish_start_step_deg = None;
        }
        config
    }
}

/// `MaterialSelection` as plain data (it has no serde of its own).
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct MaterialSelectionData {
    /// The preset or custom material name.
    pub name: Option<String>,
    /// A typed specific gravity.
    pub specific_gravity_override: Option<f64>,
    /// A typed refractive index.
    pub refractive_index_override: Option<f64>,
    /// A body-colour absorption triple.
    pub body_colour_override: Option<[f32; 3]>,
}

impl From<&MaterialSelection> for MaterialSelectionData {
    fn from(selection: &MaterialSelection) -> Self {
        Self {
            name: selection.name.clone(),
            specific_gravity_override: selection.specific_gravity_override,
            refractive_index_override: selection.refractive_index_override,
            body_colour_override: selection.body_colour_override,
        }
    }
}

impl From<MaterialSelectionData> for MaterialSelection {
    fn from(data: MaterialSelectionData) -> Self {
        Self {
            name: data.name,
            specific_gravity_override: data.specific_gravity_override,
            refractive_index_override: data.refractive_index_override,
            body_colour_override: data.body_colour_override,
        }
    }
}

/// One accepted angle change of an Optimize result.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct AngleChangeData {
    /// The tier index.
    pub index: u32,
    /// Its angle before.
    pub from_deg: f64,
    /// Its angle after.
    pub to_deg: f64,
}

/// `OptimizeOutcome` as plain data, plus the RI the run defaulted to when the design
/// named no material.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct OptimizeResultData {
    /// Windowing, extinction and tilt brilliance before, in percent.
    pub before: [f32; 3],
    /// The blended score before.
    pub before_score: f32,
    /// Yield loss before, in percent.
    pub before_yield_loss_pct: f32,
    /// Windowing, extinction and tilt brilliance after, in percent.
    pub after: [f32; 3],
    /// The blended score after.
    pub after_score: f32,
    /// Yield loss after, in percent.
    pub after_yield_loss_pct: f32,
    /// Candidate evaluations spent.
    pub evaluations: u32,
    /// The accepted angle changes.
    pub changes: Vec<AngleChangeData>,
    /// Whether the search was cancelled (the partial result is real).
    pub cancelled: bool,
    /// Evaluations spent in the polish stage.
    pub polish_evaluations: u32,
    /// Score improvement adopted from the polish stage.
    pub polish_improvement: f32,
    /// The RI the run scored against when the design named no material.
    pub defaulted_ri: Option<f64>,
}

const fn components_array(c: &ObjectiveComponents) -> [f32; 3] {
    [c.windowing_pct, c.extinction_pct, c.tilt_brilliance_pct]
}

const fn components_from_array(a: [f32; 3]) -> ObjectiveComponents {
    ObjectiveComponents {
        windowing_pct: a[0],
        extinction_pct: a[1],
        tilt_brilliance_pct: a[2],
    }
}

impl OptimizeResultData {
    /// The plain-data form of `outcome`.
    #[must_use]
    pub fn from_outcome(outcome: &OptimizeOutcome, defaulted_ri: Option<f64>) -> Self {
        Self {
            before: components_array(&outcome.before),
            before_score: outcome.before_score,
            before_yield_loss_pct: outcome.before_yield_loss_pct,
            after: components_array(&outcome.after),
            after_score: outcome.after_score,
            after_yield_loss_pct: outcome.after_yield_loss_pct,
            evaluations: u32::try_from(outcome.evaluations).unwrap_or(u32::MAX),
            changes: outcome
                .changes
                .iter()
                .map(|c| AngleChangeData {
                    index: u32::try_from(c.index).unwrap_or(u32::MAX),
                    from_deg: c.from_deg,
                    to_deg: c.to_deg,
                })
                .collect(),
            cancelled: outcome.cancelled,
            polish_evaluations: u32::try_from(outcome.polish_evaluations).unwrap_or(u32::MAX),
            polish_improvement: outcome.polish_improvement,
            defaulted_ri,
        }
    }

    /// The `OptimizeOutcome` this came from, for `optimize_view`'s tables and
    /// `EditorSession::apply_optimize_outcome`.
    #[must_use]
    pub fn to_outcome(&self) -> OptimizeOutcome {
        OptimizeOutcome {
            before: components_from_array(self.before),
            before_score: self.before_score,
            before_yield_loss_pct: self.before_yield_loss_pct,
            after: components_from_array(self.after),
            after_score: self.after_score,
            after_yield_loss_pct: self.after_yield_loss_pct,
            evaluations: self.evaluations as usize,
            changes: self
                .changes
                .iter()
                .map(|c| AngleChange {
                    index: c.index as usize,
                    from_deg: c.from_deg,
                    to_deg: c.to_deg,
                })
                .collect(),
            cancelled: self.cancelled,
            polish_evaluations: self.polish_evaluations as usize,
            polish_improvement: self.polish_improvement,
        }
    }
}

/// How Retarget builds its proposal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RetargetModeData {
    /// The deterministic critical-angle shift.
    Shift,
    /// The shift, then an `optimize_design` search over the free tiers.
    Optimize,
}

/// Retarget's settings as plain data: the target material, the crown policy, the mode and
/// (for Optimize mode) the search settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetargetParams {
    /// The target material selection (the dialog's combo and RI override).
    pub target: MaterialSelectionData,
    /// The fraction of the pavilion critical-angle delta a crown tier moves by.
    pub crown_fraction: f64,
    /// Scale the crown angle by the critical-angle ratio instead.
    pub scale_crown_by_ratio: bool,
    /// Shift or Optimize.
    pub mode: RetargetModeData,
    /// The search settings for Optimize mode (its `only_tiers` is ignored).
    pub optimize: OptimizeParams,
}

/// A retarget row's block.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BlockData {
    /// A crown tier.
    Crown,
    /// A pavilion tier.
    Pavilion,
    /// A girdle tier (never listed by `build_proposal`).
    Girdle,
}

/// A retarget row's windowing risk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskData {
    /// Margin at least 2 degrees.
    Safe,
    /// Margin in `[0, 2)` degrees.
    Marginal,
    /// Margin negative.
    Windows,
}

/// One row of a retarget proposal (`RetargetRow` as plain data).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetargetRowData {
    /// The tier index.
    pub tier_index: u32,
    /// Its block.
    pub block: BlockData,
    /// Its name.
    pub name: String,
    /// The angle when the proposal was built.
    pub old_angle: f64,
    /// The proposed angle.
    pub new_angle: f64,
    /// `new_angle`'s margin over the target's critical angle.
    pub margin_deg: f64,
    /// `new_angle`'s windowing risk in the target.
    pub risk: RiskData,
}

impl From<&RetargetRow> for RetargetRowData {
    fn from(row: &RetargetRow) -> Self {
        Self {
            tier_index: u32::try_from(row.tier_index).unwrap_or(u32::MAX),
            block: match row.block {
                Block::Crown => BlockData::Crown,
                Block::Pavilion => BlockData::Pavilion,
                Block::Girdle => BlockData::Girdle,
            },
            name: row.name.clone(),
            old_angle: row.old_angle,
            new_angle: row.new_angle,
            margin_deg: row.margin_deg,
            risk: match row.risk {
                Risk::Safe => RiskData::Safe,
                Risk::Marginal => RiskData::Marginal,
                Risk::Windows => RiskData::Windows,
            },
        }
    }
}

impl From<&RetargetRowData> for RetargetRow {
    fn from(row: &RetargetRowData) -> Self {
        Self {
            tier_index: row.tier_index as usize,
            block: match row.block {
                BlockData::Crown => Block::Crown,
                BlockData::Pavilion => Block::Pavilion,
                BlockData::Girdle => Block::Girdle,
            },
            name: row.name.clone(),
            old_angle: row.old_angle,
            new_angle: row.new_angle,
            margin_deg: row.margin_deg,
            risk: match row.risk {
                RiskData::Safe => Risk::Safe,
                RiskData::Marginal => Risk::Marginal,
                RiskData::Windows => Risk::Windows,
            },
        }
    }
}

/// A Retarget answer: exactly one of `rows` (with `notes`), `anchored_errors` or
/// `solve_error` is populated, like the desktop's `RetargetView`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct RetargetResultData {
    /// The proposal's rows (empty on either error).
    pub rows: Vec<RetargetRowData>,
    /// The proposal's notes (empty on either error).
    pub notes: Vec<String>,
    /// `#N "name"` per anchored tier when Optimize mode refused.
    pub anchored_errors: Vec<String>,
    /// The solve error's text when Optimize mode could not solve the design.
    pub solve_error: String,
}

impl RetargetResultData {
    /// The proposal these rows make against `target`, `None` for either error (or no
    /// rows).
    #[must_use]
    pub fn to_proposal(&self, target: ResolvedMaterial) -> Option<RetargetProposal> {
        (!self.rows.is_empty()).then(|| RetargetProposal {
            rows: self.rows.iter().map(RetargetRow::from).collect(),
            target,
            notes: self.notes.clone(),
        })
    }
}

/// Runs Optimize on `design` -- see the module doc comment. The design must have at least
/// one free tier; otherwise the answer is [`SolveResponse::AnalysisFailed`] with the
/// desktop's message.
#[must_use]
pub fn run_optimize(
    design: &Design,
    params: &OptimizeParams,
    custom: &[GemMaterial],
    hooks: &SolveHooks<'_>,
) -> SolveResponse {
    // The desktop's up-front refusal: the panel disables the button, so this only guards
    // a race with a concurrent edit.
    if free_tier_indices(design).is_empty() {
        return SolveResponse::AnalysisFailed {
            message: "Optimize has nothing free to move on this design right now.".to_string(),
            missing_anchor: false,
        };
    }
    let mut selection = design.material.clone();
    let defaulted_ri = default_optimize_material_ri(design, &mut selection, custom);
    let target_design = match restricted_design(design, params) {
        Ok(d) => d,
        Err(response) => return response,
    };
    let free = free_tier_indices(&target_design).len();
    let config = params.config();
    let material = resolved_gem_material(&selection, &EditorMaterialLookup::new(custom));
    match search(&target_design, &material, &config, free, hooks) {
        Ok(outcome) => {
            SolveResponse::Optimized(OptimizeResultData::from_outcome(&outcome, defaulted_ri))
        }
        Err(error) => analysis_failed(&error),
    }
}

/// `design`, or (for `only_tiers`) the clone with every other free tier pinned at its
/// solved mast; the error answer when that solve fails.
fn restricted_design(design: &Design, params: &OptimizeParams) -> Result<Design, SolveResponse> {
    let Some(only) = &params.only_tiers else {
        return Ok(design.clone());
    };
    if only.is_empty() {
        return Ok(design.clone());
    }
    let keep: BTreeSet<usize> = only.iter().map(|&i| i as usize).collect();
    match design.solve() {
        Ok(solved) => Ok(pin_non_selected_free_tiers(design, &solved, &keep)),
        Err(error) => Err(analysis_failed(&error)),
    }
}

/// [`optimize_design`] with `hooks` translated to the search's own, and progress reports
/// carrying the run's inclusive budget.
fn search(
    design: &Design,
    material: &GemMaterial,
    config: &OptimizeConfig,
    free_tier_count: usize,
    hooks: &SolveHooks<'_>,
) -> Result<OptimizeOutcome, DesignSolveError> {
    let max_evaluations = inclusive_max_evaluations(config, free_tier_count);
    let on_progress = |evaluations: usize, stage: SearchStage| {
        (hooks.on_progress)(ProgressReport {
            stage,
            evaluations,
            max_evaluations,
        });
    };
    let search_hooks = SearchHooks {
        cancel: Some(hooks.cancel),
        on_progress: Some(&on_progress),
    };
    optimize_design(design, material, config, &search_hooks)
}

fn analysis_failed(error: &DesignSolveError) -> SolveResponse {
    SolveResponse::AnalysisFailed {
        message: error.to_string(),
        missing_anchor: matches!(error, DesignSolveError::MissingAnchor(_)),
    }
}

/// Runs Retarget on `design` -- see the module doc comment.
#[must_use]
pub fn run_retarget(
    design: &Design,
    params: &RetargetParams,
    custom: &[GemMaterial],
    hooks: &SolveHooks<'_>,
) -> SolveResponse {
    let selection: MaterialSelection = params.target.clone().into();
    let target = resolved_material_from_selection(&selection, custom);
    let crown = CrownShift {
        fraction: params.crown_fraction,
        scale_by_ratio: params.scale_crown_by_ratio,
    };
    let result = match params.mode {
        RetargetModeData::Shift => proposal_result(retarget::build_proposal(
            design,
            &target,
            crown,
            RetargetMode::Shift,
            custom,
        )),
        RetargetModeData::Optimize => {
            optimize_mode_result(design, &target, crown, params, custom, hooks)
        }
    };
    SolveResponse::Retargeted(result)
}

/// A `build_proposal` answer as plain data.
fn proposal_result(result: Result<RetargetProposal, RetargetError>) -> RetargetResultData {
    match result {
        Ok(proposal) => RetargetResultData {
            rows: proposal.rows.iter().map(RetargetRowData::from).collect(),
            notes: proposal.notes,
            ..RetargetResultData::default()
        },
        Err(error) => error_result(&error),
    }
}

fn error_result(error: &RetargetError) -> RetargetResultData {
    match error {
        RetargetError::AnchoredTiers(tiers) => RetargetResultData {
            anchored_errors: tiers
                .iter()
                .map(|(index, name)| format!("#{index} \"{name}\""))
                .collect(),
            ..RetargetResultData::default()
        },
        RetargetError::Solve(err) => RetargetResultData {
            solve_error: err.to_string(),
            ..RetargetResultData::default()
        },
    }
}

/// `RetargetMode::Optimize`: the desktop worker thread's sequence -- the up-front
/// anchored-tier refusal, the shift seed, the search against the target's material, then
/// the rows and notes the synchronous path would have built.
fn optimize_mode_result(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    params: &RetargetParams,
    custom: &[GemMaterial],
    hooks: &SolveHooks<'_>,
) -> RetargetResultData {
    let (scope, blocks) = retarget::retarget_scope(design);
    let anchored = retarget::anchored_tiers_in(design, &scope);
    if !anchored.is_empty() {
        return error_result(&RetargetError::AnchoredTiers(anchored));
    }
    let n_from = design.effective_refractive_index_with(custom);
    let n_to = target.n_d;
    let seeded = retarget::seed_shift_design(design, &scope, &blocks, n_from, n_to, crown);
    let config = params.optimize.config();
    let free = free_tier_indices(&seeded).len();
    match search(&seeded, &target.gem, &config, free, hooks) {
        Ok(outcome) => {
            let rows = retarget::rows_from_outcome(
                design,
                &seeded,
                &scope,
                &blocks,
                n_to,
                &outcome.changes,
            );
            RetargetResultData {
                rows: rows.iter().map(RetargetRowData::from).collect(),
                notes: retarget::build_notes(RetargetMode::Optimize(config), n_from, n_to),
                ..RetargetResultData::default()
            }
        }
        Err(error) => error_result(&RetargetError::Solve(error)),
    }
}

#[cfg(test)]
mod tests;
