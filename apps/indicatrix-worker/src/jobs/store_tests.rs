//! Tests of the durable job store: idempotent submission, owner isolation, the limits, the
//! queue, cancellation, results, restart recovery and retention. No renderer is involved.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::{
    SceneState,
    messages::MAX_JOB_SAMPLES,
    scene::{HdrEnvironment, SceneEnvironment},
};
use std::sync::atomic::AtomicU64;

static UNIQUE: AtomicU64 = AtomicU64::new(0);
const NOW: i64 = 1_791_302_580;

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-jobs-{name}-{}-{}",
        std::process::id(),
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&dir);
    dir
}

fn scene() -> SceneState {
    SceneState {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        width: 16,
        height: 16,
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
        environment: SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
    }
}

fn submit(token: u8) -> JobSubmit {
    JobSubmit {
        client_token: [token; 16],
        label: format!("Job {token}"),
        scene: scene(),
        samples: 8,
        color_space: WireColorSpace::Srgb,
    }
}

fn open(name: &str, limits: Limits) -> (JobStore, PathBuf) {
    let dir = temp_dir(name);
    (JobStore::open(&dir, limits, NOW).unwrap(), dir)
}

#[test]
fn submitting_the_same_token_twice_returns_the_same_job() {
    let (store, dir) = open("idempotent", Limits::default());
    let first = store.submit("alice", &submit(1), NOW).unwrap();
    let again = store.submit("alice", &submit(1), NOW + 5).unwrap();
    assert_eq!(first, again);
    assert_eq!(first.phase, JobPhase::Queued);
    assert_eq!(
        (first.samples_total, first.width, first.height),
        (8, 16, 16)
    );
    assert_eq!(store.list("alice").len(), 1);
    // A different token is a different job.
    let other = store.submit("alice", &submit(2), NOW).unwrap();
    assert_ne!(other.job_id, first.job_id);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_stored_job_is_the_desktop_render_queue_job_file() {
    let (store, dir) = open("jobfile", Limits::default());
    let info = store.submit("alice", &submit(3), NOW).unwrap();
    let job = store.load_job(&info.job_id).unwrap();
    assert_eq!(job.token, job_id_hex(&info.job_id));
    assert_eq!(job.label, "Job 3");
    assert!(matches!(
        job.kind,
        JobKind::Still(StillJob {
            samples_per_pixel: 8,
            color_space: JobColorSpace::Srgb,
            ..
        })
    ));
    assert_eq!(job.scene, scene());
    assert!(job.hdr.is_none());
    assert_eq!(job.output, format!("{}.png", job_id_hex(&info.job_id)));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_hdr_scene_names_its_map_in_the_job_file() {
    let (store, dir) = open("hdr", Limits::default());
    let mut request = submit(4);
    request.scene.environment = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: [0xAB; 32],
        width: 64,
        height: 32,
    });
    let info = store.submit("alice", &request, NOW).unwrap();
    let job = store.load_job(&info.job_id).unwrap();
    let source = job.hdr.expect("the map is named");
    assert_eq!(source.sha256_hex, hash_hex(&[0xAB; 32]));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn owners_cannot_see_or_touch_each_others_jobs() {
    let (store, dir) = open("owners", Limits::default());
    let mine = store.submit("alice", &submit(5), NOW).unwrap();
    // Bob sending Alice's id learns nothing: the job is simply unknown to him.
    assert_eq!(store.status("bob", &mine.job_id), Err(JobError::Unknown));
    assert_eq!(
        store.cancel("bob", &mine.job_id, NOW),
        Err(JobError::Unknown)
    );
    assert_eq!(store.delete("bob", &mine.job_id), Err(JobError::Unknown));
    assert_eq!(store.result("bob", &mine.job_id), Err(JobError::Unknown));
    assert_eq!(store.list("bob"), []);
    // The same token from Bob is Bob's own job, with another id.
    let theirs = store.submit("bob", &submit(5), NOW).unwrap();
    assert_ne!(theirs.job_id, mine.job_id);
    assert!(store.status("alice", &mine.job_id).is_ok());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_limits_refuse_what_does_not_fit() {
    let limits = Limits {
        max_active_per_owner: 2,
        max_jobs: 3,
        ..Limits::default()
    };
    let (store, dir) = open("limits", limits);
    store.submit("alice", &submit(1), NOW).unwrap();
    store.submit("alice", &submit(2), NOW).unwrap();
    assert!(matches!(
        store.submit("alice", &submit(3), NOW),
        Err(JobError::Limit(_))
    ));
    // Another owner has room, until the store itself is full.
    store.submit("bob", &submit(1), NOW).unwrap();
    assert!(matches!(
        store.submit("carol", &submit(1), NOW),
        Err(JobError::Limit(_))
    ));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn an_invalid_submission_stores_nothing() {
    let (store, dir) = open("invalid", Limits::default());
    let mut tiny = submit(1);
    tiny.scene.width = 4;
    assert!(matches!(
        store.submit("alice", &tiny, NOW),
        Err(JobError::Invalid(_))
    ));
    let mut aces = submit(2);
    aces.color_space = WireColorSpace::AcesCg;
    assert!(matches!(
        store.submit("alice", &aces, NOW),
        Err(JobError::Invalid(_))
    ));
    let mut huge = submit(3);
    huge.scene.width = 8192;
    huge.scene.height = 8192;
    assert!(matches!(
        store.submit("alice", &huge, NOW),
        Err(JobError::Invalid(_))
    ));
    let mut many = submit(4);
    many.samples = MAX_JOB_SAMPLES;
    assert!(store.submit("alice", &many, NOW).is_ok());
    assert_eq!(store.list("alice").len(), 1);
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 2, "one job, two files");
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_queue_runs_oldest_first_and_a_job_runs_once() {
    let (store, dir) = open("queue", Limits::default());
    let a = store.submit("alice", &submit(1), NOW).unwrap();
    let b = store.submit("alice", &submit(2), NOW + 1).unwrap();
    let stop = AtomicBool::new(false);
    let first = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
    assert_eq!(first.id, a.job_id);
    assert_eq!(
        store.status("alice", &a.job_id).unwrap().phase,
        JobPhase::Running
    );
    store.finish_failed(&first.id, "no luck", NOW + 2);
    let second = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
    assert_eq!(second.id, b.job_id);
    // Nothing else is waiting: the wait ends with None, not a second claim of a running job.
    assert!(store.claim_next(&stop, Duration::from_millis(10)).is_none());
    let failed = store.status("alice", &a.job_id).unwrap();
    assert_eq!(failed.phase, JobPhase::Failed);
    assert_eq!(failed.note, "no luck");
    assert_eq!(failed.finished_unix, Some(NOW + 2));
    stop.store(true, Ordering::Release);
    assert!(store.claim_next(&stop, Duration::from_millis(10)).is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_finished_job_hands_out_its_picture_until_it_expires() {
    let limits = Limits {
        retention_secs: 100,
        ..Limits::default()
    };
    let (store, dir) = open("result", limits);
    let info = store.submit("alice", &submit(1), NOW).unwrap();
    assert!(matches!(
        store.result("alice", &info.job_id),
        Err(JobError::State(_))
    ));
    let stop = AtomicBool::new(false);
    let claim = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
    store.set_progress(&claim.id, 4);
    assert_eq!(store.status("alice", &info.job_id).unwrap().samples_done, 4);
    let png = b"\x89PNG fake".to_vec();
    store.finish_done(&claim.id, &png, NOW + 10).unwrap();

    let done = store.status("alice", &info.job_id).unwrap();
    assert_eq!(done.phase, JobPhase::Done);
    assert_eq!(done.samples_done, 8, "a finished job reports every sample");
    assert_eq!(done.result_len, Some(u32::try_from(png.len()).unwrap()));
    assert_eq!(done.expires_unix, Some(NOW + 110));
    let (info_again, bytes) = store.result("alice", &info.job_id).unwrap();
    assert_eq!(info_again, done);
    assert_eq!(bytes, png);

    assert_eq!(store.purge_expired(NOW + 109), 0);
    assert_eq!(store.purge_expired(NOW + 110), 1);
    assert_eq!(store.status("alice", &info.job_id), Err(JobError::Unknown));
    assert_eq!(
        fs::read_dir(&dir).unwrap().count(),
        0,
        "every file of the job is gone"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn cancelling_a_queued_job_ends_it_and_a_running_one_raises_its_flag() {
    let (store, dir) = open("cancel", Limits::default());
    let queued = store.submit("alice", &submit(1), NOW).unwrap();
    let running = store.submit("alice", &submit(2), NOW + 1).unwrap();
    let stop = AtomicBool::new(false);
    let claim = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
    assert_eq!(claim.id, queued.job_id);
    // The first job is running; the second is still queued and cancels at once.
    let cancelled = store.cancel("alice", &running.job_id, NOW + 2).unwrap();
    assert_eq!(cancelled.phase, JobPhase::Cancelled);
    assert!(store.claim_next(&stop, Duration::from_millis(10)).is_none());
    // The running one only gets its flag; the executor ends it.
    assert!(!claim.cancel.load(Ordering::Acquire));
    let info = store.cancel("alice", &queued.job_id, NOW + 3).unwrap();
    assert_eq!(info.phase, JobPhase::Running);
    assert!(claim.cancel.load(Ordering::Acquire));
    store.finish_cancelled(&claim.id, NOW + 4);
    assert_eq!(
        store.status("alice", &queued.job_id).unwrap().phase,
        JobPhase::Cancelled
    );
    // An ended job cannot be cancelled again, and a running one cannot be deleted.
    assert!(matches!(
        store.cancel("alice", &queued.job_id, NOW + 5),
        Err(JobError::State(_))
    ));
    assert!(store.delete("alice", &queued.job_id).is_ok());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_running_job_cannot_be_deleted() {
    let (store, dir) = open("delete-running", Limits::default());
    let info = store.submit("alice", &submit(1), NOW).unwrap();
    let stop = AtomicBool::new(false);
    let _claim = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
    assert!(matches!(
        store.delete("alice", &info.job_id),
        Err(JobError::State(_))
    ));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_restart_requeues_what_was_running_and_keeps_what_was_done() {
    let dir = temp_dir("restart");
    let (ran, waiting, finished);
    {
        let store = JobStore::open(&dir, Limits::default(), NOW).unwrap();
        ran = store.submit("alice", &submit(1), NOW).unwrap();
        finished = store.submit("alice", &submit(2), NOW + 1).unwrap();
        waiting = store.submit("alice", &submit(3), NOW + 2).unwrap();
        let stop = AtomicBool::new(false);
        let claim = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
        assert_eq!(claim.id, ran.job_id);
        store.set_progress(&claim.id, 5);
        // `ran` stays Running; `finished` completes.
        let second = store.claim_next(&stop, Duration::from_millis(10));
        // One job runs at a time in the real executor; the store itself tolerates two claims.
        let second = second.unwrap();
        assert_eq!(second.id, finished.job_id);
        store.finish_done(&second.id, b"png", NOW + 3).unwrap();
    }
    let store = JobStore::open(&dir, Limits::default(), NOW + 100).unwrap();
    let requeued = store.status("alice", &ran.job_id).unwrap();
    assert_eq!(requeued.phase, JobPhase::Queued);
    assert_eq!(requeued.samples_done, 0);
    assert_eq!(requeued.note, RESTART_NOTE);
    assert_eq!(
        store.status("alice", &finished.job_id).unwrap().phase,
        JobPhase::Done
    );
    assert_eq!(store.result("alice", &finished.job_id).unwrap().1, b"png");
    // The queue is rebuilt oldest first: the interrupted job, then the one that never ran.
    let stop = AtomicBool::new(false);
    assert_eq!(
        store
            .claim_next(&stop, Duration::from_millis(10))
            .unwrap()
            .id,
        ran.job_id
    );
    assert_eq!(
        store
            .claim_next(&stop, Duration::from_millis(10))
            .unwrap()
            .id,
        waiting.job_id
    );
    // Ownership survived the restart.
    assert_eq!(store.status("bob", &ran.job_id), Err(JobError::Unknown));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn start_up_repairs_a_damaged_directory() {
    let dir = temp_dir("repair");
    let (good, vanished, broken);
    {
        let store = JobStore::open(&dir, Limits::default(), NOW).unwrap();
        good = store.submit("alice", &submit(1), NOW).unwrap();
        vanished = store.submit("alice", &submit(2), NOW).unwrap();
        broken = store.submit("alice", &submit(3), NOW).unwrap();
        let stop = AtomicBool::new(false);
        let claim = store.claim_next(&stop, Duration::from_millis(10)).unwrap();
        store.finish_done(&claim.id, b"png", NOW).unwrap();
    }
    // The finished job lost its picture; a third lost its job file; a stray job file has no state.
    fs::remove_file(dir.join(format!("{}.png", job_id_hex(&good.job_id)))).unwrap();
    fs::remove_file(dir.join(format!("{}.job.json", job_id_hex(&broken.job_id)))).unwrap();
    fs::write(dir.join(format!("{}.job.json", "ab".repeat(16))), "{}").unwrap();
    fs::write(
        dir.join(format!("{}.state.json", "cd".repeat(16))),
        "not json",
    )
    .unwrap();
    fs::write(dir.join("leftover.png.tmp"), "x").unwrap();

    let store = JobStore::open(&dir, Limits::default(), NOW + 1).unwrap();
    let missing = store.status("alice", &good.job_id).unwrap();
    assert_eq!(missing.phase, JobPhase::Failed);
    assert!(missing.note.contains("missing"), "{}", missing.note);
    assert_eq!(
        store.status("alice", &broken.job_id),
        Err(JobError::Unknown),
        "a job whose file does not read is not served"
    );
    assert_eq!(
        store.status("alice", &vanished.job_id).unwrap().phase,
        JobPhase::Queued
    );
    let names: Vec<String> = fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        !names.iter().any(|name| name.starts_with("abab")
            || name.starts_with("cdcd")
            || Path::new(name)
                .extension()
                .is_some_and(|extension| extension == "tmp")),
        "{names:?}"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn the_state_file_round_trips_every_field() {
    let record = Record {
        owner: "certificate:ab".to_owned(),
        state: JobState::Done,
        samples_done: 8,
        samples_total: 8,
        width: 16,
        height: 16,
        label: "Round \"brilliant\"".to_owned(),
        created_unix: NOW,
        finished_unix: Some(NOW + 1),
        result_len: Some(1234),
        note: "a note".to_owned(),
    };
    let back = state_from_text(&state_to_text(&record)).unwrap();
    assert_eq!(back.owner, record.owner);
    assert_eq!(back.state, record.state);
    assert_eq!(back.label, record.label);
    assert_eq!(back.finished_unix, record.finished_unix);
    assert_eq!(back.result_len, record.result_len);
    assert_eq!(back.note, record.note);
    for bad in [
        "",
        "{}",
        r#"{"format": 3}"#,
        r#"{"format": 2}"#,
        r#"{"format": 1, "owner": "x", "state": "nope"}"#,
    ] {
        assert!(state_from_text(bad).is_none(), "{bad}");
    }
}

#[test]
fn the_state_file_is_written_as_format_two_and_reads_format_one() {
    let record = Record {
        owner: "o".to_owned(),
        state: JobState::Queued,
        samples_done: 0,
        samples_total: 8,
        width: 16,
        height: 16,
        label: "l".to_owned(),
        created_unix: NOW,
        finished_unix: None,
        result_len: None,
        note: String::new(),
    };
    let mut value: Value = serde_json::from_str(&state_to_text(&record)).unwrap();
    assert_eq!(value["format"], 2);
    value["format"] = serde_json::json!(1);
    assert!(state_from_text(&value.to_string()).is_some());
    value["format"] = serde_json::json!(3);
    assert!(state_from_text(&value.to_string()).is_none());
    value["format"] = serde_json::json!(0);
    assert!(state_from_text(&value.to_string()).is_none());
}

/// A store written under the card rule (state format 1, a job file without the backdrop
/// marker holding the old White card level 8.0) replays as the White tray of the file's own
/// preset and pose, not as an 8.0 emitter.
#[test]
fn a_format_one_store_converts_its_card_level_to_the_tray_on_read() {
    let (store, dir) = open("legacy-card", Limits::default());
    let info = store.submit("alice", &submit(7), NOW).unwrap();
    drop(store);
    // Rewrite both files the way the previous build wrote them.
    let job_path = dir.join(job_file_name(&info.job_id));
    let mut job: Value = serde_json::from_str(&fs::read_to_string(&job_path).unwrap()).unwrap();
    assert!(
        job.as_object_mut()
            .unwrap()
            .remove("scene_backdrop_model")
            .is_some()
    );
    job["scene"]["backdrop"] = serde_json::json!(8.0);
    fs::write(&job_path, job.to_string()).unwrap();
    let state_path = dir.join(state_file_name(&info.job_id));
    let mut state: Value = serde_json::from_str(&fs::read_to_string(&state_path).unwrap()).unwrap();
    state["format"] = serde_json::json!(1);
    fs::write(&state_path, state.to_string()).unwrap();

    let store = JobStore::open(&dir, Limits::default(), NOW).unwrap();
    assert_eq!(store.list("alice").len(), 1, "a format-1 state is read");
    let loaded = store.load_job(&info.job_id).unwrap();
    let reference = scene();
    let expected = indicatrix::render_setup::Backdrop::White.resolve(
        reference.lighting_preset,
        reference.light_yaw,
        reference.light_pitch,
    );
    assert_eq!(loaded.scene.backdrop.to_bits(), expected.to_bits());
    assert!(loaded.scene.backdrop < 8.0);
    assert_eq!(
        loaded.scene_backdrop_model,
        indicatrix_render_jobs::job::BACKDROP_MODEL_TRAY
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn a_ray_submission_keeps_the_engine_and_uses_job_format_two() {
    let mut submit = submit(91);
    submit.scene.engine.engine = indicatrix::renderer::engine::RenderEngine::RayTraced;
    submit.scene.engine.ray.aa_taps = 8;
    submit.samples = 8;
    let (store, dir) = open("ray-format", Limits::default());
    let info = store.submit("alice", &submit, NOW).unwrap();
    let file = store.load_job(&info.job_id).unwrap();
    assert_eq!(file.format, 2);
    assert_eq!(file.engine, submit.scene.engine);
    assert_eq!(file.scene.engine, submit.scene.engine);
    drop(store);
    std::fs::remove_dir_all(dir).unwrap();
}
