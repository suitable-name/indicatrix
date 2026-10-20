//! Cutting: the instructions of a solved design, the printable cutting sheet, the done
//! marks and (in `mode`) Cutting Mode's steps.
//!
//! Everything here is plain Rust over [`crate::app_state::AppState`]; the front end only
//! moves the results onto the screen.
//!
//! | Module | What it holds |
//! |---|---|
//! | `progress` | [`ProgressStore`]: done marks per design and per step signature, in `<files>/cut_progress/<key>.json` |
//! | `key` | which file a design's marks live in (its id, else a SHA-256 of its text) |
//! | `rows` | [`instruction_rows`] and [`summary`]: the Cut page's list and progress card |
//! | `sheet` | [`SheetJob`], [`cutting_sheet_html`], [`diagram_png`]: the sheet and the picture |
//! | `diagram` | [`DiagramJob`]: the three-panel diagram with one tier highlighted |
//! | `mode` | Cutting Mode's step state (its own lane) |
//!
//! Every function that reads instructions needs a CURRENT solve
//! (`state.solve.current_tiers(generation)`); without one it answers [`NotSolved`], and the
//! front end offers to solve.
//!
//! # Deleting marks
//!
//! [`ProgressStore::reset`] clears the open design's marks, [`delete_all_progress`] every
//! design's. [`progress_file_stems`] and [`remove_progress_files`] name and remove the files
//! of a design that is not open (a library entry being deleted). None of these needs a solve.

mod diagram;
mod key;
mod progress;
mod rows;
mod sheet;
#[cfg(test)]
mod tests;

pub mod mode;

use std::{
    cell::RefCell,
    fmt,
    path::{Path, PathBuf},
    rc::Rc,
};

use indicatrix_cut_core::Design;
use indicatrix_editor::cutting_mode::{CuttingPlan, build_plan};

pub use diagram::{DiagramJob, diagram_job, render_highlighted};
pub use progress::ProgressStore;
pub use rows::{CutRow, CutSummary, angle_text, index_text, instruction_rows, summary};
pub use sheet::{
    SheetJob, cutting_sheet_html, date_text_for, date_text_now, diagram_png, file_name, sheet_job,
};

use crate::{app_state::AppState, document};

/// The design has no solve of its current generation (or no tiers): there is nothing to
/// cut yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotSolved;

impl fmt::Display for NotSolved {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Solve the design to see cutting instructions")
    }
}

impl std::error::Error for NotSolved {}

/// Why a sheet or a picture could not be made.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CutError {
    /// See [`NotSolved`].
    NotSolved,
    /// The design could not be drawn; the sentence says why.
    Failed(String),
}

impl From<NotSolved> for CutError {
    fn from(_: NotSolved) -> Self {
        Self::NotSolved
    }
}

impl fmt::Display for CutError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotSolved => fmt::Display::fmt(&NotSolved, f),
            Self::Failed(message) => f.write_str(message),
        }
    }
}

impl std::error::Error for CutError {}

/// Cutting Mode and the done marks. See the module docs.
#[derive(Default)]
pub struct CuttingState {
    /// The done marks of the open design (bound by [`sync_progress`]).
    pub progress: ProgressStore,
    /// Cutting Mode while it is open.
    pub mode: Option<mode::ModeState>,
    /// The plan of the newest generation asked for, so a push does not rebuild it.
    plan_cache: RefCell<Option<(u64, Rc<CuttingPlan>)>>,
}

impl fmt::Debug for CuttingState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CuttingState")
            .field("progress", &self.progress)
            .field("mode_open", &self.mode.is_some())
            .finish_non_exhaustive()
    }
}

/// The cutting plan (one step per tier, in cutting order) of the open design.
///
/// # Errors
/// [`NotSolved`] when the design has no solve of its current generation, no tiers, or a
/// solve that does not fit its tiers.
pub fn plan(state: &AppState) -> Result<Rc<CuttingPlan>, NotSolved> {
    let generation = state.session.current_generation();
    if state.solve.current_tiers(generation).is_none() {
        return Err(NotSolved);
    }
    let cached = state
        .cutting
        .plan_cache
        .borrow()
        .as_ref()
        .filter(|(cached_generation, _)| *cached_generation == generation)
        .map(|(_, plan)| Rc::clone(plan));
    if let Some(plan) = cached {
        return Ok(plan);
    }
    let design = &state.session.design;
    let solved = state.solve.current_tiers(generation).ok_or(NotSolved)?;
    if solved.len() != design.tiers.len() {
        return Err(NotSolved);
    }
    let plan = build_plan(design, solved, &document::custom_catalogue(state)).ok_or(NotSolved)?;
    let plan = Rc::new(plan);
    *state.cutting.plan_cache.borrow_mut() = Some((generation, Rc::clone(&plan)));
    Ok(plan)
}

/// The folder the done-mark files live in, below the app's files directory `files`.
#[must_use]
pub fn progress_dir(files: &Path) -> PathBuf {
    files.join("cut_progress")
}

/// Binds the done marks to the open design (loading its file) unless that is already so.
///
/// Cheap when nothing changed; [`instruction_rows`], [`summary`] and [`plan`]-based callers
/// run it themselves, Cutting Mode runs it before it reads a mark.
pub fn sync_progress(state: &AppState) {
    let binding = progress::Binding {
        generation: state.session.current_generation(),
        id: state.document.metadata.id.clone(),
        dir: progress_dir(&state.platform().app_dirs().files),
    };
    if state.cutting.progress.is_bound_to(&binding) {
        return;
    }
    let keys = key::progress_keys(state);
    state
        .cutting
        .progress
        .bind(binding, &keys.primary, keys.legacy.as_deref());
}

/// The names (without `.json`) of the files `design`'s done marks can be stored in.
///
/// For a design that is not open: `id-<id>` when it carries the `[meta]` id `meta_id` (pass
/// an empty text when it has none) and `sha-<hash>`, the key of its text, which also holds
/// the marks it had before it got an id.
///
/// The text key belongs to this exact text. Marks stored under the text of an earlier edit
/// of an id-less design are not named here; [`delete_all_progress`] removes those.
#[must_use]
pub fn progress_file_stems(meta_id: &str, design: &Design) -> Vec<String> {
    key::keys_of(meta_id, design).into_stems()
}

/// Deletes the done-mark files `stems` (see [`progress_file_stems`]) name from the progress
/// folder below the app's files directory `files`. Files that are not there are fine.
pub fn remove_progress_files(files: &Path, stems: &[String]) {
    progress::remove_files(&progress_dir(files), stems);
}

/// How many designs have a done-mark file stored on this device.
#[must_use]
pub fn stored_progress_count(state: &AppState) -> usize {
    progress::count(&progress_dir(&state.platform().app_dirs().files))
}

/// Deletes the cutting progress of every design on this device, the open one included.
///
/// Its marks in memory go too, and the function returns how many designs' files were
/// deleted. The marks of designs that were edited without an id, which no design points at
/// any more, go with them.
pub fn delete_all_progress(state: &AppState) -> usize {
    let dir = progress_dir(&state.platform().app_dirs().files);
    state.cutting.progress.delete_all(&dir)
}
