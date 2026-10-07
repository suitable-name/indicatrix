//! Remote rendering wiring: the "Remote coordinator" form (one remote endpoint),
//! "Test connection", token-based enrollment, the global denoise toggle, and the
//! preview-then-handoff orchestrator that drives `bridge::remote::handoff::HandoffMachine`
//! from real camera-pose polling and dispatches to `bridge::remote::remote_render`.
//!
//! Split into [`worker_settings`] (`WorkerItem`<->`RemoteEndpoint` conversions),
//! [`worker_callbacks`] (save/remove, "Test connection", token redemption, the
//! denoise toggle, live compute target and live transfer), and [`orchestrator`] (the
//! preview-then-handoff state machine, including the async guide/denoise generation
//! it drives).
//!
//! `orchestrator::poll_tick` is also the one place that decides "is the camera currently
//! moving" for `bridge::render_thread::local_preview` -- it writes
//! `RenderContext::camera_moving` from this same `HandoffMachine` instance's state every
//! tick, so both features share one definition of "settled" rather than each running
//! its own debounce timer.
//!
//! Everything this module calls into is already unit-tested without a GUI or socket
//! (`bridge::remote::handoff::HandoffMachine`, `indicatrix_net::client`,
//! `bridge::remote::enroll`, `settings::WorkerSettings`); this module is the glue
//! wiring those tested pieces to real Slint callbacks, a real `slint::Timer`, and a
//! real socket -- it compiles but, like the rest of the GUI layer, is not exercised by
//! an automated test here.

mod advanced_in_use;
mod orchestrator;
mod worker_callbacks;
mod worker_settings;

pub use orchestrator::{RemoteOrchestratorHandle, setup_remote_rendering};
pub use worker_callbacks::setup_worker_callbacks;
pub use worker_settings::refresh_remote_ui;

use crate::settings::LiveComputeTarget;

/// `settings_dialog.slint`'s "Live Compute" pill index (0/1/2) -> [`LiveComputeTarget`].
/// Mirrors `gui::render_export::compute_target_from_index`'s int-discriminant
/// convention for the export dialog's own Compute pill, duplicated rather than shared
/// since the two pickers carry different enum types.
#[must_use]
pub const fn live_compute_target_from_index(index: i32) -> LiveComputeTarget {
    match index {
        0 => LiveComputeTarget::LocalOnly,
        1 => LiveComputeTarget::RemoteOnly,
        _ => LiveComputeTarget::Both,
    }
}

/// Inverse of [`live_compute_target_from_index`], used at startup to seed the pill from
/// a persisted `AppSettings::live_compute_target`.
#[must_use]
pub const fn live_compute_target_index(target: LiveComputeTarget) -> i32 {
    match target {
        LiveComputeTarget::LocalOnly => 0,
        LiveComputeTarget::RemoteOnly => 1,
        LiveComputeTarget::Both => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- Live rendering compute-target picker --------------------------

    #[test]
    fn live_compute_target_index_and_from_index_round_trip() {
        for target in [
            LiveComputeTarget::LocalOnly,
            LiveComputeTarget::RemoteOnly,
            LiveComputeTarget::Both,
        ] {
            let idx = live_compute_target_index(target);
            assert_eq!(live_compute_target_from_index(idx), target);
        }
    }

    #[test]
    fn live_compute_target_from_index_matches_expected_pills() {
        assert_eq!(
            live_compute_target_from_index(0),
            LiveComputeTarget::LocalOnly
        );
        assert_eq!(
            live_compute_target_from_index(1),
            LiveComputeTarget::RemoteOnly
        );
        assert_eq!(live_compute_target_from_index(2), LiveComputeTarget::Both);
    }

    #[test]
    fn live_compute_target_from_index_falls_back_to_both_for_unknown_values() {
        // Falls back to `Both`, matching `LiveComputeTarget::default()` and this
        // control's `2` initial pill state.
        assert_eq!(live_compute_target_from_index(-1), LiveComputeTarget::Both);
        assert_eq!(live_compute_target_from_index(99), LiveComputeTarget::Both);
    }
}
