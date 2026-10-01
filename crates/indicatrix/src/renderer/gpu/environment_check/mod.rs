//! Environment-sampling / CMF-integration / white-balance GPU self-tests: per-function
//! ULP budgets.
//!
//! Five independent checks, one per ported function, all dispatched from
//! `shaders/environment.wgsl`:
//! - [`cmf::run_cmf`][]: `color::cie1931::cie_1931_cmf`.
//! - [`blackbody::run_blackbody`][]: `optics::raytracer::blackbody_spectrum`.
//! - [`studio_env::run_studio_env`][]: `optics::raytracer::sample_studio_environment`
//!   (across all four [`LightingPreset`](crate::optics::raytracer::LightingPreset)
//!   variants).
//! - [`white_balance::run_white_balance`][]:
//!   `optics::raytracer::compute_illuminant_white_balance`'s 401-point (380..=780nm)
//!   quadrature.
//! - [`hdr_env::run_hdr_env_radiance`][]: `hdr_env_radiance_at` against
//!   `renderer::env_map::EnvironmentMap::radiance_at`.
//!
//! Every dense sweep also includes the adversarial points the task calls out
//! specifically: the 380/780nm band edges, every CMF table node and interpolation
//! midpoint (P4: `cie_1931_cmf` is now a 5nm-table lookup, not a Gaussian-lobe fit --
//! see [`cmf::build_cmf_lambdas`]), grazing incidence (dot products near 0 and near the
//! ring lights' `0.96` spark threshold), and the `temp_k`/exponent clamp boundaries.

use glam::Vec3;

mod blackbody;
mod cmf;
mod hdr_env;
mod studio_env;
mod white_balance;

pub use blackbody::{BLACKBODY_ABS_FLOOR, BLACKBODY_ULP_BUDGET, BlackbodyCase, run_blackbody};
pub use cmf::{CMF_ABS_FLOOR, CMF_ULP_BUDGET, build_cmf_lambdas, run_cmf};
pub(crate) use hdr_env::build_hdr_test_map;
pub use hdr_env::{
    HDR_ENV_ABS_FLOOR, HDR_ENV_ULP_BUDGET, HdrEnvCase, build_hdr_env_cases, run_hdr_env_radiance,
};
pub use studio_env::{
    STUDIO_ENV_ABS_FLOOR, STUDIO_ENV_ULP_BUDGET, StudioEnvCase, build_studio_env_cases,
    run_studio_env,
};
pub use white_balance::{WHITE_BALANCE_ABS_FLOOR, WHITE_BALANCE_ULP_BUDGET, run_white_balance};

const SHADER_SRC: &str = include_str!("../../shaders/environment.wgsl");

/// A generic "worst single scalar" ULP result, shared by every check in this module.
///
/// Comparisons use the hybrid ULP-OR-absolute-floor rule in
/// [`crate::renderer::gpu::ulp::within_tolerance`]: a comparison whose absolute
/// difference is under `abs_floor` (see each check's own `*_ABS_FLOOR` constant) is
/// exempted from the ULP budget entirely -- ULP is a poor metric exactly where a value
/// crosses zero or is photometrically negligible. `max_ulp`/`argmax` track the worst
/// GENUINE disagreement (i.e. excluding exempted comparisons); `max_raw_ulp` is purely
/// informational and tracks the single largest ULP distance observed across EVERY
/// comparison, exempted or not, so an exempted near-zero case is still visible in a
/// report rather than silently vanishing.
#[derive(Debug, Clone)]
pub struct UlpCheckResult<Case: Clone> {
    /// Short name of the check.
    pub label: &'static str,
    /// Total number of compared values.
    pub total_comparisons: usize,
    /// Maximum tolerated ULP distance.
    pub budget: u32,
    /// Absolute magnitude below which differences are exempt from the ULP budget.
    pub abs_floor: f32,
    /// Largest ULP distance among non-exempt comparisons.
    pub max_ulp: u32,
    /// Largest ULP distance including exempt comparisons.
    pub max_raw_ulp: u32,
    /// Number of comparisons that exceeded the budget.
    pub over_budget_count: usize,
    /// Number of comparisons exempted by the absolute floor.
    pub exempted_count: usize,
    /// Comparison with the largest ULP distance.
    pub argmax: Option<UlpArgmax<Case>>,
}

/// The comparison with the largest error in the ulp check.
#[derive(Debug, Clone)]
pub struct UlpArgmax<Case: Clone> {
    /// Input case that produced this result.
    pub case: Case,
    /// Name of the compared output component.
    pub component: &'static str,
    /// Value produced by the CPU reference.
    pub cpu: f32,
    /// Value produced by the GPU shader.
    pub gpu: f32,
    /// Distance between the CPU and GPU values in units in the last place.
    pub ulp: u32,
}

impl<Case: Clone> UlpCheckResult<Case> {
    /// Whether every compared value stayed within its budget.
    #[must_use]
    pub const fn passed(&self) -> bool {
        self.over_budget_count == 0
    }
}

/// Accumulates one [`UlpCheckResult`] over many [`Self::record`] calls -- shared by
/// every check submodule below.
struct UlpAccumulator<Case: Clone> {
    label: &'static str,
    budget: u32,
    abs_floor: f32,
    total: usize,
    max_ulp: u32,
    max_raw_ulp: u32,
    over_budget: usize,
    exempted: usize,
    argmax: Option<UlpArgmax<Case>>,
}

impl<Case: Clone> UlpAccumulator<Case> {
    const fn new(label: &'static str, budget: u32, abs_floor: f32) -> Self {
        Self {
            label,
            budget,
            abs_floor,
            total: 0,
            max_ulp: 0,
            max_raw_ulp: 0,
            over_budget: 0,
            exempted: 0,
            argmax: None,
        }
    }

    fn record(&mut self, case: &Case, component: &'static str, cpu: f32, gpu: f32) {
        self.total += 1;
        let ulp = crate::renderer::gpu::ulp::ulp_distance(cpu, gpu);
        if ulp > self.max_raw_ulp {
            self.max_raw_ulp = ulp;
        }
        let within =
            crate::renderer::gpu::ulp::within_tolerance(cpu, gpu, self.budget, self.abs_floor);
        if !within {
            self.over_budget += 1;
            if ulp > self.max_ulp {
                self.max_ulp = ulp;
                self.argmax = Some(UlpArgmax {
                    case: case.clone(),
                    component,
                    cpu,
                    gpu,
                    ulp,
                });
            }
        } else if ulp > self.budget {
            // Would have failed on ULP alone but was rescued by the absolute floor.
            self.exempted += 1;
        }
    }

    fn finish(self) -> UlpCheckResult<Case> {
        UlpCheckResult {
            label: self.label,
            total_comparisons: self.total,
            budget: self.budget,
            abs_floor: self.abs_floor,
            max_ulp: self.max_ulp,
            max_raw_ulp: self.max_raw_ulp,
            over_budget_count: self.over_budget,
            exempted_count: self.exempted,
            argmax: self.argmax,
        }
    }
}

/// Fibonacci-sphere direction sampling shared by [`studio_env::build_studio_env_cases`]
/// and [`hdr_env::build_hdr_env_cases`]: an even, deterministic spread of `n` unit
/// directions.
fn fibonacci_sphere(n: usize) -> Vec<Vec3> {
    let golden_angle = std::f32::consts::PI * (3.0 - 5.0_f32.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f32) / ((n - 1).max(1) as f32);
            let radius = y.mul_add(-y, 1.0).max(0.0).sqrt();
            let theta = golden_angle * i as f32;
            Vec3::new(theta.cos() * radius, y, theta.sin() * radius)
        })
        .collect()
}
