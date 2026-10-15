//! Adapter bridging `indicatrix-ai` retargeting with `indicatrix-editor::retarget`.

use crate::inference::{
    InferenceError, RlRetargetModel, RlRetargetProposal, ensure_supported_objective,
};
use indicatrix_cut_core::{Design, ObjectiveWeights, ResolvedMaterial};
use indicatrix_editor::retarget::{
    CrownShift, RetargetPlan, anchors::anchored_candidate, build_plan, validity::analyze,
};

/// Adapter executing neural RL material retargeting matching `indicatrix-editor`'s workflow.
#[derive(Debug, Clone)]
pub struct EditorRetargetAdapter {
    model: RlRetargetModel,
}

impl EditorRetargetAdapter {
    /// Construct adapter wrapping an RL retarget model.
    #[must_use]
    pub const fn new(model: RlRetargetModel) -> Self {
        Self { model }
    }

    /// Build an RL-enhanced retarget plan: the editor's critical-angle shift baseline, with the
    /// policy's refinements on top.
    ///
    /// The policy starts from the baseline (the design with the shift's angles applied and its
    /// hinge-anchored masts re-anchored, exactly as the editor would apply the baseline), so it
    /// improves on the shift instead of redoing it. Every moving row keeps its baseline angle
    /// unless the policy changed that tier. The rows' margins, risks and followers are worked
    /// out again for the merged angles by `RetargetPlan::with_angles`. When the policy finds
    /// nothing better than the baseline, the result is the baseline plan with its notes
    /// rebuilt.
    ///
    /// The policy anchors tiers about the hinges of the baseline stone, the editor about those
    /// of the original stone, so the masts the editor computes on apply can differ slightly from
    /// the ones the policy scored.
    ///
    /// # Errors
    ///
    /// [`InferenceError::UnsupportedObjective`] when `weights` weigh the face-up tone (checked
    /// before the baseline is built), [`InferenceError::Baseline`] when the original design
    /// does not solve into a closed stone, [`InferenceError::Env`] when the baseline stone
    /// cannot be scored or has no movable tier, and the errors of
    /// [`RlRetargetModel::retarget`].
    pub fn build_plan_with_rl(
        &self,
        design: &Design,
        target_material: &ResolvedMaterial,
        weights: ObjectiveWeights,
    ) -> Result<RetargetPlan, InferenceError> {
        ensure_supported_objective(&weights)?;
        let base_plan = build_plan(design, target_material, CrownShift::default(), &[]);
        let original =
            analyze(design, true).map_err(|reason| InferenceError::Baseline(reason.message()))?;
        let (baseline, _anchors) = anchored_candidate(design, &base_plan, &original);
        let proposal = self.model.retarget(&baseline, target_material, weights)?;
        Ok(base_plan.with_angles(design, &merged_angles(&base_plan, &proposal)))
    }
}

/// `(tier_index, angle)` of every moving row of `plan`: the policy's angle where it changed
/// the tier, the plan's own (baseline) angle otherwise.
///
/// `RetargetPlan::with_angles` gives a moving row it is not told about its OLD angle, which
/// would undo the baseline shift, so every moving row has to be named.
fn merged_angles(plan: &RetargetPlan, proposal: &RlRetargetProposal) -> Vec<(usize, f64)> {
    plan.rows
        .iter()
        .filter(|row| row.moves)
        .map(|row| {
            (
                row.tier_index,
                proposal.angle_of(row.tier_index).unwrap_or(row.new_angle),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{inference::InferenceConfig, model::ModelTier};
    use indicatrix_cut_core::{
        BuiltinMaterials, ConstraintTier, MaterialSelection, ObjectivePreset, PreformSpec,
        ScheduleMeta,
    };

    fn brilliant() -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            ..MaterialSelection::none()
        };
        design
    }

    fn quartz() -> ResolvedMaterial {
        MaterialSelection {
            name: Some("Quartz".to_string()),
            ..MaterialSelection::none()
        }
        .resolve(&BuiltinMaterials)
    }

    fn proposal(angles: Vec<(usize, f64)>) -> RlRetargetProposal {
        RlRetargetProposal {
            target_material_name: "Quartz".to_string(),
            base_score: 30.0,
            target_score: 29.0,
            steps_taken: 2,
            evaluations: 5,
            proposed_angles_deg: angles,
        }
    }

    #[test]
    fn a_tone_weight_is_refused_up_front() {
        let adapter = EditorRetargetAdapter::new(
            RlRetargetModel::new(
                ModelTier::Small.init_network(1),
                InferenceConfig::retarget().with_steps(1),
            )
            .expect("a freshly initialised network fits the layout"),
        );
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let result = adapter.build_plan_with_rl(&brilliant(), &quartz(), preset.weights());
            assert!(
                matches!(result, Err(InferenceError::UnsupportedObjective(_))),
                "{preset:?}: {:?}",
                result.map(|_| ())
            );
        }
    }

    #[test]
    fn untouched_rows_keep_the_baseline_shift() {
        let design = brilliant();
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let moving: Vec<_> = plan.rows.iter().filter(|row| row.moves).collect();
        assert!(moving.len() >= 2, "a brilliant has several moving tiers");

        let merged = merged_angles(&plan, &proposal(Vec::new()));
        assert_eq!(merged.len(), moving.len());
        for (&(index, angle), row) in merged.iter().zip(&moving) {
            assert_eq!(index, row.tier_index);
            assert_eq!(angle, row.new_angle);
        }
    }

    #[test]
    fn a_changed_tier_takes_the_policy_angle() {
        let design = brilliant();
        let plan = build_plan(&design, &quartz(), CrownShift::default(), &[]);
        let first = plan
            .rows
            .iter()
            .find(|row| row.moves)
            .expect("a moving row");
        let override_deg = first.new_angle + 0.5;
        let merged = merged_angles(&plan, &proposal(vec![(first.tier_index, override_deg)]));
        let entry = merged
            .iter()
            .find(|&&(index, _)| index == first.tier_index)
            .expect("the row is listed");
        assert_eq!(entry.1, override_deg);
        for (&(index, angle), row) in merged.iter().zip(plan.rows.iter().filter(|row| row.moves)) {
            if index != first.tier_index {
                assert_eq!(angle, row.new_angle);
            }
        }
    }
}
