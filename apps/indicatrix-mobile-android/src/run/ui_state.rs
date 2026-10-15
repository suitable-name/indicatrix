//! The UI thread's handle on the app state, for code that only has a closure hopped
//! over with `invoke_from_event_loop` (lifecycle events, platform callbacks).
//!
//! The state itself is an `Rc<RefCell<AppState>>` and never leaves the UI thread; this
//! module keeps one clone of the `Rc` in a thread-local and one process-wide flag that
//! other threads may read to learn whether the run loop has started.

use std::{
    cell::RefCell,
    rc::Rc,
    sync::atomic::{AtomicBool, Ordering},
};

use indicatrix_mobile_core::app_state::AppState;

thread_local! {
    static STATE: RefCell<Option<Rc<RefCell<AppState>>>> = const { RefCell::new(None) };
}

static RUNNING: AtomicBool = AtomicBool::new(false);

/// Registers `state` as the app state of this (the UI) thread and marks the app as
/// running.
pub(super) fn install(state: Rc<RefCell<AppState>>) {
    STATE.with(|slot| *slot.borrow_mut() = Some(state));
    RUNNING.store(true, Ordering::SeqCst);
}

/// Forgets the state; later lifecycle notifications are ignored.
pub(super) fn clear() {
    RUNNING.store(false, Ordering::SeqCst);
    STATE.with(|slot| *slot.borrow_mut() = None);
}

/// Whether [`install`] ran and [`clear`] did not follow. Callable from any thread.
pub(super) fn is_running() -> bool {
    RUNNING.load(Ordering::SeqCst)
}

/// Runs `f` on the app state. `None` when no state is installed on this thread or the
/// state is already borrowed (a re-entrant call), so a lifecycle event can never panic
/// the UI.
pub(super) fn with_state<R>(f: impl FnOnce(&mut AppState) -> R) -> Option<R> {
    STATE.with(|slot| {
        let slot = slot.borrow();
        let state = slot.as_ref()?;
        let mut state = state.try_borrow_mut().ok()?;
        Some(f(&mut state))
    })
}
