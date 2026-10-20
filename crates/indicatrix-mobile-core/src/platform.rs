//! The platform services a mobile front end provides to the core.
//!
//! The core never touches a file path that belongs to the user, a share sheet, a
//! printer or the clipboard itself. It asks a [`Platform`], which the Android app
//! implements over JNI (`apps/indicatrix-mobile-android/src/platform_android`), the
//! desktop preview implements with `rfd` and `std::fs`, and the tests implement with
//! [`MockPlatform`](crate::platform_mock::MockPlatform).
//!
//! Requests that need the user (pick a file, choose where to save) are asynchronous:
//! the call returns at once and the platform invokes the callback later, on a thread
//! of its choosing. A callback must therefore be `Send`; the front end decides how to
//! hop back to its UI thread.

use std::{fmt, path::PathBuf};

/// The app's private directories, fixed for the life of the process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppDirs {
    /// Persistent private storage (Android `Context.getFilesDir()`): settings, the
    /// library database and the autosaved design. The remote credentials are not kept here:
    /// the platform stores them encrypted (`Platform::store_remote_credentials`, wrapped by
    /// an Android Keystore key on the phone).
    pub files: PathBuf,
    /// Disposable private storage (Android `Context.getCacheDir()`): share-sheet
    /// copies, thumbnails, downscaled HDR maps. The system may clear it.
    pub cache: PathBuf,
}

/// What the user is asked to pick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenKind {
    /// Faceting designs: `.isdd`, legacy `.indicatrix`, `.asc`, `.gem`, `.gcs`.
    Designs,
    /// One Radiance `.hdr` environment map.
    HdrMap,
    /// Every design file below a folder the user picks (library import).
    DesignFolder,
}

/// One request to pick files.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenRequest {
    /// What to pick.
    pub kind: OpenKind,
    /// Whether several files may be picked at once (ignored for [`OpenKind::HdrMap`]
    /// and [`OpenKind::DesignFolder`]).
    pub allow_multiple: bool,
}

/// A file the user picked, read completely.
#[derive(Clone, PartialEq, Eq)]
pub struct PickedFile {
    /// The name the user sees (no directory), e.g. `"Barion.asc"`.
    pub display_name: String,
    /// The whole file.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for PickedFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PickedFile")
            .field("display_name", &self.display_name)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// A file the app hands to the user (save, share).
#[derive(Clone, PartialEq, Eq)]
pub struct OutgoingFile {
    /// The suggested file name, e.g. `"Barion.isdd"`.
    pub suggested_name: String,
    /// The MIME type, e.g. [`mime::INDICATRIX_DESIGN`].
    pub mime: String,
    /// The whole file.
    pub bytes: Vec<u8>,
}

impl fmt::Debug for OutgoingFile {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OutgoingFile")
            .field("suggested_name", &self.suggested_name)
            .field("mime", &self.mime)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// How a save request ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveOutcome {
    /// Written; the name the user gave it.
    Saved {
        /// The final display name.
        display_name: String,
    },
    /// The user backed out of the save dialog.
    Cancelled,
}

/// A platform service failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformError {
    /// A sentence the UI can show as it is.
    pub message: String,
}

impl PlatformError {
    /// An error with this message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for PlatformError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlatformError {}

/// Receives the files of one [`Platform::open_documents`] request. An empty list
/// means the user cancelled.
pub type DocumentsCallback = Box<dyn FnOnce(Result<Vec<PickedFile>, PlatformError>) + Send>;

/// Receives the end of one [`Platform::create_document`] request.
pub type SaveCallback = Box<dyn FnOnce(Result<SaveOutcome, PlatformError>) + Send>;

/// Receives files another app sends to Indicatrix ("Open with", "Share to").
pub type IncomingCallback = Box<dyn Fn(Vec<PickedFile>) + Send + Sync>;

/// The services a mobile front end provides. See the module docs.
pub trait Platform: Send + Sync {
    /// The app's private directories.
    fn app_dirs(&self) -> &AppDirs;

    /// Asks the user to pick files; `done` receives them (or an empty list on cancel).
    fn open_documents(&self, request: OpenRequest, done: DocumentsCallback);

    /// Asks the user where to save `file`; `done` receives the outcome.
    fn create_document(&self, file: OutgoingFile, done: SaveCallback);

    /// Offers `file` to other apps through the system share sheet.
    ///
    /// # Errors
    /// When the share sheet cannot be opened.
    fn share(&self, file: OutgoingFile) -> Result<(), PlatformError>;

    /// Opens the system print dialog for an HTML document (the cutting sheet).
    ///
    /// # Errors
    /// When printing is unavailable.
    fn print_html(&self, title: &str, html: &str) -> Result<(), PlatformError>;

    /// Puts `text` on the clipboard.
    ///
    /// # Errors
    /// When the clipboard cannot be written.
    fn set_clipboard_text(&self, text: &str) -> Result<(), PlatformError>;

    /// Keeps the screen on while `on` is true (Cutting Mode).
    fn keep_screen_on(&self, on: bool);

    /// The device's thermal headroom forecast: 0.0 cool, 1.0 at the throttling
    /// threshold, above 1.0 throttling. `None` when the device does not report it.
    /// Callers poll at most once every ten seconds.
    fn thermal_headroom(&self) -> Option<f32>;

    /// System power saver, independently of the app preference. `None` is unknown.
    fn system_power_save(&self) -> Option<bool> {
        None
    }

    /// Whether the active network is metered (Android `ConnectivityManager.isActiveNetworkMetered`).
    /// `None` is unknown or no network; the desktop preview and tests report `None`, which
    /// the remote policy treats as unmetered so a missing signal never blocks a render.
    fn network_metered(&self) -> Option<bool> {
        None
    }

    /// Stores an exportable credential bundle using platform-protected encryption.
    /// Implementations must not put plaintext secrets in settings, backups or logs.
    ///
    /// # Errors
    ///
    /// When the platform has no encrypted vault, or the write fails.
    fn store_remote_credentials(&self, _bundle: &str) -> Result<(), PlatformError> {
        Err(PlatformError::new(
            "This platform has no encrypted credential vault.",
        ))
    }

    /// Decrypts the bundle for one worker-owned authenticated connection.
    ///
    /// # Errors
    ///
    /// When the platform has no encrypted vault, or the bundle cannot be decrypted.
    fn load_remote_credentials(&self) -> Result<Option<String>, PlatformError> {
        Err(PlatformError::new(
            "This platform has no encrypted credential vault.",
        ))
    }

    /// Deletes the stored credential bundle and the platform key that wraps it ("Forget
    /// Server"). Succeeds when nothing is stored, and is safe to repeat. Once it has
    /// succeeded, [`Self::load_remote_credentials`] reports no bundle and a later
    /// [`Self::store_remote_credentials`] starts from a fresh key.
    ///
    /// # Errors
    ///
    /// When the platform has no encrypted vault, or the bundle or its key could not be
    /// erased completely. The caller then keeps the enrolled state so the user can try
    /// again.
    fn forget_remote_credentials(&self) -> Result<(), PlatformError> {
        Err(PlatformError::new(
            "This platform has no encrypted credential vault.",
        ))
    }

    /// Mirrors the versioned app Back target to a platform dispatcher, if present.
    fn update_back_target(&self, _target: i32, _version: i32) {}

    /// Opens a web address in the browser.
    ///
    /// # Errors
    /// When no app can open it.
    fn open_url(&self, url: &str) -> Result<(), PlatformError>;

    /// Registers the receiver of files sent from other apps. Files that arrived
    /// before registration (the app was launched by "Open with") are delivered at
    /// once.
    fn set_incoming_handler(&self, handler: IncomingCallback);
}

/// The MIME types the app reads and writes.
pub mod mime {
    /// A self-contained `.isdd` design.
    pub const INDICATRIX_DESIGN: &str = "application/vnd.indicatrix.design+toml";
    /// A `GemCAD` `.asc` file.
    pub const ASC: &str = "text/plain";
    /// The printable cutting sheet.
    pub const HTML: &str = "text/html";
    /// A rendered picture or diagram.
    pub const PNG: &str = "image/png";
    /// Anything else (`.gem`, `.gcs`, `.hdr`) when picking.
    pub const OCTET_STREAM: &str = "application/octet-stream";
}

/// A platform that implements only the methods the trait requires, so every optional
/// service (the credential vault among them) is the trait's own default. For tests of what
/// the defaults promise; [`MockPlatform`](crate::platform_mock::MockPlatform) overrides them.
#[cfg(test)]
pub(crate) mod bare {
    use std::path::PathBuf;

    use super::{
        AppDirs, DocumentsCallback, IncomingCallback, OpenRequest, OutgoingFile, Platform,
        PlatformError, SaveCallback, SaveOutcome,
    };

    /// See the module docs.
    pub struct BarePlatform {
        dirs: AppDirs,
    }

    impl BarePlatform {
        pub fn new() -> Self {
            Self {
                dirs: AppDirs {
                    files: PathBuf::default(),
                    cache: PathBuf::default(),
                },
            }
        }
    }

    impl Platform for BarePlatform {
        fn app_dirs(&self) -> &AppDirs {
            &self.dirs
        }

        fn open_documents(&self, _request: OpenRequest, done: DocumentsCallback) {
            done(Ok(Vec::new()));
        }

        fn create_document(&self, _file: OutgoingFile, done: SaveCallback) {
            done(Ok(SaveOutcome::Cancelled));
        }

        fn share(&self, _file: OutgoingFile) -> Result<(), PlatformError> {
            Ok(())
        }

        fn print_html(&self, _title: &str, _html: &str) -> Result<(), PlatformError> {
            Ok(())
        }

        fn set_clipboard_text(&self, _text: &str) -> Result<(), PlatformError> {
            Ok(())
        }

        fn keep_screen_on(&self, _on: bool) {}

        fn thermal_headroom(&self) -> Option<f32> {
            None
        }

        fn open_url(&self, _url: &str) -> Result<(), PlatformError> {
            Ok(())
        }

        fn set_incoming_handler(&self, _handler: IncomingCallback) {}
    }
}

#[cfg(test)]
mod tests {
    use super::{Platform, bare::BarePlatform};

    #[test]
    fn a_platform_without_a_vault_fails_every_credential_call() {
        let platform = BarePlatform::new();
        let message = "This platform has no encrypted credential vault.";
        assert_eq!(
            platform
                .store_remote_credentials("bundle")
                .expect_err("nothing can be stored")
                .message,
            message
        );
        assert_eq!(
            platform
                .load_remote_credentials()
                .expect_err("nothing can be loaded")
                .message,
            message
        );
        // Forgetting must not claim success: nothing was erased, so the caller keeps the
        // enrolled state.
        assert_eq!(
            platform
                .forget_remote_credentials()
                .expect_err("nothing can be erased")
                .message,
            message
        );
    }
}
