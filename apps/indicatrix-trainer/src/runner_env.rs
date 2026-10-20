//! The environments of a run: one wrapper over the two environment kinds, the target material of
//! a retarget episode and the slot that holds one running episode.

use indicatrix::optics::materials::AbsorptionUnit;
use indicatrix::optics::raytracer::LightingPreset;
use indicatrix_ai::{
    env::{
        ContinuousTierAction, EnvError, GemstoneObservation, GemstoneRlEnv, OptimizerEnv,
        OptimizerEnvConfig, OracleOutcome, RetargetEnv, RetargetEnvConfig, Step, TeacherLabel,
        TierProbe,
    },
    model::TrainingMode,
};
use indicatrix_cut_core::{
    Design, ObjectiveWeights, ResolvedMaterial, ShapeTarget, optimize::ObjectiveFidelity,
};

use super::{MaterialLabel, RunConfig, RunnerError, draws_same_material};
use crate::dataset::{MINERAL_SPECTRUM, find_spectrum_entry, resolve_entry, sample_target};

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
    pub(super) augmentation: String,
}

/// The target of the retarget episode of `design` drawn for `seed` (`None` for the optimizer
/// task).
///
/// A share of the episodes (`cfg.same_material_frac`) is judged in the source material as a
/// robustness subset; in the others the target is sampled from the spectrum or fixed material.
pub(super) fn pick_target(
    cfg: &RunConfig,
    seed: u64,
    _design: &Design,
) -> Result<Option<Target>, RunnerError> {
    if cfg.mode != TrainingMode::Retarget {
        return Ok(None);
    }
    if draws_same_material(cfg.same_material_frac, seed) {
        let (source_name, material, augmentation) = sampled_source_material(seed)?;
        return Ok(Some(Target {
            name: source_name,
            material,
            augmentation,
        }));
    }
    let entry = if cfg.universal_spectrum {
        sample_target(seed)
    } else {
        find_spectrum_entry(&cfg.fixed_target)?
    };
    let (material, augmentation) = sampled_material(resolve_entry(&entry)?, seed);
    Ok(Some(Target {
        name: entry.name.to_owned(),
        material,
        augmentation,
    }))
}

/// Bounded, deterministic physical material variation over path length and inclusion density.
pub(super) fn sampled_material(
    mut material: ResolvedMaterial,
    seed: u64,
) -> (ResolvedMaterial, String) {
    const PATH_SCALES: [f32; 5] = [0.85, 0.925, 1.0, 1.075, 1.15];
    const SCATTERING: [f32; 4] = [0.0, 0.05, 0.15, 0.35];
    let mut rng = indicatrix_ai::model::Prng::from_stream(seed, 0x4D41_5445_5249_414C);
    // ModelUnit presets may enter the fluorescent vertex sampler, which only supports unit
    // path scale for them. PerMm materials have a physical scale and can use this bounded draw.
    let path_scale = if material.gem.absorption_unit == AbsorptionUnit::PerMm {
        PATH_SCALES[rng.next_index(PATH_SCALES.len())]
    } else {
        1.0
    };
    let scattering = SCATTERING[rng.next_index(SCATTERING.len())];
    let base_scale = material.gem.absorption_path_scale.max(0.01);
    material.gem = material
        .gem
        .with_absorption_path_scale(base_scale * path_scale)
        .with_scattering_amount(scattering);
    (
        material,
        format!("[path×{path_scale:.2}, scatter={scattering:.2} /unit]"),
    )
}

pub(super) fn sampled_source_material(
    seed: u64,
) -> Result<(String, ResolvedMaterial, String), crate::dataset::DatasetError> {
    let choices: Vec<_> = MINERAL_SPECTRUM
        .iter()
        .filter(|entry| entry.preset.is_some())
        .collect();
    let mut rng = indicatrix_ai::model::Prng::from_stream(seed, 0x534F_5552_4345_4D41);
    let entry = choices[rng.next_index(choices.len())];
    let (material, augmentation) = sampled_material(resolve_entry(entry)?, seed);
    Ok((entry.name.to_owned(), material, augmentation))
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
    context_seed: u64,
) -> Result<EnvKind, EnvError> {
    let (shape_target, lighting) = sampled_context(context_seed);
    if let (TrainingMode::Retarget, Some(target)) = (cfg.mode, target) {
        let mut config = RetargetEnvConfig::new_with_target(target.material.clone());
        config.weights = weights;
        config.source_material_override = Some(
            sampled_source_material(context_seed)
                .map_err(|_| {
                    EnvError::InvalidConfig("trainer source material could not be resolved")
                })?
                .1,
        );
        config.shape_target = shape_target;
        config.lighting = lighting;
        if let Some(f) = fidelity {
            config.fidelity = f;
        } else {
            config.fidelity = cfg.train_fidelity;
        }
        if let Some(h) = cfg.max_step_deg {
            config.max_step_deg = h;
        }
        config.probe_scale_deg = cfg.label_h_deg.unwrap_or(config.max_step_deg);
        RetargetEnv::new(design.clone(), config).map(|e| EnvKind::Retarget(Box::new(e)))
    } else {
        let mut config = OptimizerEnvConfig {
            weights,
            shape_target,
            lighting,
            source_material_override: Some(
                sampled_source_material(context_seed)
                    .map_err(|_| {
                        EnvError::InvalidConfig("trainer source material could not be resolved")
                    })?
                    .1,
            ),
            ..OptimizerEnvConfig::default()
        };
        config.fidelity = fidelity.unwrap_or(cfg.train_fidelity);
        if let Some(h) = cfg.max_step_deg {
            config.max_step_deg = h;
        }
        config.probe_scale_deg = cfg.label_h_deg.unwrap_or(config.max_step_deg);
        OptimizerEnv::new(design.clone(), config).map(|e| EnvKind::Optimizer(Box::new(e)))
    }
}

/// Episode context is deterministic and shared by rollout, validation passes and endpoint rescoring.
pub(super) fn sampled_context(seed: u64) -> (Option<ShapeTarget>, LightingPreset) {
    const LIGHTING: [LightingPreset; 7] = [
        LightingPreset::IsoHemisphere,
        LightingPreset::Daylight,
        LightingPreset::Incandescent,
        LightingPreset::RingLights,
        LightingPreset::ShopLights,
        LightingPreset::WhiteTray,
        LightingPreset::Aset,
    ];
    let mut shape_rng = indicatrix_ai::model::Prng::from_stream(seed, 0x5348_4150_455f_5447);
    let shape_target = match shape_rng.next_index(4) {
        0 => None,
        1 => Some(ShapeTarget {
            table_percent: Some(52.0),
            crown_to_pavilion: None,
            weight: 1.0,
        }),
        2 => Some(ShapeTarget {
            table_percent: None,
            crown_to_pavilion: Some(0.42),
            weight: 1.0,
        }),
        _ => Some(ShapeTarget {
            table_percent: Some(58.0),
            crown_to_pavilion: Some(0.50),
            weight: 1.0,
        }),
    };
    let mut lighting_rng = indicatrix_ai::model::Prng::from_stream(seed, 0x4C49_4748_5449_4E47);
    (
        shape_target,
        LIGHTING[lighting_rng.next_index(LIGHTING.len())],
    )
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
