//! Guided tutorials on the desktop: the worked example, the welcome tour and every other
//! guide in `indicatrix_editor::guide`'s catalogue, shown by the step panel
//! (`ui/models/guide.slint`'s `GuideModel`) and listed in the tutorial browser
//! (`ui/models/tutorials.slint`'s `TutorialsModel`).
//!
//! The guides are data in the editor crate. The Slint side owns navigation
//! (`start_guide`/`next`/`back`/`close`, the short "Done" moment before an automatic
//! advance) and every lock: each control ANDs a `GuideModel.allows_*()` helper into its own
//! `enabled:`, driven by the current step's `allow` flags. This module tree is the Rust half:
//!
//! - `launch`: starts a guide -- arranges the design it starts from (a new one, a library
//!   one, the open one), pushes its steps into `GuideModel` and opens the first.
//! - `progress`: decides, from STATE, whether the current step's goal is met and reports it
//!   through `GuideModel.notify`; also takes UI events (`progress::guide_event`, the handler
//!   of `GuideModel.event`; other desktop code reports an event with `gui::tutorial_events::raise`).
//! - `browser`: the tutorial browser's list, the welcome dialog and the "finished" marks.
//! - `build_design`: "Build this design" -- reads a library design, generates its rebuild
//!   lesson off the UI thread, registers it ([`register_generated_guide`]) and starts it.
//! - `runtime`: what is remembered between steps (the catalogue, the events seen, the
//!   launch in progress).
//!
//! [`check_progress`] is called wherever the design or its solve state can have changed (see
//! its module); a UI event reaches the guide through `GuideModel.event(name)`, which Rust
//! callers use too.

mod browser;
mod build_design;
mod launch;
mod progress;
mod runtime;

pub(in crate::gui::editor) use indicatrix_editor::guide::NEW_DESIGN_CREATED;
pub(in crate::gui::editor) use progress::{check_design_progress, check_progress, notify};

use super::state::EditorState;
use crate::{MainWindow, bridge::library::source::LibrarySource};
use indicatrix_editor::guide::Guide;
use indicatrix_vault::db::sqlite::Database;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// Wires every tutorial callback (`GuideModel`, `TutorialsModel`, `BuildDesignModel`) and
/// remembers the editor state, so a UI event can re-check the current step. `db` and
/// `source` are the library "Build this design" reads the design to rebuild from. Called
/// from `gui::editor::setup_editor_callbacks`.
pub(in crate::gui::editor) fn setup_guide(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    browser::setup(ui, state);
    build_design::setup(ui, db, source);
}

/// Adds a guide generated at run time -- the lesson that rebuilds a library design, say --
/// to the catalogue, so the tutorial browser lists it and `GuideModel.start_guide(id)` can
/// start it. A second guide with the same id replaces the first.
///
/// # Errors
///
/// A guide that is not fit to run (a typo in a highlight target or an event name, a step
/// that never says what it waits for) or that takes a built-in guide's id is refused with
/// the reason.
pub(in crate::gui::editor) fn register_generated_guide(guide: Guide) -> Result<(), String> {
    runtime::register_generated(guide)
}
