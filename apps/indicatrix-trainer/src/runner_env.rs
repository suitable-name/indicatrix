//! The environments of a run: one wrapper over the two environment kinds, the target material of
//! a retarget episode and the slot that holds one running episode.

use indicatrix_ai::{
    env::{
        ContinuousTierAction, EnvError, GemstoneObservation, GemstoneRlEnv, OptimizerEnv,
        OptimizerEnvConfig, OracleOutcome, RetargetEnv, RetargetEnvConfig, Step, TeacherLabel,
        TierProbe, design_source_material,
    },
    model::TrainingMode,
};
use indicatrix_cut_core::{
    Design, ObjectiveWeights, ResolvedMaterial, optimize::ObjectiveFidelity,
};

use super::{MaterialLabel, RunConfig, RunnerError, draws_same_material};
use crate::dataset::{find_spectrum_entry, resolve_entry, sample_target};

/// Name shown for a design whose material is only a refractive index (every `.asc` import).
const DESIGN_MATERIAL_NAME: &str = "Design (I-line)";

/// One environment of either kind.
pub(super) enum EnvKind {
    // Boxed: the environments are large and differ in size.
    Optimizer(Box<OptimizerEnv>),
    Retarget(Box<RetargetEnv>),
}

impl EnvKind {
    pub(super) fn reset(&mut self, seed: u64) -> GemstoneObservation {
        match self {
            Self::Optimizer(e) => e.reset(seed),
            Self::Retarget(e) => e.reset(seed),
        }
    }

    /// One step along a line search (see `GemstoneRlEnv::step_scaled`).
    pub(super) fn step_scaled(
        &mut self,
        action: &ContinuousTierAction,
        scales: &[f64],
    ) -> Step<GemstoneObservation> {
        match self {
            Self::Optimizer(e) => e.step_scaled(action, scales),
            Self::Retarget(e) => e.step_scaled(action, scales),
        }
    }

    /// The finite-difference label of the current design (see `RetargetEnv::teacher_label`);
    /// pure, it does not move the environment. An empty `scales` asks for no oracle step.
    pub(super) fn teacher_label(&self, h_deg: f64, scales: &[f64]) -> TeacherLabel {
        match self {
            Self::Optimizer(e) => e.teacher_label(h_deg, scales),
            Self::Retarget(e) => e.teacher_label(h_deg, scales),
        }
    }

    /// The probe of one free tier of the current design (phase 1 of a labelled pass); pure.
    pub(super) fn probe_tier(&self, tier: usize, h_deg: f64) -> TierProbe {
        match self {
            Self::Optimizer(e) => e.probe_tier(tier, h_deg),
            Self::Retarget(e) => e.probe_tier(tier, h_deg),
        }
    }

    /// The oracle step along the score `slopes` of the current design (phase 2 of a labelled
    /// pass); pure.
    pub(super) fn oracle_outcome(&self, slopes: &[Option<f32>], scales: &[f64]) -> OracleOutcome {
        match self {
            Self::Optimizer(e) => e.oracle_outcome(slopes, scales),
            Self::Retarget(e) => e.oracle_outcome(slopes, scales),
        }
    }

    /// The tiers the environment moves.
    pub(super) fn free_tiers(&self) -> &[usize] {
        match self {
            Self::Optimizer(e) => e.free_tier_indices(),
            Self::Retarget(e) => e.movable_tier_indices(),
        }
    }

    /// The objective weights the environment scores with.
    pub(super) fn weights(&self) -> &ObjectiveWeights {
        match self {
            Self::Optimizer(e) => e.weights(),
            Self::Retarget(e) => e.weights(),
        }
    }

    pub(super) fn build_observation(&self) -> GemstoneObservation {
        match self {
            Self::Optimizer(e) => e.build_observation(),
            Self::Retarget(e) => e.build_observation(),
        }
    }

    pub(super) fn base_score(&self) -> f32 {
        match self {
            Self::Optimizer(e) => e.base_score(),
            Self::Retarget(e) => e.base_score(),
        }
    }

    pub(super) fn current_score(&self) -> f32 {
        match self {
            Self::Optimizer(e) => e.current_score(),
            Self::Retarget(e) => e.current_score(),
        }
    }

    pub(super) fn max_step_deg(&self) -> f64 {
        match self {
            Self::Optimizer(e) => e.config().max_step_deg,
            Self::Retarget(e) => e.config().max_step_deg,
        }
    }

    pub(super) fn current_design(&self) -> &Design {
        match self {
            Self::Optimizer(e) => e.current_design(),
            Self::Retarget(e) => e.current_design(),
        }
    }

    pub(super) fn source_material(&self) -> &ResolvedMaterial {
        match self {
            Self::Optimizer(e) => e.source_material(),
            Self::Retarget(e) => e.source_material(),
        }
    }
}

/// The target material of a retarget episode.
pub(super) struct Target {
    pub(super) name: String,
    pub(super) material: ResolvedMaterial,
}

/// The name of the material `design` was authored for: its named preset, or a placeholder for a
/// design that carries only a refractive index.
pub(super) fn design_material_name(design: &Design) -> String {
    design
        .material
        .name
        .clone()
        .unwrap_or_else(|| DESIGN_MATERIAL_NAME.to_owned())
}

/// The target of the retarget episode of `design` drawn for `seed` (`None` for the optimizer
/// task).
///
/// A share of the episodes (`cfg.same_material_frac`) is judged in the design's own material, so
/// one model learns both tasks; in the others the target is a random species of the spectrum, or
/// the configured fixed material.
pub(super) fn pick_target(
    cfg: &RunConfig,
    seed: u64,
    design: &Design,
) -> Result<Option<Target>, RunnerError> {
    if cfg.mode != TrainingMode::Retarget {
        return Ok(None);
    }
    if draws_same_material(cfg.same_material_frac, seed) {
        return Ok(Some(Target {
            name: design_material_name(design),
            material: design_source_material(design),
        }));
    }
    let entry = if cfg.universal_spectrum {
        sample_target(seed)
    } else {
        find_spectrum_entry(&cfg.fixed_target)?
    };
    Ok(Some(Target {
        name: entry.name.to_owned(),
        material: resolve_entry(&entry)?,
    }))
}

/// Builds the environment the task of `cfg` asks for around `design`, scoring with `weights`;
/// `fidelity` overrides the scoring fidelity and `cfg.max_step_deg` the step limit of either
/// environment kind.
pub(super) fn build_env(
    cfg: &RunConfig,
    design: &Design,
    target: Option<&Target>,
    weights: ObjectiveWeights,
    fidelity: Option<ObjectiveFidelity>,
) -> Result<EnvKind, EnvError> {
    if let (TrainingMode::Retarget, Some(target)) = (cfg.mode, target) {
        let mut config = RetargetEnvConfig::new_with_target(target.material.clone());
        config.weights = weights;
        if let Some(f) = fidelity {
            config.fidelity = f;
        }
        if let Some(h) = cfg.max_step_deg {
            config.max_step_deg = h;
        }
        RetargetEnv::new(design.clone(), config).map(|e| EnvKind::Retarget(Box::new(e)))
    } else {
        let mut config = OptimizerEnvConfig {
            weights,
            ..OptimizerEnvConfig::default()
        };
        if let Some(f) = fidelity {
            config.fidelity = f;
        }
        if let Some(h) = cfg.max_step_deg {
            config.max_step_deg = h;
        }
        OptimizerEnv::new(design.clone(), config).map(|e| EnvKind::Optimizer(Box::new(e)))
    }
}

/// One vectorized environment slot: a running episode.
pub(super) struct Slot {
    pub(super) env: EnvKind,
    /// Position in the train split (corpus mode cycling).
    pub(super) pos: usize,
    /// Index into the data set's `entries`.
    pub(super) entry: usize,
    pub(super) src: MaterialLabel,
    pub(super) tgt: MaterialLabel,
    pub(super) obs: GemstoneObservation,
}
