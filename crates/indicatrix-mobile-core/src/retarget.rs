//! Governed Retarget Shift/search, retaining checked complete authored candidates.
use crate::{
    app_state::AppState,
    compute_policy::ComputePolicy,
    design_state::DirtyScope,
    operations::{InputFingerprint, OperationIdentity, OperationToken},
    optimization::PreparedPreview,
};
use indicatrix::{
    geometry::meet_solver::{SolveControl, SolvedTier},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    CANONICAL_LIGHTING_PRESET, Design, Edit, History, MaterialSelection, optimize::SearchStage,
};
use indicatrix_editor::{
    EditorSession,
    optimize_view::edits_for_relation_session,
    retarget::{
        CrownShift, GirdleAllowance,
        check::{AnalysisResult, CheckInputs, RetargetCheck, run_check},
        plan::build_plan,
        search::{SearchInputs, SearchSettings, run_search},
        view::resolved_material_from_selection,
    },
};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct RetargetControls {
    pub target: MaterialSelection,
    pub crown: CrownShift,
    pub girdle: Option<GirdleAllowance>,
    pub search: Option<SearchSettings>,
}

#[derive(Default)]
pub struct RetargetWorker {
    original: Option<(InputFingerprint, Arc<AnalysisResult>)>,
}

pub struct RetargetJob {
    pub token: OperationToken,
    original_key: InputFingerprint,
    design: Design,
    custom: Vec<GemMaterial>,
    controls: RetargetControls,
    policy: Arc<ComputePolicy>,
}

pub struct RetargetResult {
    pub identity: OperationIdentity,
    pub previews: Vec<PreparedPreview>,
    pub original_solved: Vec<SolvedTier>,
    pub controls: RetargetControls,
    pub checks: Vec<RetargetCheck>,
    pub labels: Vec<String>,
    pub notes: Vec<String>,
    pub plan: indicatrix_editor::retarget::RetargetPlan,
}

/// The fingerprint of everything a Retarget result depends on.
///
/// # Errors
///
/// A message when the design or the custom materials cannot be encoded.
pub fn fingerprint(
    state: &AppState,
    controls: &RetargetControls,
) -> Result<InputFingerprint, String> {
    let original = original_key(state)?;
    Ok(InputFingerprint::from_parts([
        b"retarget-v1".as_slice(),
        &original.0,
        format!("{controls:?}").as_bytes(),
    ]))
}

fn original_key(state: &AppState) -> Result<InputFingerprint, String> {
    let design = crate::deep_solve::fingerprint(state)?;
    let custom =
        serde_json::to_vec(&state.props.custom_materials).map_err(|error| error.to_string())?;
    Ok(InputFingerprint::from_parts([
        &design.0[..],
        custom.as_slice(),
    ]))
}

impl RetargetJob {
    /// Captures an immutable job from the current design and starts its operation.
    ///
    /// # Errors
    ///
    /// A message when the current or the target material cannot be resolved, or the design
    /// cannot be encoded.
    pub fn capture(state: &mut AppState, controls: RetargetControls) -> Result<Self, String> {
        // Concave designs are accepted: the check and the search measure every column on
        // the stone with its tools (`measure_analysis_column`).
        crate::operation_material::source(&state.session.design, &state.props.custom_materials)?;
        crate::operation_material::target(&controls.target, &state.props.custom_materials)?;
        let inputs = fingerprint(state, &controls)?;
        let original_key = original_key(state)?;
        let token = state
            .operations
            .begin(state.session.current_generation(), inputs);
        Ok(Self {
            token,
            original_key,
            design: state.session.design.clone(),
            custom: state.props.custom_materials.clone(),
            controls,
            policy: state.render.compute_policy(),
        })
    }

    /// Runs the check and the optional search on the governed worker.
    ///
    /// # Errors
    ///
    /// A message when the run is interrupted or cancelled, the plan has a relation error, a
    /// material cannot be resolved, or the search or a solve fails.
    pub fn run(
        self,
        worker: &mut RetargetWorker,
        progress: &dyn Fn(usize, SearchStage),
    ) -> Result<RetargetResult, String> {
        let _permit = self
            .policy
            .acquire(&|| self.token.is_cancelled())
            .ok_or_else(|| "Retarget interrupted.".to_owned())?;
        let stop = || self.token.is_cancelled() || self.policy.paused();
        let mut target = resolved_material_from_selection(&self.controls.target, &self.custom);
        target.gem = crate::operation_material::target(&self.controls.target, &self.custom)?;
        let plan = build_plan(&self.design, &target, self.controls.crown, &self.custom);
        if let Some(error) = &plan.relation_error {
            return Err(error.clone());
        }
        let current = crate::operation_material::source(&self.design, &self.custom)?;
        let cached = worker
            .original
            .as_ref()
            .filter(|(key, _)| *key == self.original_key)
            .map(|(_, analysis)| Arc::clone(analysis));
        let (analysis, check) = run_check(
            &CheckInputs {
                design: &self.design,
                plan: &plan,
                current_gem: Some(&current),
                lighting: CANONICAL_LIGHTING_PRESET,
                girdle: self.controls.girdle,
            },
            cached,
            &stop,
        )
        .ok_or_else(|| "Retarget interrupted; run again to start a new calculation.".to_owned())?;
        worker.original = Some((self.original_key, Arc::clone(&analysis)));
        let mut checks = vec![check];
        let mut labels = vec!["Checked Shift".to_owned()];
        let mut notes = plan.notes.clone();
        if let Some(settings) = &self.controls.search {
            let report = run_search(
                &SearchInputs {
                    design: &self.design,
                    plan: &plan,
                    current_gem: Some(&current),
                    lighting: CANONICAL_LIGHTING_PRESET,
                    settings,
                },
                self.token.cancel_flag(),
                &|count, stage| {
                    if self.policy.paused() {
                        self.token.request_cancel();
                    }
                    progress(count, stage);
                },
            )
            .map_err(|error| error.to_string())?;
            let baseline = analysis
                .as_ref()
                .as_ref()
                .ok()
                .and_then(|analysis| analysis.preservation.clone())
                .ok_or_else(|| "No original preservation baseline is available.".to_owned())?;
            checks.clear();
            labels.clear();
            notes.extend(report.notes());
            for candidate in report.candidates {
                labels.push(format!(
                    "{:?} · score {:.4} · window {:.2}% · brilliance {:.2}%",
                    candidate.kind,
                    candidate.numbers.score,
                    candidate.numbers.windowing_pct,
                    candidate.numbers.brilliance_pct
                ));
                checks.push(RetargetCheck {
                    candidate: Some(candidate.design),
                    baseline: Some(Arc::clone(&baseline)),
                    validity: candidate.validity,
                    anchors: candidate.anchors,
                    metrics: candidate.metrics,
                });
            }
        }
        if stop() {
            return Err("Retarget interrupted. No completed candidate is available.".into());
        }
        let (original_solved, previews) =
            prepare_previews(&self.design, &checks, &self.controls.target, &stop)?;
        Ok(RetargetResult {
            identity: self.token.identity,
            previews,
            original_solved,
            controls: self.controls,
            checks,
            labels,
            notes,
            plan,
        })
    }
}

/// The original design's solve and one prepared preview per checked candidate.
///
/// Reproduction and solving stay on the governed worker; `stop` is checked before each one.
fn prepare_previews(
    original: &Design,
    checks: &[RetargetCheck],
    target: &MaterialSelection,
    stop: &dyn Fn() -> bool,
) -> Result<(Vec<SolvedTier>, Vec<PreparedPreview>), String> {
    let original_solved = original
        .solve_with(&SolveControl::with_stop(stop))
        .map_err(|error| error.to_string())?;
    let previews = checks
        .iter()
        .map(|check| {
            if stop() {
                return Err("Preview preparation interrupted.".to_owned());
            }
            let (design, edit) = prepare_candidate(original, check, target)?;
            let solved = design
                .solve_with(&SolveControl::with_stop(stop))
                .map_err(|error| error.to_string())?;
            Ok((design, solved, edit))
        })
        .collect();
    Ok((original_solved, previews))
}

fn materialize(
    state: &AppState,
    result: &RetargetResult,
    index: usize,
    inputs: InputFingerprint,
) -> Result<(Design, Edit), String> {
    if !state
        .operations
        .can_apply(result.identity, state.session.current_generation(), inputs)
    {
        return Err("This Retarget result is stale. Recheck the change.".into());
    }
    result
        .previews
        .get(index)
        .ok_or_else(|| "Select a checked candidate first.".to_owned())?
        .as_ref()
        .map(|(design, _, edit)| (design.clone(), edit.clone()))
        .map_err(Clone::clone)
}

fn prepare_candidate(
    original: &Design,
    check: &RetargetCheck,
    target: &MaterialSelection,
) -> Result<(Design, Edit), String> {
    let geometry = check.validated_edit(original)?;
    let mut edits = match geometry {
        Edit::Batch(edits) => edits,
        other => vec![other],
    };
    edits.push(Edit::SetMaterial {
        material: target.clone(),
    });
    let edit = edits_for_relation_session(original, Edit::Batch(edits));
    let mut probe = EditorSession::with_history(original.clone(), History::new());
    probe
        .try_apply(edit.clone())
        .map_err(|error| error.to_string())?;
    let mut expected = check
        .candidate
        .clone()
        .ok_or_else(|| "No validated candidate geometry.".to_owned())?;
    expected.material = target.clone();
    if probe.design != expected || !probe.design.tier_ids_eq(&expected) {
        return Err("The shared session could not reproduce the exact accepted candidate.".into());
    }
    Ok((probe.design, edit))
}

/// The design a checked candidate would produce, for the preview.
///
/// # Errors
///
/// A message when the result is stale, no candidate is selected, or the candidate could not
/// be prepared.
pub fn preview(
    state: &AppState,
    result: &RetargetResult,
    index: usize,
    inputs: InputFingerprint,
) -> Result<Design, String> {
    materialize(state, result, index, inputs).map(|(design, _)| design)
}

/// Applies one checked candidate as a single edit.
///
/// # Errors
///
/// The same messages as [`preview`], or the edit engine's message when it refuses the edit.
pub fn apply(
    state: &mut AppState,
    result: &RetargetResult,
    index: usize,
    inputs: InputFingerprint,
) -> Result<(), String> {
    let (_, edit) = materialize(state, result, index, inputs)?;
    let before = state.session.current_generation();
    state
        .session
        .try_apply_mapped(edit, Some("Retarget material and cutting geometry"))
        .map_err(|error| error.to_string())?;
    state
        .solve
        .note_edit(before, state.session.current_generation(), DirtyScope::All);
    state.operations.invalidate();
    Ok(())
}
