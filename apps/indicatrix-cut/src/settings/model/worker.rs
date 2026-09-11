//! Remote-worker settings: [`PreviewScale`] (a worker's live preview-stream
//! resolution), [`LocalPreviewScale`] (the local preview-then-settle resolution
//! reduction), and [`WorkerSettings`] (one configured remote render worker).

use indicatrix_net::messages::{PreviewConfig, StreamConfig, TransferMode};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// The reduced resolution a remote worker's `PREVIEW` stream is rendered at, relative
/// to the session's full render resolution -- a per-worker setting distinct from
/// [`WorkerSettings::transfer_mode`], which governs the FULL-resolution `FRAME`
/// payload instead.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PreviewScale {
    Full,
    Half,
    /// The default -- a newly added worker starts asking for a QUARTER-resolution
    /// preview stream, not full. At 1920x1080, a `Full` preview doubles per-tick
    /// bandwidth to roughly 100 MB/s; `Quarter` cuts the preview's own share to
    /// 1/16th, still plenty of detail for a live camera-drag preview about to be
    /// replaced. A user on a fast link can still pick `Full` explicitly.
    #[default]
    Quarter,
    /// A custom percentage of the session's full resolution, `1..=100`. Values outside
    /// that range are clamped by [`PreviewScale::percent`] rather than rejected, so a
    /// hand-edited or migrated settings file stays loadable.
    Custom(u32),
}

impl PreviewScale {
    /// This scale as an integer percentage of the full render resolution, clamped to
    /// `1..=100` (a `0%` preview would be a zero-area request the worker would reject).
    #[must_use]
    pub const fn percent(self) -> u32 {
        match self {
            Self::Full => 100,
            Self::Half => 50,
            Self::Quarter => 25,
            Self::Custom(p) => {
                if p == 0 {
                    1
                } else if p > 100 {
                    100
                } else {
                    p
                }
            }
        }
    }

    /// Resolves this scale against a session-wide `width x height` render resolution,
    /// floored at `1x1` so a worker is never asked for a zero-area preview regardless
    /// of how small `width`/`height` are.
    #[must_use]
    pub fn resolve(self, width: u32, height: u32) -> (u32, u32) {
        let pct = self.percent();
        let w = (width * pct / 100).max(1);
        let h = (height * pct / 100).max(1);
        (w, h)
    }
}

/// The resolution reduction applied while the camera is moving, for local
/// preview-then-settle rendering -- the same idea [`PreviewScale`] offers a remote
/// worker's `PREVIEW` stream, applied to the LOCAL render loop instead (see
/// `bridge::local_preview::effective_dimensions`), deliberately without a
/// [`PreviewScale::Custom`] equivalent since the settings-dialog control is a
/// discrete pill choice, not a slider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LocalPreviewScale {
    /// No reduction: traces at the full configured resolution regardless of camera
    /// movement -- bit-identical to this crate's behaviour before this control
    /// existed. The default.
    #[default]
    Off,
    Half,
    Quarter,
}

impl LocalPreviewScale {
    /// The integer divisor applied to each configured dimension while the camera is
    /// moving. `Off` divides by `1` -- a caller that forgets to special-case `Off`
    /// still gets the correct answer.
    #[must_use]
    pub const fn divisor(self) -> u32 {
        match self {
            Self::Off => 1,
            Self::Half => 2,
            Self::Quarter => 4,
        }
    }
}

/// Which engine(s) should contribute to the LIVE viewport once the camera settles --
/// the live-rendering analogue of `bridge::export_thread::ComputeTarget`, offered as
/// the same Local/Remote/Local+Remote choice but kept as a separate type since the two
/// features' callers live in disjoint module trees.
///
/// Unlike the export dialog's `compute_target` (re-derived fresh every dialog open,
/// never persisted), this IS a persisted `AppSettings` field: a standing preference,
/// not a one-shot dialog default. `Both` is deliberately the `Default` -- see
/// [`super::app_settings::DEFAULT_LIVE_COMPUTE_TARGET`] for why that's safe even
/// before any worker is configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LiveComputeTarget {
    LocalOnly,
    RemoteOnly,
    #[default]
    Both,
}

/// The default per-request cadence a newly added [`WorkerSettings`] starts with,
/// before clamping to a specific worker's advertised `Welcome::min_cadence_ms` floor
/// -- see [`WorkerSettings::effective_cadence_ms`].
pub const DEFAULT_WORKER_CADENCE_MS: u32 = 500;

/// One configured remote render worker: the connection details and per-request
/// preferences for a specific machine and link.
///
/// Deliberately does NOT include render resolution -- that stays session-wide (set
/// once, shared by every worker and the local CPU path), since every worker must
/// trace identical dimensions or the summed samples don't compose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkerSettings {
    /// User-facing label for this worker in the list UI. Not required to be unique --
    /// the worker-list panel addresses entries by position in `AppSettings::remote_workers`.
    pub name: String,
    /// `host:port` this worker listens on (`indicatrix-worker serve --bind`/`--allow-remote`).
    pub address: String,
    /// Path to the directory holding this worker's mutual-TLS certificate bundle --
    /// `ca.pem`, `client.pem`, and `client.key`, as `indicatrix-worker cert
    /// issue-client` produces them. See [`WorkerSettings::ca_path`] /
    /// [`WorkerSettings::client_cert_path`] / [`WorkerSettings::client_key_path`].
    ///
    /// A plain `String`, not `PathBuf` -- matches every other user-facing text field
    /// in this file and keeps this struct trivially TOML-serializable.
    pub cert_dir: String,
    pub transfer_mode: TransferMode,
    /// This worker's requested cadence, in milliseconds, before clamping to its own
    /// advertised floor -- see [`WorkerSettings::effective_cadence_ms`].
    pub cadence_ms: u32,
    pub preview_scale: PreviewScale,
}

impl Default for WorkerSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            cert_dir: String::new(),
            transfer_mode: TransferMode::LiveProgressive,
            cadence_ms: DEFAULT_WORKER_CADENCE_MS,
            // See `PreviewScale::Quarter` for why a new worker starts at quarter res.
            preview_scale: PreviewScale::Quarter,
        }
    }
}

impl WorkerSettings {
    #[must_use]
    pub fn ca_path(&self) -> PathBuf {
        Path::new(&self.cert_dir).join("ca.pem")
    }

    #[must_use]
    pub fn client_cert_path(&self) -> PathBuf {
        Path::new(&self.cert_dir).join("client.pem")
    }

    #[must_use]
    pub fn client_key_path(&self) -> PathBuf {
        Path::new(&self.cert_dir).join("client.key")
    }

    /// Clamps [`WorkerSettings::cadence_ms`] to `min_cadence_ms` -- a worker's own
    /// advertised floor from its `Welcome::min_cadence_ms`. Requesting faster than a
    /// worker can deliver just means it coalesces anyway, but clamping up front avoids
    /// the false impression of a faster cadence than will ever be observed.
    #[must_use]
    pub const fn effective_cadence_ms(&self, min_cadence_ms: u32) -> u32 {
        if self.cadence_ms < min_cadence_ms {
            min_cadence_ms
        } else {
            self.cadence_ms
        }
    }

    /// Builds the [`StreamConfig`] this worker's settings imply for a render at the
    /// session's `width x height` resolution, against a specific worker's advertised
    /// `min_cadence_ms` (from its `Welcome`).
    #[must_use]
    pub fn stream_config(&self, min_cadence_ms: u32, width: u32, height: u32) -> StreamConfig {
        let (preview_width, preview_height) = self.preview_scale.resolve(width, height);
        StreamConfig {
            transfer_mode: self.transfer_mode,
            cadence_ms: self.effective_cadence_ms(min_cadence_ms),
            preview: Some(PreviewConfig {
                width: preview_width,
                height: preview_height,
            }),
        }
    }

    /// The export's own [`StreamConfig`]: differs from [`Self::stream_config`] in three
    /// ways -- `preview: None`, a much larger export-specific cadence floor (see
    /// [`EXPORT_CADENCE_MS`]), and ALWAYS [`TransferMode::FinalOnly`], whatever this
    /// worker's own `transfer_mode` says. A separate method rather than a parameter on
    /// `stream_config`, since the live viewport legitimately uses a preview stream and
    /// progressive frames, and that caller must stay unchanged.
    ///
    /// # Why the transfer mode is forced, not inherited
    ///
    /// A progressive `FRAME` delta is always the full `width * height * 12` bytes,
    /// however few samples it carries: ~25 MB at 1080p, ~100 MB at 4K. Under
    /// `LiveProgressive` the worker emits one every cadence tick, and while a 100 MB
    /// write is draining over a wireless link (~5-8 s at 150 Mbit/s) the worker can
    /// neither trace nor heartbeat. The export's chunk watchdog
    /// (`export_thread::remote::dispatch`) then saw gaps at or past its 8 s steady-state
    /// deadline, declared the worker silent, and the export finished locally -- while
    /// 1080p, whose frames drain in ~1.5 s, worked. The export never displays those
    /// intermediate frames anyway (its thumbnail is built from whatever has been
    /// MERGED, chunk by chunk), so it only ever needs one `FRAME` per chunk: with
    /// `FinalOnly` the worker traces continuously, sends the bare `PROGRESS` heartbeat
    /// on every cadence tick, and transfers each chunk's radiance exactly once.
    ///
    /// The export never reads a preview stream: `export_thread::remote::run_remote_batch`
    /// ignores every `RemoteUpdate::Preview`, building its progress thumbnail from the
    /// FULL-resolution FRAME buffer instead. A worker-rendered preview would cost real
    /// bandwidth (up to a second full-resolution payload per emission at
    /// [`PreviewScale::Full`]) for a stream nothing reads, so `preview: None` removes
    /// that waste for the export path only.
    #[must_use]
    pub fn export_stream_config(&self, min_cadence_ms: u32) -> StreamConfig {
        StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: self
                .effective_cadence_ms(min_cadence_ms)
                .max(EXPORT_CADENCE_MS),
            preview: None,
        }
    }
}

/// A cadence floor for [`WorkerSettings::export_stream_config`] only, clamped up from
/// whatever [`WorkerSettings::effective_cadence_ms`] would otherwise pick (tuned for
/// the live viewport's smaller interactive payloads, not a full-resolution export).
///
/// A `FRAME` payload is always `width * height * 12` bytes regardless of how few new
/// samples it carries -- the emitter coalesces everything since the last emission into
/// one fixed-size payload, so cadence here controls transfer count, not size. At
/// 1920x1080 that's ~24.9 MB per `FRAME`; over a 200 Mbit/s link, one `FRAME` takes
/// ~1 second to drain, making the live viewport's 100ms-scale cadence unreachable
/// anyway -- so requesting a larger cadence up front is a straight win: 2000ms is 20x
/// fewer full-resolution transfers for the same total samples delivered.
pub const EXPORT_CADENCE_MS: u32 = 2000;
