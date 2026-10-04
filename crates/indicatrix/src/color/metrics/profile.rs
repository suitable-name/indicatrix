//! Angular-profile sweeps built on [`super::evaluate::evaluate_gem_optical_metrics`]:
//! the fixed-azimuth elevation sweep ([`evaluate_angular_profile_at_azimuth`],
//! [`evaluate_angular_profile`]) and the full `-90..=90°` tilt-away-from-table-up
//! sweep ([`evaluate_full_axis_profile_at_azimuth`]).

use super::{evaluate::evaluate_gem_optical_metrics_geom, types::PROFILE_ANGLES_DEG};
use crate::{
    geometry::{plane::GpuFacetPlane, tool::StoneGeometry},
    optics::{materials::GemMaterial, raytracer::EnvironmentSource},
};

/// Camera azimuths the Tilt Performance dialog sweeps a full tilt-elevation profile at,
/// in degrees -- see [`evaluate_angular_profile_at_azimuth`].
///
/// `0.0` looks straight down whatever direction `RenderContext::yaw == 0.0` frames (for
/// an elongated outline -- marquise, emerald cut, pear -- conventionally the table's
/// long axis). Each further entry rotates the viewpoint another 45° around the
/// table-normal axis, so `90.0` looks down the perpendicular ("width") axis and `45.0`/
/// `135.0` bisect the two -- tilting toward a non-round stone's long vs. short axis
/// windows very differently, information a single azimuth-0 sweep hides.
pub const PROFILE_AZIMUTHS_DEG: [f32; 4] = [0.0, 45.0, 90.0, 135.0];

/// Calculates a 19-point angular profile of (Brilliance %, Extinction %, Windowing %)
/// at an explicit camera azimuth.
///
/// Sampled over `PoV` tilt elevation in exact 5° steps
/// (see [`PROFILE_ANGLES_DEG`]). `cam_yaw` is in radians, matching
/// `evaluate_gem_optical_metrics`'s own convention -- see [`PROFILE_AZIMUTHS_DEG`] for
/// the four-azimuth convention the Tilt Performance dialog's axis switcher uses.
/// [`evaluate_angular_profile`] is just this function called at `cam_yaw: 0.0`.
#[must_use]
pub fn evaluate_angular_profile_at_azimuth(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    cam_yaw: f32,
    environment: EnvironmentSource<'_>,
) -> ([f32; 19], [f32; 19], [f32; 19]) {
    evaluate_angular_profile_at_azimuth_geom(
        StoneGeometry::planes_only(planes),
        material,
        cam_yaw,
        environment,
    )
}

/// [`evaluate_angular_profile_at_azimuth`] for a stone with tools; bit-identical to it
/// when `geom.tools` is empty.
#[must_use]
pub fn evaluate_angular_profile_at_azimuth_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    environment: EnvironmentSource<'_>,
) -> ([f32; 19], [f32; 19], [f32; 19]) {
    sample_elevation_sweep(
        geom,
        material,
        cam_yaw,
        &PROFILE_ANGLES_DEG,
        environment,
        &mut || true,
    )
}

/// Shared per-angle sampling loop behind [`evaluate_angular_profile_at_azimuth`] (called
/// with `&PROFILE_ANGLES_DEG`, 19 points / 5° steps) and
/// [`evaluate_full_axis_profile_at_azimuth`] (called with `&HALF_AXIS_PITCH_DEG`, 90
/// points / 1° steps -- a different grid needing its own const array rather than a finer
/// `PROFILE_ANGLES_DEG`). Factored out so both call sites run the textually identical
/// sequence of floating-point operations rather than two hand-copies that could drift
/// apart. Const-generic over `N` so one function body serves both grids without a
/// `Vec`-based version paying an allocation per sweep.
///
/// `gate` is asked before every evaluation; when it answers `false` the sweep stops and
/// the rest of the curves stay `0.0` (the caller that passed a gate that can say no
/// discards them). A gate that always answers `true` leaves the sequence of
/// floating-point operations exactly as it was without one.
fn sample_elevation_sweep<const N: usize>(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    cam_yaw: f32,
    angles_deg: &[f32; N],
    environment: EnvironmentSource<'_>,
    gate: &mut dyn FnMut() -> bool,
) -> ([f32; N], [f32; N], [f32; N]) {
    let mut brilliance_curve = [0.0f32; N];
    let mut extinction_curve = [0.0f32; N];
    let mut windowing_curve = [0.0f32; N];

    for (i, &deg) in angles_deg.iter().enumerate() {
        if !gate() {
            break;
        }
        let cam_pitch_rad = deg.to_radians();
        let m =
            evaluate_gem_optical_metrics_geom(geom, material, cam_yaw, cam_pitch_rad, environment);
        brilliance_curve[i] = m.brilliance_pct;
        extinction_curve[i] = m.extinction_pct;
        windowing_curve[i] = m.windowing_pct;
    }

    (brilliance_curve, extinction_curve, windowing_curve)
}

/// Pitch (camera-elevation) values `0..=89` degrees, ascending -- the per-half grid
/// [`evaluate_full_axis_profile_at_azimuth`] sweeps at each of the two azimuths it
/// combines. Excludes `90.0`: pitch `90` (table-up/face-up, the shared point -- see that
/// function's doc comment) is evaluated exactly once by the caller and stitched into
/// both halves, rather than swept twice only to throw one copy away.
const fn build_half_axis_pitch_deg() -> [f32; 90] {
    let mut out = [0.0f32; 90];
    let mut i = 0;
    while i < 90 {
        // out[i] = pitch i degrees: out[0] = 0.0, out[89] = 89.0.
        out[i] = i as f32;
        i += 1;
    }
    out
}
const HALF_AXIS_PITCH_DEG: [f32; 90] = build_half_axis_pitch_deg();

const fn build_tilt_angles_deg() -> [f32; 181] {
    let mut out = [0.0f32; 181];
    let mut i = 0;
    while i < 181 {
        // i=0 -> -90.0, i=90 -> 0.0, i=180 -> 90.0.
        out[i] = i as f32 - 90.0;
        i += 1;
    }
    out
}

/// 181 full-axis tilt sample points in exact 1° steps, `-90..=90` inclusive.
///
/// The VALUE is tilt AWAY FROM TABLE-UP, in degrees -- NOT camera elevation/pitch (that
/// is what [`PROFILE_ANGLES_DEG`] is). `TILT_ANGLES_DEG[90] == 0.0` is the shared
/// table-up/face-up pole (camera pitch 90°) every axis's curve passes through; `[0]`
/// and `[180]` are both edge-on/profile (camera pitch 0°), reached from the two
/// opposite azimuths of the axis pair -- see [`evaluate_full_axis_profile_at_azimuth`]'s
/// doc comment for the full geometry and the pose-to-index formula.
///
/// Table-up, not edge-on, is shared at the centre: an earlier version shared edge-on
/// instead, which is approached from two physically distinct azimuths, making the
/// merged curve discontinuous exactly at the "shared" point. Table-up is the one pose
/// in this sweep that is actually azimuth-independent (see the next function's doc
/// comment), which is what makes sharing it correct.
///
/// # Why 1° and not coarser (measured)
///
/// Reconstruction error against the true 1°-resolution curve decays roughly linearly
/// with step size (measured against round-brilliant diamond down to 2°/91 pts: 3-5
/// percentage points), since the curve carries genuine high-frequency structure
/// (individual facets flipping in and out of a fixed light as the stone tilts). That
/// error is disqualifying here since these curves back hard-threshold catalogue filters
/// (e.g. "windowing never exceeds 20% within ±45°"), where it can flip a design in or
/// out of the result set. 1° is also the natural ceiling: the graph canvas is a few
/// hundred pixels wide, so finer sampling would only resolve grid-sampling noise --
/// don't "improve" this to 0.5° later.
pub const TILT_ANGLES_DEG: [f32; 181] = build_tilt_angles_deg();

/// Merges one axis's independently-swept positive-azimuth and negative-azimuth
/// (`positive_azimuth + 180°`) pitch-0..89 halves, plus the shared table-up (pitch-90)
/// pole evaluated once, into the single 181-point `TILT_ANGLES_DEG`-indexed output
/// array -- see [`evaluate_full_axis_profile_at_azimuth`]'s doc comment for the
/// geometry and tilt-to-pose formula. `positive_pitch_sweep`/`negative_pitch_sweep` are
/// each indexed by [`HALF_AXIS_PITCH_DEG`] (pitch `i` at index `i`, i.e. ascending
/// pitch = descending `|tilt|`, since `pitch = 90 - |tilt|`).
fn merge_full_axis_halves(
    table_up_pole: f32,
    positive_pitch_sweep: &[f32; 90],
    negative_pitch_sweep: &[f32; 90],
) -> [f32; 181] {
    let mut out = [0.0f32; 181];
    // [0..90] = tilt -90..=-1: negative_pitch_sweep laid down directly (pitch = 90 +
    // tilt for tilt < 0, so out[k] is exactly negative_pitch_sweep[k]).
    out[0..90].copy_from_slice(negative_pitch_sweep);
    out[90] = table_up_pole; // tilt = 0.0: the shared table-up (pitch 90) pole.
    // [91..181] = tilt 1..=90: positive_pitch_sweep laid down reversed (pitch = 90 -
    // tilt, descending as tilt increases, ending at pitch 0/edge-on at tilt 90).
    for i in 0..90 {
        out[91 + i] = positive_pitch_sweep[89 - i];
    }
    out
}

/// Calculates a full-axis, 181-point angular profile spanning the ENTIRE `-90°..=+90°`
/// TILT range for one axis of [`PROFILE_AZIMUTHS_DEG`].
///
/// Tilt is measured AWAY FROM TABLE-UP -- see [`TILT_ANGLES_DEG`]. Returns
/// (Brilliance %, Extinction %, Windowing %) at 1° resolution. A different
/// parameterisation of the same [`evaluate_gem_optical_metrics`] machinery, not a
/// replacement for [`evaluate_angular_profile_at_azimuth`] (which sweeps camera
/// ELEVATION at a single fixed azimuth): this one sweeps TILT AWAY FROM TABLE-UP,
/// switching between two opposite azimuths partway through. Do not "unify"
/// the two -- they answer different questions.
///
/// # The tilt-to-pose formula
///
/// For tilt `t` (`-90..=+90°`) on the axis whose positive azimuth is `A =
/// positive_azimuth_deg`:
///
/// ```text
/// cam_pitch = (90 - |t|).to_radians()
/// cam_yaw   = A          when t >= 0
///             A + 180    when t <  0
/// ```
///
/// `t = 0` -> `cam_pitch = 90°`: table-up. `t = ±90` -> `cam_pitch = 0°`: edge-on,
/// approached from the two opposite azimuths. This is the relationship the catalogue's
/// performance filters are phrased against (e.g. "windowing never exceeds 20% within
/// ±45° [of table-up]").
///
/// # Why the negative half is a real sweep, not a mirror
///
/// [`evaluate_gem_optical_metrics`] takes a FIXED environment (for the studio rigs, one
/// light pose). Tilting toward vs. away from the light gives genuinely different brilliance/extinction/
/// windowing, even for a symmetric round brilliant, and an asymmetric outline (pear,
/// heart, half-moon) is not 2-fold symmetric geometrically either. So
/// `positive_azimuth_deg + 180°` is independently raytraced at every pitch, never
/// derived by reflecting the positive half.
///
/// # Why table-up (not edge-on) is the point that's genuinely shared
///
/// `Camera::new` computes `origin = (d*cos(pitch)*sin(yaw), d*sin(pitch),
/// d*cos(pitch)*cos(yaw))`. At `pitch == 0°` (edge-on), `origin` depends on `yaw` -- the
/// two azimuths put the camera in different places. At `pitch == 90°` (table-up),
/// `origin ≈ (0, d, 0)` for every yaw -- one single pose, and `Camera::new`'s
/// `world_up` fallback switches exactly at this pole too, keeping `right`/`up` from
/// spinning with `yaw` there. Table-up is therefore the one pose in this sweep that is
/// actually azimuth-independent, which is what makes evaluating it once and sharing it
/// between both halves correct rather than merely convenient (an earlier version
/// shared edge-on instead -- see [`TILT_ANGLES_DEG`]'s doc comment for why that was a
/// genuine bug).
///
/// Net cost: `90 + 1 + 90 = 181` raytrace evaluations per axis (the shared table-up
/// point is evaluated once, not twice).
#[must_use]
pub fn evaluate_full_axis_profile_at_azimuth(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    positive_azimuth_deg: f32,
    environment: EnvironmentSource<'_>,
) -> ([f32; 181], [f32; 181], [f32; 181]) {
    evaluate_full_axis_profile_at_azimuth_geom(
        StoneGeometry::planes_only(planes),
        material,
        positive_azimuth_deg,
        environment,
    )
}

/// [`evaluate_full_axis_profile_at_azimuth`] for a stone with tools; bit-identical to it
/// when `geom.tools` is empty.
#[must_use]
pub fn evaluate_full_axis_profile_at_azimuth_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    positive_azimuth_deg: f32,
    environment: EnvironmentSource<'_>,
) -> ([f32; 181], [f32; 181], [f32; 181]) {
    full_axis_profile(
        geom,
        material,
        positive_azimuth_deg,
        environment,
        &mut || true,
    )
}

/// [`evaluate_full_axis_profile_at_azimuth`] with a hook before every evaluation.
///
/// `step` is called before each of the [`EVALUATIONS_PER_AXIS`] raytrace evaluations and
/// can stop the sweep by answering `false` (then `None`). The browser app's tilt Worker
/// uses it to report progress and to honour Cancel between points; with a `step` that
/// always answers `true` the result is bit-identical to
/// [`evaluate_full_axis_profile_at_azimuth`]'s.
#[must_use]
pub fn evaluate_full_axis_profile_at_azimuth_stepped(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    positive_azimuth_deg: f32,
    environment: EnvironmentSource<'_>,
    step: &mut dyn FnMut() -> bool,
) -> Option<([f32; 181], [f32; 181], [f32; 181])> {
    evaluate_full_axis_profile_at_azimuth_stepped_geom(
        StoneGeometry::planes_only(planes),
        material,
        positive_azimuth_deg,
        environment,
        step,
    )
}

/// [`evaluate_full_axis_profile_at_azimuth_stepped`] for a stone with tools;
/// bit-identical to it when `geom.tools` is empty.
#[must_use]
pub fn evaluate_full_axis_profile_at_azimuth_stepped_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    positive_azimuth_deg: f32,
    environment: EnvironmentSource<'_>,
    step: &mut dyn FnMut() -> bool,
) -> Option<([f32; 181], [f32; 181], [f32; 181])> {
    let mut stopped = false;
    let mut gate = || {
        if !stopped && !step() {
            stopped = true;
        }
        !stopped
    };
    let curves = full_axis_profile(geom, material, positive_azimuth_deg, environment, &mut gate);
    (!stopped).then_some(curves)
}

/// Raytrace evaluations one full-axis profile costs: the shared table-up pole, then 90
/// pitches (0..=89) at each of the two opposite azimuths.
pub const EVALUATIONS_PER_AXIS: usize = 181;

/// The body of [`evaluate_full_axis_profile_at_azimuth`] with a `gate` asked before every
/// evaluation (see `sample_elevation_sweep`). The pole is always evaluated, even when the
/// gate has already said no: one evaluation is cheaper than another code path.
fn full_axis_profile(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    positive_azimuth_deg: f32,
    environment: EnvironmentSource<'_>,
    gate: &mut dyn FnMut() -> bool,
) -> ([f32; 181], [f32; 181], [f32; 181]) {
    // The gate is asked once for the pole, so it counts as one of the evaluations.
    let _ = gate();
    // The shared table-up (pitch 90) pole -- see this function's doc comment for why
    // evaluating it once is sound. Evaluated at the positive azimuth by convention
    // (azimuth is provably irrelevant here, but a concrete choice is still needed).
    let table_up_pole = evaluate_gem_optical_metrics_geom(
        geom,
        material,
        positive_azimuth_deg.to_radians(),
        90.0f32.to_radians(),
        environment,
    );
    let (positive_brilliance, positive_extinction, positive_windowing) = sample_elevation_sweep(
        geom,
        material,
        positive_azimuth_deg.to_radians(),
        &HALF_AXIS_PITCH_DEG,
        environment,
        gate,
    );
    let negative_azimuth_deg = positive_azimuth_deg + 180.0;
    let (negative_brilliance, negative_extinction, negative_windowing) = sample_elevation_sweep(
        geom,
        material,
        negative_azimuth_deg.to_radians(),
        &HALF_AXIS_PITCH_DEG,
        environment,
        gate,
    );

    (
        merge_full_axis_halves(
            table_up_pole.brilliance_pct,
            &positive_brilliance,
            &negative_brilliance,
        ),
        merge_full_axis_halves(
            table_up_pole.extinction_pct,
            &positive_extinction,
            &negative_extinction,
        ),
        merge_full_axis_halves(
            table_up_pole.windowing_pct,
            &positive_windowing,
            &negative_windowing,
        ),
    )
}

/// Calculates a 19-point angular profile of (Brilliance %, Extinction %, Windowing %)
/// at the canonical (0°) camera azimuth.
///
/// See [`evaluate_angular_profile_at_azimuth`] for the general form this delegates to,
/// and [`PROFILE_AZIMUTHS_DEG`] for what the other three azimuths mean. Sampled over
/// `PoV` tilt elevation angles in exact 5° steps: [0°, 5°, 10°, 15°, 20°, 25°, 30°,
/// 35°, 40°, 45°, 50°, 55°, 60°, 65°, 70°, 75°, 80°, 85°, 90°].
#[must_use]
pub fn evaluate_angular_profile(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> ([f32; 19], [f32; 19], [f32; 19]) {
    evaluate_angular_profile_at_azimuth(planes, material, 0.0, environment)
}

/// [`evaluate_angular_profile`] for a stone with tools; bit-identical to it when
/// `geom.tools` is empty.
#[must_use]
pub fn evaluate_angular_profile_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> ([f32; 19], [f32; 19], [f32; 19]) {
    evaluate_angular_profile_at_azimuth_geom(geom, material, 0.0, environment)
}
