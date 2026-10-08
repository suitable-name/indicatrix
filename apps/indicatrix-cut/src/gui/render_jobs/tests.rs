//! Tests of the job core: the snapshot round trip, resumable videos, the frame-folder
//! guards, the HDR checks and the still executor. Every render is CPU-only and tiny.

use super::{
    convert::{self, StillJobInputs},
    execute::{ExecContext, execute_job, remote_selection_for},
};
use crate::{
    bridge::{
        export_thread::{ExportOutcome, ExportParams, RemoteSelection, SceneSnapshot, run_export},
        render_thread::RenderContext,
    },
    gui::tilt::video_export::{frames, metrics, run::VideoExportRequest},
    settings::LocalComputeTarget,
};
use indicatrix::{
    color::ColorSpace,
    optics::{dispersion::DispersionModel, materials::GemMaterial},
};
use indicatrix_net::{
    messages::hash_hex,
    scene::{HdrEnvironment, SceneEnvironment},
};
use indicatrix_render_jobs::{
    ComputeChoice, FailureKind, JobOutcome, JobProgress, JobSink, RenderJobFile, TransferChoice,
    codec,
    job::{DesignInfo, HdrSource},
    paths::{FRAME_MARKER_FILE, marker_text},
};
use std::{
    path::{Path, PathBuf},
    sync::{
        Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

/// A fresh, empty folder for one test.
fn test_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "indicatrix_render_jobs_{name}_{}_{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn capture(ctx: RenderContext) -> SceneSnapshot {
    let mut snapshot = SceneSnapshot::capture(&Mutex::new(ctx)).expect("the material resolves");
    snapshot.max_bounces = 4;
    snapshot
}

/// A custom Sellmeier material with a frosted girdle and dimmed surface glare: the parts of
/// a scene that a naive conversion would lose.
fn custom_context() -> RenderContext {
    let material = GemMaterial::new_custom_with_dispersion(
        "Custom",
        DispersionModel::Sellmeier1 { b1: 1.4, c1: 0.012 },
        0.0,
        [0.0; 3],
    );
    RenderContext {
        material_override: Some(material),
        girdle_frosted: true,
        surface_glare: 0.25,
        ..RenderContext::default()
    }
}

fn still_inputs<'a>(
    scene: &'a SceneSnapshot,
    params: ExportParams,
    remote: &'a RemoteSelection,
    output: &'a Path,
) -> StillJobInputs<'a> {
    StillJobInputs {
        scene,
        params,
        color_space: ColorSpace::Srgb,
        remote,
        output,
        label: "Test job".to_string(),
        design: DesignInfo::default(),
        preset_label: String::new(),
    }
}

fn tiny_params() -> ExportParams {
    ExportParams {
        width: 16,
        height: 16,
        samples_per_pixel: 2,
        max_bounces: 4,
    }
}

fn assert_snapshots_equal(a: &SceneSnapshot, b: &SceneSnapshot) {
    let bits = |label: &str, x: f32, y: f32| {
        assert_eq!(x.to_bits(), y.to_bits(), "{label} differs: {x} vs {y}");
    };
    bits("yaw", a.yaw, b.yaw);
    bits("pitch", a.pitch, b.pitch);
    bits("distance", a.distance, b.distance);
    bits("light_yaw", a.light_yaw, b.light_yaw);
    bits("light_pitch", a.light_pitch, b.light_pitch);
    bits("exposure", a.exposure, b.exposure);
    bits("backdrop", a.backdrop, b.backdrop);
    bits("surface_glare", a.surface_glare, b.surface_glare);
    assert_eq!(a.max_bounces, b.max_bounces);
    assert_eq!(a.lighting_preset, b.lighting_preset);
    assert_eq!(a.material, b.material);
    // `Debug` prints the shortest text of each float, so equal text means equal bits.
    assert_eq!(format!("{:?}", a.material), format!("{:?}", b.material));
    assert_eq!(a.active_planes, b.active_planes);
    assert_eq!(a.tools, b.tools);
    assert_eq!(a.facet_finishes, b.facet_finishes);
    assert_eq!(*a.fluorescence, *b.fluorescence);
    assert_eq!(a.env_map.is_some(), b.env_map.is_some());
}

fn render_png(scene: &SceneSnapshot, path: &Path) -> Vec<u8> {
    let outcome = run_export(
        scene,
        tiny_params(),
        ColorSpace::Srgb,
        path,
        &RemoteSelection::local_only(),
        LocalComputeTarget::Cpu,
        &AtomicBool::new(false),
        |_| {},
    );
    assert!(
        matches!(outcome, ExportOutcome::Completed(_)),
        "the export must finish"
    );
    std::fs::read(path).unwrap()
}

/// J3-T1: a scene frozen into a job file and rebuilt renders the same picture, byte for
/// byte, for the default material and for a custom one with a frosted girdle.
#[test]
fn j3_t1_the_snapshot_round_trip_renders_identically() {
    let dir = test_dir("t1");
    for (name, ctx) in [
        ("default", RenderContext::default()),
        ("custom", custom_context()),
    ] {
        let original = capture(ctx);
        if name == "custom" {
            assert!(
                !original.facet_finishes.is_empty(),
                "the custom scene must carry a frosted girdle"
            );
        }
        let remote = RemoteSelection::local_only();
        let output = dir.join(format!("{name}-job.png"));
        let job = convert::build_still_job(
            &still_inputs(&original, tiny_params(), &remote, &output),
            1_791_302_580,
        )
        .expect("the job builds");
        let back = codec::from_text(&codec::to_text(&job).expect("the job encodes"))
            .expect("the job decodes");
        assert_eq!(back, job, "{name}: the job file must survive its own text");

        let rebuilt = convert::snapshot_from_scene(&back.scene, None);
        assert_snapshots_equal(&original, &rebuilt);

        let a = render_png(&original, &dir.join(format!("{name}-a.png")));
        let b = render_png(&rebuilt, &dir.join(format!("{name}-b.png")));
        assert_eq!(
            a, b,
            "{name}: the rebuilt scene must render the same picture"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn compute_choices_and_engines_map_both_ways() {
    for target in [
        crate::bridge::export_thread::ComputeTarget::LocalOnly,
        crate::bridge::export_thread::ComputeTarget::RemoteOnly,
        crate::bridge::export_thread::ComputeTarget::Both,
    ] {
        for transfer in [
            crate::settings::ExportTransfer::FullData,
            crate::settings::ExportTransfer::FinalPicture,
        ] {
            let selection = RemoteSelection {
                compute_target: target,
                worker: None,
                transfer,
                contribute_local: true,
            };
            let compute = convert::job_compute(&selection);
            assert_eq!(convert::remote_selection(compute, None), selection);
        }
    }
    for target in [
        LocalComputeTarget::Cpu,
        LocalComputeTarget::CpuGpu,
        LocalComputeTarget::Gpu,
    ] {
        assert_eq!(
            convert::local_target_of(convert::engines_of(target)),
            target
        );
    }
    for space in [ColorSpace::Srgb, ColorSpace::DisplayP3, ColorSpace::Rec2020] {
        let job_space = convert::job_color_space(space).unwrap();
        assert_eq!(convert::color_space_of(job_space), space);
    }
    assert_eq!(
        convert::job_color_space(ColorSpace::AcesCg).unwrap_err(),
        "ACEScg cannot be saved in an 8-bit picture."
    );
}

fn context(dir: &Path) -> ExecContext {
    ExecContext {
        base_dir: dir.to_path_buf(),
        local_compute: LocalComputeTarget::Cpu,
        worker: None,
        compute_override: None,
        transfer_override: None,
        contribute_local_override: None,
        output_override: None,
        restart_frames: false,
    }
}

/// A three-frame, 16 x 16, one-sample video job writing into `dir`.
fn video_job(dir: &Path, keep_frames: bool) -> RenderJobFile {
    video_job_with(dir, keep_frames, RemoteSelection::local_only())
}

/// [`video_job`] with the compute choices of `remote`.
fn video_job_with(dir: &Path, keep_frames: bool, remote: RemoteSelection) -> RenderJobFile {
    let request = VideoExportRequest {
        scene: capture(RenderContext::default()),
        axis_index: 0,
        start_deg: -2.0,
        end_deg: 2.0,
        step_deg: 2.0,
        total_frames: 3,
        fps: 30,
        width: 16,
        height: 16,
        samples_per_pixel: 1,
        color_space: ColorSpace::Srgb,
        out_dir: dir.to_path_buf(),
        out_name: "clip".to_string(),
        selection: metrics::MetricSelection::default(),
        curves: metrics::MetricCurves {
            brilliance: Vec::new(),
            windowing: Vec::new(),
            extinction: Vec::new(),
        },
        keep_frames,
        remote,
        local_compute: LocalComputeTarget::Cpu,
    };
    convert::build_video_job(
        &request,
        "Test video".to_string(),
        DesignInfo::default(),
        "clip".to_string(),
        0,
    )
    .expect("the video job builds")
}

/// Records what the executor reports, and can raise `cancel` after a given frame.
struct Recorder<'a> {
    finished: Vec<u32>,
    progress_ticks: usize,
    cancel_after: Option<(u32, &'a AtomicBool)>,
}

impl<'a> Recorder<'a> {
    const fn new() -> Self {
        Self {
            finished: Vec::new(),
            progress_ticks: 0,
            cancel_after: None,
        }
    }

    const fn cancelling_after(frames: u32, cancel: &'a AtomicBool) -> Self {
        Self {
            finished: Vec::new(),
            progress_ticks: 0,
            cancel_after: Some((frames, cancel)),
        }
    }
}

impl JobSink for Recorder<'_> {
    fn progress(&mut self, _progress: &JobProgress) {
        self.progress_ticks += 1;
    }

    fn frame_finished(&mut self, frames_done: u32, _frames_total: u32) {
        self.finished.push(frames_done);
        if let Some((after, cancel)) = self.cancel_after
            && frames_done == after
        {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

/// J3-T2: a video stopped after one frame keeps it, and the next run renders only the
/// missing frames, leaving the first frame's file untouched.
#[test]
fn j3_t2_a_resumed_video_skips_the_frames_already_on_disk() {
    let dir = test_dir("t2");
    let job = video_job(&dir, true);
    let ctx = context(&dir);

    let cancel = AtomicBool::new(false);
    let mut first = Recorder::cancelling_after(1, &cancel);
    let outcome = execute_job(&job, &ctx, &cancel, &mut first);
    assert_eq!(outcome, JobOutcome::Stopped { frames_done: 1 });
    assert_eq!(frames::existing_frames(&dir, 3), vec![true, false, false]);
    assert!(
        dir.join(FRAME_MARKER_FILE).is_file(),
        "the marker names the job"
    );
    let frame_one = frames::frame_path(&dir, 0, 3);
    let bytes = std::fs::read(&frame_one).unwrap();
    let modified = std::fs::metadata(&frame_one).unwrap().modified().unwrap();

    let mut second = Recorder::new();
    let outcome = execute_job(&job, &ctx, &AtomicBool::new(false), &mut second);
    let JobOutcome::Done { result, .. } = outcome else {
        panic!("the resumed video must finish, got {outcome:?}");
    };
    assert_eq!(
        second.finished,
        vec![2, 3],
        "only the missing frames render"
    );
    assert!(second.progress_ticks > 0);
    assert!(
        result.exists(),
        "the video, the GIF or the frame folder is reported"
    );
    assert_eq!(std::fs::read(&frame_one).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(&frame_one).unwrap().modified().unwrap(),
        modified,
        "the first frame must not be written again"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// J3-T3: the frame folder guards and the restart.
#[test]
fn j3_t3_the_frame_folder_guards_protect_other_exports() {
    // Another job's marker.
    let dir = test_dir("t3_marker");
    let job = video_job(&dir, true);
    std::fs::write(
        dir.join(FRAME_MARKER_FILE),
        marker_text("ffffffffffffffffffffffffffffffff"),
    )
    .unwrap();
    let outcome = execute_job(
        &job,
        &context(&dir),
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(
        matches!(&outcome, JobOutcome::Failed { kind: FailureKind::Input, message, .. }
            if message.contains("belongs to another render job")),
        "{outcome:?}"
    );

    // Frames from another export, no marker.
    let dir = test_dir("t3_foreign");
    let job = video_job(&dir, true);
    let foreign = frames::frame_path(&dir, 0, 3);
    std::fs::write(&foreign, b"not a picture").unwrap();
    let outcome = execute_job(
        &job,
        &context(&dir),
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(
        matches!(&outcome, JobOutcome::Failed { kind: FailureKind::Input, message, .. }
            if message.contains("already holds frames from another export")),
        "{outcome:?}"
    );
    assert_eq!(std::fs::read(&foreign).unwrap(), b"not a picture");

    // The same folder with a restart: the old frames go, the video renders.
    let restart = ExecContext {
        restart_frames: true,
        ..context(&dir)
    };
    let outcome = execute_job(
        &job,
        &restart,
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(matches!(outcome, JobOutcome::Done { .. }), "{outcome:?}");
    assert!(
        image::open(&foreign).is_ok(),
        "the foreign frame was replaced"
    );

    // A stray partial frame of this job's own folder is removed.
    let dir = test_dir("t3_partial");
    let job = video_job(&dir, true);
    std::fs::write(dir.join(FRAME_MARKER_FILE), marker_text(&job.token)).unwrap();
    let partial = dir.join("frame_1.png.partial");
    std::fs::write(&partial, b"half a frame").unwrap();
    let stopped = AtomicBool::new(true);
    let outcome = execute_job(&job, &context(&dir), &stopped, &mut Recorder::new());
    assert_eq!(outcome, JobOutcome::Stopped { frames_done: 0 });
    assert!(!partial.exists(), "the partial frame must be removed");
    let _ = std::fs::remove_dir_all(&dir);
}

fn write_test_hdr(dir: &Path) -> PathBuf {
    let pixels: Vec<image::Rgb<f32>> = (0..64)
        .map(|i| image::Rgb([(i as f32).mul_add(0.01, 0.2), 0.5, 0.25]))
        .collect();
    let mut bytes = Vec::new();
    image::codecs::hdr::HdrEncoder::new(&mut bytes)
        .encode(&pixels, 16, 4)
        .unwrap();
    let path = dir.join("studio.hdr");
    std::fs::write(&path, &bytes).unwrap();
    path
}

/// A still job whose scene names the HDR map `hash`, stored at `path`.
fn hdr_job(path: &Path, hash: &[u8; 32]) -> RenderJobFile {
    let remote = RemoteSelection::local_only();
    let output = path.with_extension("png");
    let mut job = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        0,
    )
    .unwrap();
    job.scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: *hash,
        width: 16,
        height: 4,
    });
    job.hdr = Some(HdrSource {
        path: path.to_string_lossy().into_owned(),
        sha256_hex: hash_hex(hash),
    });
    job.validate().expect("the HDR job is consistent");
    job
}

/// J3-T4: a missing HDR map and a changed one fail the job with a plain sentence.
#[test]
fn j3_t4_a_missing_or_changed_hdr_map_fails_the_job() {
    let dir = test_dir("t4");

    let missing = dir.join("missing.hdr");
    let job = hdr_job(&missing, &[7; 32]);
    let (kind, message) = convert::load_hdr(&job, &dir).unwrap_err();
    assert_eq!(kind, FailureKind::Input);
    assert_eq!(
        message,
        format!("The HDR map {} is missing.", missing.display())
    );
    let outcome = execute_job(
        &job,
        &context(&dir),
        &AtomicBool::new(false),
        &mut Recorder::new(),
    );
    assert!(
        matches!(&outcome, JobOutcome::Failed { kind: FailureKind::Input, message, .. }
            if message.ends_with("is missing.")),
        "{outcome:?}"
    );

    let path = write_test_hdr(&dir);
    let bytes = std::fs::read(&path).unwrap();
    let hash = indicatrix_net::messages::content_hash(&bytes);
    let job = hdr_job(&path, &hash);
    let loaded = convert::load_hdr(&job, &dir).expect("the unchanged map loads");
    assert!(loaded.is_some());

    let mut changed = bytes;
    changed.push(0);
    std::fs::write(&path, changed).unwrap();
    let (kind, message) = convert::load_hdr(&job, &dir).unwrap_err();
    assert_eq!(kind, FailureKind::Input);
    assert!(
        message.contains("changed since the job was added"),
        "{message}"
    );

    let mut no_hdr = job;
    no_hdr.hdr = None;
    no_hdr.scene.environment = SceneEnvironment::Studio;
    assert!(convert::load_hdr(&no_hdr, &dir).unwrap().is_none());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_still_job_renders_and_never_overwrites_an_existing_picture() {
    let dir = test_dir("still");
    let remote = RemoteSelection::local_only();
    let output = dir.join("gem.png");
    let job = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        0,
    )
    .unwrap();
    let ctx = context(&dir);

    let mut recorder = Recorder::new();
    let outcome = execute_job(&job, &ctx, &AtomicBool::new(false), &mut recorder);
    assert_eq!(
        outcome,
        JobOutcome::Done {
            result: output.clone(),
            note: None
        }
    );
    assert!(recorder.progress_ticks > 0);

    let outcome = execute_job(&job, &ctx, &AtomicBool::new(false), &mut Recorder::new());
    let JobOutcome::Done { result, .. } = outcome else {
        panic!("the second run must finish");
    };
    assert_eq!(result, dir.join("gem (2).png"));
    assert!(output.is_file() && result.is_file());

    let stopped = execute_job(&job, &ctx, &AtomicBool::new(true), &mut Recorder::new());
    assert_eq!(stopped, JobOutcome::Stopped { frames_done: 0 });
    let _ = std::fs::remove_dir_all(&dir);
}

/// A video queued with "Remote only" freezes that choice, keeps it through the job file's
/// text, and runs as `RemoteOnly` with the worker read at run time (never stored in the job).
#[test]
fn a_queued_video_keeps_its_compute_choice_through_the_job_file() {
    use crate::bridge::export_thread::ComputeTarget;
    let dir = test_dir("video_compute");
    let remote = RemoteSelection {
        compute_target: ComputeTarget::RemoteOnly,
        worker: None,
        transfer: crate::settings::ExportTransfer::FullData,
        contribute_local: false,
    };
    let job = video_job_with(&dir, false, remote);
    assert_eq!(job.compute.target, ComputeChoice::Remote);
    let back =
        codec::from_text(&codec::to_text(&job).expect("the job encodes")).expect("the job decodes");
    assert_eq!(back.compute.target, ComputeChoice::Remote);

    let worker = crate::settings::WorkerSettings {
        address: "render-box.local:7878".to_string(),
        ..crate::settings::WorkerSettings::default()
    };
    let ctx = ExecContext {
        worker: Some(worker.clone()),
        ..context(&dir)
    };
    let selection = remote_selection_for(&back, &ctx);
    assert_eq!(selection.compute_target, ComputeTarget::RemoteOnly);
    assert_eq!(selection.worker, Some(worker));

    // The default for a local video stays local.
    assert_eq!(video_job(&dir, false).compute.target, ComputeChoice::Local);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overrides_replace_the_jobs_compute_choices_and_the_worker_comes_from_the_context() {
    let dir = test_dir("override");
    let remote = RemoteSelection::local_only();
    let output = dir.join("gem.png");
    let job = convert::build_still_job(
        &still_inputs(
            &capture(RenderContext::default()),
            tiny_params(),
            &remote,
            &output,
        ),
        0,
    )
    .unwrap();
    assert_eq!(job.compute.target, ComputeChoice::Local);

    let plain = remote_selection_for(&job, &context(&dir));
    assert_eq!(plain, remote);

    let worker = crate::settings::WorkerSettings {
        address: "render-box.local:7878".to_string(),
        ..crate::settings::WorkerSettings::default()
    };
    let ctx = ExecContext {
        worker: Some(worker.clone()),
        compute_override: Some(ComputeChoice::Remote),
        transfer_override: Some(TransferChoice::FinalPicture),
        contribute_local_override: Some(true),
        ..context(&dir)
    };
    let overridden = remote_selection_for(&job, &ctx);
    assert_eq!(
        overridden.compute_target,
        crate::bridge::export_thread::ComputeTarget::RemoteOnly
    );
    assert_eq!(
        overridden.transfer,
        crate::settings::ExportTransfer::FinalPicture
    );
    assert!(overridden.contribute_local);
    assert_eq!(overridden.worker, Some(worker));
    let _ = std::fs::remove_dir_all(&dir);
}
