//! Durable server jobs (v25, `render` feature only): a still picture the server renders on its
//! own schedule.
//!
//! The server keeps the finished picture for the client to fetch later, from this connection
//! or any later one, even after the client was disconnected or killed.
//!
//! A `FINAL_IMAGE_REQUEST` ties a render to its connection: if the client drops, the work is
//! cancelled and the result is gone. A job does not. The client submits the scene once, the
//! server stores it durably (one `*.job.json` file per job, the same file shape the
//! desktop render queue uses -- see `indicatrix-render-jobs`), renders it, and keeps the
//! PNG until its retention ends. The client reconnects at any time and asks.
//!
//! ```text
//! -> Job(Submit  { client_token, label, scene, samples, color_space })
//! <- JobResponse::Accepted(info)                       -- stored; `info.job_id` is the durable id
//! <- JobResponse::NeedAsset { content_hash }           -- the scene's HDR map is not on the server:
//! -> Job(PutAsset(header)) + one raw frame             --   upload it (same bounds and hash check
//! <- JobResponse::AssetStored { content_hash }         --   as `ASSET`), then submit again
//! -> Job(Status { job_id })      <- JobResponse::Info(info)
//! -> Job(Fetch  { job_id })      <- JobResponse::Result(info) + one raw PNG frame   (phase Done)
//! -> Job(Cancel { job_id })      <- JobResponse::Info(info)
//! -> Job(Delete { job_id })      <- JobResponse::Deleted { job_id }
//! -> Job(List)                   <- JobResponse::List(infos)
//! <- JobResponse::Error(ErrorMsg)                      -- any request; the connection stays usable
//! ```
//!
//! # Identity and ownership
//!
//! The durable id is NOT chosen by the client. The server derives it from the owner (the
//! client-certificate fingerprint of the authenticated connection) and the client's random
//! `client_token`: `SHA-256("indicatrix-job-v1" | owner | client_token)`, first 16 bytes.
//! Re-submitting the same token as the same owner therefore finds the same job (so a
//! `Submit` whose answer was lost is safe to repeat), no owner can address another owner's
//! job whatever id it sends, and an id leaks nothing about the token.
//!
//! The owner is persisted next to the job and checked on every request. A request for a
//! job that does not exist for this owner is answered [`error_codes::JOB_UNKNOWN`] whether
//! the job is missing or belongs to someone else.
//!
//! # Lifecycle
//!
//! `Queued` -> `Running` -> `Done` | `Failed` | `Cancelled`. A server restart turns a
//! job that was `Running` back into `Queued` (its picture is re-rendered from sample 0; the
//! render is deterministic) and leaves terminal jobs as they were. A finished job is kept for
//! the server's retention (default 7 days) after it ended; [`JobInfo::expires_unix`] says
//! when. A job past its retention is deleted and answers `JOB_UNKNOWN`.
//!
//! A scene with colour zones (`GemMaterial::zoning`) travels beside the scene and is not part
//! of a job; a client must not submit one.

use super::{ErrorMsg, WireColorSpace, asset::AssetHeader};
use crate::{framing, scene::SceneState};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// The identity of a job: 16 bytes, shown as 32 lowercase hexadecimal characters.
pub type JobId = [u8; 16];

/// The most samples per pixel one job may ask for.
pub const MAX_JOB_SAMPLES: u32 = 32_768;
/// The most characters of a job's label.
pub const MAX_JOB_LABEL_CHARS: usize = 120;
/// The most jobs one [`JobResponse::List`] carries.
pub const MAX_JOB_LIST: usize = 64;
/// The largest result PNG a client accepts (64 MiB).
pub const MAX_JOB_RESULT_LEN: u32 = 64 * 1024 * 1024;

/// How far a job has come. Variant order is wire-load-bearing; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobPhase {
    /// Stored, waiting for the render lane.
    Queued,
    /// Rendering now.
    Running,
    /// Finished; the PNG can be fetched until the job expires.
    Done,
    /// Stopped by an error; [`JobInfo::note`] says which.
    Failed,
    /// Stopped by the owner.
    Cancelled,
}

impl JobPhase {
    /// Whether the job will not change any more.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Failed | Self::Cancelled)
    }

    /// The word shown to people.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }
}

/// What the server tells a client about one job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobInfo {
    /// The durable id.
    pub job_id: JobId,
    /// How far the job has come.
    pub phase: JobPhase,
    /// Samples per pixel traced so far (rounded down to whole render steps).
    pub samples_done: u32,
    /// Samples per pixel the job asks for.
    pub samples_total: u32,
    /// Picture width in pixels.
    pub width: u32,
    /// Picture height in pixels.
    pub height: u32,
    /// The label the client gave it.
    pub label: String,
    /// When the server stored it, in unix seconds.
    pub created_unix: i64,
    /// When it reached a terminal phase, in unix seconds.
    pub finished_unix: Option<i64>,
    /// When a terminal job will be deleted, in unix seconds.
    pub expires_unix: Option<i64>,
    /// The PNG's size in bytes, once the job is `Done`.
    pub result_len: Option<u32>,
    /// A sentence: why it failed, or that a restart re-queued it. Empty when there is nothing to say.
    pub note: String,
}

/// `-> Job(Submit)`: the scene and render settings of a new job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct JobSubmit {
    /// 16 random bytes the client keeps; submitting the same token again returns the same job.
    pub client_token: [u8; 16],
    /// A short name for the job list.
    pub label: String,
    /// The fully resolved scene; its `width` x `height` is the picture size.
    pub scene: SceneState,
    /// Samples per pixel, from sample 0.
    pub samples: u32,
    /// The colour space the PNG is tone-mapped into.
    pub color_space: WireColorSpace,
}

impl JobSubmit {
    /// Checks the parts of the submission that need no scene knowledge.
    ///
    /// # Errors
    ///
    /// A sentence naming the first broken rule.
    pub fn check_shape(&self) -> Result<(), String> {
        if self.samples == 0 || self.samples > MAX_JOB_SAMPLES {
            return Err(format!(
                "a job must ask for between 1 and {MAX_JOB_SAMPLES} samples per pixel (got {})",
                self.samples
            ));
        }
        if self.scene.engine.is_ray()
            && (!indicatrix::renderer::engine::AA_TAP_CHOICES
                .contains(&self.scene.engine.ray.aa_taps)
                || self.samples != u32::from(self.scene.engine.ray.aa_taps))
        {
            return Err(
                "a Ray-Traced job must request exactly its 1, 4, 8 or 16 AA taps".to_owned(),
            );
        }
        if self.label.chars().count() > MAX_JOB_LABEL_CHARS {
            return Err(format!(
                "a job label is at most {MAX_JOB_LABEL_CHARS} characters"
            ));
        }
        if self.scene.width == 0 || self.scene.height == 0 {
            return Err("a job's picture size must be positive".to_owned());
        }
        Ok(())
    }
}

/// The request half of the job family, wrapped in `ClientMessage::Job`. Variant order is
/// wire-load-bearing; append only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum JobRequest {
    /// Store a new job (or find the one this token already made).
    Submit(Box<JobSubmit>),
    /// Upload the HDR asset a [`JobResponse::NeedAsset`] asked for; one raw frame of
    /// `header.len` bytes follows.
    PutAsset(AssetHeader),
    /// Ask how a job stands.
    Status {
        /// The job.
        job_id: JobId,
    },
    /// Fetch a `Done` job's PNG.
    Fetch {
        /// The job.
        job_id: JobId,
    },
    /// Stop a queued or running job.
    Cancel {
        /// The job.
        job_id: JobId,
    },
    /// Delete a job and its result now.
    Delete {
        /// The job.
        job_id: JobId,
    },
    /// List this owner's jobs, newest first.
    List,
}

/// The server's single reply to a [`JobRequest`] -- see the module doc comment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum JobResponse {
    /// The job is stored (new, or already there for this token).
    Accepted(Box<JobInfo>),
    /// The scene's HDR map is not on the server; upload it with `PutAsset`, then submit again.
    NeedAsset {
        /// The map's content hash.
        content_hash: super::ContentHash,
    },
    /// The `PutAsset` bytes were verified and stored.
    AssetStored {
        /// The stored asset's content hash.
        content_hash: super::ContentHash,
    },
    /// How a job stands.
    Info(Box<JobInfo>),
    /// A `Done` job's PNG follows as one raw frame of `info.result_len` bytes.
    Result(Box<JobInfo>),
    /// This owner's jobs, newest first, at most [`MAX_JOB_LIST`].
    List(Vec<JobInfo>),
    /// The job was deleted.
    Deleted {
        /// The job.
        job_id: JobId,
    },
    /// The request failed; the connection stays usable.
    Error(ErrorMsg),
}

/// `job_id` as 32 lowercase hexadecimal characters.
#[must_use]
pub fn job_id_hex(job_id: &JobId) -> String {
    use std::fmt::Write as _;
    job_id
        .iter()
        .fold(String::with_capacity(32), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// Reads 32 lowercase hexadecimal characters back into a [`JobId`].
#[must_use]
pub fn parse_job_id_hex(text: &str) -> Option<JobId> {
    if text.len() != 32 {
        return None;
    }
    let mut id = [0_u8; 16];
    for (slot, pair) in id.iter_mut().zip(text.as_bytes().as_chunks::<2>().0) {
        let digit = |byte: u8| match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            _ => None,
        };
        *slot = (digit(pair[0])? << 4) | digit(pair[1])?;
    }
    Some(id)
}

/// The durable id of the job `client_token` makes for `owner`; see the module doc comment.
#[must_use]
pub fn derive_job_id(owner: &str, client_token: &[u8; 16]) -> JobId {
    let mut hasher = Sha256::new();
    hasher.update(b"indicatrix-job-v1");
    hasher.update((owner.len() as u64).to_le_bytes());
    hasher.update(owner.as_bytes());
    hasher.update(client_token);
    let digest: [u8; 32] = hasher.finalize().into();
    let mut id = [0_u8; 16];
    id.copy_from_slice(&digest[..16]);
    id
}

/// Writes `request` as a tagged `ClientMessage::Job` frame.
///
/// # Errors
///
/// [`super::NetError`] if encoding or writing fails.
pub fn write_job_request<W: std::io::Write>(
    writer: &mut W,
    request: &JobRequest,
) -> Result<(), super::NetError> {
    super::write_message(
        writer,
        &super::ClientMessage::Job(Box::new(request.clone())),
    )
}

/// Writes a `PutAsset` request: the tagged header, then `bytes` as one raw frame.
///
/// # Errors
///
/// [`super::NetError`] with `FrameTooLarge` when `bytes` exceeds [`super::MAX_ASSET_LEN`],
/// else whatever writing fails with.
pub fn write_job_asset<W: std::io::Write>(
    writer: &mut W,
    bytes: &[u8],
) -> Result<(), super::NetError> {
    let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
    if len > super::MAX_ASSET_LEN {
        return Err(framing::FramingError::FrameTooLarge {
            len,
            max: super::MAX_ASSET_LEN,
        }
        .into());
    }
    let header = AssetHeader {
        content_hash: super::content_hash(bytes),
        len,
    };
    write_job_request(writer, &JobRequest::PutAsset(header))?;
    framing::write_frame(writer, bytes)?;
    Ok(())
}

/// Writes `response` as one frame.
///
/// # Errors
///
/// [`super::NetError`] if encoding or writing fails.
pub fn write_job_response<W: std::io::Write>(
    writer: &mut W,
    response: &JobResponse,
) -> Result<(), super::NetError> {
    super::write_message(writer, response)
}

/// Writes a `Result` response for `info` followed by the PNG as one raw frame.
///
/// `png.len()` must equal `info.result_len`.
///
/// # Errors
///
/// [`super::NetError`] if writing fails, or `FrameTooLarge` when the PNG exceeds
/// [`MAX_JOB_RESULT_LEN`].
pub fn write_job_result<W: std::io::Write>(
    writer: &mut W,
    info: &JobInfo,
    png: &[u8],
) -> Result<(), super::NetError> {
    let len = u32::try_from(png.len()).unwrap_or(u32::MAX);
    if len > MAX_JOB_RESULT_LEN {
        return Err(framing::FramingError::FrameTooLarge {
            len,
            max: MAX_JOB_RESULT_LEN,
        }
        .into());
    }
    write_job_response(writer, &JobResponse::Result(Box::new(info.clone())))?;
    framing::write_frame(writer, png)?;
    Ok(())
}

/// Reads one [`JobResponse`].
///
/// # Errors
///
/// [`super::NetError`] if reading or decoding fails.
pub fn read_job_response<R: std::io::Read>(reader: &mut R) -> Result<JobResponse, super::NetError> {
    super::read_control_message(reader)
}

/// Reads the PNG frame that follows a [`JobResponse::Result`], checking it against `info`.
///
/// The length is bounded by [`MAX_JOB_RESULT_LEN`] and by `info.result_len` before anything
/// is allocated.
///
/// # Errors
///
/// A sentence when `info` promises no result, the frame is longer than promised, or reading
/// fails.
pub fn read_job_result<R: std::io::Read>(
    reader: &mut R,
    info: &JobInfo,
) -> Result<Vec<u8>, String> {
    let promised = info
        .result_len
        .ok_or_else(|| "the job response carries no result".to_owned())?;
    if promised > MAX_JOB_RESULT_LEN {
        return Err("the job result exceeds the client's size limit".to_owned());
    }
    let bytes = framing::read_frame_bounded(reader, promised).map_err(|error| error.to_string())?;
    if bytes.len() != promised as usize {
        return Err("the job result differs in length from what the server promised".to_owned());
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::{ClientMessage, codec::write_message, read_message};

    fn scene() -> SceneState {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };
        SceneState {
            engine: indicatrix::renderer::engine::EngineSpec::default(),
            width: 8,
            height: 6,
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
            environment: crate::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
        }
    }

    fn submit() -> JobSubmit {
        JobSubmit {
            client_token: [7; 16],
            label: "Round brilliant".to_owned(),
            scene: scene(),
            samples: 64,
            color_space: WireColorSpace::DisplayP3,
        }
    }

    fn info(phase: JobPhase) -> JobInfo {
        JobInfo {
            job_id: [9; 16],
            phase,
            samples_done: 32,
            samples_total: 64,
            width: 8,
            height: 6,
            label: "Round brilliant".to_owned(),
            created_unix: 1_791_302_580,
            finished_unix: phase.is_terminal().then_some(1_791_302_700),
            expires_unix: phase.is_terminal().then_some(1_791_907_500),
            result_len: (phase == JobPhase::Done).then_some(4),
            note: String::new(),
        }
    }

    #[test]
    fn ray_job_submissions_require_the_complete_fixed_tap_set() {
        let mut submission = submit();
        submission.scene.engine.engine = indicatrix::renderer::engine::RenderEngine::RayTraced;
        for taps in indicatrix::renderer::engine::AA_TAP_CHOICES {
            submission.scene.engine.ray.aa_taps = taps;
            submission.samples = u32::from(taps);
            assert!(submission.check_shape().is_ok());
            submission.samples += 1;
            assert!(submission.check_shape().is_err());
            submission.samples = u32::from(taps).saturating_sub(1);
            assert!(submission.check_shape().is_err());
        }
        submission.scene.engine.ray.aa_taps = 3;
        submission.samples = 3;
        assert!(submission.check_shape().is_err());
    }

    #[test]
    fn every_request_round_trips_inside_the_client_message_envelope() {
        let requests = vec![
            JobRequest::Submit(Box::new(submit())),
            JobRequest::PutAsset(AssetHeader {
                content_hash: [3; 32],
                len: 12,
            }),
            JobRequest::Status { job_id: [1; 16] },
            JobRequest::Fetch { job_id: [2; 16] },
            JobRequest::Cancel { job_id: [3; 16] },
            JobRequest::Delete { job_id: [4; 16] },
            JobRequest::List,
        ];
        for request in requests {
            let mut wire = Vec::new();
            write_job_request(&mut wire, &request).unwrap();
            let decoded: ClientMessage = read_message(&mut std::io::Cursor::new(wire)).unwrap();
            assert_eq!(decoded, ClientMessage::Job(Box::new(request)));
        }
    }

    /// Appended after `ZoningPayload` (index 9): older indices do not move.
    #[test]
    fn the_job_variant_is_appended_at_index_ten() {
        let tagged =
            postcard::to_allocvec(&ClientMessage::Job(Box::new(JobRequest::List))).unwrap();
        assert_eq!(tagged[0], 10);
        assert_eq!(
            postcard::to_allocvec(&ClientMessage::Ping { nonce: 1 }).unwrap()[0],
            4
        );
    }

    #[test]
    fn every_response_round_trips() {
        let responses = vec![
            JobResponse::Accepted(Box::new(info(JobPhase::Queued))),
            JobResponse::NeedAsset {
                content_hash: [5; 32],
            },
            JobResponse::AssetStored {
                content_hash: [6; 32],
            },
            JobResponse::Info(Box::new(info(JobPhase::Running))),
            JobResponse::Result(Box::new(info(JobPhase::Done))),
            JobResponse::List(vec![info(JobPhase::Done), info(JobPhase::Failed)]),
            JobResponse::Deleted { job_id: [8; 16] },
            JobResponse::Error(ErrorMsg {
                code: crate::messages::error_codes::JOB_UNKNOWN,
                message: "no such job".to_owned(),
                request_id: None,
            }),
        ];
        for response in responses {
            let mut wire = Vec::new();
            write_message(&mut wire, &response).unwrap();
            let decoded = read_job_response(&mut std::io::Cursor::new(wire)).unwrap();
            assert_eq!(decoded, response);
        }
    }

    #[test]
    fn a_result_is_followed_by_exactly_its_png_frame() {
        let png = vec![0x89, b'P', b'N', b'G'];
        let done = info(JobPhase::Done);
        let mut wire = Vec::new();
        write_job_result(&mut wire, &done, &png).unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let JobResponse::Result(read_info) = read_job_response(&mut cursor).unwrap() else {
            panic!("expected a Result response");
        };
        assert_eq!(*read_info, done);
        assert_eq!(read_job_result(&mut cursor, &read_info).unwrap(), png);

        // A frame longer than the promise is refused from the frame header alone.
        let mut lying = done.clone();
        lying.result_len = Some(2);
        let mut wire = Vec::new();
        framing::write_frame(&mut wire, &png).unwrap();
        assert!(read_job_result(&mut std::io::Cursor::new(wire), &lying).is_err());
        // A job that is not done promises nothing.
        assert!(
            read_job_result(
                &mut std::io::Cursor::new(Vec::new()),
                &info(JobPhase::Queued)
            )
            .is_err()
        );
    }

    #[test]
    fn put_asset_sends_a_verified_header_then_the_bytes() {
        let bytes = b"IXENVMAP and more".to_vec();
        let mut wire = Vec::new();
        write_job_asset(&mut wire, &bytes).unwrap();
        let mut cursor = std::io::Cursor::new(wire);
        let ClientMessage::Job(request) = read_message(&mut cursor).unwrap() else {
            panic!("expected a Job message");
        };
        let JobRequest::PutAsset(header) = *request else {
            panic!("expected PutAsset");
        };
        assert_eq!(header.content_hash, crate::messages::content_hash(&bytes));
        assert_eq!(
            crate::messages::read_asset_payload(&mut cursor, &header).unwrap(),
            bytes
        );
    }

    #[test]
    fn job_ids_are_owner_scoped_stable_and_hex_round_trips() {
        let token = [0x42; 16];
        let id = derive_job_id("certificate:aa", &token);
        assert_eq!(id, derive_job_id("certificate:aa", &token));
        assert_ne!(id, derive_job_id("certificate:bb", &token));
        assert_ne!(id, derive_job_id("certificate:aa", &[0x43; 16]));
        // The owner is length-framed, so a token cannot be shifted into the owner.
        assert_ne!(
            derive_job_id("a", &[b'b'; 16]),
            derive_job_id("ab", &[b'b'; 16])
        );
        let hex = job_id_hex(&id);
        assert_eq!(hex.len(), 32);
        assert!(
            hex.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(parse_job_id_hex(&hex), Some(id));
        let shorter = &hex[..31];
        let uppercase = hex.to_uppercase();
        let longer = format!("{hex}0");
        for bad in ["", "zz", shorter, uppercase.as_str(), longer.as_str()] {
            assert_eq!(parse_job_id_hex(bad), None, "{bad}");
        }
    }

    #[test]
    fn submissions_are_checked_for_shape() {
        assert!(submit().check_shape().is_ok());
        let mut zero = submit();
        zero.samples = 0;
        assert!(zero.check_shape().is_err());
        let mut many = submit();
        many.samples = MAX_JOB_SAMPLES + 1;
        assert!(many.check_shape().is_err());
        let mut long = submit();
        long.label = "x".repeat(MAX_JOB_LABEL_CHARS + 1);
        assert!(long.check_shape().is_err());
        let mut empty = submit();
        empty.scene.width = 0;
        assert!(empty.check_shape().is_err());
    }

    #[test]
    fn phases_know_when_they_are_over() {
        use JobPhase::{Cancelled, Done, Failed, Queued, Running};
        assert!(!Queued.is_terminal() && !Running.is_terminal());
        assert!(Done.is_terminal() && Failed.is_terminal() && Cancelled.is_terminal());
        for phase in [Queued, Running, Done, Failed, Cancelled] {
            assert!(phase.label().starts_with(|c: char| c.is_ascii_uppercase()));
        }
    }
}
