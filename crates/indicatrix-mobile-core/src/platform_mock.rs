//! [`MockPlatform`]: a scriptable, recording [`Platform`] for tests (and for a front end
//! that runs without a device).
//!
//! Requests that a real platform answers later ([`Platform::open_documents`],
//! [`Platform::create_document`]) are answered here synchronously: the callback runs
//! inside the call, on the calling thread, with the next scripted response. Likewise
//! [`MockPlatform::deliver_incoming`] runs the registered handler inside the call. The
//! mock never holds its internal lock while a callback runs, so a callback may call back
//! into the mock.

use std::{
    collections::VecDeque,
    path::PathBuf,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

use crate::platform::{
    AppDirs, DocumentsCallback, IncomingCallback, OpenRequest, OutgoingFile, PickedFile, Platform,
    PlatformError, SaveCallback, SaveOutcome,
};

/// A directory path under the system temp directory that no other call (in this process
/// or another) returns, e.g. `<temp>/indicatrix-mobile-<tag>-<pid>-<nanos>-<n>`. The
/// directory is not created.
#[must_use]
pub fn unique_temp_dir(tag: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_nanos());
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "indicatrix-mobile-{tag}-{}-{nanos}-{n}",
        std::process::id()
    ))
}

/// What the credential-vault calls report while the mock has no vault (the trait's own
/// default message).
const NO_VAULT: &str = "This platform has no encrypted credential vault.";

#[derive(Default)]
struct Recorded {
    pick_responses: VecDeque<Result<Vec<PickedFile>, PlatformError>>,
    save_responses: VecDeque<Result<SaveOutcome, PlatformError>>,
    open_requests: Vec<OpenRequest>,
    created: Vec<OutgoingFile>,
    shared: Vec<OutgoingFile>,
    printed: Vec<(String, String)>,
    clipboard: Vec<String>,
    keep_screen_on: Vec<bool>,
    urls: Vec<String>,
    thermal_headroom: Option<f32>,
    metered: Option<bool>,
    incoming: Option<Arc<dyn Fn(Vec<PickedFile>) + Send + Sync>>,
    /// The in-memory credential vault: the bundle [`Platform::store_remote_credentials`] kept.
    credentials: Option<String>,
    /// Whether the credential vault calls fail as on a platform without a vault.
    vault_missing: bool,
    /// Whether [`Platform::forget_remote_credentials`] fails and leaves the bundle in place.
    forget_fails: bool,
}

/// A [`Platform`] that answers from scripted responses and records every call. See the
/// module docs for the (synchronous) callback behavior.
pub struct MockPlatform {
    dirs: AppDirs,
    /// The temp directory this mock created for itself, removed on drop.
    owned_root: Option<PathBuf>,
    state: Mutex<Recorded>,
}

impl MockPlatform {
    /// A mock over the given private directories (left untouched on drop).
    #[must_use]
    pub fn new(dirs: AppDirs) -> Self {
        Self {
            dirs,
            owned_root: None,
            state: Mutex::new(Recorded::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Recorded> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues the answer to the next [`Platform::open_documents`] call. With nothing
    /// queued that call answers `Ok(vec![])`, which means the user cancelled.
    pub fn push_pick_response(&self, response: Result<Vec<PickedFile>, PlatformError>) {
        self.lock().pick_responses.push_back(response);
    }

    /// Queues the answer to the next [`Platform::create_document`] call. With nothing
    /// queued that call answers `Ok(SaveOutcome::Cancelled)`.
    pub fn push_save_response(&self, response: Result<SaveOutcome, PlatformError>) {
        self.lock().save_responses.push_back(response);
    }

    /// Sets what [`Platform::thermal_headroom`] reports (`None` is the default).
    pub fn set_thermal_headroom(&self, headroom: Option<f32>) {
        self.lock().thermal_headroom = headroom;
    }

    /// Sets what [`Platform::network_metered`] reports (`None`, unknown, is the default).
    pub fn set_network_metered(&self, metered: Option<bool>) {
        self.lock().metered = metered;
    }

    /// Whether the in-memory credential vault exists (it does by default). Without it
    /// [`Platform::store_remote_credentials`], [`Platform::load_remote_credentials`] and
    /// [`Platform::forget_remote_credentials`] fail like the trait's defaults do.
    pub fn set_vault_available(&self, available: bool) {
        self.lock().vault_missing = !available;
    }

    /// Makes [`Platform::forget_remote_credentials`] fail (true) or succeed (false). A
    /// failing call leaves the stored bundle where it is.
    pub fn set_forget_failure(&self, fails: bool) {
        self.lock().forget_fails = fails;
    }

    /// The bundle currently in the in-memory credential vault, if any.
    #[must_use]
    pub fn stored_credentials(&self) -> Option<String> {
        self.lock().credentials.clone()
    }

    /// Hands `files` to the handler registered with [`Platform::set_incoming_handler`],
    /// as if another app had sent them. Returns false when no handler is registered.
    pub fn deliver_incoming(&self, files: Vec<PickedFile>) -> bool {
        let handler = self.lock().incoming.clone();
        handler.is_some_and(|handler| {
            handler(files);
            true
        })
    }

    /// Every [`Platform::open_documents`] request so far, oldest first.
    #[must_use]
    pub fn open_requests(&self) -> Vec<OpenRequest> {
        self.lock().open_requests.clone()
    }

    /// Every file passed to [`Platform::create_document`], oldest first.
    #[must_use]
    pub fn created_files(&self) -> Vec<OutgoingFile> {
        self.lock().created.clone()
    }

    /// Every file passed to [`Platform::share`], oldest first.
    #[must_use]
    pub fn shared_files(&self) -> Vec<OutgoingFile> {
        self.lock().shared.clone()
    }

    /// Every `(title, html)` passed to [`Platform::print_html`], oldest first.
    #[must_use]
    pub fn printed(&self) -> Vec<(String, String)> {
        self.lock().printed.clone()
    }

    /// Every text passed to [`Platform::set_clipboard_text`], oldest first.
    #[must_use]
    pub fn clipboard_texts(&self) -> Vec<String> {
        self.lock().clipboard.clone()
    }

    /// Every value passed to [`Platform::keep_screen_on`], oldest first.
    #[must_use]
    pub fn keep_screen_on_history(&self) -> Vec<bool> {
        self.lock().keep_screen_on.clone()
    }

    /// Every address passed to [`Platform::open_url`], oldest first.
    #[must_use]
    pub fn opened_urls(&self) -> Vec<String> {
        self.lock().urls.clone()
    }
}

impl Default for MockPlatform {
    /// A mock over fresh directories below the system temp directory; they are created
    /// now and removed when the mock is dropped.
    fn default() -> Self {
        let root = unique_temp_dir("mock");
        let dirs = AppDirs {
            files: root.join("files"),
            cache: root.join("cache"),
        };
        // Best effort: a failure shows up as an error in whatever uses the directories.
        let _ = std::fs::create_dir_all(&dirs.files);
        let _ = std::fs::create_dir_all(&dirs.cache);
        Self {
            dirs,
            owned_root: Some(root),
            state: Mutex::new(Recorded::default()),
        }
    }
}

impl Drop for MockPlatform {
    fn drop(&mut self) {
        if let Some(root) = &self.owned_root {
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

impl Platform for MockPlatform {
    fn app_dirs(&self) -> &AppDirs {
        &self.dirs
    }

    fn open_documents(&self, request: OpenRequest, done: DocumentsCallback) {
        let response = {
            let mut state = self.lock();
            state.open_requests.push(request);
            state.pick_responses.pop_front()
        };
        done(response.unwrap_or_else(|| Ok(Vec::new())));
    }

    fn create_document(&self, file: OutgoingFile, done: SaveCallback) {
        let response = {
            let mut state = self.lock();
            state.created.push(file);
            state.save_responses.pop_front()
        };
        done(response.unwrap_or(Ok(SaveOutcome::Cancelled)));
    }

    fn share(&self, file: OutgoingFile) -> Result<(), PlatformError> {
        self.lock().shared.push(file);
        Ok(())
    }

    fn print_html(&self, title: &str, html: &str) -> Result<(), PlatformError> {
        self.lock()
            .printed
            .push((title.to_owned(), html.to_owned()));
        Ok(())
    }

    fn set_clipboard_text(&self, text: &str) -> Result<(), PlatformError> {
        self.lock().clipboard.push(text.to_owned());
        Ok(())
    }

    fn keep_screen_on(&self, on: bool) {
        self.lock().keep_screen_on.push(on);
    }

    fn thermal_headroom(&self) -> Option<f32> {
        self.lock().thermal_headroom
    }

    fn network_metered(&self) -> Option<bool> {
        self.lock().metered
    }

    fn store_remote_credentials(&self, bundle: &str) -> Result<(), PlatformError> {
        let mut state = self.lock();
        if state.vault_missing {
            return Err(PlatformError::new(NO_VAULT));
        }
        state.credentials = Some(bundle.to_owned());
        drop(state);
        Ok(())
    }

    fn load_remote_credentials(&self) -> Result<Option<String>, PlatformError> {
        let state = self.lock();
        if state.vault_missing {
            return Err(PlatformError::new(NO_VAULT));
        }
        Ok(state.credentials.clone())
    }

    fn forget_remote_credentials(&self) -> Result<(), PlatformError> {
        let mut state = self.lock();
        if state.vault_missing {
            return Err(PlatformError::new(NO_VAULT));
        }
        if state.forget_fails {
            return Err(PlatformError::new("The credentials could not be erased."));
        }
        state.credentials = None;
        drop(state);
        Ok(())
    }

    fn open_url(&self, url: &str) -> Result<(), PlatformError> {
        self.lock().urls.push(url.to_owned());
        Ok(())
    }

    fn set_incoming_handler(&self, handler: IncomingCallback) {
        self.lock().incoming = Some(Arc::from(handler));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::{OpenKind, mime};
    use std::sync::atomic::AtomicUsize;

    fn request(kind: OpenKind) -> OpenRequest {
        OpenRequest {
            kind,
            allow_multiple: false,
        }
    }

    fn file(name: &str) -> OutgoingFile {
        OutgoingFile {
            suggested_name: name.to_owned(),
            mime: mime::ASC.to_owned(),
            bytes: vec![1, 2, 3],
        }
    }

    #[test]
    fn unique_dirs_differ() {
        assert_ne!(unique_temp_dir("a"), unique_temp_dir("a"));
    }

    #[test]
    fn scripted_picks_then_cancel() {
        let mock = MockPlatform::default();
        mock.push_pick_response(Ok(vec![PickedFile {
            display_name: "a.asc".to_owned(),
            bytes: vec![9],
        }]));
        mock.push_pick_response(Err(PlatformError::new("denied")));

        let seen = Arc::new(Mutex::new(Vec::new()));
        for kind in [OpenKind::Designs, OpenKind::HdrMap, OpenKind::DesignFolder] {
            let seen = Arc::clone(&seen);
            mock.open_documents(
                request(kind),
                Box::new(move |result| seen.lock().expect("lock").push(result)),
            );
        }
        let seen = seen.lock().expect("lock");
        assert_eq!(seen[0].as_ref().map(Vec::len), Ok(1));
        assert_eq!(seen[1], Err(PlatformError::new("denied")));
        assert_eq!(seen[2], Ok(Vec::new()));
        assert_eq!(mock.open_requests().len(), 3);
        assert_eq!(mock.open_requests()[1].kind, OpenKind::HdrMap);
    }

    #[test]
    fn scripted_saves_then_cancel() {
        let mock = MockPlatform::default();
        mock.push_save_response(Ok(SaveOutcome::Saved {
            display_name: "x.asc".to_owned(),
        }));
        let outcomes = Arc::new(Mutex::new(Vec::new()));
        for name in ["x.asc", "y.asc"] {
            let outcomes = Arc::clone(&outcomes);
            mock.create_document(
                file(name),
                Box::new(move |result| outcomes.lock().expect("lock").push(result)),
            );
        }
        assert_eq!(
            *outcomes.lock().expect("lock"),
            vec![
                Ok(SaveOutcome::Saved {
                    display_name: "x.asc".to_owned()
                }),
                Ok(SaveOutcome::Cancelled)
            ]
        );
        assert_eq!(mock.created_files().len(), 2);
        assert_eq!(mock.created_files()[1].suggested_name, "y.asc");
    }

    #[test]
    fn records_the_simple_calls() {
        let mock = MockPlatform::default();
        mock.share(file("s.asc")).expect("share");
        mock.print_html("Sheet", "<p>x</p>").expect("print");
        mock.set_clipboard_text("copied").expect("clipboard");
        mock.keep_screen_on(true);
        mock.keep_screen_on(false);
        mock.open_url("https://example.org").expect("url");
        assert_eq!(mock.shared_files()[0].suggested_name, "s.asc");
        assert_eq!(
            mock.printed(),
            vec![("Sheet".to_owned(), "<p>x</p>".to_owned())]
        );
        assert_eq!(mock.clipboard_texts(), vec!["copied".to_owned()]);
        assert_eq!(mock.keep_screen_on_history(), vec![true, false]);
        assert_eq!(mock.opened_urls(), vec!["https://example.org".to_owned()]);
    }

    #[test]
    fn thermal_headroom_is_settable() {
        let mock = MockPlatform::default();
        assert_eq!(mock.thermal_headroom(), None);
        mock.set_thermal_headroom(Some(0.75));
        assert_eq!(mock.thermal_headroom(), Some(0.75));
    }

    #[test]
    fn the_credential_vault_stores_loads_and_forgets() {
        let mock = MockPlatform::default();
        assert_eq!(mock.load_remote_credentials(), Ok(None));
        mock.store_remote_credentials("bundle").expect("store");
        assert_eq!(
            mock.load_remote_credentials(),
            Ok(Some("bundle".to_owned()))
        );

        mock.set_forget_failure(true);
        assert!(mock.forget_remote_credentials().is_err());
        assert_eq!(mock.stored_credentials().as_deref(), Some("bundle"));

        mock.set_forget_failure(false);
        mock.forget_remote_credentials().expect("forget");
        assert_eq!(mock.load_remote_credentials(), Ok(None));

        mock.set_vault_available(false);
        assert!(mock.store_remote_credentials("x").is_err());
        assert!(mock.load_remote_credentials().is_err());
        assert!(mock.forget_remote_credentials().is_err());
    }

    #[test]
    fn incoming_handler_receives_delivered_files() {
        let mock = MockPlatform::default();
        assert!(!mock.deliver_incoming(Vec::new()));
        let count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&count);
        mock.set_incoming_handler(Box::new(move |files| {
            counter.fetch_add(files.len(), Ordering::SeqCst);
        }));
        let picked = || PickedFile {
            display_name: "b.asc".to_owned(),
            bytes: Vec::new(),
        };
        assert!(mock.deliver_incoming(vec![picked(), picked()]));
        assert_eq!(count.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn default_dirs_exist_and_are_removed_on_drop() {
        let mock = MockPlatform::default();
        let dirs = mock.app_dirs().clone();
        assert!(dirs.files.is_dir() && dirs.cache.is_dir());
        drop(mock);
        assert!(!dirs.files.exists());
    }
}
