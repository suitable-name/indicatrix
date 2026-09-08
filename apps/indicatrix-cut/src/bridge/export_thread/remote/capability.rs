//! Whether remote compute is even available for this export: probing a worker
//! ([`probe_remote`]) and checking its advertised limits against the requested export
//! ([`exceeds_pixel_cap`]). See this group's own `mod.rs` doc comment.

use crate::{bridge::remote::remote_render, settings::WorkerSettings};
use indicatrix_net::messages::RenderCapability;

/// Below this many REMAINING samples, dispatching to a remote worker at all (handshake
/// plus a calibration round trip) costs more than it could ever save -- same reasoning
/// as `batch::HYBRID_MIN_SPP`, for a link whose round-trip latency is typically far
/// higher than an in-process dispatch's.
pub(in crate::bridge::export_thread) const REMOTE_MIN_SPP: u32 = 32;

/// Why remote compute is unavailable for this export -- surfaced to the export dialog
/// verbatim (via [`RemoteUnavailable::message`]) so a disabled pill always explains
/// itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteUnavailable {
    NoWorkerConfigured,
    Unreachable(String),
    /// `Welcome::render` was `None` -- the worker is a library-only build. Mirrors
    /// `bridge::remote::remote_render::RemoteError::NoRenderCapacity`.
    LibraryOnly,
}

impl RemoteUnavailable {
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoWorkerConfigured => "No remote worker is configured.".to_string(),
            Self::Unreachable(e) => format!("Remote worker unreachable ({e})."),
            Self::LibraryOnly => {
                "The configured worker serves the design library only -- it has no \
                 render capacity."
                    .to_string()
            }
        }
    }
}

/// A remote worker confirmed reachable and render-capable, with everything a
/// [`indicatrix_net::messages::RenderRequest`] against it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCapability {
    pub worker: WorkerSettings,
    /// This worker's advertised `RenderCapability::max_pixels` -- checked against the
    /// export's `width * height` BEFORE ever dispatching, never assumed to be the
    /// hardcoded `indicatrix-worker` default. See `run_export`'s pixel-cap fallback.
    pub max_pixels: u32,
}

/// Whether an export at `width x height` exceeds `capability`'s advertised
/// `max_pixels`, checked BEFORE ever dispatching and always against the worker's own
/// advertised cap rather than a hardcoded constant.
#[must_use]
pub(in crate::bridge::export_thread) fn exceeds_pixel_cap(
    width: u32,
    height: u32,
    capability: &RemoteCapability,
) -> bool {
    u64::from(width) * u64::from(height) > u64::from(capability.max_pixels)
}

/// Probes the first configured worker -- the same "session-wide, first entry"
/// convention `gui::remote::orchestrator::poll_tick` uses -- for render capacity.
/// Blocking (a real TLS handshake); callers run this off the UI thread.
///
/// # Errors
///
/// See [`RemoteUnavailable`]'s variants.
pub fn probe_remote(workers: &[WorkerSettings]) -> Result<RemoteCapability, RemoteUnavailable> {
    let Some(worker) = workers.first().cloned() else {
        return Err(RemoteUnavailable::NoWorkerConfigured);
    };
    match remote_render::connect_and_handshake(&worker) {
        Ok((_stream, welcome)) => match welcome.render {
            Some(RenderCapability { max_pixels, .. }) => {
                Ok(RemoteCapability { worker, max_pixels })
            }
            None => Err(RemoteUnavailable::LibraryOnly),
        },
        Err(e) => Err(RemoteUnavailable::Unreachable(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_remote_reports_no_worker_configured_when_the_list_is_empty() {
        assert_eq!(
            probe_remote(&[]),
            Err(RemoteUnavailable::NoWorkerConfigured)
        );
    }

    #[test]
    fn probe_remote_reports_unreachable_for_a_bogus_address() {
        let worker = WorkerSettings {
            address: "127.0.0.1:1".to_string(), // nothing listens here
            cert_dir: std::env::temp_dir().display().to_string(),
            ..WorkerSettings::default()
        };
        let err = probe_remote(std::slice::from_ref(&worker)).unwrap_err();
        assert!(matches!(err, RemoteUnavailable::Unreachable(_))); // nothing listens here
    }

    fn capability(max_pixels: u32) -> RemoteCapability {
        RemoteCapability {
            worker: WorkerSettings::default(),
            max_pixels,
        }
    }

    #[test]
    fn exceeds_pixel_cap_uses_the_workers_own_advertised_cap_not_a_hardcoded_constant() {
        // A smaller-than-default advertised cap must still be honoured.
        let small_worker = capability(100);
        assert!(exceeds_pixel_cap(11, 10, &small_worker)); // 110 > 100
        assert!(!exceeds_pixel_cap(10, 10, &small_worker)); // 100 == 100, not over

        let real_default = capability(7680 * 4320);
        assert!(!exceeds_pixel_cap(3840, 2160, &real_default)); // 4K fits
        assert!(exceeds_pixel_cap(8192, 8192, &real_default)); // max custom export size does not
    }

    #[test]
    fn remote_unavailable_messages_are_distinct_and_human_readable() {
        assert!(
            RemoteUnavailable::NoWorkerConfigured
                .message()
                .contains("No remote worker")
        );
        assert!(
            RemoteUnavailable::LibraryOnly
                .message()
                .contains("library only")
        );
        assert!(
            RemoteUnavailable::Unreachable("boom".to_string())
                .message()
                .contains("boom")
        );
    }
}
