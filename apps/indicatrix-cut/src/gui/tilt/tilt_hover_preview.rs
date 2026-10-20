//! Bounded latest-pose worker. Each beauty image and diagnostic map share one finished scene.
use crate::{
    MainWindow, TiltModel,
    bridge::{export_thread::SceneSnapshot, optical_overlay::export, render_thread::RenderContext},
    gui::editor::FinishedStoneJob,
};
use glam::Vec3;
use indicatrix::{
    color::metrics::{
        OPTICAL_MODEL_VERSION, OpticalImageRequest, PROFILE_AZIMUTHS_DEG, evaluate_optical_image,
    },
    geometry::tool::StoneGeometry,
    optics::raytracer::{
        Camera, DEFAULT_FOV_DEG, EnvironmentSource, add_finite_sample, build_plane_soa,
        pixel_rotations, sample_draws, trace_spectral_ray_with_finish_soa_geom,
    },
    renderer::{
        optical_overlay::{OpticalPresentation, composite_optical, optical_rgba},
        tonemap::tonemap_to_rgba,
    },
};
use slint::ComponentHandle;
use std::{
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the camera/GPU precision; angles are small radians"
)]
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

struct Request {
    serial: u64,
    generation: u64,
    axis: usize,
    tilt: f32,
    finished: Option<FinishedStoneJob>,
    diagnostics: Option<OpticalPresentation>,
    inspect: bool,
}
struct Queue {
    pending: Mutex<Option<Request>>,
    wake: Condvar,
    latest: AtomicU64,
    closed: std::sync::atomic::AtomicBool,
}
struct Owner(Arc<Queue>);
impl Drop for Owner {
    fn drop(&mut self) {
        self.0.closed.store(true, Ordering::Release);
        self.0.latest.fetch_add(1, Ordering::AcqRel);
        self.0.wake.notify_one();
    }
}
fn unavailable(weak: &slint::Weak<MainWindow>, queue: &Arc<Queue>, serial: u64, message: String) {
    let queue = queue.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if queue.latest.load(Ordering::Acquire) != serial {
            return;
        }
        let m = ui.global::<TiltModel>();
        m.set_hover_ready(false);
        m.set_hover_status(format!("Unavailable: {message}").into());
    });
}
fn environment(scene: &SceneSnapshot) -> EnvironmentSource<'_> {
    scene.env_map.as_deref().map_or_else(
        || {
            scene
                .lighting_preset
                .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
                .with_backdrop(scene.backdrop)
                .with_surface_glare(scene.surface_glare)
                .with_head_shadow(scene.head_shadow_deg)
        },
        EnvironmentSource::HdrMap,
    )
}
fn render(
    scene: &SceneSnapshot,
    pose: [f32; 2],
    edge: u32,
    cancel: impl Fn() -> bool,
) -> Option<Vec<u8>> {
    let camera = Camera::new(pose[0], pose[1], scene.distance, DEFAULT_FOV_DEG);
    let soa = build_plane_soa(&scene.active_planes);
    let mut accum = vec![Vec3::ZERO; (edge * edge) as usize];
    for y in 0..edge {
        for x in 0..edge {
            let pixel = y * edge + x;
            let rot = pixel_rotations(pixel);
            for sample in 0..2 {
                if cancel() {
                    return None;
                }
                let draws = sample_draws(pixel, sample, &rot);
                let ray = camera.generate_ray(
                    x as f32,
                    y as f32,
                    edge as f32,
                    edge as f32,
                    draws.jitter_x,
                    draws.jitter_y,
                );
                // Same resolved material, tools, fluorescence and finish as the captured scene.
                let sample = trace_spectral_ray_with_finish_soa_geom(
                    ray,
                    StoneGeometry {
                        planes: &scene.active_planes,
                        tools: &scene.tools,
                    },
                    &soa,
                    &scene.facet_finishes,
                    &scene.material,
                    &scene.fluorescence,
                    scene.max_bounces,
                    environment(scene),
                    draws.seed,
                    draws.hero_rand,
                    None,
                );
                add_finite_sample(&mut accum[pixel as usize], sample);
            }
        }
    }
    Some(tonemap_to_rgba(&accum, 0.5))
}
const BEAUTY_STATUS: &str =
    "Finished geometry; current lighting/HDR/head shadow. Beauty preview 2 samples/pixel.";
/// The draft diagnostics status line, naming the analytical model it was traced under.
fn diagnostics_status() -> String {
    format!(
        "Draft optical diagnostics model {OPTICAL_MODEL_VERSION}: analytical light paths; Fire is dispersion potential. Advanced material transport is not reproduced. Finished geometry; current lighting."
    )
}
/// Waits for the next hover request. `None` once the owner is gone.
fn next_request(bg: &Queue) -> Option<Request> {
    let mut pending = bg
        .pending
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    while pending.is_none() && !bg.closed.load(Ordering::Acquire) {
        pending = bg
            .wake
            .wait(pending)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
    }
    if bg.closed.load(Ordering::Acquire) {
        return None;
    }
    Some(pending.take().expect("pending hover"))
}
/// Draws the diagnostic map over `rgba`. `Ok` is the status line to show (a refusal is
/// shown as status, not as a failure); `Err` is the message of a failed pass.
fn overlay_diagnostics(
    scene: &SceneSnapshot,
    style: OpticalPresentation,
    pose: [f32; 2],
    edge: u32,
    rgba: &mut [u8],
    cancelled: impl Fn() -> bool,
) -> Result<String, String> {
    if let Some(reason) = export::refusal(scene) {
        return Ok(format!("Diagnostics unavailable: {reason}"));
    }
    let camera = Camera::new(pose[0], pose[1], scene.distance, DEFAULT_FOV_DEG);
    let request = OpticalImageRequest {
        camera: &camera,
        width: edge,
        height: edge,
        samples: 1,
        projection_size: None,
        tile_origin: None,
        geometry: StoneGeometry {
            planes: &scene.active_planes,
            tools: &scene.tools,
        },
        material: &scene.material,
        environment: export::diagnostic_environment(scene),
        want_fire: style.layers[3],
        want_scintillation: style.scintillation,
        pose: Some([pose[0], pose[1], scene.distance]),
    };
    let map = evaluate_optical_image(&request, cancelled)
        .map_err(|error| format!("Diagnostic pass: {}", export::error_text(error)))?;
    composite_optical(rgba, &optical_rgba(&map, style));
    Ok(diagnostics_status())
}
/// Serves one hover request: debounce, capture the finished scene, render the beauty
/// preview (and the diagnostics over it), and hand the image to the UI thread.
fn serve(
    request: Request,
    bg: &Arc<Queue>,
    ctx: &Arc<Mutex<RenderContext>>,
    weak: &slint::Weak<MainWindow>,
) {
    let serial = request.serial;
    let generation = request.generation;
    let cancelled =
        || bg.latest.load(Ordering::Acquire) != serial || bg.closed.load(Ordering::Acquire);
    // One worker owns the debounce; new requests replace its single pending slot.
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(90) && !cancelled() {
        std::thread::sleep(Duration::from_millis(5));
    }
    if cancelled() {
        return;
    }
    let finished = match request.finished.map(FinishedStoneJob::resolve) {
        None => None,
        Some(Ok(stone)) => Some(stone),
        Some(Err(error)) => {
            unavailable(weak, bg, serial, error.export_message());
            return;
        }
    };
    if cancelled() || RenderContext::lock(ctx).scene_generation() != generation {
        return;
    }
    let mut scene = match SceneSnapshot::capture_finished(ctx, finished.as_ref()) {
        Ok(scene) => scene,
        Err(reason) => {
            unavailable(weak, bg, serial, reason);
            return;
        }
    };
    if cancelled() || RenderContext::lock(ctx).scene_generation() != generation {
        return;
    }
    let pose: [f32; 2] = camera_pose_for_hover(request.axis, request.tilt).into();
    let edge = if request.inspect { 360 } else { 96 };
    let Some(mut rgba) = render(&scene, pose, edge, cancelled) else {
        return;
    };
    let mut status = BEAUTY_STATUS.to_string();
    if let Some(style) = request.diagnostics {
        scene.optical_diagnostics = Some(style);
        match overlay_diagnostics(&scene, style, pose, edge, &mut rgba, cancelled) {
            Ok(text) => status = text,
            Err(message) => {
                if !cancelled() {
                    unavailable(weak, bg, serial, message);
                }
                return;
            }
        }
    }
    if cancelled() {
        return;
    }
    let ctx_ui = ctx.clone();
    let queue = bg.clone();
    let _ = weak.upgrade_in_event_loop(move |ui| {
        if queue.latest.load(Ordering::Acquire) != serial
            || RenderContext::lock(&ctx_ui).scene_generation() != generation
            || !ui.global::<TiltModel>().get_dialog_open()
        {
            return;
        }
        let mut buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::new(edge, edge);
        buffer.make_mut_bytes().copy_from_slice(&rgba);
        let m = ui.global::<TiltModel>();
        m.set_hover_preview_image(crate::bridge::pixel_buffer::rgba_image(buffer));
        m.set_hover_ready(true);
        m.set_hover_status(status.into());
    });
}
/// The hover worker's loop: serve requests until the owner closes the queue.
fn run_worker(bg: &Arc<Queue>, ctx: &Arc<Mutex<RenderContext>>, weak: &slint::Weak<MainWindow>) {
    while let Some(request) = next_request(bg) {
        serve(request, bg, ctx, weak);
    }
}
pub(in crate::gui) fn setup_tilt_hover_preview_callback(
    ui: &MainWindow,
    ctx: &Arc<Mutex<RenderContext>>,
) {
    let shared = Arc::new(Queue {
        pending: Mutex::new(None),
        wake: Condvar::new(),
        latest: AtomicU64::new(0),
        closed: std::sync::atomic::AtomicBool::new(false),
    });
    let owner = Arc::new(Owner(shared.clone()));
    let bg = shared;
    let ctx_bg = ctx.clone();
    let weak = ui.as_weak();
    let spawned = std::thread::Builder::new()
        .name("indicatrix-tilt-hover".to_string())
        .spawn(move || run_worker(&bg, &ctx_bg, &weak));
    if let Err(error) = spawned {
        tracing::warn!(%error, "could not start the tilt hover preview worker; hover previews stay empty");
    }
    let weak = ui.as_weak();
    let ctx = ctx.clone();
    let owner_request = owner.clone();
    ui.global::<TiltModel>()
        .on_request_tilt_hover_preview(move |axis, tilt| {
            let _keep_alive = &owner_request;
            let Some(ui) = weak.upgrade() else {
                return;
            };
            let Ok(axis_index) = usize::try_from(axis) else {
                return;
            };
            if !tilt.is_finite() {
                return;
            }
            let m = ui.global::<TiltModel>();
            m.set_hover_axis(axis);
            m.set_hover_tilt(tilt);
            m.set_hover_ready(false);
            m.set_hover_status("Updating".into());
            let serial = owner_request.0.latest.fetch_add(1, Ordering::AcqRel) + 1;
            let request = Request {
                serial,
                generation: RenderContext::lock(&ctx).scene_generation(),
                axis: axis_index,
                tilt,
                finished: crate::gui::editor::finished_stone_job(&ui, &ctx),
                diagnostics: m
                    .get_hover_diagnostics()
                    .then(|| crate::bridge::optical_overlay::presentation(&ui)),
                inspect: m.get_hover_inspect(),
            };
            *owner_request
                .0
                .pending
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(request);
            owner_request.0.wake.notify_one();
        });
    let weak = ui.as_weak();
    ui.global::<TiltModel>().on_cancel_hover(move || {
        let _keep_alive = &owner;
        owner.0.latest.fetch_add(1, Ordering::AcqRel);
        owner
            .0
            .pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(ui) = weak.upgrade() {
            ui.global::<TiltModel>().set_hover_ready(false);
        }
    });
}
#[cfg(test)]
#[path = "tilt_hover_preview_tests.rs"]
mod tests;
