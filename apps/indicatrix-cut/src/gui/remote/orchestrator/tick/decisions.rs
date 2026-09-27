//! Pure, Slint-free decisions the poll tick acts on, split out so they are unit-tested
//! without a window: whether a settled view should re-dispatch remote work after a
//! non-drag release ([`should_redispatch`]), and what the "served by" indicator should
//! say ([`served_by_label`]).

use crate::settings::{LiveComputeTarget, LiveTransfer};

/// Whether a settled epoch asks the remote for finished, denoised display frames
/// (`TransferMode::DisplayOnly`) instead of float deltas: only when
/// the user chose "Final picture", remote takes part at all, and this connection has
/// not already refused it (`UNSUPPORTED_REQUEST` from a plain worker -- then full data
/// is used until the connection is replaced).
#[must_use]
pub(super) const fn wants_display_only(
    target: LiveComputeTarget,
    transfer: LiveTransfer,
    refused_on_this_connection: bool,
) -> bool {
    matches!(transfer, LiveTransfer::FinalPicture)
        && !matches!(target, LiveComputeTarget::LocalOnly)
        && !refused_on_this_connection
}

/// Everything [`should_redispatch`] looks at, as plain booleans read by the poll tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RedispatchInputs {
    /// The handoff machine is `Idle`: no drag, no settle or epoch in progress.
    pub(super) handoff_idle: bool,
    /// The rendered image is visible somewhere (never spend worker time otherwise).
    pub(super) tab_visible: bool,
    /// An epoch is still live (in `RenderContext` or as the orchestrator's lane) --
    /// finished, gave up or not, it owns the image; nothing to re-dispatch.
    pub(super) epoch_live: bool,
    /// The scene generation has been unchanged for the settle debounce, so a burst of
    /// edits (a slider drag) does not dispatch and cancel an epoch per tick.
    pub(super) scene_stable: bool,
    /// The last epoch for exactly this scene and compute target gave up after
    /// repeated remote failures -- no retry until the scene or mode changes.
    pub(super) gave_up_for_this_scene: bool,
}

/// Whether the poll tick should start a fresh epoch without waiting for a drag: the
/// view is settled and visible, its previous epoch was released by something other
/// than a drag (a scene or settings change, a compute-target change, or the render
/// loop's scene-identity check), the scene is stable, and the remote lane did not
/// already give up on this very scene. Whether remote is ALLOWED at all (compute
/// target, configured worker, HDR guard) is decided afterwards by
/// `bridge::remote::live_remote_dispatch`, exactly as for a settle.
#[must_use]
pub(super) const fn should_redispatch(inputs: RedispatchInputs) -> bool {
    inputs.handoff_idle
        && inputs.tab_visible
        && !inputs.epoch_live
        && inputs.scene_stable
        && !inputs.gave_up_for_this_scene
}

/// What the "served by" indicator should name, `None` meaning "rendered locally".
///
/// - `Both`: the settled image is ALWAYS a combination while remote has contributed
///   anything to the live epoch (`combined_remote_samples > 0`), and stays one after the
///   remote lane finishes -- local and remote samples are both in it. It never flips to
///   a remote-only label.
/// - `RemoteOnly`: the worker once its render completed (the handoff machine's
///   `served_by`), as before.
/// - `LocalOnly`: always local.
#[must_use]
pub(super) fn served_by_label(
    mode: LiveComputeTarget,
    handoff_says_remote: bool,
    combined_remote_samples: u32,
    worker_label: &str,
) -> Option<String> {
    match mode {
        LiveComputeTarget::Both => (combined_remote_samples > 0).then(|| {
            if worker_label.is_empty() {
                "Local + Remote".to_string()
            } else {
                format!("Local + Remote ({worker_label})")
            }
        }),
        LiveComputeTarget::RemoteOnly => handoff_says_remote.then(|| worker_label.to_string()),
        LiveComputeTarget::LocalOnly => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const READY: RedispatchInputs = RedispatchInputs {
        handoff_idle: true,
        tab_visible: true,
        epoch_live: false,
        scene_stable: true,
        gave_up_for_this_scene: false,
    };

    #[test]
    fn a_settled_visible_view_with_a_released_epoch_redispatches() {
        assert!(should_redispatch(READY));
    }

    #[test]
    fn each_blocking_condition_alone_prevents_a_redispatch() {
        let blocked = [
            RedispatchInputs {
                handoff_idle: false,
                ..READY
            },
            RedispatchInputs {
                tab_visible: false,
                ..READY
            },
            RedispatchInputs {
                epoch_live: true,
                ..READY
            },
            RedispatchInputs {
                scene_stable: false,
                ..READY
            },
            RedispatchInputs {
                gave_up_for_this_scene: true,
                ..READY
            },
        ];
        for inputs in blocked {
            assert!(!should_redispatch(inputs), "{inputs:?}");
        }
    }

    /// The give-up is respected: the same scene never re-dispatches, but a changed
    /// scene (a new epoch) may.
    #[test]
    fn a_given_up_scene_is_not_retried_but_a_new_scene_is() {
        let same_scene = RedispatchInputs {
            gave_up_for_this_scene: true,
            ..READY
        };
        assert!(!should_redispatch(same_scene));
        assert!(should_redispatch(RedispatchInputs {
            gave_up_for_this_scene: false,
            ..same_scene
        }));
    }

    #[test]
    fn both_mode_names_the_combination_and_never_flips_to_remote_only() {
        let label = served_by_label(LiveComputeTarget::Both, false, 40, "GPU (X)");
        assert_eq!(label.as_deref(), Some("Local + Remote (GPU (X))"));
        // The lane finished (the handoff machine says Remote): still a combination.
        let finished = served_by_label(LiveComputeTarget::Both, true, 256, "GPU (X)");
        assert_eq!(finished.as_deref(), Some("Local + Remote (GPU (X))"));
        // Nothing remote in the image (yet, or after a release): local.
        assert_eq!(
            served_by_label(LiveComputeTarget::Both, true, 0, "GPU (X)"),
            None
        );
        assert_eq!(
            served_by_label(LiveComputeTarget::Both, false, 3, "").as_deref(),
            Some("Local + Remote")
        );
    }

    /// Final picture is used exactly when chosen, remote takes part, and the
    /// connection has not refused it; a refusal falls back to full data.
    #[test]
    fn display_only_follows_the_live_transfer_until_the_connection_refuses_it() {
        use LiveComputeTarget::{Both, LocalOnly, RemoteOnly};
        use LiveTransfer::{FinalPicture, FullData};
        assert!(wants_display_only(Both, FinalPicture, false));
        assert!(wants_display_only(RemoteOnly, FinalPicture, false));
        assert!(!wants_display_only(LocalOnly, FinalPicture, false));
        assert!(!wants_display_only(Both, FullData, false));
        assert!(
            !wants_display_only(Both, FinalPicture, true),
            "an UNSUPPORTED_REQUEST refusal falls back to full data"
        );
    }

    #[test]
    fn remote_only_and_local_only_keep_their_labels() {
        assert_eq!(
            served_by_label(LiveComputeTarget::RemoteOnly, true, 0, "CPU, 8 threads").as_deref(),
            Some("CPU, 8 threads")
        );
        assert_eq!(
            served_by_label(LiveComputeTarget::RemoteOnly, false, 99, "CPU"),
            None
        );
        assert_eq!(
            served_by_label(LiveComputeTarget::LocalOnly, true, 99, "CPU"),
            None
        );
    }
}
