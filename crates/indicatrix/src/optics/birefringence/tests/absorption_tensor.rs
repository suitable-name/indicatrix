//! [`AbsorptionTensor3`]/pleochroic-absorption tests: azimuth dependence,
//! polarization blending, and the o-ray/e-ray axis convention against real
//! material data.

use crate::optics::{
    birefringence::{
        AbsorptionTensor3, BirefringenceParams, effective_pleochroic_alpha,
        pleochroic_channel_alpha,
    },
    polarization::{StokesVector, electric_field_direction},
};
use glam::Vec3;

#[cfg(test)]
mod absorption_tensor_tests {
    use super::*;

    /// An isotropic tensor (`alpha_o` == `alpha_e`) must return the same coefficient
    /// regardless of the electric-field direction.
    #[test]
    fn isotropic_tensor_is_azimuth_independent() {
        let tensor = AbsorptionTensor3::uniaxial(1.5, 1.5, Vec3::Y);
        let dirs = [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            Vec3::new(1.0, 1.0, 1.0).normalize(),
            Vec3::new(0.3, -0.7, 0.2).normalize(),
        ];
        for d in dirs {
            let a = tensor.quadratic_form(d);
            assert!(
                (a - 1.5).abs() < 1e-5,
                "isotropic tensor must be azimuth-independent, got {a} for {d:?}"
            );
        }
    }

    /// A uniaxial tensor evaluated exactly along the c-axis must give `alpha_e`, and
    /// exactly perpendicular to it must give `alpha_o`.
    #[test]
    fn uniaxial_tensor_matches_principal_coefficients_on_axis() {
        let c_axis = Vec3::Y;
        let tensor = AbsorptionTensor3::uniaxial(2.0, 5.0, c_axis);
        assert!((tensor.quadratic_form(c_axis) - 5.0).abs() < 1e-5);
        assert!((tensor.quadratic_form(Vec3::X) - 2.0).abs() < 1e-5);
        assert!((tensor.quadratic_form(Vec3::Z) - 2.0).abs() < 1e-5);
    }

    /// The decisive test: send linearly polarized light through a strongly
    /// dichroic uniaxial material along a FIXED propagation direction and rotate the
    /// polarization azimuth through 180 degrees. Absorption must vary smoothly and
    /// return to its starting value -- a clean sinusoid in `2*psi` -- and must clear a
    /// wide range (not be constant, which is what the old propagation-angle-only
    /// blend would give for a fixed direction).
    #[test]
    fn rotating_polarization_azimuth_traces_a_clean_sinusoid() {
        let c_axis = Vec3::new(0.2, 0.6, 0.77).normalize(); // deliberately off-axis
        let propagation_dir = Vec3::new(0.9, 0.1, 0.3).normalize(); // fixed direction, oblique to c_axis
        let alpha_o = 0.5f32;
        let alpha_e = 6.0f32; // strongly dichroic
        let tensor = AbsorptionTensor3::uniaxial(alpha_o, alpha_e, c_axis);

        // Build a fixed (s_axis, p_axis) frame perpendicular to propagation_dir, as
        // trace_spectral_ray would from a plane-of-incidence normal.
        let s_axis = propagation_dir.cross(Vec3::Y).normalize();

        let samples = 37;
        let mut values = Vec::with_capacity(samples);
        for i in 0..samples {
            let psi = (i as f32) / (samples as f32 - 1.0) * std::f32::consts::PI; // sweep 0..=180deg
            let stokes = StokesVector::new(1.0, (2.0 * psi).cos(), (2.0 * psi).sin(), 0.0);
            let e_hat = electric_field_direction(&stokes, s_axis, propagation_dir);
            let eigen_a = BirefringenceParams::ordinary_eigen_polarization(propagation_dir, c_axis);
            let eigen_b =
                BirefringenceParams::extraordinary_eigen_polarization(propagation_dir, c_axis);
            let alpha = effective_pleochroic_alpha(&tensor, e_hat, eigen_a, eigen_b, 1.0);
            values.push(alpha);
        }

        // 1. Must return (close) to its starting value after a full 180 degree sweep.
        let start = values[0];
        let end = values[samples - 1];
        assert!(
            (start - end).abs() < 1e-3,
            "azimuth sweep must return to its starting value (start={start}, end={end})"
        );

        // 2. Must actually vary -- not the old azimuth-blind behaviour.
        let min = values.iter().copied().fold(f32::INFINITY, f32::min);
        let max = values.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max - min > 1.0,
            "absorption must vary substantially as azimuth rotates for a strongly dichroic material (min={min}, max={max})"
        );

        // 3. Every value must stay within the [alpha_o, alpha_e] envelope implied by
        // the quadratic form (a physical sanity bound).
        for &a in &values {
            assert!(
                a >= alpha_o - 1e-3 && a <= alpha_e + 1e-3,
                "alpha={a} out of the physical [alpha_o, alpha_e]=[{alpha_o},{alpha_e}] envelope"
            );
        }

        // 4. Smoothness: no adjacent-sample jump should be large relative to the total
        // span (catches a sign flip / discontinuity bug, as opposed to a genuine smooth
        // sinusoid).
        let span = max - min;
        for w in values.windows(2) {
            assert!(
                (w[1] - w[0]).abs() < 0.35 * span,
                "adjacent samples should vary smoothly, not jump (got delta={})",
                (w[1] - w[0]).abs()
            );
        }
    }

    /// Fully unpolarized light must be azimuth-independent (it has no well-defined
    /// azimuth) and must sit near the two-eigenmode average, not at either pure
    /// extreme -- the "graceful degradation" requirement.
    #[test]
    fn unpolarized_light_degrades_gracefully_and_is_azimuth_independent() {
        let c_axis = Vec3::Y;
        let propagation_dir = Vec3::new(0.6, 0.3, 0.74).normalize();
        let alpha_o = 1.0f32;
        let alpha_e = 4.0f32;
        let tensor = AbsorptionTensor3::uniaxial(alpha_o, alpha_e, c_axis);
        let eigen_a = BirefringenceParams::ordinary_eigen_polarization(propagation_dir, c_axis);
        let eigen_b =
            BirefringenceParams::extraordinary_eigen_polarization(propagation_dir, c_axis);

        let unpolarized = StokesVector::unpolarized(1.0);
        let s_axis = propagation_dir.cross(Vec3::Z).normalize();
        let e_hat = electric_field_direction(&unpolarized, s_axis, propagation_dir);
        let alpha_unpolarized = effective_pleochroic_alpha(
            &tensor,
            e_hat,
            eigen_a,
            eigen_b,
            unpolarized.degree_of_polarization(),
        );

        // Must sit strictly between the two pure principal coefficients (a genuine
        // blend, not collapsed to either extreme).
        assert!(
            alpha_unpolarized > alpha_o && alpha_unpolarized < alpha_e,
            "unpolarized absorption {alpha_unpolarized} should sit strictly between alpha_o={alpha_o} and alpha_e={alpha_e}"
        );

        // Must not depend on which s_axis/frame was used to build e_hat -- with p=0
        // the polarized term is weighted out entirely.
        let s_axis_2 = propagation_dir.cross(Vec3::X).normalize_or_zero();
        let e_hat_2 = electric_field_direction(&unpolarized, s_axis_2, propagation_dir);
        let alpha_unpolarized_2 = effective_pleochroic_alpha(
            &tensor,
            e_hat_2,
            eigen_a,
            eigen_b,
            unpolarized.degree_of_polarization(),
        );
        assert!(
            (alpha_unpolarized - alpha_unpolarized_2).abs() < 1e-5,
            "unpolarized result must not depend on the reference frame used to build e_hat"
        );
    }

    /// Pleochroism data pass: pins the `o_ray`/`e_ray` <-> `AbsorptionTensor3` naming
    /// convention using REAL Ruby material band data (every other test in this module
    /// uses synthetic `alpha_o`/`alpha_e` scalars) -- get this convention backwards and
    /// every populated built-in's dichroism would render mirrored (e.g. Sapphire's
    /// ~70nm band-centre shift would show up in exactly the wrong viewing direction).
    ///
    /// `quadratic_form` evaluated exactly perpendicular to `c_axis` (E-perp-c) must
    /// equal the o-ray band sum, and evaluated exactly parallel to `c_axis`
    /// (E-parallel-c) must equal the e-ray band sum, at Ruby's own real band centre
    /// 556nm -- the o-ray yellow-green Cr3+ band peak (see the Ruby entry's comment in
    /// `materials::GemMaterial::all_materials`), chosen specifically because the two
    /// rays differ substantially there (o-ray peak 2.5 vs. the e-ray's off-peak
    /// contribution), making this discriminating rather than a coincidental match.
    #[test]
    fn convention_pin_o_ray_is_perpendicular_e_ray_is_parallel_to_c_axis() {
        use crate::optics::{materials::GemMaterial, raytracer::spectral_absorption};

        const LAMBDA_NM: f32 = 556.0;
        let ruby = GemMaterial::by_name("Ruby").expect("Ruby must be a built-in material");
        let alpha_o = spectral_absorption(&ruby.absorption.o_ray, LAMBDA_NM);
        let alpha_e = spectral_absorption(&ruby.absorption.e_ray, LAMBDA_NM);
        assert!(
            alpha_o > 0.0 && alpha_e > 0.0,
            "test premise: both rays must contribute at 556nm (o={alpha_o}, e={alpha_e})"
        );
        assert!(
            (alpha_o - alpha_e).abs() > 0.1,
            "test premise: o-ray and e-ray must actually differ at 556nm for this test to be \
             discriminating (o={alpha_o}, e={alpha_e})"
        );

        let tensor = AbsorptionTensor3::uniaxial(alpha_o, alpha_e, Vec3::Y);
        let a_perp = tensor.quadratic_form(Vec3::X);
        let a_parallel = tensor.quadratic_form(Vec3::Y);

        assert!(
            (a_perp - alpha_o).abs() < 1e-4,
            "E-perp-c quadratic_form ({a_perp}) must equal the o-ray band sum ({alpha_o}) -- \
             o_ray must map to the PERPENDICULAR-to-c principal coefficient"
        );
        assert!(
            (a_parallel - alpha_e).abs() < 1e-4,
            "E-parallel-c quadratic_form ({a_parallel}) must equal the e-ray band sum \
             ({alpha_e}) -- e_ray must map to the PARALLEL-to-c principal coefficient"
        );
    }

    /// Polarized probe, extending the synthetic-coefficient tests above (which used
    /// hand-picked `alpha_o`/`alpha_e` scalars) to run against REAL material data end to
    /// end through `pleochroic_channel_alpha` -- the exact function
    /// `raytracer::apply_absorption` calls per spectral channel per bounce. For a
    /// propagation direction exactly perpendicular to `c_axis`, feeds a FULLY polarized
    /// Stokes vector (`degree_of_polarization` == 1) first with E exactly parallel to
    /// `c_axis`, then exactly perpendicular to it, and checks the returned effective
    /// coefficient equals the respective band sum exactly -- at full polarization the
    /// unpolarized eigenmode-average term in `effective_pleochroic_alpha` is weighted
    /// out entirely, so this isolates the quadratic-form term alone, run through the
    /// SAME `electric_field_direction`/azimuth machinery a real bounce uses (unlike the
    /// convention-pin test above, which calls `quadratic_form` directly).
    #[test]
    fn polarized_probe_matches_band_sums_for_real_tourmaline_data() {
        use crate::optics::{materials::GemMaterial, raytracer::spectral_absorption};

        const LAMBDA_NM: f32 = 430.0; // Fe2+-Ti4+ IVCT band, strongly dichroic at this wavelength
        let tourmaline =
            GemMaterial::by_name("Tourmaline").expect("Tourmaline must be a built-in material");
        let c_axis = tourmaline.c_axis; // Vec3::X, the documented cut-orientation override
        let alpha_o = spectral_absorption(&tourmaline.absorption.o_ray, LAMBDA_NM);
        let alpha_e = spectral_absorption(&tourmaline.absorption.e_ray, LAMBDA_NM);
        assert!(
            alpha_o > alpha_e * 2.0,
            "test premise: Tourmaline's o-ray must be substantially stronger than its e-ray at \
             430nm (o={alpha_o}, e={alpha_e})"
        );

        // Propagation exactly perpendicular to c_axis, with a stable orthonormal frame
        // built the same way `apply_absorption` builds its eigenmodes.
        let propagation_dir = if c_axis.x.abs() > 0.9 {
            Vec3::Y
        } else {
            Vec3::X
        };
        assert!(
            propagation_dir.dot(c_axis).abs() < 1e-6,
            "test premise: propagation_dir must be exactly perpendicular to c_axis"
        );
        let eigen_a = BirefringenceParams::ordinary_eigen_polarization(propagation_dir, c_axis);
        let eigen_b =
            BirefringenceParams::extraordinary_eigen_polarization(propagation_dir, c_axis);
        let s_axis = propagation_dir.cross(c_axis).normalize();

        // E exactly parallel to c_axis: azimuth chosen so `electric_field_direction`
        // recovers `c_axis` itself (psi = 90 deg from s_axis, i.e. Q=-1, U=0).
        let stokes_parallel = StokesVector::new(1.0, -1.0, 0.0, 0.0);
        let e_parallel = electric_field_direction(&stokes_parallel, s_axis, propagation_dir);
        assert!(
            (e_parallel.dot(c_axis).abs() - 1.0).abs() < 1e-4,
            "test premise: this Stokes vector's field direction must be parallel to c_axis, \
             got {e_parallel:?} vs c_axis {c_axis:?}"
        );
        let alpha_measured_parallel = pleochroic_channel_alpha(
            alpha_o,
            alpha_e,
            None,
            c_axis,
            s_axis,
            propagation_dir,
            eigen_a,
            eigen_b,
            &stokes_parallel,
        );
        assert!(
            (alpha_measured_parallel - alpha_e).abs() < 1e-4,
            "E-parallel-c fully-polarized probe ({alpha_measured_parallel}) must equal the \
             e-ray band sum ({alpha_e})"
        );

        // E exactly perpendicular to c_axis (along s_axis itself: psi = 0, Q=+1, U=0).
        let stokes_perp = StokesVector::new(1.0, 1.0, 0.0, 0.0);
        let e_perp = electric_field_direction(&stokes_perp, s_axis, propagation_dir);
        assert!(
            e_perp.dot(c_axis).abs() < 1e-4,
            "test premise: this Stokes vector's field direction must be perpendicular to \
             c_axis, got {e_perp:?} vs c_axis {c_axis:?}"
        );
        let alpha_measured_perp = pleochroic_channel_alpha(
            alpha_o,
            alpha_e,
            None,
            c_axis,
            s_axis,
            propagation_dir,
            eigen_a,
            eigen_b,
            &stokes_perp,
        );
        assert!(
            (alpha_measured_perp - alpha_o).abs() < 1e-4,
            "E-perp-c fully-polarized probe ({alpha_measured_perp}) must equal the o-ray band \
             sum ({alpha_o})"
        );
    }
}
