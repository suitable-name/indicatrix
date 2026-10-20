//! Adapter bridging `indicatrix-ai` retargeting with `indicatrix-editor::retarget`.

use crate::inference::{CertifiedRetargetResult, InferenceError, RlRetargetModel};
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

    /// Return the complete original-relative result. The policy and its immutable meet baseline
    /// both start from `design`; no shifted geometry is recaptured as a new baseline.
    pub fn retarget_certified(
        &self,
        design: &Design,
        target_material: &ResolvedMaterial,
        weights: ObjectiveWeights,
    ) -> Result<CertifiedRetargetResult, InferenceError> {
        self.model
            .retarget_certified(design, target_material, weights)
    }

    /// Compatibility helper for consumers that require the editor's angle-only plan.
    /// It returns a plan only when the editor's re-anchoring reconstructs the certified full
    /// design exactly; otherwise use [`Self::retarget_certified`] to consume exact masts.
    ///
    /// # Errors
    ///
    /// [`InferenceError::Baseline`] when the original design
    /// does not solve into a closed stone, [`InferenceError::Env`] when the baseline stone
    /// cannot be scored or has no movable tier, and the errors of
    /// [`RlRetargetModel::retarget`].
    pub fn build_plan_with_rl(
        &self,
        design: &Design,
        target_material: &ResolvedMaterial,
        weights: ObjectiveWeights,
    ) -> Result<RetargetPlan, InferenceError> {
        let base_plan = build_plan(design, target_material, CrownShift::default(), &[]);
        let original =
            analyze(design, true).map_err(|reason| InferenceError::Baseline(reason.message()))?;
        let certified = self
            .model
            .retarget_certified(design, target_material, weights)?;
        let angles: Vec<_> = base_plan
            .rows
            .iter()
            .filter(|row| row.moves)
            .map(|row| {
                (
                    row.tier_index,
                    certified.design.tiers[row.tier_index].angle_deg,
                )
            })
            .collect();
        let plan = base_plan.with_angles(design, &angles);
        let (rebuilt, _) = anchored_candidate(design, &plan, &original);
        if rebuilt != certified.design {
            return Err(InferenceError::Roundtrip(
                "the legacy RetargetPlan cannot carry the certified solved masts and materialized geometry; use retarget_certified".into(),
            ));
        }
        Ok(plan)
    }
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

    #[test]
    fn a_tone_weight_uses_the_certified_scoring_path() {
        let adapter = EditorRetargetAdapter::new(
            RlRetargetModel::new(
                ModelTier::Small.init_network(1),
                InferenceConfig::retarget().with_steps(1),
            )
            .expect("a freshly initialised network fits the layout"),
        );
        for preset in [ObjectivePreset::LightenDark, ObjectivePreset::IntensifyPale] {
            let result = adapter.retarget_certified(&brilliant(), &quartz(), preset.weights());
            assert!(result.is_ok(), "{preset:?}");
            let result = result.expect("tone objective is supported");
            assert!(result.proposal.target_score.is_finite());
            assert!(result.preservation.original_meets > 0);
        }
    }
}
