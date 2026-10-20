//! [`ProgressStore`]: the done marks of the open design.
//!
//! Marks are kept per design and per step signature (see `indicatrix_editor::cutting_mode`):
//! a mark counts only while the step's cutting values equal the ones it was set for, so a
//! step whose angle, indices or depth changed since reads "changed", not "done" (the
//! desktop's rule). The marks live in `<files>/cut_progress/<key>.json`, written atomically
//! (temp file and rename) on every change; the key comes from [`super::key`].
//!
//! The store loads lazily: [`super::sync_progress`] binds it to the open design the first
//! time anything needs it, and again after the design was replaced. Its methods take
//! `&self` (the state sits behind a `RefCell` of its own) so a front end can read marks
//! from the `&AppState` that a push receives.
//!
//! Removing marks is the other half of the job: [`ProgressStore::reset`] clears the open
//! design's marks, [`ProgressStore::delete_all`] every design's, and the free functions
//! [`remove_files`] and [`count`] work on the folder without a bound store (a deleted
//! library design's files, the "how many" of a confirmation).

use std::{
    cell::RefCell,
    collections::BTreeMap,
    fs, io,
    path::{Path, PathBuf},
};

use indicatrix_editor::cutting_mode::{
    CuttingStep,
    progress::{MarkChange, Progress, StepState},
};
use serde::{Deserialize, Serialize};

/// The version written into every progress file.
const FILE_VERSION: u32 = 1;

/// What a progress file holds.
#[derive(Debug, Default, Serialize, Deserialize)]
struct StoredProgress {
    #[serde(default)]
    version: u32,
    /// Step key (or index key) to the step signature it was marked at.
    #[serde(default)]
    marks: BTreeMap<String, String>,
}

/// What the store was bound for: the design it describes and where its file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Binding {
    /// The session generation the binding was made at.
    pub generation: u64,
    /// The design's `[meta]` id when it has one.
    pub id: String,
    /// The directory the progress files live in.
    pub dir: PathBuf,
}

#[derive(Debug, Default)]
struct Inner {
    binding: Option<Binding>,
    path: Option<PathBuf>,
    /// The file of the design's text hash, when `path` is an id file: the marks the design
    /// had before it got an id are read from it, so a reset has to remove it too. It follows
    /// every edit of the text, so it is renewed by each binding.
    legacy: Option<PathBuf>,
    marks: BTreeMap<String, String>,
    progress: Progress,
}

/// The done marks of the open design. See the module docs.
#[derive(Debug, Default)]
pub struct ProgressStore {
    inner: RefCell<Inner>,
}

impl ProgressStore {
    /// Whether the store describes the design `binding` names.
    pub(super) fn is_bound_to(&self, binding: &Binding) -> bool {
        self.inner.borrow().binding.as_ref() == Some(binding)
    }

    /// Whether the store has been bound to a design (and so can save).
    #[must_use]
    pub fn is_bound(&self) -> bool {
        self.inner.borrow().path.is_some()
    }

    /// Whether the open design has any mark stored: a done mark, a mark whose step changed
    /// since, or a mark of a step the design no longer has. A reset has something to clear
    /// exactly when this is true.
    #[must_use]
    pub fn has_marks(&self) -> bool {
        !self.inner.borrow().marks.is_empty()
    }

    /// Binds the store to the design `binding` names: its marks are read from the file
    /// `key` names (or, when that file does not exist yet, from the file `legacy` names,
    /// the marks the design had before it got an id). Binding to the file the store is
    /// already bound to only records the new binding.
    pub(super) fn bind(&self, binding: Binding, key: &str, legacy: Option<&str>) {
        let path = binding.dir.join(format!("{key}.json"));
        let legacy_path = legacy.map(|legacy| binding.dir.join(format!("{legacy}.json")));
        let mut inner = self.inner.borrow_mut();
        if inner.path.as_ref() != Some(&path) {
            let marks = if path.is_file() {
                load(&path)
            } else {
                legacy_path
                    .as_ref()
                    .filter(|legacy_path| legacy_path.is_file())
                    .map(|legacy_path| load(legacy_path))
                    .unwrap_or_default()
            };
            inner.progress = Progress::from_marks(marks.clone());
            inner.marks = marks;
            inner.path = Some(path);
        }
        inner.legacy = legacy_path;
        inner.binding = Some(binding);
    }

    /// How `step` stands: not done, done, or marked once and changed since.
    #[must_use]
    pub fn state(&self, step: &CuttingStep) -> StepState {
        self.inner.borrow().progress.state(step)
    }

    /// Whether `step` is done (a mark made for other cutting values does not count).
    #[must_use]
    pub fn is_done(&self, step: &CuttingStep) -> bool {
        self.state(step) == StepState::Done
    }

    /// Marks `step` done or takes its mark back, and saves. Taking back also clears the
    /// step's index ticks.
    pub fn set_done(&self, step: &CuttingStep, done: bool) {
        let changes = {
            let inner = self.inner.borrow();
            match (done, inner.progress.state(step)) {
                (true, StepState::Done) | (false, StepState::NotDone) => return,
                (true, _) => inner.progress.mark_done(step),
                (false, StepState::Done) => inner.progress.toggle_done(step),
                (false, StepState::Changed) => vec![MarkChange::Unmark {
                    key: step.key.clone(),
                }],
            }
        };
        self.commit(&changes);
    }

    /// Flips `step` and returns whether it is done now.
    pub fn toggle_done(&self, step: &CuttingStep) -> bool {
        let done = !self.is_done(step);
        self.set_done(step, done);
        done
    }

    /// How many of `steps` are done.
    #[must_use]
    pub fn done_count(&self, steps: &[CuttingStep]) -> usize {
        self.inner.borrow().progress.done_count(steps)
    }

    /// How many of `steps` were marked once but changed since.
    #[must_use]
    pub fn changed_count(&self, steps: &[CuttingStep]) -> usize {
        self.inner.borrow().progress.changed_count(steps)
    }

    /// How much of `steps` is done, from 0 to 1.
    #[must_use]
    pub fn progress_fraction(&self, steps: &[CuttingStep]) -> f32 {
        if steps.is_empty() {
            return 0.0;
        }
        self.done_count(steps) as f32 / steps.len() as f32
    }

    /// Where cutting resumes: the first step not done, else the last step.
    #[must_use]
    pub fn resume_position(&self, steps: &[CuttingStep]) -> usize {
        self.inner.borrow().progress.resume_position(steps)
    }

    /// Removes every mark of the open design and deletes its files: the one it is stored
    /// in and, for a design with an id, the one of its text hash that its marks may have
    /// been adopted from (left in place, the next binding would read the marks back).
    pub fn reset(&self) {
        let mut inner = self.inner.borrow_mut();
        inner.marks.clear();
        inner.progress.clear();
        for path in inner.path.iter().chain(inner.legacy.iter()) {
            remove_logged(path);
        }
    }

    /// Forgets the marks of the open design and deletes every progress file in `dir`, the
    /// ones of all other designs included. Returns how many designs' files were deleted. The
    /// folder goes with its last file; marking a step again creates it anew.
    pub(super) fn delete_all(&self, dir: &Path) -> usize {
        {
            let mut inner = self.inner.borrow_mut();
            inner.marks.clear();
            inner.progress.clear();
        }
        remove_all(dir)
    }

    /// Applies `changes` to the marks and writes the file.
    fn commit(&self, changes: &[MarkChange]) {
        let mut inner = self.inner.borrow_mut();
        inner.progress.apply(changes);
        for change in changes {
            match change {
                MarkChange::Mark { key, signature } => {
                    inner.marks.insert(key.clone(), signature.clone());
                }
                MarkChange::Unmark { key } => {
                    inner.marks.remove(key);
                }
            }
        }
        if let Some(path) = &inner.path {
            if let Err(error) = save(path, &inner.marks) {
                tracing::warn!(%error, path = %path.display(), "the done marks could not be saved");
            }
        } else {
            tracing::warn!("done marks changed before the store was bound; not saved");
        }
    }
}

/// Deletes the files `stems` name in `dir` (`<stem>.json`, and a temp file a crash left
/// beside it). A file that is not there is fine; any other failure is logged.
pub(super) fn remove_files(dir: &Path, stems: &[String]) {
    for stem in stems {
        remove_logged(&dir.join(format!("{stem}.json")));
    }
}

/// How many progress files (one per design) `dir` holds.
pub(super) fn count(dir: &Path) -> usize {
    fs::read_dir(dir).map_or(0, |entries| {
        entries
            .flatten()
            .filter(|entry| is_progress_file(&entry.path()))
            .count()
    })
}

/// Deletes every file in `dir` and then `dir` itself, and returns how many of them were
/// progress files. A file that cannot be deleted is logged and left.
fn remove_all(dir: &Path) -> usize {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(%error, dir = %dir.display(), "the done marks could not be listed");
            }
            return 0;
        }
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        match fs::remove_file(&path) {
            Ok(()) => removed += usize::from(is_progress_file(&path)),
            Err(error) => {
                tracing::warn!(%error, path = %path.display(), "a done-marks file could not be removed");
            }
        }
    }
    // Best effort: the folder stays when something in it could not be deleted.
    let _ = fs::remove_dir(dir);
    removed
}

/// Whether `path` is the file of one design's marks (and not a temp file).
fn is_progress_file(path: &Path) -> bool {
    path.extension()
        .is_some_and(|extension| extension == "json")
}

/// The temp file [`save`] writes before it renames it over `path`.
fn temp_path(path: &Path) -> PathBuf {
    path.with_extension("json.tmp")
}

/// Deletes `path` and the temp file beside it. A missing file is fine; any other failure
/// is logged.
fn remove_logged(path: &Path) {
    for target in [path.to_path_buf(), temp_path(path)] {
        if let Err(error) = fs::remove_file(&target)
            && error.kind() != io::ErrorKind::NotFound
        {
            tracing::warn!(%error, path = %target.display(), "the done marks could not be removed");
        }
    }
}

/// The marks in `path`; an unreadable or corrupt file reads as no marks.
fn load(path: &Path) -> BTreeMap<String, String> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "the done marks could not be read");
            return BTreeMap::new();
        }
    };
    match serde_json::from_str::<StoredProgress>(&text) {
        Ok(stored) => stored.marks,
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "the done marks are corrupt; starting empty");
            BTreeMap::new()
        }
    }
}

/// Writes `marks` to `path` through a temp file and a rename. An empty set removes the
/// file instead.
fn save(path: &Path, marks: &BTreeMap<String, String>) -> io::Result<()> {
    if marks.is_empty() {
        return match fs::remove_file(path) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let stored = StoredProgress {
        version: FILE_VERSION,
        marks: marks.clone(),
    };
    let text = serde_json::to_string_pretty(&stored).map_err(io::Error::other)?;
    let temp = temp_path(path);
    fs::write(&temp, text)?;
    fs::rename(&temp, path)
}
