//! The UI thread's handle on the app state, for the platform callbacks of the document
//! commands (a file was picked, a save finished, a file arrived from another app).
//!
//! Those callbacks may run on any thread, so they cannot hold the `Rc<RefCell<AppState>>`.
//! They capture a `slint::Weak` instead and hop to the UI thread with [`run_on_ui`], which
//! finds the state again here.

use std::cell::RefCell;

use slint::Weak;

use crate::{AppWindow, design::Shared};

thread_local! {
    static SHARED: RefCell<Option<Shared>> = const { RefCell::new(None) };
}

/// Remembers `state` for [`run_on_ui`]. Call once on the UI thread.
pub(super) fn install(state: &Shared) {
    SHARED.with(|slot| *slot.borrow_mut() = Some(std::rc::Rc::clone(state)));
}

/// Forgets the state (see `design::shutdown`): the app state must be dropped while its
/// threads still exist, not by the process's thread-local teardown.
pub fn forget() {
    super::library::shutdown();
    SHARED.with(|slot| slot.borrow_mut().take());
}

/// The app state, on the UI thread after [`install`].
pub fn shared() -> Option<Shared> {
    SHARED.with(|slot| slot.borrow().clone())
}

/// Runs `job` on the UI thread with the window and the app state. Callable from any
/// thread; the job is dropped (and logged) when the window or the state is gone.
pub fn run_on_ui(weak: &Weak<AppWindow>, job: impl FnOnce(&AppWindow, &Shared) + Send + 'static) {
    let queued = weak.upgrade_in_event_loop(move |window| {
        if let Some(state) = shared() {
            job(&window, &state);
        } else {
            tracing::warn!("a document job was dropped: no app state on the UI thread");
        }
    });
    if let Err(error) = queued {
        tracing::warn!(%error, "a document job was dropped: the UI thread is gone");
    }
}

/// Shows `text` in the snackbar.
pub fn toast(window: &AppWindow, text: &str) {
    crate::push::toast::show(window, text);
}
