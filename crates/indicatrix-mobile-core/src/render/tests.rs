//! Tests for the governor, the worker pool, the epochs, the restart rule and the export.
//!
//! They run on the standard round brilliant template at tiny rasters (32 x 24) and a low
//! sample target (8 per pixel), so the whole file takes a few seconds. Every wait is
//! bounded.

use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    time::{Duration, Instant},
};

use indicatrix_cut_core::templates::TEMPLATES;
use indicatrix_editor::EditorSession;
use indicatrix_web_core::{render::handle_trace_chunk, scene::OwnedScene};

use super::{
    CANCELLED, ExportColorSpace, ExportRequest, GovernorInputs, PauseReason, RenderFrame,
    RenderStatus, RenderView, base_workers, export_job, govern, live_target_spp, needs_restart,
    scene, start,
};
use crate::{
    app_state::{AppState, Lifecycle},
    design_props::{LightValues, lighting_rows, set_light_values, set_lighting},
    design_state::{Installed, begin_solve, install, run_solve, set_angle_from_text},
    platform::Platform,
    platform_mock::MockPlatform,
    settings::{RenderQuality, ThemeChoice},
};

/// The traced raster of the pool tests.
const SMALL: RenderView = RenderView {
    width: 32,
    height: 24,
};

/// The longest any test waits for the render.
const WAIT: Duration = Duration::from_secs(120);

/// The Crown Main tier of the template.
const CROWN: usize = 4;

// ---- helpers -----------------------------------------------------------------------------

/// A state over the round brilliant template with a small live sample target, no denoise,
/// not solved yet.
fn unsolved_state() -> AppState {
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state.settings.render.live_spp_compact = 8;
    state.settings.render.denoise = false;
    state
}

fn solve_now(state: &mut AppState) {
    let job = begin_solve(state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(state, job.generation, response, Duration::from_millis(5));
    assert_eq!(installed, Installed::Applied);
}

fn solved_state() -> AppState {
    let mut state = unsolved_state();
    solve_now(&mut state);
    state
}

/// Polls the status until `ready` holds. Fails on a failed render or after [`WAIT`].
fn wait_until(state: &AppState, what: &str, ready: impl Fn(&RenderStatus) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = state.render.status();
        if ready(&status) {
            return;
        }
        assert!(
            !matches!(status, RenderStatus::Failed(_)),
            "the render failed while waiting for {what}: {status:?}"
        );
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; status {status:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn wait_done(state: &AppState) {
    wait_until(state, "the render to finish", |status| {
        matches!(status, RenderStatus::Done { .. })
    });
}

/// Polls frames until one satisfies `wanted`; every frame seen goes through `seen`.
fn wait_frame(
    state: &mut AppState,
    what: &str,
    mut seen: impl FnMut(&RenderFrame),
    wanted: impl Fn(&RenderFrame) -> bool,
) -> RenderFrame {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Some(frame) = state.render.take_frame() {
            seen(&frame);
            if wanted(&frame) {
                return frame;
            }
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn final_frame(state: &mut AppState) -> RenderFrame {
    let target = live_target_spp(state);
    wait_frame(
        state,
        "the final frame",
        |_| {},
        |frame| frame.samples == target,
    )
}

// ---- governor -------------------------------------------------------------------------

fn decide(
    cores: usize,
    headroom: Option<f32>,
    foreground: bool,
    battery_saver: bool,
) -> super::GovernorDecision {
    govern(&GovernorInputs {
        cores,
        thermal_headroom: headroom,
        foreground,
        battery_saver,
    })
}

#[test]
fn base_workers_leave_two_cores_and_stay_between_one_and_six() {
    assert_eq!(base_workers(0), 1);
    assert_eq!(base_workers(1), 1);
    assert_eq!(base_workers(2), 1);
    assert_eq!(base_workers(4), 2);
    assert_eq!(base_workers(8), 6);
    assert_eq!(base_workers(64), 6);
}

#[test]
fn a_cool_foreground_device_runs_all_workers() {
    for headroom in [None, Some(0.0), Some(0.5), Some(0.849)] {
        let decision = decide(8, headroom, true, false);
        assert_eq!(decision.workers, 6, "{headroom:?}");
        assert_eq!(decision.paused, None);
        assert!(!decision.draft_only);
    }
    assert_eq!(decide(8, Some(f32::NAN), true, false).workers, 6);
}

#[test]
fn warm_devices_halve_then_drop_to_one_worker() {
    assert_eq!(decide(8, Some(0.85), true, false).workers, 3);
    assert_eq!(decide(8, Some(0.94), true, false).workers, 3);
    assert_eq!(decide(8, Some(0.95), true, false).workers, 1);
    assert_eq!(decide(8, Some(1.0), true, false).workers, 1);
    assert_eq!(decide(8, Some(1.0), true, false).paused, None);
    assert_eq!(decide(4, Some(0.9), true, false).workers, 1);
}

#[test]
fn a_throttling_device_pauses_tracing() {
    let decision = decide(8, Some(1.01), true, false);
    assert_eq!(decision.paused, Some(PauseReason::Hot));
    assert_eq!(decision.workers, 0);
}

#[test]
fn the_background_pauses_everything_and_wins_over_heat() {
    for headroom in [None, Some(0.2), Some(1.5)] {
        let decision = decide(8, headroom, false, false);
        assert_eq!(
            decision.paused,
            Some(PauseReason::Background),
            "{headroom:?}"
        );
        assert_eq!(decision.workers, 0);
    }
}

#[test]
fn battery_saver_caps_two_workers_and_asks_for_draft() {
    let decision = decide(8, None, true, true);
    assert_eq!(decision.workers, 2);
    assert!(decision.draft_only);
    assert_eq!(decide(2, None, true, true).workers, 1);
    assert_eq!(decide(8, Some(0.9), true, true).workers, 2);
    assert_eq!(decide(8, Some(0.96), true, true).workers, 1);
    assert_eq!(
        decide(8, Some(1.2), true, true).paused,
        Some(PauseReason::Hot)
    );
}

#[test]
fn the_live_target_follows_quality_window_class_and_battery_saver() {
    let mut state = unsolved_state();
    state.settings.render.live_spp_compact = 64;
    state.settings.render.live_spp_large = 128;
    assert_eq!(live_target_spp(&state), 64);
    state.settings.render.quality = RenderQuality::Fine;
    assert_eq!(live_target_spp(&state), 128);
    state.settings.render.quality = RenderQuality::Draft;
    assert_eq!(live_target_spp(&state), 32);
    state.settings.render.quality = RenderQuality::Normal;
    state.set_window_width(900.0);
    assert_eq!(live_target_spp(&state), 128);
    state.settings.battery_saver = true;
    assert_eq!(live_target_spp(&state), 32);
}

// ---- the pool --------------------------------------------------------------------------

#[test]
fn an_unsolved_design_is_not_rendered() {
    let mut state = unsolved_state();
    let message = start(&mut state, SMALL).expect_err("nothing is solved");
    assert!(message.contains("Solve the design"), "{message}");
    assert_eq!(state.render.status(), RenderStatus::Failed(message));
    assert!(state.render.take_frame().is_none());
    // The same answer again is no reason to start again.
    assert!(!needs_restart(&state));
    solve_now(&mut state);
    assert!(needs_restart(&state));
}

#[test]
fn the_pool_sums_equal_a_direct_trace_of_the_same_samples() {
    let mut state = solved_state();
    start(&mut state, SMALL).expect("starts");
    wait_done(&state);
    let (samples, sum) = state.render.debug_sum().expect("the epoch is kept");
    assert_eq!(samples, live_target_spp(&state));

    let prepared = scene::prepare(&state, SMALL).expect("a solved design prepares");
    let scene = OwnedScene::build(&prepared.key.spec, prepared.hdr).expect("builds");
    let reference = handle_trace_chunk(&scene, scene.plane_soa(), 0, 1, 0, samples);
    assert_eq!(sum.len(), reference.len());
    for (index, (got, want)) in sum.iter().zip(&reference).enumerate() {
        // The pool adds the samples pass by pass, the reference in one go: equal up to
        // the order of the float additions.
        let tolerance = 1e-4 * (1.0 + want.abs().max_element());
        assert!(
            (*got - *want).abs().max_element() <= tolerance,
            "pixel {index}: pool {got:?}, reference {want:?}"
        );
    }
}

#[test]
fn a_finished_render_delivers_its_frame_once() {
    let mut state = solved_state();
    let epoch = start(&mut state, SMALL).expect("starts");
    let frame = final_frame(&mut state);
    assert_eq!(frame.epoch, epoch);
    assert_eq!((frame.width, frame.height), (32, 24));
    assert_eq!(frame.rgba.len(), 32 * 24 * 4);
    assert_eq!(frame.samples, frame.target);
    assert!(!frame.denoised);
    assert!(
        frame
            .rgba
            .chunks(4)
            .any(|pixel| pixel[..3].iter().any(|&c| c > 0)),
        "the picture is not black"
    );
    assert!(state.render.take_frame().is_none(), "each frame comes once");
    wait_done(&state);
    assert_eq!(
        state.render.status(),
        RenderStatus::Done {
            samples: frame.target
        }
    );
}

#[test]
fn a_new_epoch_drops_the_old_frames() {
    let mut state = solved_state();
    let first = start(&mut state, SMALL).expect("starts");
    wait_until(&state, "the first epoch to merge a pass", |status| {
        matches!(
            status,
            RenderStatus::Tracing { samples, .. } if *samples > 0
        ) || matches!(status, RenderStatus::Done { .. })
    });
    state.viewport.camera.yaw += 0.4;
    let second = start(&mut state, SMALL).expect("restarts");
    assert_ne!(first, second);
    let target = live_target_spp(&state);
    let last = wait_frame(
        &mut state,
        "the second epoch's final frame",
        |frame| assert_eq!(frame.epoch, second, "a frame of the old epoch came through"),
        |frame| frame.samples == target,
    );
    assert_eq!(last.epoch, second);
}

#[test]
fn stopping_idles_the_pool_and_asks_for_a_restart() {
    let mut state = solved_state();
    start(&mut state, SMALL).expect("starts");
    wait_done(&state);
    state.render.stop();
    assert_eq!(state.render.status(), RenderStatus::Idle);
    assert!(state.render.take_frame().is_none());
    assert!(needs_restart(&state));
    start(&mut state, SMALL).expect("starts again");
    assert!(!needs_restart(&state));
    wait_done(&state);
}

#[test]
fn needs_restart_follows_the_scene_inputs_and_nothing_else() {
    let mut state = solved_state();
    assert!(needs_restart(&state), "nothing runs yet");
    start(&mut state, SMALL).expect("starts");
    assert!(!needs_restart(&state));
    assert!(!needs_restart(&state));

    // Not part of the picture.
    state.settings.theme = ThemeChoice::Dark;
    state.selected_tier = Some(0);
    assert!(!needs_restart(&state));

    // The camera.
    let camera = state.viewport.camera;
    state.viewport.camera.yaw += 0.2;
    assert!(needs_restart(&state));
    state.viewport.camera = camera;
    assert!(!needs_restart(&state));
    state.viewport.camera.distance *= 1.1;
    assert!(needs_restart(&state));
    state.viewport.camera = camera;

    // The lighting rig and the light.
    let lighting = state.settings.lighting.clone();
    let other_rig = lighting_rows()
        .into_iter()
        .map(|row| row.name)
        .find(|name| *name != lighting.preset_name)
        .expect("more than one rig is offered");
    set_lighting(&mut state, &other_rig).expect("a known rig");
    assert!(needs_restart(&state));
    state.settings.lighting = lighting.clone();
    assert!(!needs_restart(&state));
    set_light_values(
        &mut state,
        LightValues {
            exposure: lighting.exposure,
            azimuth_deg: lighting.azimuth_deg + 40.0,
            elevation_deg: lighting.elevation_deg,
            head_shadow_deg: lighting.head_shadow_deg,
            surface_glare_pct: lighting.surface_glare_pct,
        },
    );
    assert!(needs_restart(&state));
    state.settings.lighting = lighting;
    assert!(!needs_restart(&state));

    // The sample target.
    state.settings.render.quality = RenderQuality::Fine;
    assert!(needs_restart(&state));
    state.settings.render.quality = RenderQuality::Normal;
    assert!(!needs_restart(&state));
}

#[test]
fn needs_restart_follows_the_material() {
    let mut state = solved_state();
    state.props.link_material = false;
    state.settings.material_name = "Diamond".to_owned();
    start(&mut state, SMALL).expect("starts");
    assert!(!needs_restart(&state));
    state.settings.material_name = "Sapphire".to_owned();
    assert!(needs_restart(&state));
    state.settings.material_name = "Diamond".to_owned();
    assert!(!needs_restart(&state));
}

#[test]
fn editing_the_design_asks_for_a_restart_once_it_is_solved_again() {
    let mut state = solved_state();
    start(&mut state, SMALL).expect("starts");
    assert!(!needs_restart(&state));
    let _outcome = set_angle_from_text(&mut state, CROWN, "41.0");
    assert!(needs_restart(&state), "the solve is stale");
    solve_now(&mut state);
    assert!(needs_restart(&state), "the planes changed");
    start(&mut state, SMALL).expect("starts again");
    assert!(!needs_restart(&state));
    wait_done(&state);
}

#[test]
fn pausing_holds_tracing_and_resuming_finishes_it() {
    let mut state = solved_state();
    state.render.set_user_paused(true);
    assert!(state.render.user_paused());
    start(&mut state, SMALL).expect("starts");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        state.render.status(),
        RenderStatus::Paused(PauseReason::User)
    );
    assert!(state.render.take_frame().is_none());
    state.render.set_user_paused(false);
    wait_done(&state);
}

#[test]
fn the_background_pauses_and_the_foreground_continues() {
    let mut state = solved_state();
    state.render.lifecycle(Lifecycle::Paused);
    start(&mut state, SMALL).expect("starts");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        state.render.status(),
        RenderStatus::Paused(PauseReason::Background)
    );
    state.render.lifecycle(Lifecycle::Resumed);
    wait_done(&state);
}

#[test]
fn a_hot_device_pauses_and_a_cooler_one_continues() {
    let mut state = solved_state();
    state.render.govern(Some(1.2), false);
    start(&mut state, SMALL).expect("starts");
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(
        state.render.status(),
        RenderStatus::Paused(PauseReason::Hot)
    );
    // One worker is enough to finish.
    state.render.govern(Some(0.96), false);
    wait_done(&state);
}

#[test]
fn shutdown_joins_the_threads_in_bounded_time_and_refuses_new_work() {
    let mut state = solved_state();
    start(&mut state, SMALL).expect("starts");
    let began = Instant::now();
    state.render.shutdown();
    assert!(
        began.elapsed() < Duration::from_secs(5),
        "shutdown took {:?}",
        began.elapsed()
    );
    state.render.shutdown();
    let message = start(&mut state, SMALL).expect_err("the renderer is closed");
    assert!(message.contains("shut down"), "{message}");
}

#[test]
fn the_settled_picture_is_denoised_and_the_toggle_swaps_it() {
    let mut state = solved_state();
    state.settings.render.denoise = true;
    start(&mut state, SMALL).expect("starts");
    let denoised = wait_frame(
        &mut state,
        "a denoised frame",
        |_| {},
        |frame| frame.denoised,
    );
    assert_eq!(denoised.samples, denoised.target);
    assert_eq!(denoised.rgba.len(), 32 * 24 * 4);
    state.render.set_denoise(false);
    let plain = wait_frame(
        &mut state,
        "the plain frame",
        |_| {},
        |frame| !frame.denoised,
    );
    assert_eq!(plain.samples, plain.target);
    state.render.set_denoise(true);
    let again = wait_frame(
        &mut state,
        "the denoised frame again",
        |_| {},
        |frame| frame.denoised,
    );
    assert_eq!(again.epoch, denoised.epoch);
}

// ---- export ---------------------------------------------------------------------------------

fn request(long_edge_px: u32, spp: u32, color_space: ExportColorSpace) -> ExportRequest {
    ExportRequest {
        long_edge_px,
        spp,
        color_space,
    }
}

/// Whether `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn an_unsolved_design_has_no_export() {
    let state = unsolved_state();
    let result = export_job(&state, &request(32, 16, ExportColorSpace::Srgb));
    assert!(result.is_err());
}

#[test]
fn export_writes_a_png_of_the_requested_size_with_an_icc_profile() {
    let state = solved_state();
    let job = export_job(&state, &request(32, 16, ExportColorSpace::DisplayP3)).expect("a job");
    assert_eq!((job.width(), job.height(), job.spp()), (32, 24, 16));
    assert!(job.estimate_seconds().is_none(), "nothing was measured yet");

    let reports = Mutex::new(Vec::new());
    let png = job
        .run(
            &|fraction| reports.lock().expect("lock").push(fraction),
            &AtomicBool::new(false),
        )
        .expect("the export succeeds");
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(
        u32::from_be_bytes(png[16..20].try_into().expect("4 bytes")),
        32
    );
    assert_eq!(
        u32::from_be_bytes(png[20..24].try_into().expect("4 bytes")),
        24
    );
    assert!(contains(&png, b"iCCP"), "Display P3 carries its profile");

    let reports = reports.lock().expect("lock");
    assert_eq!(reports.first(), Some(&0.0));
    assert_eq!(reports.last(), Some(&1.0));
    assert!(reports.windows(2).all(|pair| pair[0] <= pair[1]));
    drop(reports);

    let srgb = export_job(&state, &request(32, 16, ExportColorSpace::Srgb)).expect("a job");
    let png = srgb
        .run(&|_| {}, &AtomicBool::new(false))
        .expect("the export succeeds");
    assert!(!contains(&png, b"iCCP"), "sRGB is untagged");
}

#[test]
fn export_obeys_the_caps_and_the_cancel_flag() {
    let state = solved_state();
    let job = export_job(&state, &request(4096, 4096, ExportColorSpace::Srgb)).expect("a job");
    assert_eq!((job.width(), job.height()), (1024, 768));
    assert_eq!(job.spp(), 256);

    let small = export_job(&state, &request(32, 16, ExportColorSpace::Srgb)).expect("a job");
    let result = small.run(&|_| {}, &AtomicBool::new(true));
    assert_eq!(result, Err(CANCELLED.to_owned()));
}

#[test]
fn export_estimates_its_time_from_the_live_render() {
    let mut state = solved_state();
    start(&mut state, SMALL).expect("starts");
    wait_done(&state);
    let job = export_job(&state, &request(32, 16, ExportColorSpace::Srgb)).expect("a job");
    let seconds = job
        .estimate_seconds()
        .expect("the live render was measured");
    assert!(seconds.is_finite() && seconds > 0.0, "{seconds}");
}
