//! Opening designs in the UI: ask the platform for files, hand them to the core, refresh.

use std::sync::Arc;

use slint::ComponentHandle;

use indicatrix_mobile_core::{
    document,
    platform::{OpenKind, OpenRequest, PickedFile},
};

use super::handle::{run_on_ui, toast};
use crate::{AppWindow, design::Shared};

/// Asks the platform for design files (several at once, so a design file and its
/// `.asc` can be picked together) and opens the best one. A cancelled pick says
/// nothing. The caller has already settled unsaved changes.
pub fn pick_and_open(window: &AppWindow, state: &Shared) {
    // Clone the Arc first: a platform may answer on the calling thread, and the state
    // must not be borrowed while it does.
    let platform = Arc::clone(state.borrow().platform());
    let weak = window.as_weak();
    platform.open_documents(
        OpenRequest {
            kind: OpenKind::Designs,
            allow_multiple: true,
        },
        Box::new(move |result| match result {
            Ok(files) if files.is_empty() => {}
            Ok(files) => run_on_ui(&weak, move |window, state| open_files(window, state, files)),
            Err(error) => run_on_ui(&weak, move |window, _state| toast(window, &error.message)),
        }),
    );
}

/// Opens `files` as the current design, then refreshes every Design surface, schedules
/// the auto-solve and toasts what happened. The current design is replaced; the caller
/// has already settled unsaved changes.
pub fn open_files(window: &AppWindow, state: &Shared, files: Vec<PickedFile>) {
    let result = document::open_picked(&mut state.borrow_mut(), files);
    match result {
        Ok(report) => {
            super::close_gallery(window);
            crate::design::after_edit(window, state);
            toast(window, &report.message());
        }
        Err(error) => toast(window, &error.message),
    }
}
