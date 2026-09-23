//! Off-UI-thread mini preview render for the Tilt Performance dialog: as the user
//! hovers/scrubs the curve, a small thumbnail shows the stone at the hovered tilt
//! angle and axis, so the numbers on the chart have a picture to go with them.
//!
//! [`camera_pose_for_hover`] derives `(cam_yaw, cam_pitch)` from the hovered
//! `(axis_index, tilt_deg)` using exactly the tilt-to-pose formula
//! `evaluate_full_axis_profile_at_azimuth` sweeps under: `tilt_deg` is tilt away from
//! table-up (not camera elevation), so `cam_pitch = 90° - |tilt_deg|` and `cam_yaw` is
//! that axis's `PROFILE_AZIMUTHS_DEG` entry, shifted by 180° whenever `tilt_deg` is
//! negative. Reusing the identical formula is what makes this preview agree with the
//! curve it illustrates, rather than showing some other, merely-plausible pose.
//!
//! This is its own one-off render, not a reuse of the live viewport: the live
//! viewport (`bridge::render_thread`) continuously accumulates samples into buffers
//! sized to the configured render resolution, driven by the actual camera the user is
//! orbiting. Hijacking that loop for a hover-driven pose would either fight the user's
//! own camera for the same buffers or need a second full accumulation buffer at full
//! viewport resolution. Instead, following the same shape `bridge::export_thread`
//! uses for its own independent one-off render, this module traces its own tiny
//! buffer directly, with its own single-shot low-sample accumulation, on its own
//! thread -- it never touches `RenderContext`'s accumulation buffers and never blocks
//! the UI thread or the live render thread.
//!
//! Debounced rather than throttled like `PreviewThrottle`'s fixed-minimum-interval
//! export thumbnail: dragging across the 181-point curve fires a request per pixel of
//! mouse movement, and an intermediate point from three renders ago is actively
//! useless the moment the user has moved on -- only wherever the cursor settles is
//! worth spending a render on. So each request starts a short sleep, and only
//! actually renders if no newer request has arrived by the time it wakes up (checked
//! via a generation counter, the same superseded-work pattern `gui::tilt_profile`
//! uses for its background sweep). A fast scrub therefore queues many cheap
//! sleep-then-bail threads but renders at most once per quiet period.

use crate::{
    MainWindow, TiltModel,
    bridge::render_thread::{RenderContext, resolve_material_with_override},
};
use glam::Vec3;
use indicatrix::{
    color::metrics::PROFILE_AZIMUTHS_DEG,
    geometry::plane::GpuFacetPlane,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, pixel_rotations, sample_draws, trace_spectral_ray_with_finish},
    },
    renderer::tonemap::tonemap_to_rgba,
};
use slint::{ComponentHandle, Rgba8Pixel, SharedPixelBuffer};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

/// Preview thumbnail's edge length, in pixels. Small on purpose -- this is a "which
/// pose am I looking at" glance, not a viewport: `bridge::export_thread::preview`'s
/// `PREVIEW_MAX_LONG_EDGE` (360px) is sized for reading a full composition, this only
/// to read silhouette/brilliance-pattern at a glance.
const PREVIEW_SIZE: u32 = 96;

/// Samples per pixel for the hover preview. Deliberately tiny -- this trades visible
/// noise for staying well under the per-render cost that would make even a
/// settled-cursor preview feel laggy; the preview only needs to communicate
/// silhouette, facet pattern and rough colour/brightness, not serve as a converged
/// reference image.
const PREVIEW_SPP: u32 = 2;

/// Quiet period a hover/scrub request must go unsuperseded for before this module
/// actually renders it. Short enough that a cursor that pauses briefly still gets a
/// prompt preview, long enough that a fast drag across many of the 181 hoverable
/// points never starts more than a small, bounded number of renders.
const HOVER_DEBOUNCE: Duration = Duration::from_millis(90);

/// Same tilt-to-pose formula [`evaluate_full_axis_profile_at_azimuth`] uses to build
/// the curve: given the hovered/swept axis and tilt angle (tilt away from table-up,
/// not camera elevation), returns `(cam_yaw_rad, cam_pitch_rad)` for [`Camera::new`]:
/// `cam_pitch = 90° - |tilt_deg|` (`90°` = table-up at `tilt_deg == 0`, `0°` = edge-on
/// at `|tilt_deg| == 90`), and the sign of `tilt_deg` selects which of the axis's two
/// opposite azimuths (`PROFILE_AZIMUTHS_DEG[axis_index]` or that plus 180°) it's on.
///
/// Takes `tilt_deg` as `f64` (rather than this module's own `f32`) so
/// `gui::tilt::video_export` can pose a frame at an EXACT fractional angle (e.g. a
/// 0.01°-step sweep) without the precision loss an `f32` round-trip would add on top
/// of an already-fine step -- see [`camera_pose_for_hover`], this module's own `f32`
/// call site, for the hover tooltip's coarser (mouse-pixel-snapped) needs.
///
/// [`evaluate_full_axis_profile_at_azimuth`]: indicatrix::color::metrics::evaluate_full_axis_profile_at_azimuth
pub(in crate::gui) fn camera_pose_for_axis_tilt(axis_index: usize, tilt_deg: f64) -> (f32, f32) {
    let base_azimuth_deg = f64::from(PROFILE_AZIMUTHS_DEG.get(axis_index).copied().unwrap_or(0.0));
    let azimuth_deg = if tilt_deg < 0.0 {
        base_azimuth_deg + 180.0
    } else {
        base_azimuth_deg
    };
    let pitch_deg = 90.0 - tilt_deg.abs();
    (
        azimuth_deg.to_radians() as f32,
        pitch_deg.to_radians() as f32,
    )
}

/// `f32` convenience wrapper around [`camera_pose_for_axis_tilt`] for this module's own
/// mouse-pixel-driven hover preview, which never needs more than `f32` precision.
fn camera_pose_for_hover(axis_index: usize, tilt_deg: f32) -> (f32, f32) {
    camera_pose_for_axis_tilt(axis_index, f64::from(tilt_deg))
}

/// Bundles every input [`render_hover_preview`] needs -- geometry, material, the
/// derived camera pose, and the scene's lighting/quality settings -- into one struct
/// rather than a ten-parameter function signature.
struct HoverPreviewScene<'a> {
    planes: &'a [GpuFacetPlane],
    material: &'a GemMaterial,
    cam_yaw: f32,
    cam_pitch: f32,
    distance: f32,
    lighting_preset: indicatrix::optics::raytracer::LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    max_bounces: u32,
}

/// Traces the small preview thumbnail directly, following `bridge::export_thread`'s
/// "call the raytracer directly rather than reworking the progressive-render loop"
/// pattern at a fixed small size/sample count. Runs entirely on the calling
/// (background) thread -- callers must not call this from the UI thread.
///
/// Material resolution deliberately mirrors `gui::tilt_profile`'s background sweep
/// (`resolve_material_with_override`): the RI override/named custom material DOES
/// reach this preview, same as the curve it sits beside. What still does NOT reach
/// it -- deliberately, not a bug -- is any of the live viewport's DISPLAY overrides
/// (inclusion/subsurface scattering, c-axis orientation, edge rounding, physical
/// stone size, frosted girdle): `caller` below always traces with `&[]` facet
/// finishes (an all-polished stone) against the bare resolved material, so the
/// curve and this thumbnail describe the design's real optics under a fixed,
/// comparable presentation rather than whatever display knobs happen to be dialled
/// in on screen. `performance_graph_dialog.slint`'s caption states this on screen
/// rather than leaving it for a cutter to discover by comparing images.
fn render_hover_preview(scene: &HoverPreviewScene<'_>) -> SharedPixelBuffer<Rgba8Pixel> {
    let width = PREVIEW_SIZE;
    let height = PREVIEW_SIZE;
    // Same FOV convention as every other one-off `Camera::new` call site in this app.
    let camera = Camera::new(scene.cam_yaw, scene.cam_pitch, scene.distance, 42.0);
    let environment = scene
        .lighting_preset
        .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
        .with_backdrop(indicatrix::optics::raytracer::BACKDROP_GREY);

    let mut accum = vec![Vec3::ZERO; (width * height) as usize];
    for y in 0..height {
        for (x, pixel) in accum
            .iter_mut()
            .skip((y * width) as usize)
            .take(width as usize)
            .enumerate()
        {
            let global_pixel_idx = y * width + x as u32;
            let rot = pixel_rotations(global_pixel_idx);
            let mut sample_sum = Vec3::ZERO;
            for sample_num in 0..PREVIEW_SPP {
                let draws = sample_draws(global_pixel_idx, sample_num, &rot);
                let ray = camera.generate_ray(
                    x as f32,
                    y as f32,
                    width as f32,
                    height as f32,
                    draws.jitter_x,
                    draws.jitter_y,
                );
                // Preview always renders an all-polished stone (`&[]` facet finishes)
                // regardless of the live viewport's frosted-girdle toggle -- this
                // illustrates the curve's own pose/geometry, not every display option.
                sample_sum += trace_spectral_ray_with_finish(
                    ray,
                    scene.planes,
                    &[],
                    scene.material,
                    scene.max_bounces,
                    environment,
                    draws.seed,
                    draws.hero_rand,
                    None,
                );
            }
            *pixel = sample_sum;
        }
    }

    let rgba = tonemap_to_rgba(&accum, 1.0 / PREVIEW_SPP as f32);
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(width, height);
    let dst = buffer.make_mut_slice();
    let src: &[Rgba8Pixel] = bytemuck::cast_slice(&rgba);
    dst.copy_from_slice(src);
    buffer
}

/// Wires `MainWindow::request_tilt_hover_preview(axis_index, tilt_deg)` (fired by
/// `performance_graph_dialog.slint`'s hover tooltip) to the debounced background
/// render above, pushing the finished thumbnail into `tilt_hover_preview_image` once
/// it lands. Split out of `run_gui`/`build_main_window` purely to keep those
/// functions under clippy's function-length lint.
pub(in crate::gui) fn setup_tilt_hover_preview_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // Bumped on every hover/scrub request; a sleeping worker that wakes up to find
    // it's no longer the latest generation renders nothing at all, so a scrub across
    // many points starts many cheap sleep-then-bail threads but at most one render.
    let generation = Arc::new(AtomicU64::new(0));

    let ui_weak = ui.as_weak();
    let render_ctx = render_ctx.clone();
    ui.global::<TiltModel>().on_request_tilt_hover_preview(
        move |axis_index: i32, tilt_deg: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            if axis_index < 0 {
                return;
            }

            let my_generation = generation.fetch_add(1, Ordering::SeqCst) + 1;
            let generation = generation.clone();
            let render_ctx = render_ctx.clone();
            let ui_weak_bg = ui.as_weak();
            let axis_index = axis_index as usize;

            std::thread::spawn(move || {
                std::thread::sleep(HOVER_DEBOUNCE);
                if generation.load(Ordering::SeqCst) != my_generation {
                    // Superseded before the debounce window even closed -- the cursor kept
                    // moving, so there is no point rendering a pose nobody is looking at
                    // anymore.
                    return;
                }

                let (
                    planes,
                    material_name,
                    material_override,
                    custom_materials,
                    distance,
                    lighting_preset,
                    exposure,
                    light_yaw,
                    light_pitch,
                    max_bounces,
                ) = {
                    let ctx = render_ctx
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    (
                        ctx.active_planes.clone(),
                        ctx.material_name.clone(),
                        ctx.material_override.clone(),
                        ctx.custom_materials.clone(),
                        ctx.distance,
                        ctx.lighting_preset,
                        ctx.exposure,
                        ctx.light_yaw,
                        ctx.light_pitch,
                        ctx.max_bounces,
                    )
                };
                // Same override preference as the sweep itself -- see
                // `tilt_profile`'s own comment.
                let material = resolve_material_with_override(
                    &GemMaterial::all_materials(),
                    &custom_materials,
                    material_override.as_ref(),
                    &material_name,
                );
                let (cam_yaw, cam_pitch) = camera_pose_for_hover(axis_index, tilt_deg);

                let buffer = render_hover_preview(&HoverPreviewScene {
                    planes: &planes,
                    material: &material,
                    cam_yaw,
                    cam_pitch,
                    distance,
                    lighting_preset,
                    exposure,
                    light_yaw,
                    light_pitch,
                    max_bounces,
                });

                let _ = ui_weak_bg.upgrade_in_event_loop(move |ui| {
                    if generation.load(Ordering::SeqCst) != my_generation {
                        // Superseded while the (comparatively slow) render itself was
                        // running -- drop it rather than flash an already-stale pose on
                        // screen right before the fresher one lands.
                        return;
                    }
                    ui.global::<TiltModel>()
                        .set_hover_preview_image(slint::Image::from_rgba8(buffer));
                });
            });
        },
    );
}

#[cfg(test)]
mod tests {
    use super::camera_pose_for_hover;
    use indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG;

    /// Positive tilt stays on the axis's own (positive) azimuth, pitch equal to
    /// `90 - tilt` (tilt is measured away from table-up, not camera elevation).
    #[test]
    fn positive_tilt_uses_the_axis_own_azimuth() {
        let (yaw, pitch) = camera_pose_for_hover(1, 30.0);
        assert!((yaw - PROFILE_AZIMUTHS_DEG[1].to_radians()).abs() < 1e-6);
        assert!((pitch - (90.0 - 30.0f32).to_radians()).abs() < 1e-6);
    }

    /// Negative tilt flips to the opposite (+180 deg) azimuth, pitch equal to
    /// `90 - |tilt|`.
    #[test]
    fn negative_tilt_uses_the_opposite_azimuth_and_positive_pitch() {
        let (yaw, pitch) = camera_pose_for_hover(2, -25.0);
        let expected_yaw = (PROFILE_AZIMUTHS_DEG[2] + 180.0).to_radians();
        assert!((yaw - expected_yaw).abs() < 1e-6);
        assert!((pitch - (90.0 - 25.0f32).to_radians()).abs() < 1e-6);
    }

    /// The shared table-up point: `tilt_deg == 0` must resolve to pitch 90°
    /// (table-up), on the axis's own positive azimuth.
    #[test]
    fn zero_tilt_is_table_up_on_the_positive_azimuth() {
        let (yaw, pitch) = camera_pose_for_hover(0, 0.0);
        assert!((yaw - PROFILE_AZIMUTHS_DEG[0].to_radians()).abs() < 1e-6);
        assert!((pitch - 90.0f32.to_radians()).abs() < 1e-6);
    }

    /// The two edge-on extremes: `tilt_deg == +-90` must resolve to pitch 0°
    /// (edge-on/profile), on the positive/negative azimuth respectively.
    #[test]
    fn extreme_tilt_is_edge_on() {
        let (positive_yaw, positive_pitch) = camera_pose_for_hover(3, 90.0);
        assert!((positive_yaw - PROFILE_AZIMUTHS_DEG[3].to_radians()).abs() < 1e-6);
        assert!(positive_pitch.abs() < 1e-6);

        let (negative_yaw, negative_pitch) = camera_pose_for_hover(3, -90.0);
        assert!((negative_yaw - (PROFILE_AZIMUTHS_DEG[3] + 180.0).to_radians()).abs() < 1e-6);
        assert!(negative_pitch.abs() < 1e-6);
    }

    /// An out-of-range axis index falls back to azimuth 0 rather than panicking --
    /// `PROFILE_AZIMUTHS_DEG.get(axis_index)` is `None` past index 3.
    #[test]
    fn out_of_range_axis_index_falls_back_to_azimuth_zero() {
        let (yaw, _pitch) = camera_pose_for_hover(99, 10.0);
        assert!((yaw - 0.0f32.to_radians()).abs() < 1e-6);
    }
}
