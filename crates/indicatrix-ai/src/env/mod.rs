//! Reinforcement learning environments and Markov Decision Processes for gemstone design.
//!
//! Both environments are deterministic and greedy (see [`traits`]), take a per-tier action
//! ([`ContinuousTierAction`]) and describe the stone with the observation layout of
//! [`observation`], versioned by [`OBS_LAYOUT_VERSION`]. A step may try its action at several
//! scales and keep the first improving one ([`GemstoneRlEnv::step_scaled`]). [`teacher`]
//! holds what the simulator can say about the direction to move: the finite-difference
//! labels of the per-tier component sensitivities and the steepest-descent oracle step. A
//! label is the join ([`assemble_label`]) of one probe per free tier ([`TierProbe`]) and an
//! oracle step ([`OracleOutcome`]); both environments expose the pieces (`probe_tier`,
//! `oracle_outcome`) so a trainer can schedule the probes of many designs as independent
//! tasks.

pub mod action;
pub(crate) mod common;
pub mod error;
mod features;
pub(crate) mod moves;
pub mod observation;
mod observe;
pub mod optimizer_env;
pub mod retarget_env;
pub mod reward;
pub(crate) mod stepping;
pub mod teacher;
#[cfg(test)]
mod test_fixtures;
pub mod traits;

pub use action::ContinuousTierAction;
pub use common::design_source_material;
pub use error::EnvError;
pub use moves::{MAX_SAFE_CANDIDATE_ANGLE_DEG, MIN_SAFE_CANDIDATE_ANGLE_DEG};
pub use observation::{
    CROWN_MAX_ANGLE_DEG, CROWN_MIN_ANGLE_DEG, GLOBAL_DIM, GemstoneObservation, GlobalFeature,
    IMMOBILE_INDEX, MOBILITY_INDEX, OBS_LAYOUT_VERSION, ObservationError,
    PAVILION_CRITICAL_MARGIN_DEG, PAVILION_MAX_ANGLE_DEG, TASK_KIND_INDEX, TIER_DIM, TierFeature,
    WEIGHT_FRACTION_INDEX, angle_envelope, flat_len, objective_coefficients, split_flat,
    weight_fractions,
};
pub use optimizer_env::{OptimizerEnv, OptimizerEnvConfig};
pub use retarget_env::{RetargetEnv, RetargetEnvConfig};
pub use reward::{RewardConfig, RewardEvaluator, StepFlags};
pub use teacher::{
    COMPONENT_BRILLIANCE_LOSS, COMPONENT_DEEPER_TONE_LOSS, COMPONENT_EXTINCTION,
    COMPONENT_LIGHTER_TONE_LOSS, COMPONENT_SHAPE_DISTANCE, COMPONENT_WINDOWING,
    COMPONENT_YIELD_LOSS, DEFAULT_LINE_SEARCH, MEAN_BOUND, N_COMPONENTS, OracleOutcome, SLOPE_EPS,
    SLOPE_NORM_FLOOR, TeacherLabel, TierProbe, TierSensitivity, action_from_direction,
    assemble_label, direction_from_slopes, score_slopes,
};
pub use traits::{GemstoneRlEnv, Step, StepInfo, StepRejection};
