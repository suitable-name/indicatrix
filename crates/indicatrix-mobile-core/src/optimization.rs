//! Governed Optimize jobs retaining the engine's complete certified candidates.
use crate::{
    app_state::AppState,
    compute_policy::ComputePolicy,
    design_state::DirtyScope,
    operations::{InputFingerprint, OperationIdentity, OperationToken},
};
use indicatrix::{geometry::meet_solver::SolveControl, optics::materials::GemMaterial};
use indicatrix_cut_core::{
    Design, Edit, History, OptimizeResult, SearchHooks, optimize::SearchStage, optimize_design_with,
};
use indicatrix_editor::{
    material_lookup::sized_material_for_optimize,
    optimize_view::{
        RunPlan, edits_for_relation_session, measure_anchor_hinges, optimize_availability,
    },
    solve_policy::design_to_gpu_planes_from_solved,
};
use std::{collections::BTreeSet, sync::Arc};

pub struct OptimizationJob {
    pub token: OperationToken,
    design: Design,
    material: GemMaterial,
    plan: RunPlan,
    only: Option<BTreeSet<usize>>,
    policy: Arc<ComputePolicy>,
}

pub struct OptimizationResult {
    pub identity: OperationIdentity,
    pub result: OptimizeResult,
    pub partial: bool,
    pub previews: Vec<PreparedPreview>,
    pub original_solved: Vec<indicatrix::geometry::meet_solver::SolvedTier>,
}

/// One candidate, prepared on the worker: the design with it applied, that design's solve
/// and the edit that makes it; or the reason it could not be prepared.
pub type PreparedPreview = Result<
    (
        Design,
        Vec<indicatrix::geometry::meet_solver::SolvedTier>,
        Edit,
    ),
    String,
>;

/// The fingerprint of everything an Optimize result depends on.
///
/// # Errors
///
/// A message when the design or the custom materials cannot be encoded.
pub fn fingerprint(
    state: &AppState,
    plan: &RunPlan,
    only: Option<&BTreeSet<usize>>,
) -> Result<InputFingerprint, String> {
    let authoring = crate::deep_solve::fingerprint(state)?;
    let controls = format!("{plan:?}|{only:?}");
    let materials =
        serde_json::to_vec(&state.props.custom_materials).map_err(|error| error.to_string())?;
    Ok(InputFingerprint::from_parts([
        b"optimize-v1".as_slice(),
        &authoring.0,
        controls.as_bytes(),
        &materials,
    ]))
}

impl OptimizationJob {
    /// Captures an immutable job from the current design and starts its operation.
    ///
    /// # Errors
    ///
    /// The availability hint when Optimize cannot run, a message when the selected scope is
    /// empty, or the encoding or material lookup error.
    pub fn capture(
        state: &mut AppState,
        mut plan: RunPlan,
        only: Option<BTreeSet<usize>>,
    ) -> Result<Self, String> {
        // A design with concave tiers is optimized too: the engine scores every candidate
        // on the stone with its tools (`ScoredStone`, the `*_geom_under` objectives), and
        // only the flat tiers' angles are variables.
        let (available, hint) = optimize_availability(
            &state.session.design,
            plan.config.max_evaluations,
            plan.options.vary_anchored,
        );
        if !available {
            return Err(hint);
        }
        if only.as_ref().is_some_and(BTreeSet::is_empty) {
            return Err("Select at least one flat tier for selected scope.".into());
        }
        let inputs = fingerprint(state, &plan, only.as_ref())?;
        plan.config.max_lanes = 1; // one shared permit, never a second all-core pool
        plan.options.only_tiers.clone_from(&only);
        let material = crate::operation_material::source(
            &state.session.design,
            &state.props.custom_materials,
        )?;
        let token = state
            .operations
            .begin(state.session.current_generation(), inputs);
        Ok(Self {
            token,
            design: state.session.design.clone(),
            material,
            plan,
            only,
            policy: state.render.compute_policy(),
        })
    }

    /// Runs the search on the governed worker.
    ///
    /// # Errors
    ///
    /// A message when the search is interrupted before it starts, or when the plain solve or
    /// the search itself fails.
    pub fn run(
        mut self,
        progress: &dyn Fn(usize, SearchStage),
    ) -> Result<OptimizationResult, String> {
        let _permit = self
            .policy
            .acquire(&|| self.token.is_cancelled())
            .ok_or_else(|| "Optimization interrupted.".to_owned())?;
        let stop = || self.token.is_cancelled() || self.policy.paused();
        let solved = self
            .design
            .solve_with(&SolveControl::with_stop(&stop))
            .map_err(|error| error.to_string())?;
        measure_anchor_hinges(
            &mut self.plan.options,
            &self.design,
            &solved,
            self.only.as_ref(),
        );
        let material = sized_material_for_optimize(
            self.material,
            &self.design,
            &design_to_gpu_planes_from_solved(&self.design, &solved),
        );
        let on_progress = |count, stage| {
            if self.policy.paused() {
                self.token.request_cancel();
            }
            progress(count, stage);
        };
        let hooks = SearchHooks {
            cancel: Some(self.token.cancel_flag()),
            on_progress: Some(&on_progress),
            on_start: None,
        };
        let result = optimize_design_with(
            &self.design,
            &material,
            &self.plan.config,
            &self.plan.options,
            &hooks,
        )
        .map_err(|error| error.to_string())?;
        // Reproduction, preservation checks and solving stay on this governed worker.
        // Interrupted runs may retain engine-certified partial candidates, but a thermal
        // pause aborts preview preparation instead of running unbudgeted solve work.
        let previews = result
            .candidates
            .iter()
            .map(|candidate| {
                if self.policy.paused() {
                    return Err("Preview preparation interrupted.".to_owned());
                }
                let (design, edit, _) =
                    indicatrix_cut_core::optimize::materialize_candidate(&self.design, candidate)
                        .map_err(|error| error.to_string())?;
                let edit = edits_for_relation_session(&self.design, edit);
                let mut probe = indicatrix_editor::EditorSession::with_history(
                    self.design.clone(),
                    History::new(),
                );
                probe
                    .try_apply(edit.clone())
                    .map_err(|error| error.to_string())?;
                if indicatrix_cut_core::optimize::authoring_identity(&probe.design)
                    .map_err(|error| error.to_string())?
                    != indicatrix_cut_core::optimize::authoring_identity(&design)
                        .map_err(|error| error.to_string())?
                {
                    return Err(
                        "The shared session could not reproduce the accepted candidate.".to_owned(),
                    );
                }
                let solved = design
                    .solve_with(&SolveControl::with_stop(&|| self.policy.paused()))
                    .map_err(|error| error.to_string())?;
                Ok((design, solved, edit))
            })
            .collect();
        Ok(OptimizationResult {
            identity: self.token.identity,
            result,
            partial: self.token.is_cancelled(),
            previews,
            original_solved: solved,
        })
    }
}

/// The design a candidate would produce, for the preview.
///
/// # Errors
///
/// A message when the result is interrupted or stale, no candidate is selected, or the
/// candidate could not be prepared.
pub fn preview(
    state: &AppState,
    result: &OptimizationResult,
    index: usize,
    inputs: InputFingerprint,
) -> Result<Design, String> {
    if !state
        .operations
        .can_apply(result.identity, state.session.current_generation(), inputs)
    {
        return Err("This search result is interrupted or stale. Run it again.".into());
    }
    result
        .previews
        .get(index)
        .ok_or_else(|| "Select a candidate first.".to_owned())?
        .as_ref()
        .map(|(design, _, _)| design.clone())
        .map_err(Clone::clone)
}

/// Applies one candidate as a single edit and returns how many tiers it changed.
///
/// # Errors
///
/// The same messages as [`preview`], or the edit engine's message when it refuses the edit.
pub fn apply(
    state: &mut AppState,
    result: &OptimizationResult,
    index: usize,
    inputs: InputFingerprint,
) -> Result<usize, String> {
    let design = preview(state, result, index, inputs)?;
    let prepared = result
        .previews
        .get(index)
        .ok_or_else(|| "Select a candidate first.".to_owned())?
        .as_ref()
        .map_err(Clone::clone)?;
    let changed = state
        .session
        .design
        .tiers
        .iter()
        .zip(&design.tiers)
        .filter(|(before, after)| before != after)
        .count();
    if changed == 0 {
        return Ok(0);
    }
    let before = state.session.current_generation();
    state
        .session
        .try_apply_mapped(prepared.2.clone(), Some("Optimize cutting geometry"))
        .map_err(|error| error.to_string())?;
    let after = state.session.current_generation();
    if after != before {
        state.solve.note_edit(before, after, DirtyScope::All);
        state.operations.invalidate();
    }
    Ok(changed)
}
