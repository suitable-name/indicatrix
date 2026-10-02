//! Unit tests for the spectral transport loop, split by topic: `mode_coupling_tests`
//! (o<->e re-coupling at internal reflections), `exit_splitting_tests` (exit-event
//! spectral splitting's unbiasedness/variance), `nee_tests` (next-event-estimation
//! unbiasedness), and `wave_normal_tests` (the wave-normal-vs-Poynting-direction walk-off
//! through a plane-parallel uniaxial slab).

/// O<->e mode re-coupling at internal reflections.
mod mode_coupling_tests;

/// Empirical unbiasedness check plus a variance-ratio measurement, via
/// `trace_spectral_ray_inner`'s own `enable_exit_splitting` A/B switch (the same
/// pattern `mode_coupling_tests` uses for `enable_internal_mode_coupling`). Two
/// independent sample sets (disjoint seed ranges) so the two-sample z-test is
/// the standard unpaired form.
mod exit_splitting_tests;

/// Next-event-estimation unbiasedness, via `trace_spectral_ray_inner`'s own
/// `enable_nee` A/B switch (the same pattern `mode_coupling_tests`/`exit_splitting_tests`
/// use for their own on/off flags).
mod nee_tests;

/// The surface-glare scale on the first-surface specular reflection.
mod surface_glare_tests;

/// Plane-parallel uniaxial slab, e-mode forced. Drives
/// `compute_bounce_refraction_geometry`/`apply_partial_fresnel_bounce` directly
/// (bypassing the full stochastic bounce loop) at exactly two facets -- the slab's
/// entry and exit faces -- so the resulting `k`/`S` at each step can be inspected.
mod wave_normal_tests;
