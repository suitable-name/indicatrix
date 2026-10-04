//! The Tilt Performance dialog's whole sweep: the full-axis profile
//! ([`super::profile::evaluate_full_axis_profile_at_azimuth`]) at every [`PROFILE_AZIMUTHS_DEG`] axis.
//!
//! This is the loop the desktop's `gui::tilt::tilt_profile` used to hold itself; both apps
//! now call it from here. [`evaluate_all_axes_profiles_stepped`] is the same sweep with a
//! progress/cancel hook between the raytrace evaluations, for the browser app's Worker.

use super::profile::{
    EVALUATIONS_PER_AXIS, PROFILE_AZIMUTHS_DEG, evaluate_full_axis_profile_at_azimuth_geom,
    evaluate_full_axis_profile_at_azimuth_stepped_geom,
};
use crate::{
    geometry::{plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{materials::GemMaterial, raytracer::EnvironmentSource},
};

/// One axis's three curves, each `TILT_ANGLES_DEG`-indexed (181 points, -90 to +90 degrees).
#[derive(Debug, Clone, PartialEq)]
pub struct AxisProfile {
    /// Brilliance %.
    pub brilliance: [f32; 181],
    /// Extinction %.
    pub extinction: [f32; 181],
    /// Windowing %.
    pub windowing: [f32; 181],
}

impl From<([f32; 181], [f32; 181], [f32; 181])> for AxisProfile {
    fn from((brilliance, extinction, windowing): ([f32; 181], [f32; 181], [f32; 181])) -> Self {
        Self {
            brilliance,
            extinction,
            windowing,
        }
    }
}

/// Where a running sweep is, reported before each raytrace evaluation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SweepProgress {
    /// The axis being swept, an index into [`PROFILE_AZIMUTHS_DEG`].
    pub axis: usize,
    /// Evaluations finished so far, over all axes.
    pub done: usize,
    /// Evaluations the whole sweep takes ([`total_evaluations`]).
    pub total: usize,
}

/// Raytrace evaluations the whole sweep takes: 181 per axis, four axes (724).
#[must_use]
pub const fn total_evaluations() -> usize {
    PROFILE_AZIMUTHS_DEG.len() * EVALUATIONS_PER_AXIS
}

/// Sweeps every [`PROFILE_AZIMUTHS_DEG`] axis, each a full 181-point +-90 degree profile,
/// in axis order.
///
/// Axis 0 is swept too, rather than reusing the live render's own value: that one covers
/// only the positive half and cannot supply the negative half a full-axis sweep needs.
/// About 724 raytrace evaluations, roughly 1.4 s natively.
#[must_use]
pub fn evaluate_all_axes_profiles(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> Vec<AxisProfile> {
    evaluate_all_axes_profiles_geom(StoneGeometry::planes_only(planes), material, environment)
}

/// [`evaluate_all_axes_profiles`] for a stone with tools; bit-identical to it when
/// `geom.tools` is empty.
#[must_use]
pub fn evaluate_all_axes_profiles_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> Vec<AxisProfile> {
    PROFILE_AZIMUTHS_DEG
        .iter()
        .map(|&azimuth_deg| {
            evaluate_full_axis_profile_at_azimuth_geom(geom, material, azimuth_deg, environment)
                .into()
        })
        .collect()
}

/// [`evaluate_all_axes_profiles`] with a hook before every evaluation.
///
/// `step` is called with the sweep's [`SweepProgress`] and can stop the sweep by
/// answering `false`, which returns `None`. When it never does, the curves are
/// bit-identical to [`evaluate_all_axes_profiles`]'s.
#[must_use]
pub fn evaluate_all_axes_profiles_stepped(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    step: &mut dyn FnMut(SweepProgress) -> bool,
) -> Option<Vec<AxisProfile>> {
    evaluate_all_axes_profiles_stepped_geom(
        StoneGeometry::planes_only(planes),
        material,
        environment,
        step,
    )
}

/// [`evaluate_all_axes_profiles_stepped`] for a stone with tools; bit-identical to it
/// when `geom.tools` is empty.
#[must_use]
pub fn evaluate_all_axes_profiles_stepped_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    step: &mut dyn FnMut(SweepProgress) -> bool,
) -> Option<Vec<AxisProfile>> {
    let total = total_evaluations();
    let mut done = 0usize;
    let mut axes = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    for (axis, &azimuth_deg) in PROFILE_AZIMUTHS_DEG.iter().enumerate() {
        let curves = evaluate_full_axis_profile_at_azimuth_stepped_geom(
            geom,
            material,
            azimuth_deg,
            environment,
            &mut || {
                let go = step(SweepProgress { axis, done, total });
                done += 1;
                go
            },
        )?;
        axes.push(curves.into());
    }
    Some(axes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::metrics::{
            TILT_ANGLES_DEG, evaluate_full_axis_profile_at_azimuth,
            evaluate_full_axis_profile_at_azimuth_stepped, evaluate_gem_optical_metrics,
        },
        geometry::cuts::StandardGemCuts,
        optics::raytracer::LightingPreset,
    };

    /// The light pose the sweep tests run under.
    const fn studio() -> EnvironmentSource<'static> {
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
    }

    fn bits(curves: &([f32; 181], [f32; 181], [f32; 181])) -> Vec<u32> {
        curves
            .0
            .iter()
            .chain(&curves.1)
            .chain(&curves.2)
            .map(|v| v.to_bits())
            .collect()
    }

    /// The full-axis function, re-derived from the metric it is built on (tilt `t` looks
    /// from pitch `90 - |t|`, at the axis azimuth for `t >= 0` and its opposite for
    /// `t < 0`), and the stepped variant: bit-identical, one gate call per evaluation.
    #[test]
    fn a_full_axis_profile_is_the_metric_at_each_tilt_pose_and_the_stepped_one_matches() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let plain = evaluate_full_axis_profile_at_azimuth(&planes, &material, 45.0, studio());
        for &index in &[0usize, 37, 90, 101, 180] {
            let tilt = TILT_ANGLES_DEG[index];
            let azimuth = if tilt >= 0.0 { 45.0f32 } else { 225.0 };
            let m = evaluate_gem_optical_metrics(
                &planes,
                &material,
                azimuth.to_radians(),
                (90.0 - tilt.abs()).to_radians(),
                studio(),
            );
            assert_eq!(plain.0[index].to_bits(), m.brilliance_pct.to_bits());
            assert_eq!(plain.1[index].to_bits(), m.extinction_pct.to_bits());
            assert_eq!(plain.2[index].to_bits(), m.windowing_pct.to_bits());
        }
        let mut gate_calls = 0usize;
        let stepped = evaluate_full_axis_profile_at_azimuth_stepped(
            &planes,
            &material,
            45.0,
            studio(),
            &mut || {
                gate_calls += 1;
                true
            },
        )
        .expect("never stopped");
        assert_eq!(bits(&stepped), bits(&plain));
        assert_eq!(gate_calls, EVALUATIONS_PER_AXIS);
    }

    #[test]
    fn the_stepped_sweep_reports_every_evaluation_and_stops_when_told() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let material = GemMaterial::diamond();
        let mut seen = Vec::new();
        let stopped = evaluate_all_axes_profiles_stepped(&planes, &material, studio(), &mut |p| {
            seen.push(p);
            seen.len() < 190
        });
        assert!(stopped.is_none());
        assert_eq!(seen.len(), 190, "nothing is asked for after the refusal");
        let first = SweepProgress {
            axis: 0,
            done: 0,
            total: 724,
        };
        assert_eq!(seen[0], first);
        assert_eq!(seen[180].axis, 0);
        assert_eq!(
            seen[181].axis, 1,
            "the second axis starts after 181 evaluations"
        );
        assert_eq!(seen[181].done, 181);
        assert_eq!(total_evaluations(), 724);
    }
}
