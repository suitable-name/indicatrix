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
    /// library database, the autosaved design, the remote certificate bundle.
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
