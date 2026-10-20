//! Launching the real app on Slint's testing backend and tearing it down.
//!
//! The backend itself (mock time, [`pump`], [`wait_until`], [`settle`]) is `uiprobe`'s
//! `backend` module, re-exported here. Timers and animations advance only through [`pump`],
//! while work done by the app's own threads (library load, solves, searches) runs in real time
//! and hands its results back through the event queue, which [`pump`] drains.
//!
//! The launch and shutdown sequence stays Indicatrix's own (the generic `Ctx::launch` pumps less
//! and only hides windows): 80 ms of launch pumping, a close through
//! `harness_support::close_all_windows`, and the wait for the design library handle to be let go.

use super::{
    profile::{Profile, ProfileExt, SettingsSeed},
    query::WindowView,
};
use indicatrix_cut::{MainWindow, gui::MainWindowHandle, harness_support};
use slint::ComponentHandle;
use std::time::Duration;
use uiprobe::{backend::wait_until_labeled, timing};

pub use uiprobe::backend::{drain_events, pump, settle, wait_until};

/// Window size used by default (a common laptop screen).
pub const DEFAULT_SIZE: (u32, u32) = (1366, 768);
/// The main window's declared minimum size.
pub const MINIMUM_SIZE: (u32, u32) = (720, 480);

/// How long a shutdown waits for the last holder of the design library handle to let go.
const DATABASE_RELEASE_WAIT: Duration = Duration::from_secs(3);
/// The same wait once an earlier shutdown already left a holder behind.
const DATABASE_RELEASE_WAIT_AFTER_LEAK: Duration = Duration::from_millis(300);

/// The running application: the real [`MainWindowHandle`] built by the real wiring.
pub struct HarnessApp {
    handle: Option<MainWindowHandle>,
}

impl HarnessApp {
    /// An app that is not running yet; [`HarnessApp::relaunch`] starts it.
    #[must_use]
    pub const fn empty() -> Self {
        Self { handle: None }
    }

    /// Builds and shows the main window at [`DEFAULT_SIZE`].
    ///
    /// # Errors
    ///
    /// The error text of `build_main_window` or `show`.
    pub fn launch() -> Result<Self, String> {
        timing::time("launch", || {
            let handle = indicatrix_cut::gui::build_main_window().map_err(|e| format!("{e:#}"))?;
            handle.ui.show().map_err(|e| e.to_string())?;
            let app = Self {
                handle: Some(handle),
            };
            app.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
            pump(50);
            Ok(app)
        })
    }

    /// The main window.
    ///
    /// # Panics
    ///
    /// When the app was shut down.
    #[must_use]
    pub const fn ui(&self) -> &MainWindow {
        &self.handle.as_ref().expect("the app is shut down").ui
    }

    /// The main window as a queryable view with its current size.
    #[must_use]
    pub fn main_view(&self) -> WindowView {
        WindowView::of("MainWindow", self.ui())
    }

    /// Resizes the main window (logical pixels) and lets the layout settle.
    pub fn set_size(&self, width: u32, height: u32) {
        self.ui()
            .window()
            .set_size(slint::LogicalSize::new(width as f32, height as f32));
        pump(30);
    }

    /// Closes the main window (stopping the render thread, flushing settings, closing the help,
    /// compare and planner windows) and drops it. Safe to call twice.
    pub fn shutdown(&mut self) {
        let Some(handle) = self.handle.take() else {
            return;
        };
        timing::time("shutdown", || {
            // `close_all_windows` presses Discard on the close guard, so a dirty design cannot
            // keep the window (and its render thread) alive.
            let _ = harness_support::close_all_windows(&handle);
            // Was a fixed `settle(150)`: wait for the observable effect instead (the main
            // window is hidden), with the old 150 ms as the limit.
            let _ = wait_until(Duration::from_millis(150), || {
                !handle.ui.window().is_visible()
            });
            drop(handle);
            pump(20);
            // The thread-locals of the app still hold this window's database, editor state and
            // settings writer: let them go before the profile is wiped.
            harness_support::release_window_state();
            pump(20);
            // Background threads (the thumbnail decoder, a finishing library load) drop their
            // clone of the database handle some milliseconds after they are told to stop; the
            // file is only closed, and deletable on Windows, once the last one is gone. Wait
            // for that instead of racing it. After a shutdown that still had survivors the wait
            // is short: the holder is a leak, and waiting longer would not release it.
            let patience = if harness_support::shutdowns_with_survivors() == 0 {
                DATABASE_RELEASE_WAIT
            } else {
                DATABASE_RELEASE_WAIT_AFTER_LEAK
            };
            let _ = wait_until_labeled(
                "db-release-wait",
                patience,
                harness_support::database_released,
            );
            let _ = harness_support::report_shutdown_leftovers();
        });
    }

    /// Shuts the app down, wipes everything it persisted, seeds `settings.toml` as `seed` says
    /// and launches a fresh instance. This is the "freshly re-established state" of the
    /// click-everything engine.
    ///
    /// # Errors
    ///
    /// The launch error.
    pub fn relaunch(&mut self, profile: &Profile, seed: &SettingsSeed) -> Result<(), String> {
        timing::time("relaunch", || {
            self.shutdown();
            profile.wipe_app_data();
            profile.seed_settings(seed);
            // The hooks crate's `picker::clear`, through the app's seam.
            harness_support::clear_picker_answers();
            *self = Self::launch()?;
            Ok(())
        })
    }

    /// The main window's toast, `(type, message)`, when one is showing; `None` also when the app
    /// is shut down (never panics, so the crate runner can ask after a scenario).
    #[must_use]
    pub fn error_notice(&self) -> Option<(String, String)> {
        let ui = &self.handle.as_ref()?.ui;
        ui.get_toast_visible().then(|| {
            (
                ui.get_toast_type().to_string(),
                ui.get_toast_message().to_string(),
            )
        })
    }

    /// Hides the main window's toast; nothing when the app is shut down.
    pub fn clear_notice(&self) {
        if let Some(handle) = self.handle.as_ref() {
            handle.ui.set_toast_visible(false);
        }
    }

    /// Whether the main window is still alive and shown.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.handle
            .as_ref()
            .is_some_and(|h| h.ui.window().is_visible())
    }
}
