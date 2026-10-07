//! Reports what happens on screen to the running tutorial.
//!
//! A tutorial step reads most of what it waits for from the design or from the view mode. The
//! rest leaves no trace there: a facet was clicked, the Cut slider moved, a file was exported,
//! the glossary opened. The code that does the action calls [`raise`] with one of the names in
//! `indicatrix_editor::guide::viewing_events` at the place the action succeeds, and a step that
//! waits for the name with `Goal::Event` completes.
//!
//! [`raise`] only passes the name on while a tutorial is open, so a call costs one property read
//! the rest of the time. It goes through `GuideModel.event`, the same entry the command palette
//! and the Slint components use, which records the event for the current step and re-checks the
//! step without holding the editor state: the check waits a moment when someone else is writing
//! it. A call is therefore safe from any callback on the UI thread, even in the middle of an
//! edit.
//!
//! The desktop's guard test (`tests/guide_highlights.rs`) checks that every event name has at
//! least one call site, so a name nobody raises cannot be added to the lessons unnoticed.

use crate::{GuideModel, MainWindow};
use slint::{ComponentHandle, Weak};

/// Reports UI event `name` to the open tutorial, if one is open. Use this for every event
/// except the one a launching tutorial waits for with none open yet (New Design, reported
/// through `gui::editor::guide::notify`).
pub fn raise(ui: &MainWindow, name: &str) {
    let guide = ui.global::<GuideModel>();
    if guide.get_open() {
        guide.invoke_event(name.into());
    }
}

/// [`raise`] for a callback that holds only a weak handle to the window. Does nothing when the
/// window is gone.
pub fn raise_weak(ui: &Weak<MainWindow>, name: &str) {
    if let Some(ui) = ui.upgrade() {
        raise(&ui, name);
    }
}
