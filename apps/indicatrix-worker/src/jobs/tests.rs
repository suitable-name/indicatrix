//! End-to-end tests of the durable job protocol on the server side: real store, real asset
//! cache, real executor on the CPU, requests and replies through the wire codecs.

use super::*;
use crate::assets::AssetCache;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
    renderer::gpu_backend::GpuBackend,
};
use indicatrix_net::{
    SceneState, framing,
    messages::{
        AssetHeader, JobId, JobInfo, JobPhase, WireColorSpace, content_hash,
        envmap_asset::{encode_envmap_asset, envmap_asset_hash},
        read_job_response, read_job_result,
    },
    scene::{HdrEnvironment, SceneEnvironment},
};
use std::{io::Cursor, sync::atomic::AtomicU64, time::Instant};

static UNIQUE: AtomicU64 = AtomicU64::new(0);

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-jobsvc-{name}-{}-{}",
        std::process::id(),
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

/// Scripted input, captured output.
#[derive(Default)]
struct Duplex {
    input: Cursor<Vec<u8>>,
    out: Vec<u8>,
}

impl Read for Duplex {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for Duplex {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl TimeoutRead for Duplex {
    fn set_read_timeout(&mut self, _: Option<Duration>) -> std::io::Result<()> {
        Ok(())
    }
}

fn scene(environment: SceneEnvironment) -> SceneState {
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
        environment,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
    }
}

fn submit(token: u8, environment: SceneEnvironment) -> JobRequest {
    JobRequest::Submit(Box::new(JobSubmit {
        client_token: [token; 16],
        label: format!("Job {token}"),
        scene: scene(environment),
        samples: 2,
        color_space: WireColorSpace::Srgb,
    }))
}

fn service(name: &str) -> JobService {
    let store = JobStore::open(&temp_dir(name), Limits::default(), unix_now()).unwrap();
    let cache = AssetCache::open(&temp_dir(&format!("{name}-assets")), 1 << 26).unwrap();
    JobService::new(store, Some(Arc::new(cache)))
}

/// Serves `request` (with `payload` as the frame that follows it, if any) and returns what was
/// written back, plus whether the whole input was consumed.
fn call(
    service: Option<&JobService>,
    owner: Option<&str>,
    request: JobRequest,
    payload: Option<&[u8]>,
) -> (Vec<u8>, bool) {
    let mut wire = Vec::new();
    if let Some(bytes) = payload {
        framing::write_frame(&mut wire, bytes).unwrap();
    }
    let mut stream = Duplex {
        input: Cursor::new(wire),
        out: Vec::new(),
    };
    serve_job(&mut stream, request, service, owner).unwrap();
    let consumed = stream.input.position() == stream.input.get_ref().len() as u64;
    (stream.out, consumed)
}

fn respond(service: &JobService, owner: &str, request: JobRequest) -> JobResponse {
    let (out, _) = call(Some(service), Some(owner), request, None);
    read_job_response(&mut Cursor::new(out)).unwrap()
}

fn accepted(response: JobResponse) -> JobInfo {
    match response {
        JobResponse::Accepted(info) => *info,
        other => panic!("expected Accepted, got {other:?}"),
    }
}

fn error_code(response: &JobResponse) -> u32 {
    match response {
        JobResponse::Error(error) => error.code,
        other => panic!("expected an error, got {other:?}"),
    }
}

fn wait_finished(service: &JobService, owner: &str, id: &JobId) -> JobInfo {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let info = service.store().status(owner, id).unwrap();
        if info.phase.is_terminal() {
            return info;
        }
        assert!(
            Instant::now() < deadline,
            "the job did not finish: {info:?}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn start_executor(service: &Arc<JobService>) -> JoinHandle<()> {
    service
        .spawn_executor(RenderLane {
            prefer_gpu: false,
            gpu: Arc::new(GpuBackend::disabled()),
            threads: 2,
        })
        .unwrap()
}

#[test]
fn a_job_is_submitted_rendered_fetched_and_deleted() {
    let service = Arc::new(service("lifecycle"));
    let first = accepted(respond(
        &service,
        "alice",
        submit(1, SceneEnvironment::Studio),
    ));
    assert_eq!(first.phase, JobPhase::Queued);
    assert_eq!(first.job_id, derive_job_id("alice", &[1; 16]));
    // Without an executor nothing runs: the picture is not ready, and asking says so.
    let (out, _) = call(
        Some(&service),
        Some("alice"),
        JobRequest::Fetch {
            job_id: first.job_id,
        },
        None,
    );
    assert_eq!(
        error_code(&read_job_response(&mut Cursor::new(out)).unwrap()),
        error_codes::JOB_STATE
    );
    // Submitting the same token again is the same job.
    let again = accepted(respond(
        &service,
        "alice",
        submit(1, SceneEnvironment::Studio),
    ));
    assert_eq!(again.job_id, first.job_id);

    let executor = start_executor(&service);
    let finished = wait_finished(&service, "alice", &first.job_id);
    assert_eq!(finished.phase, JobPhase::Done, "{finished:?}");
    assert_eq!((finished.samples_done, finished.samples_total), (2, 2));

    // The picture comes back as a Result followed by exactly its PNG.
    let (out, consumed) = call(
        Some(&service),
        Some("alice"),
        JobRequest::Fetch {
            job_id: first.job_id,
        },
        None,
    );
    assert!(consumed);
    let mut cursor = Cursor::new(out);
    let JobResponse::Result(info) = read_job_response(&mut cursor).unwrap() else {
        panic!("expected a Result");
    };
    let png = read_job_result(&mut cursor, &info).unwrap();
    let decoded = image::load_from_memory(&png).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (16, 16));
    assert_eq!(cursor.position(), cursor.get_ref().len() as u64);

    // Status, list and delete.
    let JobResponse::Info(status) = respond(
        &service,
        "alice",
        JobRequest::Status {
            job_id: first.job_id,
        },
    ) else {
        panic!("expected Info");
    };
    assert_eq!(*status, finished);
    let JobResponse::List(jobs) = respond(&service, "alice", JobRequest::List) else {
        panic!("expected a List");
    };
    assert_eq!(jobs, vec![finished]);
    assert_eq!(
        respond(
            &service,
            "alice",
            JobRequest::Delete {
                job_id: first.job_id
            }
        ),
        JobResponse::Deleted {
            job_id: first.job_id
        }
    );
    assert_eq!(
        error_code(&respond(
            &service,
            "alice",
            JobRequest::Status {
                job_id: first.job_id
            }
        )),
        error_codes::JOB_UNKNOWN
    );

    service.stop();
    executor.join().unwrap();
}

#[test]
fn one_owner_cannot_reach_another_owners_job_over_the_protocol() {
    let service = service("owners");
    let mine = accepted(respond(
        &service,
        "alice",
        submit(1, SceneEnvironment::Studio),
    ));
    for request in [
        JobRequest::Status {
            job_id: mine.job_id,
        },
        JobRequest::Fetch {
            job_id: mine.job_id,
        },
        JobRequest::Cancel {
            job_id: mine.job_id,
        },
        JobRequest::Delete {
            job_id: mine.job_id,
        },
    ] {
        assert_eq!(
            error_code(&respond(&service, "bob", request)),
            error_codes::JOB_UNKNOWN
        );
    }
    let JobResponse::List(none) = respond(&service, "bob", JobRequest::List) else {
        panic!("expected a List");
    };
    assert_eq!(none, []);
    // Alice still has her job, and cancelling it works for her alone.
    let JobResponse::Info(cancelled) = respond(
        &service,
        "alice",
        JobRequest::Cancel {
            job_id: mine.job_id,
        },
    ) else {
        panic!("expected Info");
    };
    assert_eq!(cancelled.phase, JobPhase::Cancelled);
}

#[test]
fn an_hdr_job_asks_for_its_float_map_and_renders_with_it() {
    let service = Arc::new(service("hdr"));
    let texels: Vec<[f32; 3]> = (0..32_u32)
        .map(|i| [(i as f32).mul_add(0.5, 1.0), 0.5, 2.0 / (1.0 + i as f32)])
        .collect();
    let asset = encode_envmap_asset(8, 4, &texels).unwrap();
    let environment = SceneEnvironment::Hdr(HdrEnvironment {
        content_hash: envmap_asset_hash(8, 4, &texels).unwrap(),
        width: 8,
        height: 4,
    });

    // The server does not have the map: it says which one it needs and stores nothing.
    let needed = respond(&service, "alice", submit(9, environment));
    assert_eq!(
        needed,
        JobResponse::NeedAsset {
            content_hash: content_hash(&asset)
        }
    );
    assert_eq!(service.store().list("alice"), []);

    // The upload is verified, stored and acknowledged; the same submission is then accepted.
    let header = AssetHeader {
        content_hash: content_hash(&asset),
        len: u32::try_from(asset.len()).unwrap(),
    };
    let (out, consumed) = call(
        Some(&service),
        Some("alice"),
        JobRequest::PutAsset(header),
        Some(&asset),
    );
    assert!(consumed);
    assert_eq!(
        read_job_response(&mut Cursor::new(out)).unwrap(),
        JobResponse::AssetStored {
            content_hash: header.content_hash
        }
    );
    let info = accepted(respond(&service, "alice", submit(9, environment)));

    let executor = start_executor(&service);
    let finished = wait_finished(&service, "alice", &info.job_id);
    assert_eq!(finished.phase, JobPhase::Done, "{finished:?}");
    service.stop();
    executor.join().unwrap();
}

#[test]
fn a_tampered_asset_upload_is_refused_and_not_kept() {
    let service = service("tamper");
    let asset = encode_envmap_asset(1, 1, &[[1.0, 1.0, 1.0]]).unwrap();
    let lying = AssetHeader {
        content_hash: [0x11; 32],
        len: u32::try_from(asset.len()).unwrap(),
    };
    let (out, consumed) = call(
        Some(&service),
        Some("alice"),
        JobRequest::PutAsset(lying),
        Some(&asset),
    );
    assert!(
        consumed,
        "the payload frame is consumed so the stream stays in sync"
    );
    assert_eq!(
        error_code(&read_job_response(&mut Cursor::new(out)).unwrap()),
        error_codes::ASSET_FAILED
    );
    assert!(
        service
            .assets
            .as_deref()
            .is_some_and(|cache| !cache.contains(&[0x11; 32]))
    );

    // A float asset whose header lies about its size is refused before it is stored.
    let mut broken = asset;
    broken[12..16].copy_from_slice(&5_u32.to_le_bytes());
    let header = AssetHeader {
        content_hash: content_hash(&broken),
        len: u32::try_from(broken.len()).unwrap(),
    };
    let (out, _) = call(
        Some(&service),
        Some("alice"),
        JobRequest::PutAsset(header),
        Some(&broken),
    );
    assert_eq!(
        error_code(&read_job_response(&mut Cursor::new(out)).unwrap()),
        error_codes::ASSET_FAILED
    );
    assert!(
        service
            .assets
            .as_deref()
            .is_some_and(|cache| !cache.contains(&header.content_hash))
    );
}

#[test]
fn a_server_without_jobs_refuses_everything_and_stays_in_sync() {
    let asset = b"IXENVMAP".to_vec();
    let header = AssetHeader {
        content_hash: content_hash(&asset),
        len: u32::try_from(asset.len()).unwrap(),
    };
    // A server with no job service (no own render lane) refuses, and consumes the upload.
    let (out, consumed) = call(
        None,
        Some("alice"),
        JobRequest::PutAsset(header),
        Some(&asset),
    );
    assert!(consumed);
    assert_eq!(
        error_code(&read_job_response(&mut Cursor::new(out)).unwrap()),
        error_codes::UNSUPPORTED_REQUEST
    );
    // A connection with no viewer identity is refused even by a server that has jobs.
    let real = service("refuse");
    let (out, _) = call(Some(&real), None, JobRequest::List, None);
    assert_eq!(
        error_code(&read_job_response(&mut Cursor::new(out)).unwrap()),
        error_codes::UNSUPPORTED_REQUEST
    );
}

#[test]
fn invalid_submissions_are_validation_errors_and_store_nothing() {
    let service = service("invalid");
    let mut request = submit(1, SceneEnvironment::Studio);
    if let JobRequest::Submit(submit) = &mut request {
        submit.samples = 0;
    }
    assert_eq!(
        error_code(&respond(&service, "alice", request)),
        error_codes::VALIDATION_FAILED
    );
    let mut bad_scene = submit(2, SceneEnvironment::Studio);
    if let JobRequest::Submit(submit) = &mut bad_scene {
        submit.scene.planes[0].normal = [0.0, 0.0, 0.0];
    }
    assert_eq!(
        error_code(&respond(&service, "alice", bad_scene)),
        error_codes::VALIDATION_FAILED
    );
    assert_eq!(service.store().list("alice"), []);
}

#[test]
fn the_per_owner_limit_is_reported_as_job_limit() {
    let store = JobStore::open(
        &temp_dir("limit"),
        Limits {
            max_active_per_owner: 1,
            ..Limits::default()
        },
        unix_now(),
    )
    .unwrap();
    let service = JobService::new(store, None);
    accepted(respond(
        &service,
        "alice",
        submit(1, SceneEnvironment::Studio),
    ));
    assert_eq!(
        error_code(&respond(
            &service,
            "alice",
            submit(2, SceneEnvironment::Studio)
        )),
        error_codes::JOB_LIMIT
    );
}

#[test]
fn an_in_flight_job_request_is_ignored_but_its_payload_is_consumed() {
    let asset = vec![7_u8; 100];
    let header = AssetHeader {
        content_hash: content_hash(&asset),
        len: 100,
    };
    let mut wire = Vec::new();
    framing::write_frame(&mut wire, &asset).unwrap();
    let mut input = Cursor::new(wire);
    ignore_in_flight(&mut input, &JobRequest::PutAsset(header)).unwrap();
    assert_eq!(input.position(), input.get_ref().len() as u64);
    // Anything else carries no payload and is simply dropped.
    ignore_in_flight(&mut Cursor::new(Vec::new()), &JobRequest::List).unwrap();
}

#[test]
fn the_job_directory_and_retention_come_from_the_environment_defaults() {
    // Only the defaults are asserted: the environment of a parallel test run is not ours.
    if std::env::var_os(JOB_DIR_ENV).is_none() {
        let (dir, _) = configured_location(Path::new("/srv/data/library.sqlite"));
        assert_eq!(dir, Path::new("/srv/data").join(DEFAULT_DIR_NAME));
    }
    if std::env::var_os(JOB_RETENTION_HOURS_ENV).is_none() {
        let (_, limits) = configured_location(Path::new("library.sqlite"));
        assert_eq!(limits.retention_secs, 7 * 24 * 3600);
    }
}
