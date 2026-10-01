//! Fresnel reflection/transmission and Total Internal Reflection.
//!
//! The per-bounce refractive-index/incidence-angle geometry ([`RayMaterialContext`],
//! [`BounceRefractionGeometry`]) and the TIR/partial-reflect/refract bounce appliers
//! built on top of it.
//!
//! ## Uniaxial dispatch
//!
//! Every uniaxial (`is_anisotropic && !is_biaxial`, [`BounceRefractionGeometry::
//! uniaxial_frame`] `Some`) bounce runs the exact closed-form [`uniaxial_fresnel`]
//! solve rather than a scalar "effective index" approximation: [`apply_uniaxial_entry_bounce`]
//! at an isotropic-air -> crystal entry, [`apply_tir_bounce`]'s own branch at a
//! hero-forced (`sin2_t > 1.0`) internal reflection, and [`apply_uniaxial_internal_bounce`]
//! at every other internal event -- the general sub-critical partial reflection and
//! the uniaxial -> isotropic exit transmission, both from one
//! [`uniaxial_fresnel::internal_solve`] call per channel. [`apply_partial_reflect_bounce`]/
//! [`apply_refract_bounce`]/[`apply_refract_channel`]'s scalar-Fresnel machinery is
//! reached only by an isotropic material, a biaxial material, or the degenerate
//! wave-normal-parallel-to-optic-axis limit (see [`apply_partial_fresnel_bounce`] for
//! where each guard sits).
//!
//! ## Exit-event spectral splitting
//!
//! Implemented in [`apply_refract_channel`] (isotropic/biaxial exit, and the degenerate
//! uniaxial wave-normal-parallel-to-optic-axis case sharing its scalar machinery) and
//! in [`apply_uniaxial_internal_transmit_channels`] (the exact uniaxial exit);
//! [`try_split_exit_channel`] is the bounded fan-out primitive both share, and
//! [`ExitSplitCtx`] carries the per-trace state. Gated on [`ExitSplitCtx::enabled`];
//! `false` reproduces the legacy chromatic-termination estimator bit for bit (that
//! estimator is unbiased, verified against a single-wavelength reference).
//!
//! **Interior path unchanged.** Every bounce up to, but not including, the one where
//! the ray leaves the gem back into air is still one shared, hero-driven geometric
//! path. A channel whose own refracted direction at an interior dispersive event (an
//! entry into the gem) diverges from the hero's beyond `DIRECTION_MATCH_COS_TOL` still
//! loses its radiance for the rest of the trace.
//!
//! **The estimator, in three parts (they only work together):**
//!
//! 1. *The exit event.* A still-alive companion `k` whose own Snell direction
//!    diverges from the hero's, at the bounce where the hero-driven path leaves the
//!    gem, gets its own Fresnel transmission at its own index instead of being zeroed
//!    (the same formula the matching-direction case always uses, factored into
//!    [`compute_channel_transmission`] / [`compute_uniaxial_exit_transmission`]), and
//!    [`try_split_exit_channel`] resolves its own continuation with exactly one extra
//!    intersection test: a deterministic environment lookup at `k`'s own wavelength
//!    along `k`'s own direction (a re-entry probe declines rather than recursing).
//!    Staged in [`ExitSplitCtx::split_radiance`] and committed into `radiance` only if
//!    the shared path itself goes on to escape, rescaled on every Russian-roulette
//!    survival exactly like `stokes` (`transport::apply_russian_roulette`) so a split
//!    contribution committed with probability `q` is compensated by the same `1/q`.
//!
//! 2. *`path_pdf` never stops accumulating for a live technique.* `path_pdf[k]` is
//!    technique k's (channel k as hero) density of having produced this geometric
//!    path. A companion that lost its radiance at an interior mismatch is still a
//!    technique that would have produced this path for every channel it remains
//!    compatible with (see 3), so its density keeps accumulating -- the mismatch site
//!    folds in the same transmit factor the matching case does, and the exit event
//!    folds its own transmit factor (`1 - r_unpol_k`, or `t_unpol_k` on the uniaxial
//!    path) into split and matching channels alike. The only genuine zero stays a
//!    genuine zero: a channel that cannot transmit at all where the hero did
//!    (`sin2_t_k > 1`) has density 0 for any path through that exit.
//!
//! 3. *Per-channel MIS families.* `color::spectral_mis_weight`'s balance heuristic
//!    `N * p_hero / sum_j p_j` is Veach's one-sample combination over the N techniques
//!    that could have produced the realized path. With splitting, every live companion
//!    contributes past the exit, and the set of techniques under which channel `c` is
//!    alive is NOT the hero's own alive set (direction tolerance keeps only channels
//!    within some spectral distance of a given hero alive, and that set shifts with
//!    the hero). Normalising `c`'s weight over the hero's set instead of its own makes
//!    weights summed over the heroes that see it exceed 1 (measured +7% Diamond / +12%
//!    Synthetic Moissanite bias against a single-wavelength reference). The fix:
//!    [`ExitSplitCtx::compat`] tracks the pairwise compatibility of all channels
//!    through every interior dispersive event ([`narrow_compat`], reusing the same
//!    `direction_matches` verdict for every pair involving the hero), and
//!    `color::integrate_channels_to_xyz_families` weights channel `c` by
//!    `N * path_pdf[hero] / sum_{j in compat[c]} path_pdf[j]` -- which sums to exactly
//!    1 over `c`'s own family, making the combined estimator unbiased. The exit event
//!    never narrows a family: every technique in `c`'s family produces `c`'s
//!    (deterministic) exit path.
//!
//! **Root cause of the bias.** Leaving `path_pdf[k]` at its prefix value for a split
//! channel, or at 0, or applying the exit factor some other way -- none of it fixes
//! the family-normalization problem in point 3; each choice only relocates the bias.
//!
//! **Verification.** `transport::exit_splitting_tests` compares splitting on vs. off
//! on paired draws (the same seed and hero for both arms, so the shared path noise
//! cancels) across several materials and a bounce-budget sweep, and asserts the means
//! stay within one percent plus noise and the Y variance does not rise beyond its own
//! estimate's noise: it drops by 30-80% for Zircon and Synthetic Moissanite, while
//! Rutile's heavy-tailed radiance under the ring lights leaves its 10% drop within one
//! standard error. The one percent is the shared-geometry approximation both arms rest
//! on: a companion rides the hero's path while its own refracted direction is within
//! `DIRECTION_MATCH_COS_TOL` of the hero's, and splitting leans on that harder, so the
//! arms differ by a small amount that scales with the tolerance (Rutile +0.4% / -0.9%
//! in X / Z at the current tolerance, +0.1% at 1e-7, +2.4% / -2.3% at 1e-5; Diamond
//! none). That establishes the split estimator agrees with the unsplit one to that
//! level, and that the two stay in step as the bounce budget shrinks. Both arms are the
//! same tracer,
//! so a bias they share would not show up: no independent ground truth (for example a
//! plain per-wavelength Monte Carlo with every companion terminated before the first
//! bounce) is compared against. The GPU mirror in
//! `renderer/shaders/spectral_transport.wgsl` implements the same three points.
//!
//! # Module layout
//!
//! Split from a single `refraction.rs` by responsibility: [`context`] holds the
//! shared per-ray/per-sample/per-bounce state types, [`geometry`] builds
//! [`BounceRefractionGeometry`] each bounce, [`exit_split`] and [`wavelength_cache`]
//! are small focused helpers, [`tir`] is the Total Internal Reflection bounce,
//! [`reflect_refract`] is the scalar-Fresnel reflect/refract machinery,
//! [`uniaxial_entry`]/[`uniaxial_internal`] are the closed-form uniaxial bounces, and
//! [`dispatch`] is the top-level per-bounce entry point
//! ([`apply_partial_fresnel_bounce`]). Every path reachable as `refraction::X` before
//! the split is still reachable at exactly that path via the re-exports below.

pub mod context;
mod dispatch;
mod exit_split;
pub mod geometry;
mod reflect_refract;
#[cfg(test)]
mod tests;
pub mod tir;
mod uniaxial_entry;
mod uniaxial_internal;
mod wavelength_cache;

/// Smallest cosine between two channels' refracted directions for them to count as the
/// same geometric path (`1 - 1e-6`).
///
/// A channel whose own direction at a dispersive event falls below it against the
/// hero's has diverged from the shared hero-driven path. The GPU megakernel carries the
/// identical constant in `shaders/transport_bounce/01_header_and_constants.wgsl`.
pub const DIRECTION_MATCH_COS_TOL: f32 = 1.0 - 1e-6;

/// Lower clamp on the per-channel unpolarised reflectance or transmittance.
///
/// It scales `path_pdf` (never divides `stokes` directly), keeping the density strictly
/// positive for every live technique.
pub const R_UNPOL_PDF_MIN: f32 = 1e-4;

/// Upper clamp paired with [`R_UNPOL_PDF_MIN`]: `1 - 1e-4`.
pub const R_UNPOL_PDF_MAX: f32 = 1.0 - 1e-4;

/// Lower clamp on the reflect-vs-transmit selection probability.
///
/// `[0.02, 0.98]` caps the `1/p` and `1/(1-p)` weights at 50x where the unclamped
/// reflectance approaches 0 or 1 at grazing incidence, without biasing the estimator
/// (the contribution is weighted by the true `R`/`T` over this same `p`).
pub const R_UNPOL_SELECT_MIN: f32 = 0.02;

/// Upper clamp paired with [`R_UNPOL_SELECT_MIN`].
pub const R_UNPOL_SELECT_MAX: f32 = 0.98;

pub(in crate::optics::raytracer) use context::{
    BounceContext, BounceRay, BounceState, ExitEvent, ExitSplitCtx, PathModeState, RngDraw,
};
pub(crate) use context::{RayMaterialContext, RayWavelengthCache};
pub(in crate::optics::raytracer) use dispatch::apply_partial_fresnel_bounce;
pub(in crate::optics::raytracer) use geometry::compute_bounce_refraction_geometry;
#[cfg(feature = "gpu")]
pub(crate) use geometry::per_channel_uniaxial_indices;
pub(crate) use geometry::{BounceRefractionGeometry, poynting_dir_for_mode, theta_c_for_bounce};
pub(in crate::optics::raytracer) use tir::apply_tir_bounce;
pub(crate) use tir::tir_phase_delta;
pub(in crate::optics::raytracer) use uniaxial_entry::entry_eigenmode_selection;
pub(in crate::optics::raytracer) use wavelength_cache::build_ray_wavelength_cache;
