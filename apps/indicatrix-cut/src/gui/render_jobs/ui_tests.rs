//! Tests of the render queue's Slint-free layer: the Jobs window's rows (J4-T1), the
//! controller's decisions (J4-T2), the script folder (J4-T3) and the "Add to Queue" helpers
//! (J4-T4). Nothing here needs a window, a database or a renderer.

use super::{
    capture_still::{plan_still_outputs, still_label, still_summary},
    capture_video::{VideoFacts, video_label, video_summary},
    controller::{after_finish, next_job},
    rows::{LiveProgress, queue_view, row_view},
    script_export::{BundleOptions, build_bundle, bundle_folder_name, write_bundle},
};
use crate::bridge::export_thread::ExportParams;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{fluorescence::Fluorescence, materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{
    SceneState,
    messages::{content_hash, hash_hex},
    scene::{HdrEnvironment, SceneEnvironment},
};
use indicatrix_render_jobs::{
    ComputeChoice, FailureKind, JobColorSpace, JobKind, JobOutcome, JobProgress, LocalEngines,
    RenderJobFile, StillJob, TiltVideoJob, TransferChoice, codec,
    job::{DesignInfo, HdrSource, JOB_FORMAT_VERSION, JobCompute, OverlaySelection},
    script::{RemoteArgs, ScriptFlavor},
    state::{APP_CLOSING_NOTE, INTERRUPTED_NOTE, JobState, StopReason},
};
use indicatrix_vault::model::render_job::RenderJobMeta;
use std::path::{Path, PathBuf};

// ---- Fixtures --------------------------------------------------------------------------

fn meta(id: i64, kind: &str, state: &str, done: u32, total: u32) -> RenderJobMeta {
    RenderJobMeta {
        job_id: id,
        position: id,
        kind: kind.to_string(),
        label: format!("Job {id}"),
        summary: "A summary".to_string(),
        state: state.to_string(),
        frames_done: done,
        frames_total: total,
        output_path: "/out/gem.png".to_string(),
        result_path: None,
        error_text: None,
        created_at: 1,
        updated_at: 1,
        started_at: None,
        finished_at: None,
    }
}

fn still(id: i64, state: &str) -> RenderJobMeta {
    meta(id, "still", state, 0, 1)
}

fn video(id: i64, state: &str, done: u32) -> RenderJobMeta {
    meta(id, "tilt_video", state, done, 181)
}

fn progress(done: u32, total: u32, fraction: f32, eta: Option<f64>) -> JobProgress {
    JobProgress {
        frames_done: done,
        frames_total: total,
        fraction,
        eta_secs: eta,
        note: None,
    }
}

/// The buttons of a row as a word list, in the window's order.
fn buttons(row: &super::rows::RowView) -> Vec<&'static str> {
    [
        (row.can_run_next, "run_next"),
        (row.can_move_up, "up"),
        (row.can_move_down, "down"),
        (row.can_pause, "pause"),
        (row.can_resume, "resume"),
        (row.can_restart, "restart"),
        (row.can_cancel, "cancel"),
        (row.can_show, "show"),
        (row.can_delete, "delete"),
    ]
    .into_iter()
    .filter_map(|(on, word)| on.then_some(word))
    .collect()
}

/// A lone row (so neither Up nor Down shows).
fn single(meta: &RenderJobMeta, live: Option<&JobProgress>) -> super::rows::RowView {
    row_view(meta, 0, 1, live)
}

// ---- J4-T1: rows --------------------------------------------------------------------------

#[test]
fn a_waiting_still_row() {
    let row = single(&still(7, "queued"), None);
    assert_eq!(row.id, 7);
    assert_eq!(row.number_text, "1");
    assert_eq!(row.label, "Job 7");
    assert_eq!(row.kind_text, "Still image");
    assert_eq!(row.state_word, "queued");
    assert_eq!(row.state_text, "Queued");
    assert_eq!(row.summary, "A summary");
    assert_eq!(row.progress_text, "Waiting");
    assert_eq!(row.progress, -1.0);
    assert_eq!(row.error_text, "");
    assert_eq!(buttons(&row), ["run_next", "pause", "cancel", "delete"]);
}

#[test]
fn a_running_still_row_shows_percent_and_time_left() {
    let live = progress(0, 1, 0.42, Some(190.0));
    let row = single(&still(1, "running"), Some(&live));
    assert_eq!(row.state_text, "Running");
    assert_eq!(row.progress_text, "42% \u{b7} about 3 min left");
    assert_eq!(row.progress, 0.42);
    assert_eq!(buttons(&row), ["pause", "cancel"]);

    let no_estimate = progress(0, 1, 0.42, None);
    assert_eq!(
        single(&still(1, "running"), Some(&no_estimate)).progress_text,
        "42%"
    );
    let starting = single(&still(1, "running"), None);
    assert_eq!(starting.progress_text, "Starting");
    assert_eq!(starting.progress, 0.0);
}

#[test]
fn a_paused_still_starts_again_from_the_beginning() {
    let row = single(&still(1, "paused"), None);
    assert_eq!(
        row.progress_text,
        "Paused \u{b7} starts again from the beginning"
    );
    assert_eq!(row.progress, -1.0);
    assert_eq!(
        buttons(&row),
        ["run_next", "resume", "restart", "cancel", "delete"]
    );

    let mut interrupted = still(1, "paused");
    interrupted.error_text = Some(INTERRUPTED_NOTE.to_string());
    assert_eq!(
        single(&interrupted, None).progress_text,
        format!("Paused \u{b7} starts again from the beginning \u{b7} {INTERRUPTED_NOTE}")
    );
    assert_eq!(single(&interrupted, None).error_text, "");
}

#[test]
fn a_done_still_names_its_file_and_can_be_shown() {
    let mut done = still(1, "done");
    done.result_path = Some("/out/gem (2).png".to_string());
    let row = single(&done, None);
    assert_eq!(row.progress_text, "Saved: gem (2).png");
    assert_eq!(row.progress, 1.0);
    assert!(row.can_show);
    assert_eq!(buttons(&row), ["restart", "show", "delete"]);

    // Without a recorded result there is nothing to show.
    assert!(!single(&still(1, "done"), None).can_show);
}

#[test]
fn a_failed_row_carries_the_error_in_red_text() {
    let mut failed = still(1, "failed");
    failed.error_text = Some("The HDR map is missing.".to_string());
    let row = single(&failed, None);
    assert_eq!(row.state_text, "Failed");
    assert_eq!(row.error_text, "The HDR map is missing.");
    assert_eq!(row.progress_text, "");
    assert_eq!(buttons(&row), ["run_next", "resume", "restart", "delete"]);

    let no_text = single(&still(1, "failed"), None);
    assert_eq!(no_text.error_text, "The job failed.");
}

#[test]
fn a_cancelled_row_can_only_be_restarted_or_deleted() {
    let row = single(&still(1, "cancelled"), None);
    assert_eq!(row.progress_text, "Cancelled");
    assert_eq!(row.progress, -1.0);
    assert_eq!(buttons(&row), ["restart", "delete"]);
}

#[test]
fn tilt_video_rows_speak_in_frames() {
    let live = progress(36, 181, 0.2, Some(720.0));
    let running = single(&video(1, "running", 36), Some(&live));
    assert_eq!(running.kind_text, "Tilt video");
    assert_eq!(
        running.progress_text,
        "Frame 37 of 181 \u{b7} about 12 min left"
    );
    assert_eq!(running.progress, 0.2);
    assert_eq!(
        single(&video(1, "running", 0), None).progress_text,
        "Frame 1 of 181"
    );

    let waiting_fresh = single(&video(1, "queued", 0), None);
    assert_eq!(waiting_fresh.progress_text, "Waiting");
    assert_eq!(waiting_fresh.progress, -1.0);
    let waiting_partway = single(&video(1, "queued", 36), None);
    assert_eq!(
        waiting_partway.progress_text,
        "Waiting \u{b7} 36 of 181 frames done"
    );
    assert_eq!(waiting_partway.progress, 36.0_f32 / 181.0_f32);

    let paused = single(&video(1, "paused", 36), None);
    assert_eq!(paused.progress_text, "Paused at frame 37 of 181");
    assert_eq!(paused.progress, 36.0_f32 / 181.0_f32);
    assert_eq!(
        single(&video(1, "paused", 0), None).progress_text,
        "Paused at frame 1 of 181"
    );

    let mut failed = video(1, "failed", 5);
    failed.error_text = Some("A frame failed.".to_string());
    let failed = single(&failed, None);
    assert_eq!(failed.progress_text, "5 of 181 frames done");
    assert_eq!(failed.error_text, "A frame failed.");

    let mut done = video(1, "done", 181);
    done.result_path = Some("/out/video/tilt.mp4".to_string());
    assert_eq!(single(&done, None).progress_text, "Saved: tilt.mp4");
}

#[test]
fn only_the_first_row_hides_up_and_only_the_last_hides_down() {
    let metas = [still(1, "queued"), still(2, "queued"), still(3, "queued")];
    let first = row_view(&metas[0], 0, 3, None);
    let middle = row_view(&metas[1], 1, 3, None);
    let last = row_view(&metas[2], 2, 3, None);
    assert_eq!((first.can_move_up, first.can_move_down), (false, true));
    assert_eq!((middle.can_move_up, middle.can_move_down), (true, true));
    assert_eq!((last.can_move_up, last.can_move_down), (true, false));
    assert_eq!(
        (first.number_text, middle.number_text, last.number_text),
        ("1".to_string(), "2".to_string(), "3".to_string())
    );
}

#[test]
fn an_unknown_state_word_gives_a_neutral_row_with_only_delete() {
    let row = row_view(&still(4, "frozen"), 1, 3, None);
    assert_eq!(row.state_word, "unknown");
    assert_eq!(row.state_text, "Unknown");
    assert_eq!(row.progress, -1.0);
    assert_eq!(row.progress_text, "");
    assert_eq!(row.error_text, "");
    assert_eq!(buttons(&row), ["delete"]);
}

#[test]
fn the_chip_is_hidden_with_no_jobs_and_when_only_finished_jobs_remain() {
    let empty = queue_view(&[], false, None);
    assert_eq!(empty.rows, Vec::<super::rows::RowView>::new());
    assert!(!empty.chip_visible);
    assert!(!empty.can_start_queue);
    assert_eq!(empty.queue_status, "");
    assert_eq!(empty.chip_progress, -1.0);

    let finished = queue_view(&[still(1, "done"), still(2, "cancelled")], false, None);
    assert!(!finished.chip_visible);
    assert!(!finished.can_start_queue);
    assert_eq!(finished.queue_status, "1 done");

    // A started queue shows the chip even when the last job just finished.
    let started = queue_view(&[still(1, "done")], true, None);
    assert!(started.chip_visible);
    assert!(started.queue_running);
    assert_eq!(started.chip_text, "Jobs");
}

#[test]
fn waiting_jobs_show_the_chip_and_the_queue_line() {
    let metas = [still(1, "done"), still(2, "queued"), still(3, "queued")];
    let view = queue_view(&metas, false, None);
    assert!(view.chip_visible);
    assert!(view.can_start_queue);
    assert_eq!(view.chip_text, "Jobs: 2 waiting");
    assert_eq!(view.chip_progress, -1.0);
    assert_eq!(view.queue_status, "2 waiting \u{b7} 1 done");
    assert_eq!(view.rows.len(), 3);
}

#[test]
fn a_running_job_gives_the_chip_its_place_and_percent() {
    let metas = [
        still(1, "done"),
        still(2, "running"),
        still(3, "queued"),
        still(4, "queued"),
        still(5, "paused"),
    ];
    let live = LiveProgress {
        job_id: 2,
        progress: progress(0, 1, 0.42, Some(60.0)),
    };
    let view = queue_view(&metas, true, Some(&live));
    assert!(view.chip_visible);
    assert_eq!(view.chip_text, "Rendering 2 of 4 \u{b7} 42%");
    assert_eq!(view.chip_progress, 0.42);
    assert_eq!(
        view.queue_status,
        "1 running \u{b7} 2 waiting \u{b7} 1 paused \u{b7} 1 done"
    );
    assert_eq!(view.rows[1].progress_text, "42% \u{b7} about 1 min left");

    // Progress of another job is ignored, and a running job without progress shows 0%.
    let other = LiveProgress {
        job_id: 9,
        progress: progress(0, 1, 0.9, None),
    };
    let view = queue_view(&metas, true, Some(&other));
    assert_eq!(view.chip_text, "Rendering 2 of 4 \u{b7} 0%");
    assert_eq!(view.chip_progress, 0.0);
    assert_eq!(view.rows[1].progress_text, "Starting");
}

// ---- J4-T2: the controller's decisions ---------------------------------------------------

fn done_outcome() -> JobOutcome {
    JobOutcome::Done {
        result: PathBuf::from("/out/a.png"),
        note: None,
    }
}

fn failed_outcome(frames_done: u32) -> JobOutcome {
    JobOutcome::Failed {
        kind: FailureKind::Render,
        message: "boom".to_string(),
        frames_done,
    }
}

#[test]
fn a_finished_run_is_done_and_the_queue_goes_on() {
    let plan = after_finish(&done_outcome(), None, true, 181);
    assert_eq!(plan.new_state, JobState::Done);
    assert_eq!(plan.frames_done, 181);
    assert_eq!(
        plan.result_path,
        Some(PathBuf::from("/out/a.png").to_string_lossy().into_owned())
    );
    assert_eq!(plan.error_text, None);
    assert!(plan.start_next);
    assert!(plan.queue_running);
}

#[test]
fn pause_queue_and_a_closing_app_stop_the_queue_after_any_ending() {
    for stop in [StopReason::PauseQueue, StopReason::AppClosing] {
        let done = after_finish(&done_outcome(), Some(stop), true, 1);
        assert_eq!(done.new_state, JobState::Done);
        assert!(!done.start_next);
        assert!(!done.queue_running);

        let failed = after_finish(&failed_outcome(2), Some(stop), true, 10);
        assert_eq!(failed.new_state, JobState::Failed);
        assert!(!failed.start_next);
    }
}

#[test]
fn a_queue_that_was_not_running_starts_nothing_after_a_run() {
    let plan = after_finish(&done_outcome(), None, false, 1);
    assert_eq!(plan.new_state, JobState::Done);
    assert!(!plan.start_next);
    assert!(!plan.queue_running);
}

#[test]
fn a_failed_run_keeps_its_message_and_frames() {
    let plan = after_finish(&failed_outcome(3), None, true, 181);
    assert_eq!(plan.new_state, JobState::Failed);
    assert_eq!(plan.frames_done, 3);
    assert_eq!(plan.error_text.as_deref(), Some("boom"));
    assert_eq!(plan.result_path, None);
    assert!(plan.start_next);
}

#[test]
fn a_stopped_run_follows_the_spec_table() {
    let stopped = JobOutcome::Stopped { frames_done: 36 };
    // (why it was stopped, new state, the queue goes on, the note left)
    let table = [
        (Some(StopReason::Pause), JobState::Paused, true, None),
        (Some(StopReason::PauseQueue), JobState::Paused, false, None),
        (Some(StopReason::Cancel), JobState::Cancelled, true, None),
        (
            Some(StopReason::AppClosing),
            JobState::Paused,
            false,
            Some(APP_CLOSING_NOTE),
        ),
        (None, JobState::Paused, true, None),
    ];
    for (stop, state, goes_on, note) in table {
        let plan = after_finish(&stopped, stop, true, 181);
        assert_eq!(plan.new_state, state, "{stop:?}");
        assert_eq!(plan.start_next, goes_on, "{stop:?}");
        assert_eq!(plan.queue_running, goes_on, "{stop:?}");
        assert_eq!(plan.frames_done, 36, "{stop:?}");
        assert_eq!(plan.result_path, None, "{stop:?}");
        assert_eq!(plan.error_text.as_deref(), note, "{stop:?}");
    }
}

#[test]
fn the_next_job_is_the_first_waiting_one_and_unknown_states_are_skipped() {
    let metas = [
        still(1, "done"),
        still(2, "frozen"),
        still(3, "paused"),
        still(4, "queued"),
        still(5, "queued"),
    ];
    assert_eq!(next_job(&metas), Some(4));
    assert_eq!(next_job(&metas[..3]), None);
    assert_eq!(next_job(&[]), None);
    // A "Run Next" row (moved to the top) goes first.
    let moved = [metas[4].clone(), metas[3].clone()];
    assert_eq!(next_job(&moved), Some(5));
}

// ---- J4-T3: the script folder ----------------------------------------------------------

const TOKEN_A: &str = "9f2c4a7d1e0b48c3a65d2f71c8e4b903";
const TOKEN_B: &str = "1a2b3c4d5e6f47a8b9c0d1e2f3a4b5c6";

fn scene(hash: Option<[u8; 32]>) -> SceneState {
    SceneState {
        width: 64,
        height: 48,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: hash.map_or(SceneEnvironment::Studio, |content_hash| {
            SceneEnvironment::Hdr(HdrEnvironment {
                content_hash,
                width: 64,
                height: 32,
            })
        }),
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

fn file(
    token: &str,
    label: &str,
    output: &str,
    hdr: Option<(&str, [u8; 32])>,
    kind: JobKind,
) -> RenderJobFile {
    RenderJobFile {
        format: JOB_FORMAT_VERSION,
        token: token.to_string(),
        label: label.to_string(),
        created_at: 1_791_302_580,
        design: DesignInfo::default(),
        scene: scene(hdr.map(|(_, hash)| hash)),
        hdr: hdr.map(|(path, hash)| HdrSource {
            path: path.to_string(),
            sha256_hex: hash_hex(&hash),
        }),
        compute: JobCompute {
            target: ComputeChoice::Local,
            transfer: TransferChoice::FullData,
            contribute_local: false,
        },
        output: output.to_string(),
        kind,
    }
}

fn still_kind() -> JobKind {
    JobKind::Still(StillJob {
        samples_per_pixel: 8,
        color_space: JobColorSpace::Srgb,
        preset_label: String::new(),
    })
}

fn video_kind() -> JobKind {
    JobKind::TiltVideo(TiltVideoJob {
        axis_index: 1,
        start_deg: -90.0,
        end_deg: 90.0,
        step_deg: 1.0,
        total_frames: 181,
        fps: 30,
        samples_per_pixel: 8,
        color_space: JobColorSpace::Srgb,
        overlay: OverlaySelection::default(),
        curves: None,
        keep_frames: false,
        video_name: "round".to_string(),
    })
}

fn options(outputs_beside: bool, flavors: Vec<ScriptFlavor>) -> BundleOptions {
    BundleOptions {
        flavors,
        engines: LocalEngines::Cpu,
        remote: Some(RemoteArgs {
            address: "render-box.local:7878".to_string(),
            cert_dir: "/certs".to_string(),
        }),
        outputs_beside,
        header_note: "Exported 2026-10-06 14:03 UTC by Indicatrix Cut 0.0.0.".to_string(),
    }
}

/// A still and a video that share one HDR map.
fn two_jobs(hash: [u8; 32]) -> Vec<RenderJobFile> {
    let hdr = Some(("/hdr/studio.hdr", hash));
    vec![
        file(
            TOKEN_A,
            "Round still",
            "/renders/gem.png",
            hdr,
            still_kind(),
        ),
        file(
            TOKEN_B,
            "Round video",
            "/renders/frames-45",
            hdr,
            video_kind(),
        ),
    ]
}

fn text_of<'a>(bundle: &'a super::script_export::Bundle, relative: &str) -> &'a [u8] {
    // `Path` equality ignores the separator, so `jobs/x` finds `jobs\x` on Windows.
    &bundle
        .files
        .iter()
        .find(|(path, _)| path == Path::new(relative))
        .unwrap_or_else(|| panic!("{relative} is not in the bundle"))
        .1
}

/// The job file `relative` of the bundle, read back and validated.
fn job_of(bundle: &super::script_export::Bundle, relative: &str) -> RenderJobFile {
    codec::from_text(std::str::from_utf8(text_of(bundle, relative)).unwrap()).unwrap()
}

#[test]
fn the_bundle_rewrites_paths_and_copies_each_hdr_map_once() {
    let hash = [7_u8; 32];
    let sha = hash_hex(&hash);
    let both = vec![ScriptFlavor::PowerShell, ScriptFlavor::Sh];
    let bundle = build_bundle(two_jobs(hash), &options(true, both)).unwrap();

    let names: Vec<PathBuf> = bundle.files.iter().map(|(path, _)| path.clone()).collect();
    assert_eq!(
        names,
        [
            Path::new("jobs").join("01-round-still.job.json"),
            Path::new("jobs").join("02-round-video.job.json"),
            PathBuf::from("run-render-jobs.ps1"),
            PathBuf::from("run-render-jobs.sh"),
        ]
    );

    // One HDR copy for the two jobs, into jobs/assets.
    assert_eq!(
        bundle.hdr_copies,
        [(
            PathBuf::from("/hdr/studio.hdr"),
            Path::new("jobs").join("assets").join(format!("{sha}.hdr")),
            sha.clone()
        )]
    );

    // Each job file reads back, with its paths relative to the jobs folder.
    let first = job_of(&bundle, "jobs/01-round-still.job.json");
    assert_eq!(first.output, "../renders/gem.png");
    assert_eq!(
        first.hdr.as_ref().unwrap().path,
        format!("assets/{sha}.hdr")
    );
    assert_eq!(first.token, TOKEN_A);
    let second = job_of(&bundle, "jobs/02-round-video.job.json");
    assert_eq!(second.output, "../renders/frames-45");
    assert_eq!(
        second.hdr.as_ref().unwrap().path,
        format!("assets/{sha}.hdr")
    );

    // Both scripts list both jobs, in order, with the right command.
    let ps1 = String::from_utf8_lossy(text_of(&bundle, "run-render-jobs.ps1")).into_owned();
    let sh = String::from_utf8_lossy(text_of(&bundle, "run-render-jobs.sh")).into_owned();
    for script in [&ps1, &sh] {
        let render = script
            .find("'render', 'jobs/01-round-still.job.json'")
            .or_else(|| script.find("run_job render 'jobs/01-round-still.job.json'"));
        let tilt = script
            .find("'tilt-video', 'jobs/02-round-video.job.json'")
            .or_else(|| script.find("run_job tilt-video 'jobs/02-round-video.job.json'"));
        assert!(render.is_some(), "{script}");
        assert!(tilt.is_some(), "{script}");
        assert!(render < tilt, "{script}");
    }
    assert!(text_of(&bundle, "run-render-jobs.ps1").starts_with(&[0xEF, 0xBB, 0xBF]));
    assert!(!text_of(&bundle, "run-render-jobs.sh").starts_with(&[0xEF, 0xBB, 0xBF]));
}

#[test]
fn one_flavor_writes_one_script_and_outputs_stay_when_not_beside() {
    let bundle = build_bundle(
        two_jobs([7_u8; 32]),
        &options(false, vec![ScriptFlavor::Sh]),
    )
    .unwrap();
    let scripts: Vec<PathBuf> = bundle
        .files
        .iter()
        .map(|(path, _)| path.clone())
        .filter(|path| path.extension().is_some_and(|e| e == "sh" || e == "ps1"))
        .collect();
    assert_eq!(scripts, [PathBuf::from("run-render-jobs.sh")]);

    let first = job_of(&bundle, "jobs/01-round-still.job.json");
    assert_eq!(first.output, "/renders/gem.png");
}

#[test]
fn two_outputs_with_one_name_stay_apart_beside_the_script() {
    let jobs = vec![
        file(TOKEN_A, "One", "/a/gem.png", None, still_kind()),
        file(TOKEN_B, "Two", "/b/GEM.png", None, still_kind()),
    ];
    let bundle = build_bundle(jobs, &options(true, vec![ScriptFlavor::Sh])).unwrap();
    let outputs: Vec<String> = ["01-one", "02-two"]
        .iter()
        .map(|slug| job_of(&bundle, &format!("jobs/{slug}.job.json")).output)
        .collect();
    assert_eq!(outputs, ["../renders/gem.png", "../renders/GEM (2).png"]);
}

#[test]
fn a_bundle_of_no_jobs_is_refused() {
    let error = build_bundle(Vec::new(), &options(false, vec![ScriptFlavor::Sh])).unwrap_err();
    assert_eq!(error, "There are no unfinished jobs to export.");
}

#[test]
fn the_bundle_folder_is_named_by_date_and_time() {
    assert_eq!(
        bundle_folder_name((2026, 10, 6, 14, 3)),
        "render-jobs-2026-10-06-1403"
    );
    assert_eq!(
        bundle_folder_name((2027, 1, 12, 0, 59)),
        "render-jobs-2027-01-12-0059"
    );
}

#[test]
fn writing_a_bundle_copies_the_map_checks_its_hash_and_never_reuses_a_folder() {
    let base = std::env::temp_dir().join(format!(
        "indicatrix-cut-script-export-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let map = base.join("studio.hdr");
    let bytes = b"not really an hdr map".to_vec();
    std::fs::write(&map, &bytes).unwrap();
    let hash = content_hash(&bytes);
    let map_text = map.to_string_lossy().into_owned();
    let make_jobs = || {
        vec![file(
            TOKEN_A,
            "Round still",
            "/renders/gem.png",
            Some((&map_text, hash)),
            still_kind(),
        )]
    };
    let now = 1_791_295_380; // 2026-10-06 14:03:00 UTC (day 20732, 14 h 3 min)
    let options = options(false, vec![ScriptFlavor::Sh]);

    let (folder, count) = write_bundle(&base, make_jobs(), &options, now).unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        folder.file_name().unwrap().to_string_lossy(),
        "render-jobs-2026-10-06-1403"
    );
    assert!(folder.join("run-render-jobs.sh").is_file());
    assert!(
        folder
            .join("jobs")
            .join("01-round-still.job.json")
            .is_file()
    );
    let copied = folder
        .join("jobs")
        .join("assets")
        .join(format!("{}.hdr", hash_hex(&hash)));
    assert_eq!(std::fs::read(&copied).unwrap(), bytes);

    // The same minute again makes a second folder instead of writing into the first.
    let (second, _) = write_bundle(&base, make_jobs(), &options, now).unwrap();
    assert_eq!(
        second.file_name().unwrap().to_string_lossy(),
        "render-jobs-2026-10-06-1403 (2)"
    );

    // A map that changed since the job was added is refused, and nothing is left behind.
    std::fs::write(&map, b"changed").unwrap();
    let error = write_bundle(&base, make_jobs(), &options, now).unwrap_err();
    assert!(error.contains("changed since the job was added"), "{error}");
    assert!(!base.join("render-jobs-2026-10-06-1403 (3)").exists());

    let _ = std::fs::remove_dir_all(&base);
}

// ---- J4-T4: "Add to Queue" helpers ----------------------------------------------------

#[test]
fn still_outputs_are_reserved_against_the_disk_the_queue_and_each_other() {
    let dir = Path::new("/out");
    let names = [
        "gem.png".to_string(),
        "gem.png".to_string(),
        "other.png".to_string(),
        "gem.png".to_string(),
    ];
    // `other.png` is already the output of a queued job; `gem.png` exists on disk.
    let taken = [dir.join("other.png")];
    let on_disk = dir.join("gem.png");
    let planned = plan_still_outputs(dir, &names, &taken, &|path| path == on_disk);
    assert_eq!(
        planned,
        [
            dir.join("gem (2).png"),
            dir.join("gem (3).png"),
            dir.join("other (2).png"),
            dir.join("gem (4).png"),
        ]
    );

    // With nothing in the way, every name is kept.
    let free = plan_still_outputs(
        dir,
        &["a.png".to_string(), "b.png".to_string()],
        &[],
        &|_| false,
    );
    assert_eq!(free, [dir.join("a.png"), dir.join("b.png")]);
}

#[test]
fn still_labels_and_summaries_use_the_plain_wording() {
    assert_eq!(
        still_label("Barion Oval", "", 1920, 1080),
        "Barion Oval \u{b7} Current view \u{b7} 1920\u{d7}1080"
    );
    assert_eq!(
        still_label("Barion Oval", "Studio", 3840, 2160),
        "Barion Oval \u{b7} Studio \u{b7} 3840\u{d7}2160"
    );
    let params = ExportParams {
        width: 1920,
        height: 1080,
        samples_per_pixel: 1024,
        max_bounces: 12,
    };
    assert_eq!(
        still_summary(&params, "sRGB", "Daylight"),
        "1920 \u{d7} 1080 \u{b7} 1024 samples \u{b7} 12 bounces \u{b7} sRGB \u{b7} Daylight"
    );
}

#[test]
fn video_labels_and_summaries_use_the_plain_wording() {
    assert_eq!(
        video_label("Barion Oval", 45.0),
        "Barion Oval \u{b7} tilt video \u{b7} axis 45\u{b0}"
    );
    let facts = VideoFacts {
        start_deg: -90.0,
        end_deg: 90.0,
        step_deg: 1.0,
        frames: 181,
        fps: 30,
        width: 1280,
        height: 720,
        samples: 256,
    };
    assert_eq!(
        video_summary(&facts),
        "-90\u{b0} to 90\u{b0} in 1\u{b0} steps \u{b7} 181 frames \u{b7} 30 fps \u{b7} 1280 \u{d7} 720 \u{b7} 256 samples"
    );
    let fine = VideoFacts {
        step_deg: 0.5,
        start_deg: 0.0,
        end_deg: 45.5,
        frames: 92,
        ..facts
    };
    assert!(video_summary(&fine).starts_with("0\u{b0} to 45.5\u{b0} in 0.5\u{b0} steps"));
}
