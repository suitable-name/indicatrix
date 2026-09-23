//! UI-thread stall detector. [`stall_guard`] wraps a Slint callback body's work
//! and logs a `tracing::warn!` naming the callback and its elapsed time whenever it
//! runs longer than [`STALL_THRESHOLD`] (the frame budget a 60 Hz UI needs to stay
//! responsive). The app's own `tracing` subscriber writes `indicatrix-cut.log`
//! beside the executable, so a stall shows up there without any new UI surface.
//!
//! # Why a plain wrapper function, not a macro or a `#[instrument]`-style attribute
//!
//! Every callback is registered the same way: `ui.global::<...>()
//! .on_some_callback(move |args| { ... })`. Wrapping the closure BODY in
//! `stall_guard::stall_guard("name", || { ... })` needs no new syntax at each call
//! site beyond the one added wrapper call, and keeps the callback's own early
//! returns (`let Some(x) = ... else { return; }`) working exactly as before -- the
//! closure `stall_guard` wraps still owns its own control flow, `stall_guard` only
//! measures how long it took.
//!
//! # Why not a stall reported for the WHOLE closure life, including a spawned worker
//!
//! A callback that dispatches work to a background thread (`SolveService`,
//! `deep_solve`, `optimize_solve`, `auto_solve::dispatch_background_solve`) returns
//! almost immediately once the dispatch itself is queued -- `stall_guard` measures
//! exactly that return, never the eventual `Weak::upgrade_in_event_loop` completion
//! closure's own (separately wrapped, if it is one) work.

use std::time::{Duration, Instant};

/// A callback body running longer than this on the UI/event-loop thread is a
/// visible stall at 60 Hz (one frame is ~16.7 ms) -- see the module doc comment.
const STALL_THRESHOLD: Duration = Duration::from_millis(16);

/// Runs `body`, logging a `tracing::warn!` naming `name` and the elapsed time if it
/// took longer than [`STALL_THRESHOLD`]. Returns `body`'s own result unchanged --
/// see the module doc comment for why this is a plain wrapper, not a macro.
pub(super) fn stall_guard<R>(name: &str, body: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let result = body();
    let elapsed = start.elapsed();
    if is_stall(elapsed) {
        tracing::warn!(
            callback = name,
            elapsed_ms = elapsed.as_millis(),
            "UI-thread callback exceeded the 16ms stall threshold"
        );
    }
    result
}

/// The threshold check itself, pulled out so a test can drive it directly without
/// needing a closure that actually sleeps [`STALL_THRESHOLD`] or longer.
const fn is_stall(elapsed: Duration) -> bool {
    elapsed.as_millis() > STALL_THRESHOLD.as_millis()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stall_guard_returns_the_bodys_own_result() {
        let result = stall_guard("test_callback", || 42);
        assert_eq!(result, 42);
    }

    #[test]
    fn stall_guard_runs_the_body_exactly_once() {
        let mut calls = 0;
        stall_guard("test_callback", || {
            calls += 1;
        });
        assert_eq!(calls, 1);
    }

    #[test]
    fn is_stall_is_false_at_and_under_the_threshold() {
        assert!(!is_stall(Duration::from_millis(0)));
        assert!(!is_stall(Duration::from_millis(16)));
    }

    #[test]
    fn is_stall_is_true_over_the_threshold() {
        assert!(is_stall(Duration::from_millis(17)));
        assert!(is_stall(Duration::from_secs(1)));
    }
}
