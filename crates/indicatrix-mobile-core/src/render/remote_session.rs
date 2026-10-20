//! The interactive remote session of one render epoch, with reconnection.
//!
//! A remote session is one TLS connection and one request; it cannot be resumed. When it
//! is interrupted the only correct recovery is a new connection that renders from sample
//! zero, which this module does:
//!
//! - the app leaving the foreground (an activity stop, a recreation) cancels the session;
//!   the epoch then waits for the app to return and starts over, without counting it as a
//!   failure;
//! - a lost transport (socket reset, stalled link) is retried up to [`MAX_RECONNECTS`]
//!   times with a growing pause;
//! - a refusal or invalid data ends the epoch with the endpoint's sentence, as before;
//! - a superseded epoch ends silently.

use std::{sync::Arc, time::Duration};

use indicatrix::renderer::{engine::EngineSpec, env_map::EnvironmentMap};
use indicatrix_web_core::scene::{CustomMaterialSpec, GeometrySceneSpec};

use super::pool::Shared;
use crate::remote::rendering::{self, FrameUpdate, RemoteConfig, RunError};

/// Reconnection attempts after a lost transport, before the epoch fails.
pub(super) const MAX_RECONNECTS: u32 = 3;

/// How often a waiting epoch looks at the app state.
const POLL: Duration = Duration::from_millis(250);

/// What to do after a session ended with an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Next {
    /// The epoch is no longer the current one: nothing to report.
    Stop,
    /// The app is in the background: wait for the foreground, then start over.
    WaitForeground,
    /// Start over after a pause.
    Retry,
    /// Report the error and end the epoch.
    GiveUp,
}

/// The pure retry decision. `attempts` is how many reconnects were already made.
pub(super) const fn next_step(
    error: &RunError,
    current: bool,
    background: bool,
    attempts: u32,
) -> Next {
    if !current {
        return Next::Stop;
    }
    match error {
        RunError::Failed(_) => Next::GiveUp,
        RunError::Interrupted(_) if background => Next::WaitForeground,
        RunError::Interrupted(_) if attempts >= MAX_RECONNECTS => Next::GiveUp,
        RunError::Interrupted(_) => Next::Retry,
    }
}

/// The sentence shown when the epoch gives up.
pub(super) fn give_up_message(error: &RunError, attempts: u32) -> String {
    if attempts == 0 {
        error.message().to_owned()
    } else {
        format!(
            "{} (the connection was lost and {attempts} reconnect attempts did not succeed; the render restarts from zero when retried)",
            error.message()
        )
    }
}

/// Runs the epoch's remote session to completion, reconnecting as described in the module
/// doc. Blocks the calling (render-aux) thread until the epoch finishes, fails or is
/// superseded.
#[expect(
    clippy::too_many_arguments,
    reason = "one remote epoch: the pool, the epoch, the endpoint, the scene with its material recipe and lighting map, and the sample plan; a bag struct would only rename them"
)]
pub(super) fn run(
    shared: &Shared,
    id: u64,
    remote: &RemoteConfig,
    spec: &GeometrySceneSpec,
    customs: &[CustomMaterialSpec],
    engine: EngineSpec,
    hdr: Option<&Arc<EnvironmentMap>>,
    target: u32,
    denoise: bool,
) {
    let mut attempts = 0;
    loop {
        let cancelled = || !shared.is_current(id) || shared.control().background;
        let result = rendering::run(
            remote.platform.as_ref(),
            &remote.settings,
            spec,
            customs,
            engine,
            hdr,
            target,
            denoise,
            indicatrix_net::messages::RequestIntent::Interactive,
            &shared.policy,
            &cancelled,
            &|label| shared.backend.remote_report(id, label),
            &|frame: FrameUpdate| {
                shared.remote_frame(id, spec.scene.width, spec.scene.height, target, frame);
            },
        );
        let Err(error) = result else {
            return;
        };
        let step = next_step(
            &error,
            shared.is_current(id),
            shared.control().background,
            attempts,
        );
        match step {
            Next::Stop => return,
            Next::GiveUp => {
                shared.remote_failed(id, &give_up_message(&error, attempts));
                return;
            }
            Next::WaitForeground => {
                shared.backend.remote_report(
                    id,
                    "Remote · paused while the app is in the background".into(),
                );
                if !wait_while(shared, id, |shared| shared.control().background) {
                    return;
                }
                shared
                    .backend
                    .remote_report(id, "Remote · reconnecting".into());
            }
            Next::Retry => {
                attempts += 1;
                shared.backend.remote_report(
                    id,
                    format!("Remote · connection lost, retrying ({attempts} of {MAX_RECONNECTS})"),
                );
                let pause = Duration::from_secs(u64::from(attempts));
                if !wait_for(shared, id, pause) {
                    return;
                }
            }
        }
        shared.remote_restarted(id);
    }
}

/// Sleeps while `condition` holds; false when the epoch was superseded meanwhile.
fn wait_while(shared: &Shared, id: u64, condition: impl Fn(&Shared) -> bool) -> bool {
    while shared.is_current(id) {
        if !condition(shared) {
            return true;
        }
        std::thread::sleep(POLL);
    }
    false
}

/// Sleeps `pause` in short steps; false when the epoch was superseded meanwhile.
fn wait_for(shared: &Shared, id: u64, pause: Duration) -> bool {
    let mut left = pause;
    while shared.is_current(id) {
        if left.is_zero() {
            return true;
        }
        let step = left.min(POLL);
        std::thread::sleep(step);
        left -= step;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lost() -> RunError {
        RunError::Interrupted("link down".into())
    }

    #[test]
    fn a_superseded_epoch_ends_silently_whatever_the_error() {
        assert_eq!(next_step(&lost(), false, false, 0), Next::Stop);
        assert_eq!(
            next_step(&RunError::Failed("no".into()), false, false, 0),
            Next::Stop
        );
    }

    #[test]
    fn a_refusal_is_never_retried() {
        let refused = RunError::Failed("The endpoint refused".into());
        assert_eq!(next_step(&refused, true, false, 0), Next::GiveUp);
        assert_eq!(next_step(&refused, true, true, 0), Next::GiveUp);
    }

    #[test]
    fn the_background_waits_and_does_not_spend_an_attempt() {
        // Even with the attempts used up: coming back from the background is not a failure.
        assert_eq!(
            next_step(&lost(), true, true, MAX_RECONNECTS),
            Next::WaitForeground
        );
    }

    #[test]
    fn a_lost_transport_retries_a_bounded_number_of_times() {
        for attempts in 0..MAX_RECONNECTS {
            assert_eq!(next_step(&lost(), true, false, attempts), Next::Retry);
        }
        assert_eq!(
            next_step(&lost(), true, false, MAX_RECONNECTS),
            Next::GiveUp
        );
    }

    #[test]
    fn the_give_up_sentence_names_the_attempts_only_after_a_retry() {
        assert_eq!(give_up_message(&lost(), 0), "link down");
        let after = give_up_message(&lost(), 3);
        assert!(after.starts_with("link down"), "{after}");
        assert!(after.contains("3 reconnect attempts"), "{after}");
    }
}
