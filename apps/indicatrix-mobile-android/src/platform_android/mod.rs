//! The Android implementation of `indicatrix_mobile_core::platform::Platform`: JNI calls
//! into the Kotlin `MainActivity`.
//!
//! * [`AndroidPlatform`] is the `Platform` the app runs on.
//! * `jvm` attaches threads to the JVM and holds the activity reference.
//! * `pending` is the request-id table that joins Kotlin's asynchronous answers to the
//!   Rust callbacks waiting for them, and stages files other apps send.
//! * `natives` holds the `extern "system"` functions Kotlin calls back with.
//! * `logging` sends `log`, panics and `tracing` to logcat.
//!
//! Requests the user must answer (pick, save) are asynchronous: the Kotlin call returns
//! at once and the answer arrives later through `natives`. The other services (share,
//! print, clipboard, keep-awake, thermal, open URL) are synchronous JNI calls; Kotlin
//! hops to the UI thread itself where Android requires it.

pub(crate) mod back;
mod jvm;
mod logging;
mod natives;
mod pending;

use std::{fs, path::PathBuf};

use indicatrix_mobile_core::platform::{
    AppDirs, DocumentsCallback, IncomingCallback, OpenKind, OpenRequest, OutgoingFile,
    PlatformError, SaveCallback,
};
use jni::{JValue, jni_str, objects::JString};
use slint::android::AndroidApp;

use jvm::{
    Jvm, SIG_CACHE_DIR, SIG_CREATE_DOCUMENT, SIG_KEEP_SCREEN_ON, SIG_ONE_STRING,
    SIG_OPEN_DOCUMENTS, SIG_PRINT_HTML, SIG_SHARE, SIG_THERMAL_HEADROOM, error_message, into_unit,
    new_byte_array,
};

pub(crate) use logging::init as init_logging;

/// The `kind` codes `MainActivity.openDocuments` understands.
const KIND_DESIGNS: i32 = 0;
const KIND_HDR_MAP: i32 = 1;
const KIND_DESIGN_FOLDER: i32 = 2;

/// What the user is told when the stored remote credentials cannot be unlocked.
const UNLOCK_FAILED: &str =
    "Could not unlock remote credentials. Enroll again if the Android key was lost.";

/// The platform services of a running Android activity.
pub struct AndroidPlatform {
    dirs: AppDirs,
    jvm: Jvm,
}

impl AndroidPlatform {
    /// Connects to the activity and prepares the app's private directories.
    ///
    /// # Errors
    /// When the activity cannot be reached over JNI or a private directory cannot be
    /// created.
    pub fn new(app: &AndroidApp) -> Result<Self, PlatformError> {
        let files = app.internal_data_path().ok_or_else(|| {
            PlatformError::new("The app's private storage directory is unavailable.")
        })?;
        let jvm = Jvm::new(app)?;

        let cache = match jvm.with_env(|env, activity| {
            let value = env.call_method(activity, jni_str!("cacheDirPath"), SIG_CACHE_DIR, &[])?;
            error_message(env, value)
        }) {
            Ok(Some(path)) => PathBuf::from(path),
            Ok(None) | Err(_) => files
                .parent()
                .map_or_else(|| files.join("cache"), |parent| parent.join("cache")),
        };

        for dir in [&files, &cache] {
            fs::create_dir_all(dir).map_err(|error| {
                PlatformError::new(format!("Could not create {}: {error}", dir.display()))
            })?;
        }
        tracing::info!(files = %files.display(), cache = %cache.display(), "platform ready");
        Ok(Self {
            dirs: AppDirs { files, cache },
            jvm,
        })
    }
}

impl indicatrix_mobile_core::platform::Platform for AndroidPlatform {
    fn update_back_target(&self, target: i32, version: i32) {
        let result = self.jvm.with_env(|env, activity| {
            env.call_method(
                activity,
                jni_str!("setBackTarget"),
                jni::jni_sig!("(II)V"),
                &[JValue::Int(target), JValue::Int(version)],
            )?;
            Ok(())
        });
        if let Err(error) = result {
            tracing::warn!(%error, "could not mirror Android Back state");
        }
    }

    fn app_dirs(&self) -> &AppDirs {
        &self.dirs
    }

    fn open_documents(&self, request: OpenRequest, done: DocumentsCallback) {
        let id = pending::register_documents(done);
        let kind = match request.kind {
            OpenKind::Designs => KIND_DESIGNS,
            OpenKind::HdrMap => KIND_HDR_MAP,
            OpenKind::DesignFolder => KIND_DESIGN_FOLDER,
        };
        let multiple = request.allow_multiple && request.kind == OpenKind::Designs;
        let outcome = into_unit(self.jvm.with_env(|env, activity| {
            let value = env.call_method(
                activity,
                jni_str!("openDocuments"),
                SIG_OPEN_DOCUMENTS,
                &[
                    JValue::Long(id as i64),
                    JValue::Int(kind),
                    JValue::Bool(multiple),
                ],
            )?;
            error_message(env, value)
        }));
        if let Err(error) = outcome {
            pending::fail(id, error);
        }
    }

    fn create_document(&self, file: OutgoingFile, done: SaveCallback) {
        let id = pending::register_save(done);
        let outcome = into_unit(self.jvm.with_env(|env, activity| {
            let name = JString::new(env, &file.suggested_name)?;
            let mime = JString::new(env, &file.mime)?;
            let bytes = new_byte_array(env, &file.bytes)?;
            let value = env.call_method(
                activity,
                jni_str!("createDocument"),
                SIG_CREATE_DOCUMENT,
                &[
                    JValue::Long(id as i64),
                    JValue::Object(&name),
                    JValue::Object(&mime),
                    JValue::Object(&bytes),
                ],
            )?;
            error_message(env, value)
        }));
        if let Err(error) = outcome {
            pending::fail(id, error);
        }
    }

    fn share(&self, file: OutgoingFile) -> Result<(), PlatformError> {
        into_unit(self.jvm.with_env(|env, activity| {
            let name = JString::new(env, &file.suggested_name)?;
            let mime = JString::new(env, &file.mime)?;
            let bytes = new_byte_array(env, &file.bytes)?;
            let value = env.call_method(
                activity,
                jni_str!("share"),
                SIG_SHARE,
                &[
                    JValue::Object(&name),
                    JValue::Object(&mime),
                    JValue::Object(&bytes),
                ],
            )?;
            error_message(env, value)
        }))
    }

    fn print_html(&self, title: &str, html: &str) -> Result<(), PlatformError> {
        into_unit(self.jvm.with_env(|env, activity| {
            let title = JString::new(env, title)?;
            let html = JString::new(env, html)?;
            let value = env.call_method(
                activity,
                jni_str!("printHtml"),
                SIG_PRINT_HTML,
                &[JValue::Object(&title), JValue::Object(&html)],
            )?;
            error_message(env, value)
        }))
    }

    fn set_clipboard_text(&self, text: &str) -> Result<(), PlatformError> {
        self.call_with_string(jni_str!("setClipboardText"), text)
    }

    fn keep_screen_on(&self, on: bool) {
        let outcome = into_unit(self.jvm.with_env(|env, activity| {
            let value = env.call_method(
                activity,
                jni_str!("keepScreenOn"),
                SIG_KEEP_SCREEN_ON,
                &[JValue::Bool(on)],
            )?;
            error_message(env, value)
        }));
        if let Err(error) = outcome {
            tracing::warn!(%error, on, "could not change the keep-screen-on flag");
        }
    }

    fn thermal_headroom(&self) -> Option<f32> {
        let headroom = self
            .jvm
            .with_env(|env, activity| {
                env.call_method(
                    activity,
                    jni_str!("thermalHeadroom"),
                    SIG_THERMAL_HEADROOM,
                    &[],
                )?
                .f()
            })
            .ok()?;
        // Kotlin returns NaN when the device (or an Android below 11) does not report it.
        headroom.is_finite().then_some(headroom)
    }

    fn store_remote_credentials(&self, bundle: &str) -> Result<(), PlatformError> {
        self.call_with_string(jni_str!("storeRemoteCredentials"), bundle)
    }
    fn load_remote_credentials(&self) -> Result<Option<String>, PlatformError> {
        self.jvm
            .with_env(|env, activity| {
                let value = env.call_method(
                    activity,
                    jni_str!("loadRemoteCredentials"),
                    jvm::SIG_NO_ARGS_STRING,
                    &[],
                )?;
                error_message(env, value)
            })
            .map_err(|_| PlatformError::new(UNLOCK_FAILED))
    }
    fn forget_remote_credentials(&self) -> Result<(), PlatformError> {
        into_unit(self.jvm.with_env(|env, activity| {
            let value = env.call_method(
                activity,
                jni_str!("forgetRemoteCredentials"),
                jvm::SIG_NO_ARGS_STRING,
                &[],
            )?;
            error_message(env, value)
        }))
    }

    fn system_power_save(&self) -> Option<bool> {
        self.jvm
            .with_env(|env, activity| {
                env.call_method(
                    activity,
                    jni_str!("systemPowerSave"),
                    jvm::SIG_SYSTEM_POWER_SAVE,
                    &[],
                )?
                .z()
            })
            .ok()
    }

    fn network_metered(&self) -> Option<bool> {
        let code = self
            .jvm
            .with_env(|env, activity| {
                env.call_method(
                    activity,
                    jni_str!("networkMetered"),
                    jvm::SIG_NETWORK_METERED,
                    &[],
                )?
                .i()
            })
            .ok()?;
        match code {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        self.call_with_string(jni_str!("openUrl"), url)
    }

    fn set_incoming_handler(&self, handler: IncomingCallback) {
        pending::set_incoming_handler(handler);
    }
}

impl AndroidPlatform {
    /// Calls a Kotlin `fun name(text: String): String?` service method.
    fn call_with_string(
        &self,
        method: &'static jni::strings::JNIStr,
        text: &str,
    ) -> Result<(), PlatformError> {
        into_unit(self.jvm.with_env(|env, activity| {
            let text = JString::new(env, text)?;
            let value =
                env.call_method(activity, method, SIG_ONE_STRING, &[JValue::Object(&text)])?;
            error_message(env, value)
        }))
    }
}
