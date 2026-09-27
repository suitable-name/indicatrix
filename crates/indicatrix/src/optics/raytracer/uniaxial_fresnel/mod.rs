//! Exact anisotropic-interface Fresnel solution for uniaxial crystals.
//!
//! Replaces the "effective index" scalar-Fresnel-per-mode approximation that used to
//! live at every `is_anisotropic && !is_biaxial` interface in [`super::refraction`].
//!
//! # Physics and derivation
//!
//! At an isotropic/uniaxial interface, an incident s- or p-polarized wave does NOT
//! reflect/transmit as if the crystal had one scalar "effective index" -- it couples
//! into the ordinary (o) AND extraordinary (e) eigenmodes simultaneously, and (except
//! when the optic axis lies in the plane of incidence) the reflected wave in the
//! isotropic medium is itself a genuine s/p mixture. This module implements the
//! closed-form solution for that boundary-value problem: Lekner, J. Phys.: Condens.
//! Matter 3 (1991) 6121-6133 ("Reflection and refraction by uniaxial crystals"),
//! cross-checked against Yeh, J. Opt. Soc. Am. 69 (1979) 742 and Yeh, "Optical Waves in
//! Layered Media" (1988) ch. 9 -- both derive the same boundary-matched amplitudes for
//! a uniaxial half-space; this module solves the identical 4-equation boundary system
//! Lekner's `r_ss, r_sp, r_ps, r_pp, t_so, t_se, t_po, t_pe` name, via the SAME
//! closed-form uniaxial dispersion relation (not a general numerical Berreman 4x4
//! eigenvalue solve -- the ordinary root is a plain square root, the extraordinary root
//! solves an explicit quadratic, both closed-form; see [`uniaxial_q_roots`]).
//!
//! ## Units and frame convention
//!
//! Wavevectors are measured in units of `k0 = omega/c`, so a wavevector's magnitude
//! equals the local refractive index directly (`|k| = n`), and the universal relation
//! `H = k x E` holds in ANY medium (isotropic or anisotropic alike) given Maxwell's
//! equations `curl E = i*omega*mu0*H`, `curl H = -i*omega*eps0*eps*E` with `mu0 = eps0 =
//! c = 1` -- see each function's own doc comment. All geometry is built directly from
//! the SAME world-space `k_hat` (wave normal) / `normal` (facet normal, oriented to
//! face the incident ray, i.e. `(-k_hat).dot(normal) == cos_i >= 0` -- exactly
//! `compute_bounce_refraction_geometry`'s own convention) / `c_axis` vectors the rest of
//! this crate already uses, projected onto the local orthonormal frame `(that, s_axis,
//! zhat)`:
//!   - `zhat = -normal` (forward propagation direction, incidence medium -> far medium)
//!   - `that` = the in-plane (tangential) component of `k_hat`, i.e. the propagation
//!     azimuth
//!   - `s_axis = k_hat.cross(normal).normalize()` -- IDENTICAL to
//!     `rotate_stokes_to_plane_of_incidence`'s `current_plane_normal` (see
//!     `absorption::rotate_stokes_to_plane_of_incidence`'s doc comment), so a caller's
//!     already-`current_plane_normal`-referenced Stokes vector needs no extra rotation
//!     to feed into this module's Jones/Mueller machinery, and this module's own output
//!     Mueller matrices apply directly in that same frame.
//!
//! `(alpha, beta, gamma) = (c_axis.dot(that), c_axis.dot(s_axis), c_axis.dot(zhat))` are
//! the optic-axis direction cosines Lekner's own notation uses.
//!
//! ## Validation
//!
//! Before transcribing this into Rust, the exact algebra below (q-root formula,
//! eigenvector construction, boundary-matching linear solve, Poynting-flux energy
//! accounting) was independently validated in Python (native `complex`, no numeric
//! libraries) against:
//!   - a brute-force from-scratch boundary-value solve (own sanity check the isotropic
//!     limit reproduces this crate's exact `r_s`/`r_p` sign convention, see this
//!     module tree's own tests),
//!   - R + T == 1 (Poynting-projected) to machine precision (< 3e-15) across 3 uniaxial
//!     materials x multiple optic-axis orientations (axis-aligned and random) x 6-9
//!     incidence angles x both polarizations, for BOTH the entry and the
//!     internal-reflection/exit interface, including 204 genuine TIR (complex,
//!     evanescent) cases,
//!   - continuity: `Delta n -> 0` reduces to this crate's own isotropic Fresnel `r_s`/
//!     `r_p` to within `O(Delta n)`,
//!   - the normal-incidence, optic-axis-in-surface-plane special case.
//!
//! See `entry_energy_conservation_matches_isotropic_at_zero_birefringence` and the
//! other tests in this module tree for the Rust-side re-derivation of those same checks.
//!
//! ## Wiring
//!
//! - **Entry** (`entry_solve_pair`/`entry_incidence_frame`/
//!   `entry_solve_pair_with_incidence`): wired into
//!   `refraction::apply_uniaxial_entry_bounce`.
//! - **Internal reflection and exit transmission** (`internal_solve`): wired into both
//!   `refraction::apply_tir_bounce`'s hero-forced-TIR branch and the general
//!   (sub-critical, non-forced) internal-reflection/exit-transmission path via
//!   `refraction::apply_uniaxial_internal_bounce` --
//!   `apply_partial_reflect_bounce`/`apply_refract_channel`'s scalar-Fresnel machinery
//!   is reached only by an isotropic or biaxial material, or the degenerate
//!   wave-normal-parallel-to-optic-axis limit (see `apply_partial_fresnel_bounce`'s own
//!   doc comment).
//! - **o<->e internal re-coupling** (`transport::apply_internal_mode_coupling`): uses
//!   the exact Poynting-weighted `R_o/(R_o+R_e)` split from the same `internal_solve`
//!   call already run for the reflection event's own energy accounting, rather than a
//!   polarization-projection heuristic (still the fallback for a biaxial material,
//!   which has no uniaxial closed form to draw an exact split from).
//! - **Performance**: `EntryIncidenceFrame`/`entry_incidence_frame` share the
//!   isotropic incidence-side boundary-matching data (provably independent of
//!   `n_o(lambda)`/`n_e(lambda)`) across all 8 hero channels of one bounce instead of
//!   rebuilding it per channel; [`uniaxial_q_roots`] short-circuits to the isotropic
//!   limit exactly when `n_o == n_e`; `apply_uniaxial_internal_bounce` reuses its own
//!   hero-channel `internal_solve` result at `k == hero_idx` instead of solving the
//!   identical boundary system twice.
//! - **WGSL mirror**: `shaders/transport_physics.wgsl`'s own full uniaxial Fresnel
//!   (Lekner 1991) section is a direct, op-for-op port of this entire module tree
//!   (`Cplx`/`CVec3`/`UniaxialFrameW`/`uniaxial_q_roots`/mode fields/`solve4`-family/
//!   `entry_solve_pair`/`internal_solve`/`jones_to_mueller`/`mode_power`/
//!   `azimuth2_in_frame`), wired into `spectral_transport.wgsl`'s megakernel bounce
//!   dispatch (entry, hero-forced TIR, and the general internal/exit path) mirroring
//!   this module tree's own CPU wiring bounce-for-bounce. Verified via
//!   `renderer::gpu::transport_check::p2_uniaxial_fresnel`'s kernel-level ULP checks (0
//!   genuine ULP divergence against this module's own CPU functions) and a Tier 3
//!   statistical image comparison on Zircon/Tourmaline/Quartz/Rutile, all passing on
//!   real AMD Radeon (Vulkan) hardware via `examples/gpu_equivalence_harness`.
//! - **GPU-parity fix**: `Cplx::sqrt_forward_branch` special-cases a pure
//!   negative-real input directly (bypassing an `atan2`/`cos`/`sin` round-trip whose
//!   near-zero-magnitude branch-cut decision is otherwise rounding noise that CPU and
//!   GPU transcendental implementations can resolve to opposite signs) -- found by,
//!   and fixed to pass, the `internal_solve` ULP check above; see that function's own
//!   doc comment.
//!
//! Split from a single `uniaxial_fresnel.rs` into this module tree by seam:
//! [`complex`] holds the `Cplx`/`CVec3` arithmetic, [`frame`] holds the incidence frame
//! and the closed-form mode fields/Poynting flux built from it, [`linalg`] holds the
//! shared 4x4 boundary-matching solve, [`entry`] and [`internal`] hold the two
//! interfaces' own boundary-value solutions, and [`mueller`] holds the Stokes/Mueller
//! integration. Every path reachable as `uniaxial_fresnel::X` before the split stays
//! reachable at exactly that path via the re-exports below.

// `pub`, not private, for every submodule holding a `pub(crate)` item: a `pub(crate)`
// item inside a private module is only reachable via this file's re-export, which
// clippy's `redundant_pub_crate` (nursery) then flags as suspicious -- see
// `raytracer::mod`'s own identical comment for this exact pattern. Making the
// submodule `pub` resolves it without widening any item's own effective visibility,
// still capped at its declared visibility. `linalg` holds only `pub(super)` items, so
// it stays private.
pub mod complex;
pub mod entry;
pub mod frame;
pub mod internal;
mod linalg;
pub mod mueller;
#[cfg(test)]
mod tests;

// complex.rs
// Used only by the GPU twin checks (`renderer::gpu::transport_check`) and tests.
#[cfg(feature = "gpu")]
pub(crate) use complex::Cplx;

// frame.rs
pub(crate) use frame::UniaxialFrame;

// entry.rs
#[cfg(feature = "gpu")]
pub(crate) use entry::EntryPolarizationSolution;
#[cfg(any(test, feature = "gpu"))]
pub(crate) use entry::entry_solve_pair;
pub(crate) use entry::{
    EntryIncidenceFrame, entry_incidence_frame, entry_solve_pair_with_incidence,
};

// internal.rs
pub(crate) use internal::{InternalPolarizationSolution, internal_solve};

// mueller.rs
pub(crate) use mueller::{azimuth2_in_frame, jones_to_mueller, mode_power};
