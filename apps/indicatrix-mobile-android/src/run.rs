//! The run loop: builds the window and the app state, wires the Slint callbacks to
//! the core, pushes the first state to the UI, runs the event loop and finally tells
//! the core the app is stopping.
//!
//! Threading: the `AppState` lives in an `Rc<RefCell<..>>` on the UI thread. Code on
//! other threads (platform callbacks, lifecycle notifications) reaches it only through
//! `slint::invoke_from_event_loop` / `Weak::upgrade_in_event_loop`.
//!
//! The Back key: the `Navigation` global (`ui/navigation.slint`) names what Back would
//! close (dialog, info sheet, a Design screen or sheet, a Library or More page, the
//! Design destination) as a target plus a version, and reports every change to the
//! platform (`Platform::update_back_target`). On Android 13 and later the activity owns
//! the dispatcher callback and answers with `nativeBackCommitted` / `nativeBackProgress`
//! (`platform_android/back.rs`, `crate::navigation::back`); the pinned Slint backend
//! (`vendor/slint-android-activity-1.18.1/PATCH.md`) leaves Back to the app. Older
//! versions keep the legacy key route: `ui/app.slint` handles `Key.Back` (and Escape) in
//! a root `FocusScope`, and a key nothing consumed is rejected so the activity's default
//! Back action runs. At the root Back is left to Android.

mod ui_state;

use std::{cell::RefCell, path::PathBuf, rc::Rc, sync::Arc, time::Duration};

use indicatrix_mobile_core::{
    app_state::{AppState, Lifecycle},
    layout::Destination,
    platform::Platform,
};
use slint::ComponentHandle;

use crate::{AppWindow, PropsModel, Shell, ViewportModel, commands, push};

/// Options for [`run_with`] that only the desktop preview sets.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunOptions {
    /// The window's initial logical size in px (width, height). `None` keeps the
    /// size the platform gives the window (always the case on Android).
    pub initial_size: Option<(f32, f32)>,
    /// Opens this destination instead of the remembered one.
    pub destination: Option<Destination>,
    /// Saves a PNG of the window to this path shortly after start, then quits (the
    /// lead's and the owner's layout checks without a device).
    pub screenshot: Option<PathBuf>,
    /// Starts on a new design from this template card (0 is the blank design), replacing
    /// whatever the autosave restored.
    pub template: Option<usize>,
    /// Opens this tab of the Design page (0 Tiers, 1 Stone, 2 Light, 3 Checks).
    pub tab: Option<i32>,
    /// Puts the Design page's sheet at this detent (0 peek, 1 half, 2 full).
    pub sheet: Option<i32>,
    /// Puts the Design viewport in this mode (0 Solid, 1 Diagram, 2 Render).
    pub view: Option<i32>,
    /// Preview-only explicit image engine: 0 Path-Traced or 1 Ray-Traced.
    pub engine: Option<u8>,
    /// How long the screenshot run waits before capturing, instead of the default (longer in
    /// Render mode). A large library takes a while to open.
    pub screenshot_delay: Option<Duration>,
}

/// How long the screenshot run waits for the first frames before capturing.
#[cfg(feature = "desktop-preview")]
const SCREENSHOT_DELAY: Duration = Duration::from_millis(2500);

/// The screenshot delay in Render mode (the trace needs a few passes).
#[cfg(feature = "desktop-preview")]
const RENDER_SCREENSHOT_DELAY: Duration = Duration::from_secs(8);

/// Runs the app until its window closes.
///
/// `platform` provides files, sharing, printing and the rest (the Android shim, or
/// the desktop preview's stand-in). On Android the caller initializes the Slint
/// backend first (`slint::android::init`).
///
/// # Errors
/// When the window or the event loop cannot be created or run.
pub fn run(platform: &Arc<dyn Platform>) -> Result<(), slint::PlatformError> {
    run_with(platform, &RunOptions::default())
}

/// [`run`] with [`RunOptions`].
///
/// # Errors
/// When the window or the event loop cannot be created or run.
pub fn run_with(
    platform: &Arc<dyn Platform>,
    options: &RunOptions,
) -> Result<(), slint::PlatformError> {
    let window = AppWindow::new()?;
    if let Some((width, height)) = options.initial_size {
        window
            .window()
            .set_size(slint::LogicalSize::new(width, height));
    }

    let state = Rc::new(RefCell::new(AppState::new(Arc::clone(platform))));
    if let Some(destination) = options.destination {
        state.borrow_mut().destination = destination;
    }
    ui_state::install(Rc::clone(&state));

    commands::wire(&window, &state);
    commands::register_incoming(&window, platform);
    if let Some(index) = options.template {
        let created =
            indicatrix_mobile_core::document::new_from_template(&mut state.borrow_mut(), index);
        match created {
            Ok(()) => crate::design::after_edit(&window, &state),
            Err(message) => tracing::warn!(%message, "the requested template was not opened"),
        }
    }
    push::all(&window, &state.borrow());
    apply_view_options(&window, options);

    #[cfg(feature = "desktop-preview")]
    if let Some(path) = options.screenshot.clone() {
        // A render needs time to converge before the picture is worth keeping.
        let delay = options.screenshot_delay.unwrap_or_else(|| {
            if options.view == Some(2) {
                RENDER_SCREENSHOT_DELAY
            } else {
                SCREENSHOT_DELAY
            }
        });
        schedule_screenshot(&window, path, delay);
    }

    tracing::info!("mobile app: event loop starting");
    let result = window.run();

    // Cancel long operations before joining their worker queues.
    state.borrow_mut().operations.cancel();
    crate::remote::shutdown();
    crate::design::shutdown();
    state.borrow_mut().shutdown();
    ui_state::clear();
    tracing::info!("mobile app: event loop ended");
    result
}

/// Applies the options that only select what the Design page shows (the tab and the sheet
/// detent). Runs after the first push, so every surface is wired.
fn apply_view_options(window: &AppWindow, options: &RunOptions) {
    if let Some(engine) = options.engine {
        window
            .global::<crate::RenderModel>()
            .invoke_engine_selected(i32::from(engine));
    }
    if let Some(tab) = options.tab {
        window
            .global::<PropsModel>()
            .set_design_tab(tab.clamp(0, 3));
    }
    if let Some(detent) = options.sheet {
        window
            .global::<Shell>()
            .set_sheet_detent(detent.clamp(0, 2));
    }
    if let Some(view) = options.view {
        window
            .global::<ViewportModel>()
            .invoke_view_mode_chosen(view.clamp(0, 2));
    }
}

/// Captures the window to `path` as a PNG after [`SCREENSHOT_DELAY`], then quits the
/// event loop. Failures are logged; the loop quits either way.
#[cfg(feature = "desktop-preview")]
fn schedule_screenshot(window: &AppWindow, path: PathBuf, delay: Duration) {
    let weak = window.as_weak();
    slint::Timer::single_shot(delay, move || {
        if let Some(window) = weak.upgrade() {
            match window.window().take_snapshot() {
                Ok(buffer) => {
                    let saved = image::save_buffer(
                        &path,
                        buffer.as_bytes(),
                        buffer.width(),
                        buffer.height(),
                        image::ExtendedColorType::Rgba8,
                    );
                    match saved {
                        Ok(()) => tracing::info!(path = %path.display(), "screenshot saved"),
                        Err(error) => tracing::warn!(%error, "screenshot could not be saved"),
                    }
                }
                Err(error) => tracing::warn!(%error, "screenshot could not be taken"),
            }
        }
        if let Err(error) = slint::quit_event_loop() {
            tracing::warn!(%error, "the event loop could not be quit after the screenshot");
        }
    });
}

/// Tells the core about an Android lifecycle change (`onPause`, `onResume`, `onStop`).
///
/// Callable from any thread; the event is queued to the UI thread. Does nothing (but
/// logs) before [`run`] started or after it ended.
pub fn notify_lifecycle(event: Lifecycle) {
    if !ui_state::is_running() {
        tracing::debug!(
            ?event,
            "lifecycle event ignored: the run loop is not active"
        );
        return;
    }
    let queued = slint::invoke_from_event_loop(move || {
        if matches!(event, Lifecycle::Paused | Lifecycle::Stopping) {
            crate::remote::interrupt();
        }
        let handled = ui_state::with_state(|state| {
            state.lifecycle(event);
            // Cutting Mode lets the screen sleep while paused and keeps it on again after.
            crate::cut::mode::on_lifecycle(state, event);
        });
        if handled.is_some() {
            ui_state::refresh_editor();
        }
        if handled.is_none() {
            tracing::warn!(
                ?event,
                "lifecycle event dropped: no app state on the UI thread"
            );
        }
    });
    if let Err(error) = queued {
        tracing::warn!(?event, %error, "lifecycle event could not be queued to the UI thread");
    }
}
