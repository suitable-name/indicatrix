//! The sweep itself, local and remote: what one design's four-axis tilt curves are computed
//! from, split from `engine.rs` (which keeps the resolve/save/lane bookkeeping) so each file
//! stays readable.
//!
//! Both halves honour the design's frosting, the way the render does. The local sweep takes
//! the stone's facet finishes ([`FinishedStone`]); the remote request carries the same
//! information in its scene (`SceneState::frosted_planes`, the planes the design frosts), from
//! which the worker rebuilds the finishes with the function a `RENDER` of that scene uses, so a
//! design's curves are the same wherever they are computed.

use crate::{
    bridge::{
        preview_render::{BATCH_TILT_LIGHTING_PRESET, PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW},
        remote::remote_render,
    },
    gui::batch::batch_queue::local_lane_count,
    settings::WorkerSettings,
};
use indicatrix::{
    color::metrics::{
        FinishedStone, PROFILE_AZIMUTHS_DEG, SweepProgress,
        evaluate_all_axes_profiles_stepped_finished_threaded,
        evaluate_full_axis_profile_at_azimuth_finished_threaded,
    },
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{DEFAULT_MAX_BOUNCES, EnvironmentSource},
    },
};
use indicatrix_net::{
    SceneState,
    messages::{AxisTiltCurves as WireAxisTiltCurves, TiltCurvesRequest, TiltCurvesResponse},
};
use indicatrix_vault::model::tilt_curves::{
    AxisTiltCurves as StorageAxisTiltCurves, TILT_CURVE_AXIS_COUNT, TILT_CURVE_POINTS_PER_AXIS,
    TiltPerformanceCurves,
};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    thread,
};
use tracing::warn;

/// `max_bounces` on the [`SceneState`] a remote tilt-curve request carries -- IGNORED
/// by the worker's `TILT_CURVES` handler entirely (see `indicatrix_net::messages::tilt`'s
/// module doc comment: only `planes`/`material`/`light_yaw`/`light_pitch` and the facet
/// finishes actually feed the computation), but still validated, so this must be a plausible
/// value (`1..=128`, see `apps/indicatrix-worker::validate::MAX_BOUNCES`). It is the
/// raytracer's [`DEFAULT_MAX_BOUNCES`] purely for familiarity -- no render this batch
/// performs ever actually bounces a ray, so the specific value is otherwise arbitrary.
const REMOTE_SCENE_MAX_BOUNCES: u32 = DEFAULT_MAX_BOUNCES;

/// The environment the batch's tilt curves are scored under: [`BATCH_TILT_LIGHTING_PRESET`]
/// at the preview light pose.
const fn batch_environment() -> EnvironmentSource<'static> {
    BATCH_TILT_LIGHTING_PRESET.studio(1.0, PREVIEW_LIGHT_YAW, PREVIEW_LIGHT_PITCH)
}

/// A single fixed request id for every tilt-curve remote dispatch -- safe for the same
/// reason `bridge::preview_render::PREVIEW_REQUEST_ID` gives: each dispatch opens its
/// OWN fresh one-shot connection (see [`fetch_tilt_curves_remote`]), so nothing is ever
/// pipelined behind an unrelated request on the same socket.
const TILT_REQUEST_ID: u32 = 1;

/// Converts one wire [`WireAxisTiltCurves`] (`Vec<f32>` fields, forced by `serde`'s
/// array-length-32 ceiling -- see that type's own doc comment) into the storage
/// [`StorageAxisTiltCurves`] (`[f32; 181]` fields) a fixed-length remote reply's axis
/// must decode into.
///
/// # Errors
///
/// Returns an error string if any curve's length isn't exactly
/// `indicatrix_vault::model::tilt_curves::TILT_CURVE_POINTS_PER_AXIS` -- would mean a
/// worker on a different build with a different `TILT_CURVE_POINTS_PER_AXIS` replied
/// (the wire type's own length isn't enforced by the type system, only by every
/// production call site sending exactly that many points; see that constant's own doc
/// comment), not an ordinary failure this batch should silently swallow the same way as
/// a connection error.
fn wire_axis_to_storage(axis: WireAxisTiltCurves) -> Result<StorageAxisTiltCurves, String> {
    let to_array = |v: Vec<f32>, name: &str| -> Result<[f32; TILT_CURVE_POINTS_PER_AXIS], String> {
        v.try_into().map_err(|v: Vec<f32>| {
            format!(
                "{name} has {} points, expected {TILT_CURVE_POINTS_PER_AXIS}",
                v.len()
            )
        })
    };
    Ok(StorageAxisTiltCurves {
        brilliance_pct: to_array(axis.brilliance_pct, "brilliance_pct")?,
        extinction_pct: to_array(axis.extinction_pct, "extinction_pct")?,
        windowing_pct: to_array(axis.windowing_pct, "windowing_pct")?,
    })
}

/// Dispatches one design's full tilt-curve sweep to `worker` as a single `TILT_CURVES`
/// request, blocking until it finishes, fails, or `cancel` is already set. See this
/// group's `mod.rs` doc comment's "Remote dispatch has no mid-request cancellation or
/// progress" section for why `cancel` is checked only once, up front, rather than
/// during the request. Returns `None` on ANY shortfall (connection failure, missing
/// `tilt_curves` capability, a `Cancelled`/`Error` reply, or a malformed reply) so the
/// caller can decide what to do next -- see this group's "Local + remote" section:
/// [`Both`] requeues for guaranteed local processing, [`RemoteOnly`] surfaces the
/// failure directly.
///
/// `frosted` lists the planes (ascending indices into `stone.planes`) the design frosts; the
/// scene carries them, so the worker sweeps the frosted stone.
///
/// [`Both`]: crate::settings::LiveComputeTarget::Both
/// [`RemoteOnly`]: crate::settings::LiveComputeTarget::RemoteOnly
pub(super) fn fetch_tilt_curves_remote(
    worker: &WorkerSettings,
    stone: StoneGeometry<'_>,
    frosted: &[u32],
    material: &GemMaterial,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let scene = SceneState {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        // Ignored by the `TILT_CURVES` handler (see this group's `mod.rs` doc
        // comment) -- `1` is a plausible, `validate_scene`-legal placeholder, not a
        // real render target.
        width: 1,
        height: 1,
        yaw: 0.0,
        pitch: 0.0,
        distance: 1.0,
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        exposure: 1.0,
        max_bounces: REMOTE_SCENE_MAX_BOUNCES,
        lighting_preset: BATCH_TILT_LIGHTING_PRESET,
        material: material.clone(),
        planes: stone.planes.to_vec(),
        // The girdle toggle is a live-view setting no catalogue design stores; the design's
        // own frosting is `frosted_planes`.
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: stone.tools.to_vec(),
        fluorescence: Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: frosted.to_vec(),
    };
    let (mut stream, welcome) = remote_render::connect_and_handshake(worker).ok()?;
    if !welcome.tilt_curves {
        return None;
    }
    let request = TiltCurvesRequest {
        request_id: TILT_REQUEST_ID,
        scene,
    };
    indicatrix_net::client::send_tilt_curves_request(&mut stream, &request).ok()?;
    match indicatrix_net::client::recv_tilt_curves_response(&mut stream) {
        Ok(TiltCurvesResponse::Curves(result)) => {
            let mut axes_vec = Vec::with_capacity(result.axes.len());
            for axis in result.axes {
                match wire_axis_to_storage(axis) {
                    Ok(axis) => axes_vec.push(axis),
                    Err(e) => {
                        warn!("Remote tilt-curve reply had a malformed axis: {e}");
                        return None;
                    }
                }
            }
            let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes_vec.try_into().ok()?;
            Some(TiltPerformanceCurves { axes })
        }
        Ok(TiltCurvesResponse::Cancelled { .. } | TiltCurvesResponse::Error(_)) | Err(_) => None,
    }
}

/// Threads each local lane may spend inside one design's sweep: the machine's parallelism
/// divided by the lane count, floored at `1`, so lanes x inner threads never exceeds the
/// cores. The sweep itself is parallel now
/// (`evaluate_full_axis_profile_at_azimuth_finished_threaded`), and the lanes already run
/// whole designs side by side.
fn inner_threads_per_lane() -> usize {
    let cores = thread::available_parallelism().map_or(1, std::num::NonZeroUsize::get);
    (cores / local_lane_count()).max(1)
}

/// Computes all 4 axis sweeps for one design LOCALLY, checking `cancel` before each --
/// the finer-grained cancellation the local path affords over the remote one (see this
/// group's `mod.rs` doc comment). Returns `None` the moment `cancel` is observed,
/// abandoning this one in-flight design without saving anything for it (this group's
/// own "all-or-nothing" section).
///
/// `stone` carries the design's facet finishes: a frosted facet scatters light, so a frosted
/// design's curves are not its polished ones.
///
/// Reports no per-axis progress to the caller -- see this group's `mod.rs` doc
/// comment's "Progress with N local lanes" section for why a per-axis readout isn't
/// meaningful once more than one local lane can be mid-design at once. `cancel` is
/// still checked once per axis regardless: that granularity is about how quickly a
/// cancel is honoured, unrelated to what the UI displays.
pub(super) fn compute_tilt_curves_locally(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    cancel: &AtomicBool,
) -> Option<TiltPerformanceCurves> {
    let mut axes = Vec::with_capacity(PROFILE_AZIMUTHS_DEG.len());
    for &azimuth_deg in &PROFILE_AZIMUTHS_DEG {
        if cancel.load(Ordering::Relaxed) {
            return None;
        }
        let (brilliance_pct, extinction_pct, windowing_pct) =
            evaluate_full_axis_profile_at_azimuth_finished_threaded(
                stone,
                material,
                azimuth_deg,
                batch_environment(),
                Some(inner_threads_per_lane()),
            );
        axes.push(StorageAxisTiltCurves {
            brilliance_pct,
            extinction_pct,
            windowing_pct,
        });
    }
    let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().ok()?;
    Some(TiltPerformanceCurves { axes })
}

/// [`compute_tilt_curves_locally`] with `on_step` called before every one of the
/// sweep's raytrace evaluations, for the single-design run's progress bar. Uses the
/// shared stepped sweep (`indicatrix::color::metrics::evaluate_all_axes_profiles_stepped`),
/// whose curves are bit-identical to the plain per-axis calls, and honours `cancel`
/// between evaluations instead of between axes.
pub(super) fn compute_tilt_curves_locally_stepped(
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    cancel: &AtomicBool,
    on_step: &mut dyn FnMut(SweepProgress),
) -> Option<TiltPerformanceCurves> {
    let profiles = evaluate_all_axes_profiles_stepped_finished_threaded(
        stone,
        material,
        batch_environment(),
        None,
        &mut |progress| {
            on_step(progress);
            !cancel.load(Ordering::Relaxed)
        },
    )?;
    let axes: Vec<StorageAxisTiltCurves> = profiles
        .into_iter()
        .map(|profile| StorageAxisTiltCurves {
            brilliance_pct: profile.brilliance,
            extinction_pct: profile.extinction,
            windowing_pct: profile.windowing,
        })
        .collect();
    let axes: [StorageAxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().ok()?;
    Some(TiltPerformanceCurves { axes })
}
