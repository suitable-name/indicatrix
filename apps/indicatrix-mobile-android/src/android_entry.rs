//! The Android entry point: `android_main`, called by the NativeActivity glue on the
//! thread it creates for the app.
//!
//! It installs logging, starts Slint's Android backend with a listener that forwards the
//! activity lifecycle to the UI, builds the JNI [`AndroidPlatform`], and hands both to
//! [`crate::run::run`]. Nothing here may panic out of `android_main`: a failure is
//! logged and the function returns, which ends the activity's native thread.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    sync::Arc,
};

use indicatrix_mobile_core::app_state::Lifecycle;
use slint::android::{
    AndroidApp,
    android_activity::{MainEvent, PollEvent},
};

use crate::platform_android::{AndroidPlatform, init_logging};

/// Called by the `android-activity` glue once the native activity exists.
#[unsafe(no_mangle)]
fn android_main(app: AndroidApp) {
    init_logging();
    tracing::info!("Indicatrix starting (android_main)");
    if catch_unwind(AssertUnwindSafe(|| start(app))).is_err() {
        tracing::error!("android_main ended by a panic; the backtrace is in the log above");
    }
}

/// Starts the backend, the platform and the UI, and runs until the activity is destroyed.
fn start(app: AndroidApp) {
    if let Err(error) = slint::android::init_with_event_listener(app.clone(), forward_lifecycle) {
        tracing::error!(%error, "could not start Slint's Android backend");
        return;
    }
    let platform = match AndroidPlatform::new(&app) {
        Ok(platform) => platform,
        Err(error) => {
            tracing::error!(%error, "could not set up the Android platform services");
            return;
        }
    };
    let platform: Arc<dyn indicatrix_mobile_core::platform::Platform> = Arc::new(platform);
    if let Err(error) = crate::run::run(&platform) {
        tracing::error!(%error, "the UI ended with an error");
    }
    tracing::info!("Indicatrix finished (android_main returns)");
}

/// Maps the activity lifecycle events Slint's loop sees onto [`Lifecycle`].
fn forward_lifecycle(event: &PollEvent<'_>) {
    let PollEvent::Main(main_event) = event else {
        return;
    };
    let lifecycle = match main_event {
        MainEvent::Pause => Lifecycle::Paused,
        MainEvent::Resume { .. } => Lifecycle::Resumed,
        MainEvent::Stop | MainEvent::Destroy => Lifecycle::Stopping,
        _ => return,
    };
    crate::run::notify_lifecycle(lifecycle);
}
