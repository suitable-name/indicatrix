//! The solve Worker's job: a design as native TOML in, everything the page needs to
//! install the result out.
//!
//! # Same solve as the desktop
//!
//! [`solve_design`] is the desktop's background-solve body
//! (`apps/indicatrix-cut/src/gui/editor/auto_solve/dispatch.rs`,
//! `solve_and_build_result` + `panel_inputs`) minus the parts that need the page's own
//! state: ONE `solve_policy::solve_cancellably`, then the status line, the
//! tier-tagged manufacturability warnings and the viewport planes built from that same
//! solved list, and the plane-cap diagnosis behind its cheap pre-filter. When the design
//! does not solve, each readout falls back to its solving form, exactly like
//! `panel_inputs`.
//!
//! The tier rows and yield texts are NOT built here: they depend on the page's custom
//! materials (RI, specific gravity) and are cheap `*_from_solved` calls once the page
//! has [`SolveOutcome::solved`].
//!
//! # Transport
//!
//! The design travels as a self-contained native file
//! (`native::save_native_only_toml` / `native::load_native_only`, the desktop's
//! autosave pair), which carries every tier's angle, indices, constraint, target and id
//! plus the schedule metadata. See [`design_to_toml`].

use std::sync::atomic::{AtomicBool, Ordering};

use indicatrix::{
    geometry::meet_solver::{SolveError, SolveStrategy, SolvedTier},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    Design, DesignSolveError,
    native::{SaveExtras, load_native_only, save_native_only_toml},
};
use indicatrix_editor::{
    solve_policy::{
        design_to_gpu_planes_from_solved, likely_hit_plane_cap, solve_cancellably,
        too_many_planes_message,
    },
    view_model::{
        rows::manufacturability_warnings_tagged,
        solid_status::{
            design_to_gpu_planes, status_text_and_is_problem,
            status_text_and_is_problem_from_solved,
        },
    },
};
use serde::{Deserialize, Serialize};

use crate::scene::{PlaneData, planes_to_data};

mod analysis;
mod body_color;
mod optical;
#[cfg(test)]
mod tests;

pub use analysis::{
    AngleChangeData, BlockData, MaterialSelectionData, OptimizeParams, OptimizeResultData,
    ProgressReport, RetargetModeData, RetargetParams, RetargetResultData, RetargetRowData,
    RiskData, SolveHooks, run_optimize, run_retarget,
};
pub use body_color::{BodyColorParams, BodyColorResultData, run_body_color};
pub use optical::{
    MetricsParams, MetricsResultData, ScoredUnder, TiltAxisData, TiltParams, TiltResultData,
    run_metrics, run_metrics_with_map, run_tilt, sweep_fraction, sweep_status, tilt_lighting_note,
};

/// The `.asc` file name recorded inside a transport TOML. Never used to find a file:
/// a self-contained native file has no paired `.asc`.
pub const TRANSPORT_ASC_NAME: &str = "design.asc";

/// What the solve Worker is asked to do.
///
/// New variants go at the END of this enum; postcard encodes variants by index, so
/// existing ones keep their encoding.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SolveRequest {
    /// A full solve, like the desktop's background solve.
    Solve,
    /// The desktop's Optimize action on the design (progress reports stream back).
    Optimize {
        /// Weights, budget, seed, polish and the "only selected tiers" restriction.
        params: OptimizeParams,
        /// The session's custom materials, for resolving the design's material.
        custom_materials: Vec<GemMaterial>,
    },
    /// The desktop's "Retarget for material" proposal for a target material (Optimize
    /// mode streams progress reports back).
    Retarget {
        /// The target material, crown policy, mode and search settings.
        params: RetargetParams,
        /// The session's custom materials, for resolving both materials.
        custom_materials: Vec<GemMaterial>,
    },
    /// The five optical metrics of the frame being rendered (no design needed: the
    /// message's `design_toml` is ignored), scored under the HDR map the Worker holds when
    /// the params name it.
    Metrics {
        /// The stone, material, pose and HDR map.
        params: MetricsParams,
    },
    /// The Tilt Performance sweep: four axes of 181 points (progress reports stream back;
    /// no design needed).
    Tilt {
        /// The stone, material and light.
        params: TiltParams,
    },
    /// The path-aware L*C*h body-colour solve of the Design settings colour editor (no design
    /// needed: the message's `design_toml` is ignored; the answer is
    /// [`SolveResponse::BodyColor`]).
    BodyColor {
        /// The typed colour and the reference path.
        params: BodyColorParams,
    },
}

impl SolveRequest {
    /// Whether a running job of this kind can be asked to stop early with a partial
    /// answer (it polls the page's cancel URL): the searches and the tilt sweep.
    #[must_use]
    pub const fn supports_graceful_cancel(&self) -> bool {
        !matches!(self, Self::Solve | Self::Metrics { .. })
    }

    /// Whether the job reports progress while it runs, so a long one is only "hung"
    /// after a silence (`SolveClient`'s inactivity timeout) rather than after a fixed time.
    #[must_use]
    pub const fn streams_progress(&self) -> bool {
        matches!(
            self,
            Self::Optimize { .. } | Self::Retarget { .. } | Self::Tilt { .. }
        )
    }
}

/// `SolveStrategy` as plain data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StrategyData {
    /// See `SolveStrategy::ScaleReference`.
    ScaleReference,
    /// See `SolveStrategy::DependencyOrder`.
    DependencyOrder,
    /// See `SolveStrategy::JointGroup`.
    JointGroup,
    /// See `SolveStrategy::LeastSquaresFallback`.
    LeastSquaresFallback,
    /// See `SolveStrategy::Failed`.
    Failed,
}

impl From<SolveStrategy> for StrategyData {
    fn from(strategy: SolveStrategy) -> Self {
        match strategy {
            SolveStrategy::ScaleReference => Self::ScaleReference,
            SolveStrategy::DependencyOrder => Self::DependencyOrder,
            SolveStrategy::JointGroup => Self::JointGroup,
            SolveStrategy::LeastSquaresFallback => Self::LeastSquaresFallback,
            SolveStrategy::Failed => Self::Failed,
        }
    }
}

impl From<StrategyData> for SolveStrategy {
    fn from(strategy: StrategyData) -> Self {
        match strategy {
            StrategyData::ScaleReference => Self::ScaleReference,
            StrategyData::DependencyOrder => Self::DependencyOrder,
            StrategyData::JointGroup => Self::JointGroup,
            StrategyData::LeastSquaresFallback => Self::LeastSquaresFallback,
            StrategyData::Failed => Self::Failed,
        }
    }
}

/// One solved tier (`SolvedTier`) as plain data; converts back losslessly.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolvedTierData {
    /// The solved mast.
    pub mast: f64,
    /// How it was obtained.
    pub strategy: StrategyData,
    /// The solver's prose detail (tooltip text).
    pub detail: String,
}

impl From<&SolvedTier> for SolvedTierData {
    fn from(tier: &SolvedTier) -> Self {
        Self {
            mast: tier.mast,
            strategy: tier.strategy.into(),
            detail: tier.detail.clone(),
        }
    }
}

impl SolvedTierData {
    /// The `SolvedTier` this came from, for the editor's `*_from_solved` helpers.
    #[must_use]
    pub fn to_solved_tier(&self) -> SolvedTier {
        SolvedTier {
            mast: self.mast,
            strategy: self.strategy.into(),
            detail: self.detail.clone(),
        }
    }
}

/// Converts a whole solved list back (see [`SolvedTierData::to_solved_tier`]).
#[must_use]
pub fn to_solved_tiers(tiers: &[SolvedTierData]) -> Vec<SolvedTier> {
    tiers.iter().map(SolvedTierData::to_solved_tier).collect()
}

/// A manufacturability warning tagged with the tier it is about.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierWarning {
    /// Tier index in the design.
    pub tier_index: u32,
    /// The warning line.
    pub text: String,
}

/// The result of a solve that ran to the end (solved or not).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SolveOutcome {
    /// Every tier's solved mast, `None` when the design does not solve.
    pub solved: Option<Vec<SolvedTierData>>,
    /// Why it did not solve (`DesignSolveError`'s text), `None` when it did.
    pub error: Option<String>,
    /// The solid-status line (`status_text_and_is_problem*`).
    pub status_text: String,
    /// Whether that line reports a problem.
    pub status_is_problem: bool,
    /// Manufacturability warnings, tier-tagged.
    pub warnings: Vec<TierWarning>,
    /// The viewport planes for the solved design (or the solving fallback).
    pub planes: Vec<PlaneData>,
    /// The design is over the solver's plane cap (the desktop's one-time toast).
    pub too_many_planes: bool,
    /// Tiers in the solved design.
    pub tier_count: u32,
}

/// The solve Worker's answer.
///
/// Later request variants add their response variants at the END (see
/// [`SolveRequest`]).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SolveResponse {
    /// The solve ran; see [`SolveOutcome`].
    Solved(SolveOutcome),
    /// The design TOML could not be loaded.
    InvalidDesign {
        /// Why.
        message: String,
    },
    /// The job was cancelled before it finished.
    Cancelled,
    /// An Optimize run finished (or was cancelled with a partial result).
    Optimized(OptimizeResultData),
    /// An Optimize run that could not start or solve: `message` is the reason, and
    /// `missing_anchor` says it was a `MissingAnchor` (the desktop then appends the
    /// remedy).
    AnalysisFailed {
        /// Why.
        message: String,
        /// Whether the design lacks a scale anchor for a block.
        missing_anchor: bool,
    },
    /// A Retarget proposal (or why there is none).
    Retargeted(RetargetResultData),
    /// The optical metrics of a [`SolveRequest::Metrics`], with what they were scored
    /// under (a failure is [`Self::AnalysisFailed`]).
    Metrics(MetricsResultData),
    /// A finished [`SolveRequest::Tilt`] sweep (a stopped one is [`Self::Cancelled`]).
    TiltCurves(TiltResultData),
    /// A finished [`SolveRequest::BodyColor`] solve (a superseded one is [`Self::Cancelled`]).
    BodyColor(BodyColorResultData),
}

/// Encodes `design` for [`handle_solve`]: the self-contained native file, as the
/// desktop's autosave writes it.
///
/// # Errors
///
/// The native writer's error text (in practice unreachable -- see
/// `native::save_native_only_toml`).
pub fn design_to_toml(design: &Design) -> Result<String, String> {
    save_native_only_toml(design, TRANSPORT_ASC_NAME, None, &SaveExtras::default())
        .map_err(|e| e.to_string())
}

/// Loads a design sent through [`design_to_toml`].
///
/// # Errors
///
/// The loader's error text.
pub fn design_from_toml(design_toml: &str) -> Result<Design, String> {
    load_native_only(design_toml)
        .map(|loaded| loaded.design)
        .map_err(|e| e.to_string())
}

/// Runs `request` on the design in `design_toml` -- the solve Worker's handler, with no
/// progress reports and nothing to cancel it (see [`handle_solve_with`]).
#[must_use]
pub fn handle_solve(design_toml: &str, request: &SolveRequest) -> SolveResponse {
    let cancel = AtomicBool::new(false);
    handle_solve_with(
        design_toml,
        request,
        &SolveHooks {
            cancel: &cancel,
            on_progress: &|_| {},
            on_sweep: &|_| {},
        },
    )
}

/// [`handle_solve`] with `hooks`: the flag a search polls for cancellation, and where
/// its progress reports go.
#[must_use]
pub fn handle_solve_with(
    design_toml: &str,
    request: &SolveRequest,
    hooks: &SolveHooks<'_>,
) -> SolveResponse {
    let with_design = |run: &dyn Fn(&Design) -> SolveResponse| match design_from_toml(design_toml) {
        Ok(design) => run(&design),
        Err(message) => SolveResponse::InvalidDesign { message },
    };
    match request {
        // These three carry their own inputs and never look at the design.
        SolveRequest::Metrics { params } => run_metrics(&mut None, params),
        SolveRequest::Tilt { params } => run_tilt(params, hooks),
        SolveRequest::BodyColor { params } => run_body_color(params, hooks.cancel),
        SolveRequest::Solve => with_design(&|design| solve_design(design, hooks.cancel)),
        SolveRequest::Optimize {
            params,
            custom_materials,
        } => with_design(&|design| run_optimize(design, params, custom_materials, hooks)),
        SolveRequest::Retarget {
            params,
            custom_materials,
        } => with_design(&|design| run_retarget(design, params, custom_materials, hooks)),
    }
}

/// Solves `design` like the desktop's background solve -- see the module doc comment.
/// `cancel` is polled by the solver; a set flag returns [`SolveResponse::Cancelled`].
#[must_use]
pub fn solve_design(design: &Design, cancel: &AtomicBool) -> SolveResponse {
    let solved = solve_cancellably(design, cancel);
    if matches!(solved, Err(DesignSolveError::Solve(SolveError::Cancelled)))
        || cancel.load(Ordering::Relaxed)
    {
        return SolveResponse::Cancelled;
    }
    let too_many_planes = solved.as_ref().is_ok_and(|s| likely_hit_plane_cap(s))
        && too_many_planes_message(design).is_some();
    let tier_count = design.tiers.len() as u32;
    let outcome = match solved {
        Ok(solved) => {
            let (status_text, status_is_problem) =
                status_text_and_is_problem_from_solved(design, &solved);
            SolveOutcome {
                warnings: tag_warnings(manufacturability_warnings_tagged(design, Some(&solved))),
                planes: planes_to_data(&design_to_gpu_planes_from_solved(design, &solved)),
                solved: Some(solved.iter().map(SolvedTierData::from).collect()),
                error: None,
                status_text,
                status_is_problem,
                too_many_planes,
                tier_count,
            }
        }
        Err(error) => {
            let (status_text, status_is_problem) = status_text_and_is_problem(design);
            SolveOutcome {
                solved: None,
                error: Some(error.to_string()),
                status_text,
                status_is_problem,
                warnings: tag_warnings(manufacturability_warnings_tagged(design, None)),
                planes: planes_to_data(&design_to_gpu_planes(design)),
                too_many_planes,
                tier_count,
            }
        }
    };
    SolveResponse::Solved(outcome)
}

fn tag_warnings(pairs: Vec<(usize, String)>) -> Vec<TierWarning> {
    pairs
        .into_iter()
        .map(|(tier_index, text)| TierWarning {
            tier_index: tier_index as u32,
            text,
        })
        .collect()
}
