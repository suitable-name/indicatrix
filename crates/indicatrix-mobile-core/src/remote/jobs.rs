//! Durable server jobs from the phone: submit a still picture to an endpoint, leave, and
//! fetch it later (from this connection, a later one, or after the app was killed).
//!
//! The server owns the job: it stores the scene, renders it, keeps the PNG for its
//! retention and answers only the certificate that submitted it. So the phone needs no job
//! database of its own: "which jobs do I have" is the endpoint's `List`, reached with the
//! enrolled credentials, and a job survives the phone going offline, the app being
//! killed, or the activity being recreated. Every call here opens its own authenticated
//! connection and closes it again; nothing holds a socket between calls.
//!
//! A `Submit` carries a random token the server turns into the durable id, so repeating a
//! `Submit` whose answer was lost finds the same job instead of making a second one: the
//! submit retries transport losses with the SAME token.
//!
//! The protocol is `indicatrix_net::messages::job`; the HDR lighting map travels as the
//! same content-addressed float asset the interactive session uses.

use super::{
    DeadlineStream,
    rendering::{
        HdrAsset, Purpose, RunError, check_sample_target, metered_refusal, net_error, open_session,
    },
};
use crate::{compute_policy::ComputePolicy, platform::Platform, settings::RemoteSettings};
use indicatrix::{
    color::ColorSpace,
    renderer::{engine::EngineSpec, env_map::EnvironmentMap},
};
use indicatrix_net::messages::{
    self, ErrorMsg, JobRequest, JobResponse, JobSubmit, MAX_JOB_LABEL_CHARS,
};
use indicatrix_web_core::scene::{CustomMaterialSpec, GeometrySceneSpec};
use sha2::{Digest, Sha256};
use std::{
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

pub use indicatrix_net::messages::{JobId, JobInfo, JobPhase, job_id_hex, parse_job_id_hex};

/// Transport losses a `submit` retries (with the same token) before giving up.
const SUBMIT_ATTEMPTS: u32 = 3;

/// How long one request waits for its answer.
const ANSWER_WAIT: Duration = Duration::from_secs(12);

/// A fresh 16-byte submission token: unique per call, not secret (ownership rests on the
/// client certificate, the token only makes a repeated `Submit` idempotent).
fn new_token(label: &str) -> [u8; 16] {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let mut hasher = Sha256::new();
    hasher.update(b"indicatrix-mobile-job-token");
    hasher.update(nanos.to_le_bytes());
    hasher.update(COUNTER.fetch_add(1, Ordering::Relaxed).to_le_bytes());
    hasher.update(u64::from(std::process::id()).to_le_bytes());
    hasher.update(label.as_bytes());
    let digest: [u8; 32] = hasher.finalize().into();
    let mut token = [0_u8; 16];
    token.copy_from_slice(&digest[..16]);
    token
}

/// The label a job gets: the design's title trimmed to what the server accepts.
#[must_use]
pub fn job_label(title: &str) -> String {
    title
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(MAX_JOB_LABEL_CHARS)
        .collect()
}

/// One line for a job list row: label, phase with progress, and size.
#[must_use]
pub fn describe(info: &JobInfo) -> String {
    let label = if info.label.is_empty() {
        "Untitled"
    } else {
        info.label.as_str()
    };
    let progress = match info.phase {
        JobPhase::Queued | JobPhase::Running => format!(
            "{} {}/{} samples",
            info.phase.label(),
            info.samples_done,
            info.samples_total
        ),
        phase => phase.label().to_owned(),
    };
    format!("{label} · {progress} · {} x {} px", info.width, info.height)
}

/// Reads the next response and turns the server's `Error` into a [`RunError::Failed`].
fn read_response(stream: &mut DeadlineStream<'_>) -> Result<JobResponse, RunError> {
    stream.deadline = Instant::now() + ANSWER_WAIT;
    match messages::read_job_response(stream).map_err(|error| net_error(&error))? {
        JobResponse::Error(ErrorMsg { message, .. }) => Err(RunError::Failed(message)),
        other => Ok(other),
    }
}

fn send(stream: &mut DeadlineStream<'_>, request: &JobRequest) -> Result<(), RunError> {
    stream.deadline = Instant::now() + ANSWER_WAIT;
    messages::write_job_request(stream, request).map_err(|error| net_error(&error))
}

fn unexpected() -> RunError {
    RunError::Failed("The endpoint answered a job request with something else.".into())
}

/// Submits the scene of `spec` as a durable job and returns how the server stored it.
///
/// `hdr` is the device's effective lighting map, required exactly when the scene uses
/// one; the server asks for it (`NeedAsset`) when it does not hold it yet, and it is
/// uploaded once, verified by hash on the server.
///
/// # Errors
///
/// [`RunError::Interrupted`] when the transport kept failing or `cancelled` fired,
/// [`RunError::Failed`] for a refusal (zoned materials, a size over the server's limit, no
/// job service on this endpoint) or invalid data.
#[expect(
    clippy::too_many_arguments,
    reason = "one remote call takes the endpoint, the scene, its lighting map, the sample plan and the cancel and report hooks; a bag struct would only rename them"
)]
pub fn submit(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    spec: &GeometrySceneSpec,
    customs: &[CustomMaterialSpec],
    engine: EngineSpec,
    hdr: Option<&Arc<EnvironmentMap>>,
    samples: u32,
    color_space: ColorSpace,
    label: &str,
    policy: &ComputePolicy,
    cancelled: &dyn Fn() -> bool,
) -> Result<JobInfo, RunError> {
    check_sample_target(engine, samples)?;
    let token = new_token(label);
    let mut last = RunError::Interrupted("No attempt was made.".into());
    for _ in 0..SUBMIT_ATTEMPTS {
        match submit_once(
            platform,
            settings,
            spec,
            customs,
            engine,
            hdr,
            samples,
            color_space,
            label,
            token,
            policy,
            cancelled,
        ) {
            Err(error @ RunError::Interrupted(_)) if !cancelled() => last = error,
            result => return result,
        }
    }
    Err(last)
}

#[expect(
    clippy::too_many_arguments,
    reason = "one remote call takes the endpoint, the scene, its lighting map, the sample plan and the cancel and report hooks; a bag struct would only rename them"
)]
fn submit_once(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    spec: &GeometrySceneSpec,
    customs: &[CustomMaterialSpec],
    engine: EngineSpec,
    hdr: Option<&Arc<EnvironmentMap>>,
    samples: u32,
    color_space: ColorSpace,
    label: &str,
    token: [u8; 16],
    policy: &ComputePolicy,
    cancelled: &dyn Fn() -> bool,
) -> Result<JobInfo, RunError> {
    let session = open_session(
        platform,
        settings,
        spec,
        customs,
        engine,
        hdr,
        Purpose::Job,
        policy,
        cancelled,
    )?;
    let super::rendering::Session {
        mut stream,
        scene,
        asset,
        ..
    } = session;
    if !scene.engine.is_ray() && scene.material.zoning.is_some() {
        return Err("Server jobs do not support color-zoned materials. Render this design on the device or as a live remote render.".into());
    }
    let submit = JobSubmit {
        client_token: token,
        label: job_label(label),
        scene,
        samples,
        color_space: color_space.into(),
    };
    submit.check_shape().map_err(RunError::Failed)?;
    let request = JobRequest::Submit(Box::new(submit));
    let mut uploaded = false;
    // Submit, then (at most once) upload the map the server asks for and submit again.
    for _ in 0..2 {
        send(&mut stream, &request)?;
        match read_response(&mut stream)? {
            JobResponse::Accepted(info) => return Ok(*info),
            JobResponse::NeedAsset { content_hash } => {
                upload(&mut stream, asset.as_ref(), content_hash, &mut uploaded)?;
            }
            _ => return Err(unexpected()),
        }
    }
    Err(RunError::Failed(
        "The endpoint kept asking for the lighting map after it was sent.".into(),
    ))
}

fn upload(
    stream: &mut DeadlineStream<'_>,
    asset: Option<&HdrAsset>,
    requested: [u8; 32],
    uploaded: &mut bool,
) -> Result<(), RunError> {
    let asset = asset.ok_or(
        "The endpoint asked for an asset this job does not use; no substitute lighting was sent.",
    )?;
    let bytes = asset.bytes_for(requested, uploaded)?;
    stream.deadline = Instant::now() + ANSWER_WAIT;
    messages::write_job_asset(stream, &bytes).map_err(|error| net_error(&error))?;
    match read_response(stream)? {
        JobResponse::AssetStored { content_hash } if content_hash == requested => Ok(()),
        _ => Err(unexpected()),
    }
}

/// Opens one authenticated connection for a job operation that carries no scene.
fn connect<'a>(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &'a dyn Fn() -> bool,
) -> Result<DeadlineStream<'a>, RunError> {
    let (stream, _welcome) = super::connect(platform, &settings.address, cancelled)?;
    Ok(stream)
}

/// One request, one `Info` answer.
fn info_of(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &dyn Fn() -> bool,
    request: &JobRequest,
) -> Result<JobInfo, RunError> {
    let mut stream = connect(platform, settings, cancelled)?;
    send(&mut stream, request)?;
    match read_response(&mut stream)? {
        JobResponse::Info(info) => Ok(*info),
        _ => Err(unexpected()),
    }
}

/// This phone's jobs on the endpoint, newest first.
///
/// # Errors
///
/// A [`RunError`] with the sentence to show.
pub fn list(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &dyn Fn() -> bool,
) -> Result<Vec<JobInfo>, RunError> {
    let mut stream = connect(platform, settings, cancelled)?;
    send(&mut stream, &JobRequest::List)?;
    match read_response(&mut stream)? {
        JobResponse::List(jobs) => Ok(jobs),
        _ => Err(unexpected()),
    }
}

/// How one job stands.
///
/// # Errors
///
/// A [`RunError`] with the sentence to show; an id the endpoint does not know (deleted,
/// expired, or not this phone's) is a [`RunError::Failed`].
pub fn status(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &dyn Fn() -> bool,
    job_id: JobId,
) -> Result<JobInfo, RunError> {
    info_of(
        platform,
        settings,
        cancelled,
        &JobRequest::Status { job_id },
    )
}

/// Stops a queued or running job.
///
/// # Errors
///
/// As [`status`].
pub fn cancel(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &dyn Fn() -> bool,
    job_id: JobId,
) -> Result<JobInfo, RunError> {
    info_of(
        platform,
        settings,
        cancelled,
        &JobRequest::Cancel { job_id },
    )
}

/// Deletes a job and its result on the endpoint.
///
/// # Errors
///
/// As [`status`].
pub fn delete(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &dyn Fn() -> bool,
    job_id: JobId,
) -> Result<(), RunError> {
    let mut stream = connect(platform, settings, cancelled)?;
    send(&mut stream, &JobRequest::Delete { job_id })?;
    match read_response(&mut stream)? {
        JobResponse::Deleted { job_id: gone } if gone == job_id => Ok(()),
        _ => Err(unexpected()),
    }
}

/// Downloads a finished job's PNG. The metered-network policy applies (the picture is
/// megabytes); the bytes are bounded by what the server promised and by the client limit.
///
/// # Errors
///
/// As [`status`]; a job that is not `Done` is a [`RunError::Failed`] from the server.
pub fn fetch(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    cancelled: &dyn Fn() -> bool,
    job_id: JobId,
) -> Result<(JobInfo, Vec<u8>), RunError> {
    metered_refusal(platform, settings, true)?;
    let mut stream = connect(platform, settings, cancelled)?;
    send(&mut stream, &JobRequest::Fetch { job_id })?;
    let JobResponse::Result(info) = read_response(&mut stream)? else {
        return Err(unexpected());
    };
    // The PNG follows as one raw frame; it may take longer than a control answer.
    stream.deadline = Instant::now() + ANSWER_WAIT;
    let png = messages::read_job_result(&mut stream, &info).map_err(RunError::Failed)?;
    Ok((*info, png))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(phase: JobPhase) -> JobInfo {
        JobInfo {
            job_id: [1; 16],
            phase,
            samples_done: 64,
            samples_total: 256,
            width: 1024,
            height: 768,
            label: "Round brilliant".into(),
            created_unix: 1_791_302_580,
            finished_unix: None,
            expires_unix: None,
            result_len: None,
            note: String::new(),
        }
    }

    #[test]
    fn tokens_are_unique_per_call() {
        assert_ne!(new_token("a"), new_token("a"));
    }

    #[test]
    fn labels_are_trimmed_to_what_the_server_accepts() {
        assert_eq!(job_label("  Round\tbrilliant \n"), "Roundbrilliant");
        let long = "x".repeat(MAX_JOB_LABEL_CHARS + 30);
        assert_eq!(job_label(&long).chars().count(), MAX_JOB_LABEL_CHARS);
        assert_eq!(job_label(""), "");
    }

    #[test]
    fn rows_show_progress_only_while_the_job_runs() {
        assert_eq!(
            describe(&info(JobPhase::Running)),
            "Round brilliant · Running 64/256 samples · 1024 x 768 px"
        );
        assert_eq!(
            describe(&info(JobPhase::Done)),
            "Round brilliant · Done · 1024 x 768 px"
        );
        let mut unnamed = info(JobPhase::Failed);
        unnamed.label.clear();
        assert!(describe(&unnamed).starts_with("Untitled · Failed"));
    }

    #[test]
    fn a_metered_network_blocks_a_download_unless_allowed() {
        let platform = crate::platform_mock::MockPlatform::default();
        platform.set_network_metered(Some(true));
        let mut settings = RemoteSettings::default();
        let error = fetch(&platform, &settings, &|| false, [0; 16]).unwrap_err();
        assert!(error.message().contains("metered"), "{error}");
        settings.allow_metered = true;
        // With the policy satisfied the call goes on to connect, which fails here only
        // because nothing is enrolled in the mock's (empty) credential vault -- not
        // because of the network.
        let error = fetch(&platform, &settings, &|| false, [0; 16]).unwrap_err();
        assert!(!error.message().contains("metered"), "{error}");
    }

    #[test]
    fn a_job_needs_samples() {
        let platform = crate::platform_mock::MockPlatform::default();
        let spec = GeometrySceneSpec {
            scene: indicatrix_web_core::scene::SceneSpec {
                planes: indicatrix_web_core::scene::planes_to_data(
                    &indicatrix::geometry::cuts::StandardGemCuts::standard_round_brilliant(),
                ),
                finishes: indicatrix_web_core::scene::FinishSpec::AllPolished,
                material: indicatrix_web_core::scene::MaterialSpec::catalogue("Diamond"),
                camera: indicatrix_web_core::scene::CameraSpec {
                    yaw: 0.0,
                    pitch: 0.5,
                    distance: 3.0,
                },
                lighting: indicatrix_web_core::scene::LightingSpec::new(
                    indicatrix::optics::raytracer::LightingPreset::Daylight,
                    1.0,
                    0.3,
                    0.4,
                    indicatrix::render_setup::Backdrop::Grey,
                    12.0,
                ),
                max_bounces: 4,
                width: 8,
                height: 6,
                hdr_id: None,
            },
            tools: Vec::new(),
        };
        let policy = ComputePolicy::new(1);
        let error = submit(
            &platform,
            &RemoteSettings::default(),
            &spec,
            &[],
            indicatrix::renderer::engine::EngineSpec::default(),
            None,
            0,
            ColorSpace::Srgb,
            "x",
            &policy,
            &|| false,
        )
        .unwrap_err();
        assert!(
            error.message().contains("samples must be positive"),
            "{error}"
        );
    }
}
