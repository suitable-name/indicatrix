//! The snackbar: show a message, dismiss it after four seconds.

use std::time::Duration;

use slint::{ComponentHandle, Timer, TimerMode};

use crate::{AppWindow, Shell};

/// How long a toast stays up when nobody dismisses it.
pub const TOAST_DURATION: Duration = Duration::from_secs(4);

thread_local! {
    /// The single dismissal timer; showing a new toast restarts it.
    static DISMISS: Timer = Timer::default();
}

/// Shows `text` in the snackbar and (re)starts the four-second dismissal timer. Call
/// on the UI thread.
pub fn show(window: &AppWindow, text: &str) {
    let shell = window.global::<Shell>();
    shell.set_toast_text(text.into());
    shell.set_toast_visible(true);

    let weak = window.as_weak();
    DISMISS.with(|timer| {
        timer.start(TimerMode::SingleShot, TOAST_DURATION, move || {
            if let Some(window) = weak.upgrade() {
                window.global::<Shell>().set_toast_visible(false);
            }
        });
    });
}

/// Stops the dismissal timer (the user dismissed the toast by hand).
pub fn cancel() {
    DISMISS.with(Timer::stop);
}
