//! File commands: open a design, new design, import to the library, and files other apps
//! send to Indicatrix.
//!
//! Open, New Design and the incoming files belong to the document surface
//! (`crate::design::document`, which asks about unsaved changes first); import belongs to
//! the Library (`crate::library`). This module only connects the Shell's callbacks and
//! the platform's incoming-files handler to them.

use std::{cell::RefCell, rc::Rc, sync::Arc};

use indicatrix_mobile_core::{app_state::AppState, platform::Platform};
use slint::ComponentHandle;

use crate::{AppWindow, Shell, design::document};

/// Wires `Shell`'s file callbacks.
pub(super) fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    let shell = window.global::<Shell>();

    shell.on_import_files({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move || {
            if let Some(window) = weak.upgrade() {
                crate::library::import_files(&window, &state);
            }
        }
    });

    shell.on_open_design({
        let (state, weak) = (Rc::clone(state), window.as_weak());
        move || {
            if let Some(window) = weak.upgrade() {
                document::request_open(&window, &state);
            }
        }
    });

    shell.on_new_design({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                document::request_new_design(&window);
            }
        }
    });
}

/// Registers the receiver of files other apps send ("Open with", "Share to"). The
/// platform may call it from any thread, so the work hops to the UI thread, where the
/// files become the current design after the unsaved-changes question.
pub fn register_incoming(window: &AppWindow, platform: &Arc<dyn Platform>) {
    let weak = window.as_weak();
    platform.set_incoming_handler(Box::new(move |files| {
        document::run_on_ui(&weak, move |window, state| {
            document::open_incoming(window, state, files);
        });
    }));
}
