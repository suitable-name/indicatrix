//! Remote rendering wiring: worker-list CRUD, "Test connection", token-based worker
//! enrollment, the global denoise toggle, and the preview-then-handoff orchestrator that
//! drives `bridge::handoff::HandoffMachine` from real camera-pose polling and dispatches
//! to `bridge::remote_render`.
//!
//! Split into [`worker_settings`] (`WorkerItem`<->`WorkerSettings` conversions),
//! [`worker_callbacks`] (worker-list CRUD, "Test connection", token redemption, the
//! denoise toggle), and [`orchestrator`] (the preview-then-handoff state machine,
//! including the async guide/denoise generation it drives).
//!
//! `orchestrator::poll_tick` is also the one place that decides "is the camera currently
//! moving" for `bridge::local_preview` -- it writes `RenderContext::camera_moving` from
//! this same `HandoffMachine` instance's state every tick, so both features share one
//! definition of "settled" rather than each running its own debounce timer.
//!
//! Everything this module calls into is already unit-tested without a GUI or socket
//! (`bridge::handoff::HandoffMachine`, `indicatrix_net::client`, `bridge::enroll`,
//! `settings::WorkerSettings`); this module is the glue wiring those tested pieces to
//! real Slint callbacks, a real `slint::Timer`, and a real socket -- it compiles but,
//! like the rest of the GUI layer, is not exercised by an automated test here.

mod orchestrator;
mod worker_callbacks;
mod worker_settings;

pub use orchestrator::setup_remote_rendering;
pub use worker_callbacks::setup_worker_callbacks;
pub use worker_settings::refresh_worker_options;

use crate::{gui::render::sample_scale, settings::LiveComputeTarget};

/// Slider bounds for the user-configurable remote sample budget. Wider than the local
/// interactive target's own `sample_scale::MIN_EXPONENT..=MAX_EXPONENT` (`8..=1024`
/// samples): a remote render is a one-shot converge-then-display request on
/// (potentially) much more capable hardware, not an ongoing 30-60fps interactive loop,
/// so its ceiling is set past the local slider's -- `2^13 = 8192` samples, 8x the local
/// ceiling; the floor, `2^7 = 128`, is still a legitimate "full quality" one-shot for a
/// quick connectivity check or a modest worker. `RenderContext::remote_render_samples`'s
/// default (`512`) sits comfortably inside this range.
pub const REMOTE_SAMPLES_MIN_EXPONENT: u32 = 7; // 128 samples
pub const REMOTE_SAMPLES_MAX_EXPONENT: u32 = 13; // 8192 samples

/// Converts the "Remote Render Samples" slider's exponent to the actual sample count,
/// reusing `gui::sample_scale`'s bounded exponent<->count mapping clamped to this
/// control's own, wider range.
#[must_use]
pub const fn remote_samples_exponent_to_count(exponent: u32) -> u32 {
    sample_scale::exponent_to_count_bounded(
        exponent,
        REMOTE_SAMPLES_MIN_EXPONENT,
        REMOTE_SAMPLES_MAX_EXPONENT,
    )
}

/// Inverse of [`remote_samples_exponent_to_count`], used at startup to turn a persisted
/// `AppSettings::remote_render_samples` count back into the slider's starting exponent.
#[must_use]
pub fn remote_samples_count_to_exponent(count: u32) -> u32 {
    sample_scale::count_to_exponent_bounded(
        count,
        REMOTE_SAMPLES_MIN_EXPONENT,
        REMOTE_SAMPLES_MAX_EXPONENT,
    )
}

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

    // ---- Remote render sample budget ----------------------------------

    #[test]
    fn remote_samples_round_trip_the_default_and_endpoints() {
        assert_eq!(
            remote_samples_count_to_exponent(remote_samples_exponent_to_count(
                REMOTE_SAMPLES_MIN_EXPONENT
            )),
            REMOTE_SAMPLES_MIN_EXPONENT
        );
        assert_eq!(
            remote_samples_count_to_exponent(remote_samples_exponent_to_count(
                REMOTE_SAMPLES_MAX_EXPONENT
            )),
            REMOTE_SAMPLES_MAX_EXPONENT
        );
        // 512, this control's persisted default, must resolve to some legal exponent
        // inside the slider's range and round-trip.
        let exponent = remote_samples_count_to_exponent(512);
        assert_eq!(remote_samples_exponent_to_count(exponent), 512);
    }

    #[test]
    fn remote_samples_exponent_to_count_matches_expected_powers_of_two() {
        assert_eq!(remote_samples_exponent_to_count(7), 128);
        assert_eq!(remote_samples_exponent_to_count(9), 512);
        assert_eq!(remote_samples_exponent_to_count(13), 8192);
    }

    #[test]
    fn remote_samples_range_exceeds_the_local_interactive_targets_own_ceiling() {
        // Must exceed the local interactive target's ceiling (1024 samples), since a
        // remote render is a one-shot request, not an ongoing interactive loop.
        assert!(remote_samples_exponent_to_count(REMOTE_SAMPLES_MAX_EXPONENT) > 1024);
    }
}
