//! Immutable verified-solve jobs, governed execution and explicit fresh-result pinning.

use crate::{
    app_state::AppState,
    compute_policy::ComputePolicy,
    design_state::DirtyScope,
    operations::{InputFingerprint, OperationIdentity, OperationToken},
};
use indicatrix::geometry::{
    meet_solver::{
        SolveControl, SolveProgress, SolvedTier, VerifiedSolveReport,
        solve_meet_points_verified_with,
    },
    stone_metrics::ExternalProportions,
};
use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, design_to_string},
};
use std::{sync::Arc, time::Duration};

pub struct DeepSolveJob {
    pub token: OperationToken,
    design: Design,
    targets: ExternalProportions,
    policy: Arc<ComputePolicy>,
}

pub struct DeepSolveResult {
    pub identity: OperationIdentity,
    pub solved: Vec<SolvedTier>,
    pub baseline: Vec<SolvedTier>,
    pub report: VerifiedSolveReport,
}

/// The fingerprint of everything a Deep Solve result depends on.
///
/// The native authoring encoding includes exact constraints, identities, material,
/// rough and printed targets. No measured candidate targets enter the fingerprint.
///
/// # Errors
///
/// The encoder's message when the design cannot be written in the native encoding.
pub fn fingerprint(state: &AppState) -> Result<InputFingerprint, String> {
    let text = design_to_string(
        &state.session.design,
        state.document.printed_proportions.as_ref(),
        &DesignExtras::default(),
    )
    .map_err(|error| error.to_string())?;
    Ok(InputFingerprint::from_parts([
        b"deep-solve-v1".as_slice(),
        text.as_bytes(),
    ]))
}

impl DeepSolveJob {
    /// Captures an immutable job from the current design and starts its operation.
    ///
    /// # Errors
    ///
    /// The availability hint when Deep Solve cannot run on this design, the fingerprint
    /// error when the design cannot be encoded, or a missing-targets message.
    pub fn capture(state: &mut AppState) -> Result<Self, String> {
        let (available, hint) = indicatrix_editor::deep_solve::availability(
            &state.session.design,
            state.document.printed_proportions.as_ref(),
        );
        if !available {
            return Err(hint);
        }
        let inputs = fingerprint(state)?;
        let token = state
            .operations
            .begin(state.session.current_generation(), inputs);
        Ok(Self {
            token,
            design: state.session.design.clone(),
            targets: state
                .document
                .printed_proportions
                .ok_or_else(|| "Missing printed targets".to_owned())?,
            policy: state.render.compute_policy(),
        })
    }

    /// Runs the verified search.
    ///
    /// Runs only on a dedicated operation queue. Thermal/background interruption
    /// cancels the search at real solver checkpoints; rerunning is a new search.
    ///
    /// # Errors
    ///
    /// A message when the search is interrupted or cancelled, or when the plain or the
    /// verified solve fails.
    pub fn run(self, progress: &dyn Fn(SolveProgress)) -> Result<DeepSolveResult, String> {
        let _permit = self
            .policy
            .acquire(&|| self.token.is_cancelled())
            .ok_or_else(|| "Deep Solve interrupted.".to_owned())?;
        let stop = || self.token.is_cancelled() || self.policy.paused();
        let control = SolveControl::with_stop(&stop).reporting(progress);
        let baseline = self
            .design
            .solve_with(&control)
            .map_err(|error| error.to_string())?;
        if stop() {
            return Err("Deep Solve interrupted.".into());
        }
        let tiers = self
            .design
            .resolved_meet_tier_inputs()
            .map_err(|error| error.to_string())?;
        if stop() {
            return Err("Deep Solve interrupted.".into());
        }
        let (solved, report) = solve_meet_points_verified_with(
            self.design.meta.gear_teeth_abs(),
            &tiers,
            &self.targets,
            &[],
            &control,
        )
        .map_err(|error| error.to_string())?;
        if stop() {
            return Err("Deep Solve interrupted; run again to start a new search.".into());
        }
        Ok(DeepSolveResult {
            identity: self.token.identity,
            solved,
            baseline,
            report,
        })
    }
}

/// Pin one explicitly chosen verified mast, through the shared edit engine.
///
/// # Errors
///
/// A message when the result is stale or interrupted, the report was not accepted, the
/// tier no longer exists, or the edit is refused.
pub fn pin(
    state: &mut AppState,
    result: &DeepSolveResult,
    tier: usize,
    now: Duration,
) -> Result<bool, String> {
    let inputs = fingerprint(state)?;
    let before = state.session.current_generation();
    if !state.operations.can_apply(result.identity, before, inputs) {
        return Err("Deep Solve is stale or interrupted. Run it again before pinning.".into());
    }
    if !result.report.accepted {
        return Err("Only a verified accepted report can pin a mast.".into());
    }
    let mast = result
        .solved
        .get(tier)
        .ok_or_else(|| "This tier no longer exists.".to_owned())?
        .mast;
    let changed = state
        .session
        .pin_tier_mast(tier, mast, now)
        .map_err(|error| error.to_string())?
        .is_some();
    if changed {
        state
            .solve
            .note_edit(before, state.session.current_generation(), DirtyScope::All);
        state.operations.invalidate();
    }
    Ok(changed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{operations::OperationPhase, platform_mock::MockPlatform};
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::templates::TEMPLATES;
    use indicatrix_editor::EditorSession;

    fn printed() -> ExternalProportions {
        ExternalProportions {
            vol_w3: Some(0.62),
            lw: Some(1.0),
            ..ExternalProportions::default()
        }
    }

    /// A session on the first template with the given printed proportions.
    fn state_with(targets: Option<ExternalProportions>) -> AppState {
        let mut state = AppState::new(Arc::new(MockPlatform::default()));
        let template = &TEMPLATES[0];
        state.session = EditorSession::from_template(
            template.fresh_spec(template.default_material_selection()),
            1,
        );
        state.document.printed_proportions = targets;
        state
    }

    /// [`state_with`] after "Adopt" on the first tier: it now follows a meet instead of
    /// being pinned to a fixed mast, which is what gives Deep Solve something to repair.
    /// A freshly built template has every tier pinned, and Deep Solve declines that.
    fn adopted_state() -> AppState {
        let mut state = state_with(Some(printed()));
        state.session.design.tiers[0].constraint = MeetConstraint::MeetExisting;
        state
    }

    /// A completed operation of the current inputs, as the worker's answer would leave it.
    fn completed(state: &mut AppState, accepted: bool) -> DeepSolveResult {
        let inputs = fingerprint(state).expect("the design encodes");
        let generation = state.session.current_generation();
        let token = state.operations.begin(generation, inputs);
        for phase in [OperationPhase::Running, OperationPhase::Completed] {
            assert!(
                state
                    .operations
                    .publish(token.identity, generation, inputs, phase)
            );
        }
        DeepSolveResult {
            identity: token.identity,
            solved: Vec::new(),
            baseline: Vec::new(),
            report: VerifiedSolveReport {
                initial_score: 0.2,
                score_after_calibration: 0.1,
                final_score: 0.01,
                accepted,
                overrides_applied: 0,
                anchor_moves_applied: 0,
                pipeline_runs: 1,
            },
        }
    }

    #[test]
    fn a_design_without_printed_proportions_is_declined_before_any_operation_starts() {
        let mut state = state_with(None);
        let Err(error) = DeepSolveJob::capture(&mut state) else {
            panic!("Deep Solve must decline a design that has no printed proportions");
        };
        assert!(error.contains("printed proportions"), "{error}");
        assert!(state.operations.phase().is_none());
    }

    #[test]
    fn capture_starts_one_operation_on_the_current_generation() {
        let mut state = adopted_state();
        let job = DeepSolveJob::capture(&mut state).expect("the template is available");
        assert_eq!(
            job.token.identity.generation,
            state.session.current_generation()
        );
        assert_eq!(state.operations.phase(), Some(&OperationPhase::Preparing));
    }

    #[test]
    fn the_fingerprint_follows_the_design_and_the_printed_figures() {
        let mut state = state_with(Some(printed()));
        let first = fingerprint(&state).expect("encodes");
        assert_eq!(first, fingerprint(&state).expect("encodes"));

        state.document.printed_proportions = Some(ExternalProportions {
            vol_w3: Some(0.7),
            ..printed()
        });
        let other_targets = fingerprint(&state).expect("encodes");
        assert_ne!(first, other_targets);

        state.document.printed_proportions = Some(printed());
        state.session.design.tiers[0].angle_deg += 1.0;
        assert_ne!(first, fingerprint(&state).expect("encodes"));
    }

    #[test]
    fn a_job_cancelled_before_it_runs_returns_no_result() {
        let mut state = adopted_state();
        let job = DeepSolveJob::capture(&mut state).expect("the template is available");
        job.token.request_cancel();
        assert!(job.run(&|_| {}).is_err());
    }

    #[test]
    fn only_an_accepted_report_can_pin_a_mast() {
        let mut state = state_with(Some(printed()));
        let result = completed(&mut state, false);
        assert_eq!(
            pin(&mut state, &result, 0, Duration::ZERO),
            Err("Only a verified accepted report can pin a mast.".to_owned())
        );
    }

    #[test]
    fn a_tier_the_result_does_not_hold_cannot_be_pinned() {
        let mut state = state_with(Some(printed()));
        let result = completed(&mut state, true);
        assert_eq!(
            pin(&mut state, &result, 0, Duration::ZERO),
            Err("This tier no longer exists.".to_owned())
        );
    }

    #[test]
    fn a_result_of_inputs_that_changed_is_stale_and_pins_nothing() {
        let mut state = state_with(Some(printed()));
        let result = completed(&mut state, true);
        // The same generation, other authoring data: the fingerprint, not the generation,
        // is what rejects it.
        state.session.design.tiers[0].angle_deg += 1.0;
        let error = pin(&mut state, &result, 0, Duration::ZERO).expect_err("stale");
        assert!(error.contains("stale"), "{error}");
    }
}
