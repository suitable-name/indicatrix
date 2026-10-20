//! Slint callbacks -> state changes.
//!
//! One module per surface. A command takes what the callback carries, changes the
//! `AppState` (or asks the platform for something) and pushes the result back; the
//! logic that belongs to the core stays in the core.

use std::{cell::RefCell, rc::Rc};

use indicatrix_mobile_core::app_state::AppState;

use crate::AppWindow;

mod files;
mod prefs;
mod shell;

pub use files::register_incoming;

/// Connects every callback of the UI to its command.
pub fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    shell::wire(window, state);
    files::wire(window, state);
    prefs::wire(window, state);
    crate::remote::wire(window, state);
    crate::design::wire(window, state);
    crate::cut::wire(window, state);
}
