//! Pure, Slint-free decisions the poll tick acts on, split out so they are unit-tested
//! without a window: whether a settled view should re-dispatch remote work after a
//! non-drag release ([`should_redispatch`]), whether the live remote lane may transmit
//! anything at all right now ([`live_remote_may_run`]), and what the "served by"
//! indicator should say ([`served_by_label`]).

use crate::settings::{LiveComputeTarget, LiveTransfer};
use std::time::Duration;

/// Everything [`live_remote_may_run`] looks at, as plain booleans read from
/// `RenderContext` by `poll::live_remote_allowed` (the impure wrapper every one of the
/// orchestrator's remote dispatch/continue/resume gates calls).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct LiveRemoteAllowedInputs {
    /// The rendered image is visible somewhere -- see `RenderContext::tab_visible`.
    pub(super) tab_visible: bool,
    /// The user paused live rendering -- see `RenderContext::paused`.
    pub(super) paused: bool,
    /// A high-resolution export/batch job is running -- see
    /// `RenderContext::export_active`.
    pub(super) export_active: bool,
}

/// Whether the live remote lane may transmit anything right now: visible, not paused,
/// and no export/batch job stealing the GPU/worker. The single gate every one of the
/// orchestrator's remote dispatch/continue/resume decisions applies -- see
/// `poll::live_remote_allowed` (the lock-reading wrapper) and `poll::
/// suspend_live_remote` (what happens on the `true -> false` edge).
#[must_use]
pub(super) const fn live_remote_may_run(inputs: LiveRemoteAllowedInputs) -> bool {
    inputs.tab_visible && !inputs.paused && !inputs.export_active
}

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
    /// Live rendering is paused or a high-resolution export/batch job is running --
    /// the same two conditions [`live_remote_may_run`] adds on top of `tab_visible`.
    /// Kept as its own field rather than folded into `tab_visible` itself, so a
    /// dragging-while-paused scene change still blocks a redispatch even though
    /// `tab_visible` alone would have allowed one -- see [`LiveRemoteAllowedInputs`].
    pub(super) suspended: bool,
    /// A mouse button is held for an orbit or light drag (`poll::drag_held_effective`,
    /// so a stale flag past the watchdog no longer counts) -- a redispatch now would
    /// be cancelled by the very next move.
    pub(super) drag_held: bool,
}

/// Whether a held mouse button still counts as held: a flag with no pose change for
/// `watchdog` or longer is treated as released (see `poll::DRAG_HELD_WATCHDOG`), which
/// bounds the cost of a release event that never reached the viewport to a delay
/// instead of a permanent low-resolution preview.
#[must_use]
pub(super) fn drag_held_effective(held: bool, quiet_for: Duration, watchdog: Duration) -> bool {
    held && quiet_for < watchdog
}

/// Whether the settle (full resolution, remote handoff) may fire this tick: the handoff
/// machine is `Previewing`, no mouse button is held, and the pose has been quiet -- since
/// the last pose change OR the last button release, whichever is later -- for at least
/// `debounce`.
#[must_use]
pub(super) fn pose_settle_due(
    previewing: bool,
    drag_held: bool,
    quiet_for: Duration,
    debounce: Duration,
) -> bool {
    previewing && !drag_held && quiet_for >= debounce
}

/// Whether the poll tick should start a fresh epoch without waiting for a drag: the
/// view is settled, visible, not suspended (paused or exporting), its previous epoch
/// was released by something other than a drag (a scene or settings change, a
/// compute-target change, or the render loop's scene-identity check), the scene is
/// stable, and the remote lane did not already give up on this very scene. Whether
/// remote is ALLOWED at all (compute target, configured worker, HDR guard) is decided
/// afterwards by `bridge::remote::live_remote_dispatch`, exactly as for a settle.
#[must_use]
pub(super) const fn should_redispatch(inputs: RedispatchInputs) -> bool {
    inputs.handoff_idle
        && inputs.tab_visible
        && !inputs.suspended
        && !inputs.drag_held
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
        suspended: false,
        drag_held: false,
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
            RedispatchInputs {
                suspended: true,
                ..READY
            },
            RedispatchInputs {
                drag_held: true,
                ..READY
            },
        ];
        for inputs in blocked {
            assert!(!should_redispatch(inputs), "{inputs:?}");
        }
    }

    // ---- pose_settle_due / drag_held_effective: the button-held settle gate ---------

    const DEBOUNCE: Duration = Duration::from_millis(250);
    const WATCHDOG: Duration = Duration::from_secs(20);

    #[test]
    fn a_held_button_blocks_the_settle_however_long_the_pose_rests() {
        assert!(!pose_settle_due(
            true,
            true,
            Duration::from_secs(10),
            DEBOUNCE
        ));
    }

    #[test]
    fn a_released_button_settles_after_the_debounce() {
        assert!(pose_settle_due(true, false, DEBOUNCE, DEBOUNCE));
        assert!(!pose_settle_due(
            true,
            false,
            Duration::from_millis(100),
            DEBOUNCE
        ));
    }

    #[test]
    fn an_idle_machine_never_settles() {
        assert!(!pose_settle_due(
            false,
            false,
            Duration::from_secs(10),
            DEBOUNCE
        ));
    }

    #[test]
    fn the_watchdog_releases_a_stale_held_flag() {
        assert!(!drag_held_effective(true, WATCHDOG, WATCHDOG));
        assert!(drag_held_effective(true, Duration::from_secs(19), WATCHDOG));
        assert!(!drag_held_effective(false, Duration::ZERO, WATCHDOG));
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

    /// The scenario `suspended` exists for: the scene changes (a new epoch releases the
    /// old one) while live rendering is paused or an export is running -- a redispatch
    /// must not fire just because `tab_visible` alone would have allowed one.
    #[test]
    fn a_scene_change_while_suspended_does_not_redispatch_until_unsuspended() {
        let while_suspended = RedispatchInputs {
            suspended: true,
            ..READY
        };
        assert!(!should_redispatch(while_suspended));
        assert!(should_redispatch(RedispatchInputs {
            suspended: false,
            ..while_suspended
        }));
    }

    // ---- live_remote_may_run: the single remote-transmission gate -------------------

    #[test]
    fn remote_may_run_only_when_visible_unpaused_and_not_exporting() {
        assert!(live_remote_may_run(LiveRemoteAllowedInputs {
            tab_visible: true,
            paused: false,
            export_active: false,
        }));
    }

    #[test]
    fn each_condition_alone_blocks_remote_from_running() {
        let blocked = [
            LiveRemoteAllowedInputs {
                tab_visible: false,
                paused: false,
                export_active: false,
            },
            LiveRemoteAllowedInputs {
                tab_visible: true,
                paused: true,
                export_active: false,
            },
            LiveRemoteAllowedInputs {
                tab_visible: true,
                paused: false,
                export_active: true,
            },
        ];
        for inputs in blocked {
            assert!(!live_remote_may_run(inputs), "{inputs:?}");
        }
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
