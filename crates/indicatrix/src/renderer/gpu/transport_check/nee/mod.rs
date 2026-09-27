//! Next-event estimation (NEE), balance-heuristic MIS, and 1D/2D distribution
//! importance sampling ULP equivalence checks.
//!
//! Compares `transport_functions.wgsl`'s GPU implementations against the real CPU
//! functions in `optics::raytracer::scattering` and `renderer::env_map`. Split into one
//! file per case bank: [`balance_heuristic`], [`dist1d`], [`dist2d_sample`],
//! [`dist2d_pdf`], [`frosted_exterior`], and [`hg_scatter`]; [`synthetic_test_map`] is
//! the shared 8x4 fixture every distribution/NEE check but the balance heuristic renders
//! against.

use crate::renderer::env_map::EnvironmentMap;

mod balance_heuristic;
mod dist1d;
mod dist2d_pdf;
mod dist2d_sample;
mod frosted_exterior;
mod hg_scatter;

pub use balance_heuristic::{BalanceHeuristicCase, run_balance_heuristic};
pub use dist1d::{Dist1dFindBucketCase, run_dist1d_find_bucket};
pub use dist2d_pdf::{Dist2dPdfCase, run_dist2d_pdf};
pub use dist2d_sample::{Dist2dSampleCase, run_dist2d_sample};
pub use frosted_exterior::{NeeFrostedExteriorCase, run_nee_frosted_exterior};
pub use hg_scatter::{NeeHgScatterCase, run_nee_hg_scatter};

/// A small, deterministic 8x4 synthetic environment map shared by every
/// distribution/NEE check below except the balance heuristic (which needs no
/// environment at all).
fn synthetic_test_map() -> EnvironmentMap {
    let width = 8;
    let height = 4;
    let mut pixels = Vec::with_capacity(width * height);
    for y in 0..height {
        for x in 0..width {
            let r = (((x * 3 + y * 5) % 7) as f32).mul_add(0.4, 0.1);
            let g = (((x * 2 + y * 3) % 5) as f32).mul_add(0.3, 0.1);
            let b = (((x * 5 + y * 2) % 6) as f32).mul_add(0.5, 0.1);
            pixels.push([r, g, b]);
        }
    }
    EnvironmentMap::from_rgb(width, height, pixels).expect("valid synthetic env map")
}
