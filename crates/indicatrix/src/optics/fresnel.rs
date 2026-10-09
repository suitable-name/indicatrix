//! Unpolarised Fresnel reflectance of a dielectric interface in `f64`
//! (the `zoning` feature).
//!
//! The spectral tracer has no reusable public Fresnel function: its reflectance is an
//! inline `f32` expression inside the bounce geometry (`raytracer::refraction::dispatch::
//! compute_entry_reflect_probability`, clamped to a sampling range) and a private helper in
//! `color::metrics::ray_trace`. Neither can be called with plain `(cos_i, n_from, n_to)`, so
//! this is a standalone `f64` implementation of the same textbook formula (the mean of the s
//! and p power reflectances). The tests compare it with the tracer's `f32` expression
//! transcribed verbatim.
//!
//! Used by the rig forward renderer (lane F1) together with
//! `indicatrix_cut_core::rough_plan::locate::{refract, reflect}`.

/// Unpolarised Fresnel power reflectance `R` of a smooth dielectric interface.
///
/// - `cos_i`: cosine of the angle between the incident ray and the surface normal, measured
///   in the incident medium (clamped to `[0, 1]`; the sign of the normal does not matter).
/// - `n_from`: refractive index of the medium the ray comes from.
/// - `n_to`: refractive index of the medium it enters.
///
/// Returns `R` in `[0, 1]`; the transmittance is `1 - R`. Total internal reflection
/// (`n_from > n_to` beyond the critical angle) returns exactly `1.0`, as does grazing
/// incidence (`cos_i == 0`). Pure and deterministic.
#[must_use]
pub fn fresnel_dielectric(cos_i: f64, n_from: f64, n_to: f64) -> f64 {
    let ci = cos_i.clamp(0.0, 1.0);
    let eta = n_from / n_to;
    let sin2_t = eta * eta * ci.mul_add(-ci, 1.0);
    if sin2_t >= 1.0 {
        // Total internal reflection.
        return 1.0;
    }
    let ct = (1.0 - sin2_t).max(0.0).sqrt();
    let denom_s = n_from.mul_add(ci, n_to * ct);
    let denom_p = n_to.mul_add(ci, n_from * ct);
    if denom_s.abs() < f64::MIN_POSITIVE || denom_p.abs() < f64::MIN_POSITIVE {
        // Only reachable at grazing incidence with a vanishing index; reflectance tends to 1.
        return 1.0;
    }
    let rs = n_from.mul_add(ci, -(n_to * ct)) / denom_s;
    let rp = n_to.mul_add(ci, -(n_from * ct)) / denom_p;
    (0.5 * rp.mul_add(rp, rs * rs)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::fresnel_dielectric;

    /// The tracer's own expression (`compute_entry_reflect_probability` in
    /// `optics/raytracer/refraction/dispatch.rs`, without its sampling clamp), in `f32`.
    fn tracer_reflectance_f32(n1: f32, n2: f32, cos_i: f32) -> f32 {
        let eta = n1 / n2;
        let sin2_t = eta * eta * cos_i.mul_add(-cos_i, 1.0);
        let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
        let r_s = f32::mul_add(n2, -cos_t, n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
        let r_p = f32::mul_add(n1, -cos_t, n2 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);
        0.5 * r_p.mul_add(r_p, r_s * r_s)
    }

    #[test]
    fn normal_incidence_is_the_textbook_value() {
        for (n1, n2) in [
            (1.0, 1.5),
            (1.5, 1.0),
            (1.0, 2.417),
            (1.544, 1.333),
            (1.7, 1.7),
        ] {
            let expect = ((n1 - n2) / (n1 + n2)) * ((n1 - n2) / (n1 + n2));
            let got = fresnel_dielectric(1.0, n1, n2);
            assert!(
                (got - expect).abs() < 1e-12,
                "n {n1} -> {n2}: got {got}, expected {expect}"
            );
        }
    }

    #[test]
    fn matching_indices_never_reflect() {
        for k in 0..=20 {
            let ci = f64::from(k) / 20.0;
            assert!(fresnel_dielectric(ci, 1.62, 1.62).abs() < 1e-12);
        }
    }

    #[test]
    fn total_internal_reflection_returns_one() {
        // Diamond to air: critical angle asin(1/2.417) = 24.4 degrees; 45 degrees is beyond it.
        let ci = 45.0_f64.to_radians().cos();
        assert!((fresnel_dielectric(ci, 2.417, 1.0) - 1.0).abs() < f64::EPSILON);
        // Just inside the critical angle it is below one but large.
        let ci_in = 20.0_f64.to_radians().cos();
        let r = fresnel_dielectric(ci_in, 2.417, 1.0);
        assert!(r < 1.0 && r > 0.1, "{r}");
        // Grazing incidence from the dense side and from the rare side.
        assert!((fresnel_dielectric(0.0, 1.0, 1.5) - 1.0).abs() < 1e-12);
        assert!((fresnel_dielectric(0.0, 1.5, 1.0) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn reciprocity_between_the_two_directions() {
        // R(n1 -> n2 at theta_i) == R(n2 -> n1 at theta_t) away from total reflection.
        let cases = [
            (1.0, 1.5, 0.3),
            (1.0, 1.5, 0.9),
            (1.33, 1.76, 0.5),
            (1.0, 2.417, 0.1),
        ];
        for (n1, n2, ci) in cases {
            let sin2_t = (n1 / n2) * (n1 / n2) * f64::mul_add(ci, -ci, 1.0);
            let ct = (1.0_f64 - sin2_t).sqrt();
            let forward = fresnel_dielectric(ci, n1, n2);
            let back = fresnel_dielectric(ct, n2, n1);
            assert!(
                (forward - back).abs() < 1e-12,
                "n {n1} -> {n2}, cos_i {ci}: {forward} vs {back}"
            );
        }
    }

    #[test]
    fn rises_monotonically_toward_grazing_incidence() {
        let mut last = 0.0;
        for k in (0..=100).rev() {
            let r = fresnel_dielectric(f64::from(k) / 100.0, 1.0, 1.5);
            assert!(
                r >= last - 1e-15,
                "R must not fall as cos_i falls: {r} < {last}"
            );
            last = r;
        }
        assert!((last - 1.0).abs() < 1e-12);
    }

    #[test]
    fn agrees_with_the_tracers_own_f32_expression() {
        let cases = [
            (1.0_f32, 1.544_f32),
            (1.544, 1.0),
            (1.0, 2.417),
            (1.333, 1.76),
            (1.76, 1.333),
        ];
        for (n1, n2) in cases {
            for k in 1..=40_u8 {
                let ci = f32::from(k) / 40.0;
                let sin2_t = (n1 / n2) * (n1 / n2) * f32::mul_add(ci, -ci, 1.0);
                if sin2_t >= 0.999 {
                    continue; // at and beyond the critical angle the f32 form is ill-conditioned
                }
                let tracer = f64::from(tracer_reflectance_f32(n1, n2, ci));
                let mine = fresnel_dielectric(f64::from(ci), f64::from(n1), f64::from(n2));
                assert!(
                    (tracer - mine).abs() < 2e-5,
                    "n {n1} -> {n2}, cos_i {ci}: tracer {tracer}, f64 {mine}"
                );
            }
        }
    }

    #[test]
    fn out_of_range_cosines_are_clamped() {
        assert!(
            (fresnel_dielectric(1.5, 1.0, 1.5) - fresnel_dielectric(1.0, 1.0, 1.5)).abs() < 1e-15
        );
        assert!((fresnel_dielectric(-0.5, 1.0, 1.5) - 1.0).abs() < 1e-12);
    }
}
