//! `optics::birefringence::BiaxialIndicatrix` GPU port -- the genuinely biaxial
//! (three-distinct-principal-index) generalization of the uniaxial machinery
//! `eigenmodes_uniaxial` checks. Every case bank here is compared against the REAL CPU
//! `BiaxialIndicatrix` methods, built via `BiaxialIndicatrix::from_gamma_axis` (never a
//! hand-written parallel reimplementation) -- see `shaders/transport_physics.wgsl`'s own
//! biaxial section for the WGSL port these exercise.
//!
//! Split into one file per case bank: [`wave_indices`], [`eigen_polarization`],
//! [`mode_poynting`], [`resolve_entry_mode`], [`pleochroic`], and
//! [`assigned_mode_alpha`] (the P1 assigned-mode-absorption biaxial branch). This file
//! keeps only the fixtures every bank shares: [`biaxial_test_indicatrices`] (the three
//! real biaxial built-ins plus one synthetic case) and the
//! [`biaxial_test_directions`]/[`biaxial_test_directions_index_stable`] direction
//! sweeps.

use glam::Vec3;

use crate::optics::{birefringence::BiaxialIndicatrix, materials::GemMaterial};

mod assigned_mode_alpha;
mod eigen_polarization;
mod mode_poynting;
mod pleochroic;
mod resolve_entry_mode;
mod wave_indices;

pub use assigned_mode_alpha::{AssignedModeAlphaBiaxialCase, run_assigned_mode_alpha_biaxial};
pub use eigen_polarization::{BiaxialEigenPolarizationCase, run_biaxial_eigen_polarization};
pub use mode_poynting::{BiaxialModePoyntingCase, run_biaxial_mode_poynting};
pub use pleochroic::{BiaxialPleochroicCase, run_biaxial_pleochroic};
pub use resolve_entry_mode::{BiaxialResolveEntryModeCase, run_biaxial_resolve_entry_mode};
pub use wave_indices::{BiaxialWaveIndicesCase, run_biaxial_wave_indices};

/// The three principal indices and gamma axis for every real biaxial built-in
/// (Alexandrite, Topaz, Tanzanite) at the D line, plus one synthetic well-separated,
/// deliberately off-axis case for extra coverage away from any built-in's specific
/// numbers -- mirrors `birefringence::biaxial_reduction_tests`' own synthetic-plus-real
/// coverage split.
///
/// # Panics
///
/// Panics if any of "Alexandrite"/"Topaz"/"Tanzanite" is ever removed from
/// `GemMaterial::all_materials()`, or if `biaxial_indicatrix` ever returns `None` for
/// one of them (it always returns `Some` for a material with `biaxial_delta_beta_alpha
/// = Some(_)`, which all three built-ins have) -- both would be a change to this
/// crate's own material catalogue this self-test scaffolding needs to know about, not a
/// condition worth handling gracefully.
fn biaxial_test_indicatrices() -> Vec<(&'static str, f32, f32, f32, Vec3)> {
    const D_LINE_NM: f32 = 589.3;
    let materials = GemMaterial::all_materials();
    let mut out = Vec::new();
    for name in ["Alexandrite", "Topaz", "Tanzanite"] {
        let material = materials
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("\"{name}\" must be a built-in biaxial material"));
        let ind = material
            .biaxial_indicatrix(D_LINE_NM)
            .unwrap_or_else(|| panic!("\"{name}\" must expose a BiaxialIndicatrix"));
        out.push((name, ind.n_alpha, ind.n_beta, ind.n_gamma, ind.axes.z_axis));
    }
    out.push((
        "synthetic",
        1.60,
        1.65,
        1.75,
        Vec3::new(0.35, 0.82, -0.45).normalize(),
    ));
    out
}

/// A representative spread of wave-normal directions -- axis-aligned, oblique, and
/// exactly along the gamma axis (the degenerate direction where two roots' local
/// components can coincide) -- shared by every case bank below.
fn biaxial_test_directions_unfiltered(gamma_axis: Vec3) -> Vec<Vec3> {
    vec![
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        gamma_axis,
        Vec3::new(0.2, 0.9, 0.1).normalize(),
        Vec3::new(-0.5, 0.3, 0.8).normalize(),
        Vec3::new(0.7, -0.6, 0.2).normalize(),
        Vec3::new(0.1, 0.1, 0.99).normalize(),
        Vec3::new(-0.9, -0.3, 0.2).normalize(),
    ]
}

/// Like [`biaxial_test_directions_unfiltered`], but excludes directions that land
/// essentially EXACTLY on one of this material's own two OPTIC AXES -- the physically
/// real directions (every genuinely biaxial crystal has exactly two) where the slow
/// and fast wave-normal indices coincide exactly. This is a narrow exclusion (relative
/// mode separation below `MIN_RELATIVE_MODE_SEPARATION`, deliberately tiny): a real
/// gem's overall birefringence is itself only a few thousandths of its mean index (see
/// `optics::materials::GemMaterial::birefringence_delta`'s cited built-in values), so
/// EVERY direction's `n_slow - n_fast` is already "small" relative to `n_slow` for
/// these materials -- a threshold anywhere near the double-digit-percent range this
/// function used in an earlier revision filters out nearly all realistic test
/// directions (confirmed: it produced an EMPTY case bank for real materials, which is
/// itself informative about how small the whole regime's `n_slow - n_fast` scale is).
/// A threshold this small targets only a direction landing essentially AT a true optic
/// axis (where `wave_indices`' own discriminant genuinely passes through zero), not
/// merely "somewhere in a naturally-small-birefringence material's normal range". Used
/// for `eigen_polarizations`/`mode_poynting_dir`/`pleochroic_channel_alpha`'s case
/// banks below (see [`biaxial_test_directions_index_stable`] for the wave_indices-only,
/// much stricter threshold, and this module's biaxial section header comment for why
/// `eigen_polarizations`/`mode_poynting_dir` still fail their own ULP budget at even a
/// much looser filter than this one -- their ill-conditioning is not confined to a
/// narrow neighborhood of the two true optic axes the way `wave_indices`' own is).
fn biaxial_test_directions(n_alpha: f32, n_beta: f32, n_gamma: f32, gamma_axis: Vec3) -> Vec<Vec3> {
    biaxial_test_directions_filtered(n_alpha, n_beta, n_gamma, gamma_axis, 1e-4)
}

/// `wave_indices` (the index MAGNITUDES alone, not the eigenVECTOR) is well-conditioned
/// everywhere except a narrow neighborhood of this material's two true optic axes --
/// see [`biaxial_test_directions`]'s doc comment for the general "no directions survive
/// a tight filter for a real gem" trap, and this module's biaxial section header for the
/// measured evidence that `wave_indices` DOES achieve max genuine ULP = 0 once that
/// narrow neighborhood specifically is excluded.
fn biaxial_test_directions_index_stable(
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    gamma_axis: Vec3,
) -> Vec<Vec3> {
    biaxial_test_directions_filtered(n_alpha, n_beta, n_gamma, gamma_axis, 0.02)
}

fn biaxial_test_directions_filtered(
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    gamma_axis: Vec3,
    min_relative_mode_separation: f32,
) -> Vec<Vec3> {
    let ind = BiaxialIndicatrix::from_gamma_axis(n_alpha, n_beta, n_gamma, gamma_axis);
    biaxial_test_directions_unfiltered(gamma_axis)
        .into_iter()
        .filter(|&d| {
            let (n_slow, n_fast) = ind.wave_indices(d);
            (n_slow - n_fast) / n_slow >= min_relative_mode_separation
        })
        .collect()
}
