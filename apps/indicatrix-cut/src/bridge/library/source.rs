//! Abstraction over "where designs come from" for the viewer's library UI: the local
//! `indicatrix_vault` database, or a remote `indicatrix-worker`'s design library reached
//! over the network.
//!
//! # Why an enum, not a trait object
//!
//! There are exactly two sources with genuinely different shapes (a local call is a
//! synchronous, already-open `rusqlite::Connection`; a remote call is a blocking
//! network round trip that must never run on the UI thread), and nothing else will
//! ever implement a third.
//!
//! # Local stays synchronous and unchanged; remote is always backgrounded
//!
//! [`LibrarySource::Local`] is a thin marker -- callers keep calling
//! `indicatrix_vault::db::sqlite::Database` directly, the same synchronous calls as
//! before this module existed.
//!
//! [`LibrarySource::Remote`] calls are network I/O and must run off the UI thread.
//! [`spawn_library_request`] is the one place that does this: it runs the blocking
//! request on its own `thread::spawn` worker and marshals the result back to the Slint
//! event loop via `Weak::upgrade_in_event_loop`. Every UI entry point that can switch
//! to a remote source goes through this, never a direct blocking call from a callback.

use crate::{bridge::library::client as library_client, settings::WorkerSettings};
use indicatrix_net::library::{LibraryRequest, LibraryResponse};
use slint::{ComponentHandle, Weak};

/// Where the viewer's library UI is currently reading designs from.
///
/// [`Default`] is [`Self::Local`] -- an app that has never touched this feature (or has
/// no remote worker configured at all) is always in this state, matching the
/// pre-Phase-2 behaviour this module adds to, not replaces.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum LibrarySource {
    #[default]
    Local,
    /// Browsing `worker`'s design library instead of the local database. Does not imply
    /// anything has been mirrored locally -- see `bridge::library_mirror` for that
    /// separate operation.
    Remote(WorkerSettings),
}

impl LibrarySource {
    #[must_use]
    pub const fn is_remote(&self) -> bool {
        matches!(self, Self::Remote(_))
    }

    /// The configured worker this source reads from, or `None` for [`Self::Local`].
    #[must_use]
    pub const fn worker(&self) -> Option<&WorkerSettings> {
        match self {
            Self::Remote(w) => Some(w),
            Self::Local => None,
        }
    }

    /// A short label for the "which library am I looking at" badge the UI shows. Never
    /// empty, so a caller can always display it directly with no fallback logic.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Local => "Local library".to_string(),
            Self::Remote(w) => {
                let name = if w.name.trim().is_empty() {
                    w.address.as_str()
                } else {
                    w.name.as_str()
                };
                format!("Remote: {name}")
            }
        }
    }
}

/// Runs one [`LibraryRequest`] against `worker` on a background thread and delivers the
/// result to `on_done` on the Slint event loop -- see the module doc comment's "Local
/// stays synchronous ... remote is always backgrounded" section for why every remote
/// library call goes through this rather than a direct blocking call from a callback.
pub fn spawn_library_request<T, F>(
    ui_weak: Weak<T>,
    worker: WorkerSettings,
    req: LibraryRequest,
    on_done: F,
) where
    T: ComponentHandle + 'static,
    F: FnOnce(&T, Result<LibraryResponse, library_client::LibraryClientError>) + Send + 'static,
{
    std::thread::spawn(move || {
        let result = library_client::request(&worker, &req);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| on_done(&ui, result));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_source_is_local() {
        assert_eq!(LibrarySource::default(), LibrarySource::Local);
        assert!(!LibrarySource::default().is_remote());
    }

    #[test]
    fn local_has_no_worker_and_a_plain_label() {
        let src = LibrarySource::Local;
        assert_eq!(src.worker(), None);
        assert_eq!(src.label(), "Local library");
    }

    #[test]
    fn remote_reports_its_worker_and_is_remote() {
        let worker = WorkerSettings {
            name: "Office workstation".to_string(),
            address: "10.0.0.5:9443".to_string(),
            ..WorkerSettings::default()
        };
        let src = LibrarySource::Remote(worker.clone());
        assert!(src.is_remote());
        assert_eq!(src.worker(), Some(&worker));
        assert_eq!(src.label(), "Remote: Office workstation");
    }

    #[test]
    fn remote_label_falls_back_to_address_when_unnamed() {
        let worker = WorkerSettings {
            name: String::new(),
            address: "10.0.0.5:9443".to_string(),
            ..WorkerSettings::default()
        };
        let src = LibrarySource::Remote(worker);
        assert_eq!(src.label(), "Remote: 10.0.0.5:9443");
    }
}
