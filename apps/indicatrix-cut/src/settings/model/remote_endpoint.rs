//! [`RemoteEndpoint`]: the ONE remote this viewer talks to (a coordinator, rendering
//! itself when started with `--render`), plus the two "final picture only" transfer
//! preferences ([`ExportTransfer`], [`LiveTransfer`]) and the one-time migration from
//! the old multi-worker list ([`migrate_legacy_workers`]).

use super::worker::WorkerSettings;
use serde::{Deserialize, Serialize};

/// How a still export or a tilt video gets its result from the remote.
///
/// `FullData` is the behaviour every earlier build had: the remote streams float
/// radiance, the viewer merges it with its own local lanes and tone-maps. `FinalPicture`
/// asks the remote for the whole job as one finished 8-bit PNG (`FinalImageRequest`) --
/// roughly a tenth of the bytes over the home link, but the local CPU/GPU do not take
/// part. A server that doesn't support it refuses with `UNSUPPORTED_REQUEST` and the
/// viewer falls back to `FullData` with a note.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ExportTransfer {
    /// Float radiance, merged with the local lanes (today's behaviour, the default).
    #[default]
    FullData,
    /// One finished PNG per image (per frame for a video); local lanes stay idle.
    FinalPicture,
}

impl ExportTransfer {
    /// Index of the "Transfer" pill (0 = Full data, 1 = Final picture only).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::FullData => 0,
            Self::FinalPicture => 1,
        }
    }

    /// Inverse of [`Self::index`]; anything but `1` is `FullData`.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        if index == 1 {
            Self::FinalPicture
        } else {
            Self::FullData
        }
    }
}

/// Which computers render a tilt video: the "Compute" pill of the video section, remembered
/// across restarts. Mirrors `export_thread::ComputeTarget` without the settings layer
/// depending on the bridge; the pill order is the still-image export's own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum TiltVideoCompute {
    /// This computer only.
    Local,
    /// The remote only: nothing is traced on this computer.
    Remote,
    /// This computer and the remote together (today's behaviour, the default).
    #[default]
    Both,
}

impl TiltVideoCompute {
    /// Index of the "Compute" pill (0 = Local only, 1 = Remote only, 2 = Local + Remote).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Local => 0,
            Self::Remote => 1,
            Self::Both => 2,
        }
    }

    /// Inverse of [`Self::index`]; anything unknown is `Both`.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            0 => Self::Local,
            1 => Self::Remote,
            _ => Self::Both,
        }
    }
}

/// How the live view gets remote contributions once the camera settles.
///
/// `FullData` (the default) is today's behaviour: float deltas, combined with local
/// samples under `LiveComputeTarget::Both`. `FinalPicture` asks for finished, denoised
/// 8-bit frames (`TransferMode::DisplayOnly`): less bandwidth, but 8-bit pictures cannot
/// be merged with local samples, so the settled image is remote-only and local tracing
/// pauses after the handoff exactly like `LiveComputeTarget::RemoteOnly`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LiveTransfer {
    /// Float deltas, combinable with local samples (the default).
    #[default]
    FullData,
    /// Finished, denoised display frames; the settled image is remote-only.
    FinalPicture,
}

impl LiveTransfer {
    /// Index of the settings dialog's "Live transfer" pill (0 = Full data, 1 = Final
    /// picture).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::FullData => 0,
            Self::FinalPicture => 1,
        }
    }

    /// Inverse of [`Self::index`]; anything but `1` is `FullData`.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        if index == 1 {
            Self::FinalPicture
        } else {
            Self::FullData
        }
    }
}

/// The one configured remote: connection details (the same [`WorkerSettings`] shape the
/// connection code has always taken) plus the "final picture only" transfer preferences.
///
/// Stored as `[settings.remote]` with the connection in `[settings.remote.connection]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct RemoteEndpoint {
    /// Name, address, certificate folder and live-stream preferences.
    pub connection: WorkerSettings,
    /// The default "Transfer" choice of the export dialog and the tilt-video section.
    pub export_transfer: ExportTransfer,
    /// The live view's transfer choice (settings dialog, next to "Live Compute").
    pub live_transfer: LiveTransfer,
}

impl RemoteEndpoint {
    /// An endpoint for `connection` with both transfer preferences at their defaults.
    #[must_use]
    pub fn new(connection: WorkerSettings) -> Self {
        Self {
            connection,
            ..Self::default()
        }
    }
}

/// What [`migrate_legacy_workers`] did with an old `remote_workers` list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LegacyWorkerMigration {
    /// The address of the entry that became the remote endpoint (`None` when an endpoint
    /// was already configured and every legacy entry was dropped).
    pub kept: Option<String>,
    /// Addresses of the entries that were dropped (every entry after the first).
    pub dropped: Vec<String>,
}

/// Folds an old multi-worker list into the single-endpoint model: the FIRST entry
/// becomes `remote` (unless one is already configured) and every other entry is dropped,
/// logged by address so nothing disappears silently. `None` when there was nothing to
/// migrate. Every render feature already used only the first entry, so the kept one is
/// exactly the worker the app was rendering with.
pub fn migrate_legacy_workers(
    legacy: &mut Vec<WorkerSettings>,
    remote: &mut Option<RemoteEndpoint>,
) -> Option<LegacyWorkerMigration> {
    if legacy.is_empty() {
        return None;
    }
    let mut entries = std::mem::take(legacy).into_iter();
    let first = entries.next()?;
    let mut dropped: Vec<String> = entries.map(|w| w.address).collect();
    let kept = if remote.is_some() {
        dropped.insert(0, first.address);
        None
    } else {
        let address = first.address.clone();
        *remote = Some(RemoteEndpoint::new(first));
        Some(address)
    };
    if let Some(address) = &kept {
        tracing::info!(
            "settings: migrated the remote-worker list to a single remote endpoint ({address})"
        );
    }
    if !dropped.is_empty() {
        tracing::warn!(
            "settings: only one remote endpoint is supported now; dropped {} older \
             worker entr{}: {}",
            dropped.len(),
            if dropped.len() == 1 { "y" } else { "ies" },
            dropped.join(", ")
        );
    }
    Some(LegacyWorkerMigration { kept, dropped })
}
