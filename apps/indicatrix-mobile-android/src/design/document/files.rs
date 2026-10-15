//! Saving, sharing and exporting the open design: build the bytes in the core, hand them
//! to the platform, and (for a save) tell the core when the platform confirmed.

use std::sync::Arc;

use slint::ComponentHandle;

use indicatrix_mobile_core::{
    document::{self, SavedFile},
    platform::{OutgoingFile, Platform, PlatformError, SaveOutcome},
};

use super::{
    guard,
    handle::{run_on_ui, toast},
};
use crate::{AppWindow, design::Shared};

/// What to do with the design's bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Save the design: into its library entry when it came from the library, else write
    /// the `.isdd` file where the user chooses; the design then reads saved.
    Save,
    /// Write the `.isdd` file where the user chooses, even for a library design.
    SaveAs,
    /// Offer the `.isdd` file to other apps.
    Share,
    /// Write the `.asc` schedule where the user chooses.
    ExportAsc,
    /// Offer the `.asc` schedule to other apps.
    ShareAsc,
}

impl Action {
    /// Whether the platform's share sheet (instead of a save dialog) takes the file.
    const fn is_share(self) -> bool {
        matches!(self, Self::Share | Self::ShareAsc)
    }
}

/// Runs `action` on the open design. With `then_pending`, a save that completes also runs
/// the action waiting behind the unsaved-changes question (see [`guard`]); a save that
/// does not complete drops it.
pub fn start(window: &AppWindow, state: &Shared, action: Action, then_pending: bool) {
    if action == Action::Save && state.borrow().document.library_entry().is_some() {
        super::library::save(window, state, then_pending);
        return;
    }
    let built = {
        let mut app = state.borrow_mut();
        match action {
            Action::Save | Action::SaveAs | Action::Share => document::save_bytes(&mut app),
            Action::ExportAsc | Action::ShareAsc => document::asc_bytes(&app),
        }
    };
    let SavedFile { file, warning } = match built {
        Ok(saved) => saved,
        Err(error) => {
            if then_pending {
                guard::clear_pending();
            }
            toast(window, &error.message);
            return;
        }
    };
    // Clone the Arc first: a platform may answer on the calling thread, and the state
    // must not be borrowed while it does.
    let platform = Arc::clone(state.borrow().platform());
    if action.is_share() {
        share(window, platform.as_ref(), file, warning.as_deref());
        return;
    }
    let weak = window.as_weak();
    platform.create_document(
        file,
        Box::new(move |result| {
            run_on_ui(&weak, move |window, state| {
                finished(
                    window,
                    state,
                    action,
                    then_pending,
                    result,
                    warning.as_deref(),
                );
            });
        }),
    );
}

/// Offers `file` to other apps.
fn share(window: &AppWindow, platform: &dyn Platform, file: OutgoingFile, warning: Option<&str>) {
    match platform.share(file) {
        Ok(()) => {
            if let Some(warning) = warning {
                toast(window, warning);
            }
        }
        Err(error) => toast(window, &error.message),
    }
}

/// The platform answered a save or export request.
fn finished(
    window: &AppWindow,
    state: &Shared,
    action: Action,
    then_pending: bool,
    result: Result<SaveOutcome, PlatformError>,
    warning: Option<&str>,
) {
    match result {
        Ok(SaveOutcome::Saved { display_name }) => {
            let verb = if matches!(action, Action::Save | Action::SaveAs) {
                document::mark_saved(&mut state.borrow_mut(), &display_name);
                crate::design::refresh(window, state);
                "Saved"
            } else {
                "Exported"
            };
            let text = warning.map_or_else(
                || format!("{verb} {display_name}"),
                |warning| format!("{verb} {display_name}. {warning}"),
            );
            toast(window, &text);
            if then_pending {
                guard::run_pending(window, state);
            }
        }
        Ok(SaveOutcome::Cancelled) => {
            if then_pending {
                guard::clear_pending();
            }
        }
        Err(error) => {
            if then_pending {
                guard::clear_pending();
            }
            toast(window, &error.message);
        }
    }
}
