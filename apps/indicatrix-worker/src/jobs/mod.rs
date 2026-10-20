//! Durable server jobs (protocol v25): a still picture the server renders on its own
//! schedule and keeps for its owner to fetch later, from any later connection.
//!
//! The wire side is `indicatrix_net::messages::job` (read its module doc first); this module
//! is the server side:
//!
//! - [`store`]: the on-disk store -- one `*.job.json` per job (the desktop render queue's
//!   `RenderJobFile`), a state file with the owner and progress, and the result PNG; start-up
//!   recovery, retention and the limits.
//! - [`run`]: the executor thread that renders queued jobs one at a time on the own lane.
//! - this file: [`JobService`] (store, asset cache and executor together) and
//!   [`serve_job`], which answers one `JobRequest` on a viewer connection.
//!
//! # Who may ask
//!
//! The owner of a job is the viewer key of the connection that submitted it: the SHA-256
//! fingerprint of the client certificate (`certificate:<hex>`), or -- under
//! `--insecure-no-tls`, loopback only -- the peer address. Every request is checked against
//! the persisted owner; a job that is not the caller's answers `JOB_UNKNOWN`, exactly like a
//! job that does not exist.
//!
//! # Configuration
//!
//! The job directory is `$INDICATRIX_JOB_DIR` if set, else `jobs` next to the served library
//! database (the same anchor the HDR asset cache uses). A finished job is kept for
//! `$INDICATRIX_JOB_RETENTION_HOURS` hours (default 168, one week). Jobs need the own render
//! lane (`serve --render`); a server without one answers every job request
//! `UNSUPPORTED_REQUEST`.

mod run;
mod store;

#[cfg(test)]
mod tests;

pub use run::{Outcome, RenderLane, render_job};
pub use store::{
    CANCELLED_NOTE, Claim, JobError, JobStore, Limits, MAX_JOB_PIXELS, RESTART_NOTE, unix_now,
};

use crate::{
    assets::{self, AssetCache},
    stream_emit::TimeoutRead,
    validate,
};
use indicatrix_net::{
    framing::FramingError,
    messages::{
        AssetError, ErrorMsg, JobRequest, JobResponse, JobSubmit, MAX_ASSET_LEN, NetError,
        derive_job_id, envmap_asset, error_codes, read_asset_payload, write_job_response,
        write_job_result,
    },
    scene::HdrEnvironment,
};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

/// Environment variable overriding the job directory.
pub const JOB_DIR_ENV: &str = "INDICATRIX_JOB_DIR";
/// Environment variable overriding the retention, in hours.
pub const JOB_RETENTION_HOURS_ENV: &str = "INDICATRIX_JOB_RETENTION_HOURS";
/// The directory name used next to the anchor path when [`JOB_DIR_ENV`] is unset.
pub const DEFAULT_DIR_NAME: &str = "jobs";

/// How long an uploaded asset's payload may stall before the connection is dropped.
const PAYLOAD_READ_TIMEOUT: Duration = Duration::from_secs(30);

/// The job directory and limits from the environment (see the module doc comment).
#[must_use]
pub fn configured_location(anchor: &Path) -> (PathBuf, Limits) {
    let dir = std::env::var_os(JOB_DIR_ENV).map_or_else(
        || {
            anchor
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(DEFAULT_DIR_NAME)
        },
        PathBuf::from,
    );
    let mut limits = Limits::default();
    if let Some(hours) = std::env::var(JOB_RETENTION_HOURS_ENV)
        .ok()
        .and_then(|text| text.trim().parse::<i64>().ok())
        .filter(|hours| *hours > 0)
    {
        limits.retention_secs = hours.saturating_mul(3600);
    }
    (dir, limits)
}

/// The job store, the asset cache jobs' HDR maps live in, and the executor's stop flag.
pub struct JobService {
    store: JobStore,
    assets: Option<Arc<AssetCache>>,
    stop: AtomicBool,
}

impl JobService {
    /// A service over `store`; `assets` is where HDR jobs' maps are found and uploaded.
    #[must_use]
    pub const fn new(store: JobStore, assets: Option<Arc<AssetCache>>) -> Self {
        Self {
            store,
            assets,
            stop: AtomicBool::new(false),
        }
    }

    /// Opens the configured store (see [`configured_location`]) and recovers it. `None` --
    /// durable jobs then unavailable, never half-working -- when it cannot be opened.
    #[must_use]
    pub fn open_configured(anchor: &Path, assets: Option<Arc<AssetCache>>) -> Option<Arc<Self>> {
        let (dir, limits) = configured_location(anchor);
        match JobStore::open(&dir, limits, unix_now()) {
            Ok(store) => {
                tracing::info!(
                    "indicatrix-worker: durable jobs at {} (kept {} h after they finish)",
                    dir.display(),
                    limits.retention_secs / 3600
                );
                Some(Arc::new(Self::new(store, assets)))
            }
            Err(error) => {
                tracing::warn!(
                    "indicatrix-worker: cannot open the job directory {} ({error}); durable jobs are \
                     unavailable (set {JOB_DIR_ENV} to a writable directory)",
                    dir.display()
                );
                None
            }
        }
    }

    /// The store.
    #[must_use]
    pub const fn store(&self) -> &JobStore {
        &self.store
    }

    /// Starts the executor thread on `lane`.
    ///
    /// # Errors
    ///
    /// The I/O error if the thread cannot be spawned.
    pub fn spawn_executor(self: &Arc<Self>, lane: RenderLane) -> std::io::Result<JoinHandle<()>> {
        let service = Arc::clone(self);
        std::thread::Builder::new()
            .name("job-executor".to_owned())
            .spawn(move || {
                run::run(
                    &service.store,
                    &lane,
                    service.assets.as_deref(),
                    &service.stop,
                );
            })
    }

    /// Asks the executor to stop after its current slice (for tests and shutdown).
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.store.wake_all();
    }
}

fn error_response(code: u32, message: impl Into<String>) -> JobResponse {
    JobResponse::Error(ErrorMsg {
        code,
        message: message.into(),
        request_id: None,
    })
}

/// The reply a failed store call owes.
fn job_error_response(error: JobError) -> JobResponse {
    match error {
        JobError::Unknown => error_response(error_codes::JOB_UNKNOWN, "no such job"),
        JobError::State(message) => error_response(error_codes::JOB_STATE, message),
        JobError::Invalid(message) => error_response(error_codes::VALIDATION_FAILED, message),
        JobError::Limit(message) => error_response(error_codes::JOB_LIMIT, message),
        JobError::Io(message) => {
            tracing::warn!("jobs: {message}");
            error_response(
                error_codes::JOB_STATE,
                "the server could not read or write its job store",
            )
        }
    }
}

/// Consumes the payload frame a `PutAsset` carries, if `request` is one.
fn discard_payload<S: Read>(stream: &mut S, request: &JobRequest) -> Result<(), NetError> {
    if let JobRequest::PutAsset(header) = request {
        assets::discard_asset(stream, header)?;
    }
    Ok(())
}

/// Ignores a job request that arrived while another request is in flight on this connection.
///
/// A job client uses a connection of its own, one request at a time. Nothing is answered,
/// but a `PutAsset`'s payload frame is consumed so the stream stays in sync.
///
/// # Errors
///
/// [`NetError`] when the payload cannot be read in sync.
pub fn ignore_in_flight<S: Read>(stream: &mut S, request: &JobRequest) -> Result<(), NetError> {
    tracing::debug!(
        "ignoring a durable job request that arrived while another request was in flight"
    );
    discard_payload(stream, request)
}

/// Answers one job request with `UNSUPPORTED_REQUEST` or another refusal, consuming a
/// `PutAsset`'s payload frame first.
///
/// # Errors
///
/// [`NetError`] if the payload cannot be read in sync or the answer cannot be written.
pub fn refuse<S: Read + Write>(
    stream: &mut S,
    request: &JobRequest,
    code: u32,
    message: &str,
) -> Result<(), NetError> {
    discard_payload(stream, request)?;
    write_job_response(stream, &error_response(code, message))
}

/// Answers one `JobRequest` for `owner`.
///
/// `service` is `None` on a server without durable jobs and `owner` is `None` on a connection
/// that has no viewer identity; both are refused with `UNSUPPORTED_REQUEST`.
///
/// # Errors
///
/// [`NetError`] for a transport failure (including an asset payload that cannot be read in
/// sync). Every other failure is a `JobResponse::Error` and the connection stays usable.
pub fn serve_job<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: JobRequest,
    service: Option<&JobService>,
    owner: Option<&str>,
) -> Result<(), NetError> {
    let (Some(service), Some(owner)) = (service, owner) else {
        return refuse(
            stream,
            &request,
            error_codes::UNSUPPORTED_REQUEST,
            "this server does not keep durable jobs (it needs `serve --render`)",
        );
    };
    let store = service.store();
    let response = match request {
        JobRequest::Submit(submit) => submit_job(service, owner, &submit),
        JobRequest::PutAsset(header) => put_asset(stream, service, &header)?,
        JobRequest::Status { job_id } => match store.status(owner, &job_id) {
            Ok(info) => JobResponse::Info(Box::new(info)),
            Err(error) => job_error_response(error),
        },
        JobRequest::Fetch { job_id } => {
            return match store.result(owner, &job_id) {
                Ok((info, png)) => write_job_result(stream, &info, &png),
                Err(error) => write_job_response(stream, &job_error_response(error)),
            };
        }
        JobRequest::Cancel { job_id } => match store.cancel(owner, &job_id, unix_now()) {
            Ok(info) => JobResponse::Info(Box::new(info)),
            Err(error) => job_error_response(error),
        },
        JobRequest::Delete { job_id } => match store.delete(owner, &job_id) {
            Ok(()) => JobResponse::Deleted { job_id },
            Err(error) => job_error_response(error),
        },
        JobRequest::List => JobResponse::List(store.list(owner)),
    };
    write_job_response(stream, &response)
}

/// Checks that the asset `environment` names decodes to the declared size, decoding it into the
/// process-wide registry (so the executor finds it there).
fn check_asset(cache: &AssetCache, environment: &HdrEnvironment) -> Result<(), String> {
    if assets::lookup(&environment.content_hash).is_some() {
        return Ok(());
    }
    let bytes = cache
        .get(&environment.content_hash)
        .ok_or_else(|| "the HDR map is not in the server's asset cache".to_owned())?;
    assets::decode_and_register(environment, &bytes).map(|_| ())
}

fn submit_job(service: &JobService, owner: &str, submit: &JobSubmit) -> JobResponse {
    // A repeated submission finds its job before anything else is looked at, so a client
    // that lost the answer can always ask again.
    let id = derive_job_id(owner, &submit.client_token);
    if let Ok(info) = service.store().status(owner, &id) {
        return JobResponse::Accepted(Box::new(info));
    }
    if let Err(message) = submit.check_shape().and_then(|()| {
        if submit.scene.engine.is_ray() {
            validate::validate_request(&submit.scene, 0, submit.samples)
        } else {
            validate::validate_scene(&submit.scene)
        }
    }) {
        return error_response(error_codes::VALIDATION_FAILED, message);
    }
    if let Some(environment) = submit.scene.hdr() {
        let Some(cache) = service.assets.as_deref() else {
            return error_response(
                error_codes::UNSUPPORTED_REQUEST,
                "this server has no HDR asset cache, so it cannot keep a job lit by an HDR map",
            );
        };
        if !cache.contains(&environment.content_hash) {
            return JobResponse::NeedAsset {
                content_hash: environment.content_hash,
            };
        }
        if let Err(message) = check_asset(cache, environment) {
            return error_response(error_codes::ASSET_FAILED, message);
        }
    }
    match service.store().submit(owner, submit, unix_now()) {
        Ok(info) => JobResponse::Accepted(Box::new(info)),
        Err(error) => job_error_response(error),
    }
}

fn put_asset<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    service: &JobService,
    header: &indicatrix_net::messages::AssetHeader,
) -> Result<JobResponse, NetError> {
    let Some(cache) = service.assets.as_deref() else {
        assets::discard_asset(stream, header)?;
        return Ok(error_response(
            error_codes::UNSUPPORTED_REQUEST,
            "this server has no HDR asset cache",
        ));
    };
    let _ = stream.set_read_timeout(Some(PAYLOAD_READ_TIMEOUT));
    let bytes = match read_asset_payload(stream, header) {
        Ok(bytes) => bytes,
        Err(error @ (AssetError::HashMismatch | AssetError::LengthMismatch { .. })) => {
            return Ok(error_response(error_codes::ASSET_FAILED, error.to_string()));
        }
        Err(AssetError::TooLarge { len }) => {
            return Err(NetError::Framing(FramingError::FrameTooLarge {
                len,
                max: MAX_ASSET_LEN,
            }));
        }
        Err(AssetError::Framing(error)) => return Err(NetError::Framing(error)),
    };
    // A float asset is checked structurally before it is kept; a Radiance file is decoded
    // (and its size checked against the scene's declaration) when a job names it.
    if envmap_asset::is_envmap_asset(&bytes)
        && let Err(error) = envmap_asset::envmap_asset_header(&bytes)
    {
        return Ok(error_response(error_codes::ASSET_FAILED, error.to_string()));
    }
    Ok(match cache.put(&header.content_hash, &bytes) {
        Ok(()) => JobResponse::AssetStored {
            content_hash: header.content_hash,
        },
        Err(error) => error_response(
            error_codes::ASSET_FAILED,
            format!("the server could not store the asset: {error}"),
        ),
    })
}
