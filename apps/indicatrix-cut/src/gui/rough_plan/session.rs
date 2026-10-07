//! UI-thread session state for the rough planner window.

use super::{
    base::blank_model, inputs::FilterSnapshot, mesh_task::MeshJobs, run::RunState,
    saved::SavedState, view::ViewState,
};
use indicatrix::geometry::stone_metrics::SolidMesh;
use indicatrix_cut_core::rough_plan::{RoughMeasure, RoughModel};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex, atomic::AtomicBool},
};

/// The material the window selects on its first open.
pub const DEFAULT_MATERIAL: &str = "Quartz";

/// The message shown when the active library is a remote one.
pub const REMOTE_MESSAGE: &str = "Switch to the local library to plan a rough.";

/// How many earlier models the undo and redo stacks each keep.
pub(super) const UNDO_CAP: usize = 100;

/// One entry of the window's material list: a name and its known specific gravity.
#[derive(Debug, Clone, PartialEq)]
pub struct MaterialChoice {
    /// The material's name.
    pub name: String,
    /// Its specific gravity.
    pub specific_gravity: f64,
}

/// The candidate design ids the two counts in the window were taken from, filled by the
/// background query that opening the window starts. A display aid only: Plan resolves
/// its own ids on the worker thread, and the lists keep the designs excluded from the
/// planner that the shown counts leave out.
#[derive(Default)]
pub struct CachedIds {
    /// Bumped on every open; a slower, older query sees the mismatch and drops its result.
    pub generation: u64,
    /// The ids of the current library filter, sorted.
    pub filter: Option<Vec<i64>>,
    /// The ids of the whole library, sorted.
    pub library: Option<Vec<i64>>,
}

impl CachedIds {
    /// Forgets the cached ids and returns the new generation.
    pub fn reset(&mut self) -> u64 {
        self.generation += 1;
        self.filter = None;
        self.library = None;
        self.generation
    }
}

/// What the callbacks share on the UI thread (they all run there).
pub(super) struct Session {
    /// The model being edited, including edits that are not committed yet.
    pub(super) model: RoughModel,
    /// Earlier committed models, oldest first (at most [`UNDO_CAP`]).
    pub(super) undo: Vec<RoughModel>,
    /// Models undone, most recently undone last.
    pub(super) redo: Vec<RoughModel>,
    /// The model as of the last commit, undo or redo: what [`Session::commit`] compares
    /// the live model against.
    pub(super) committed: RoughModel,
    /// Bumped on every model change (live or committed); older shape results are stale.
    pub(super) revision: u64,
    /// The materials behind `RoughPlanModel.materials`, index for index.
    pub(super) choices: Vec<MaterialChoice>,
    /// The running plan's cancel flag, if a plan was ever started.
    pub(super) cancel: Option<Arc<AtomicBool>>,
    /// The design ids the open-time background query found, shared with that thread.
    pub(super) ids: Arc<Mutex<CachedIds>>,
    /// The designs excluded from the planner, as last read from the library; the rows'
    /// Excluded pills and the Candidate designs list mirror it.
    pub(super) excluded: BTreeMap<i64, String>,
    /// The latest shape worker mesh, in the centred frame (bounding-box centre at the
    /// origin, mm).
    pub(super) model_mesh: Option<Arc<SolidMesh>>,
    /// The latest shape worker measurement.
    pub(super) model_measure: Option<RoughMeasure>,
    /// The carat text the window last wrote into the carat field itself (the model's own
    /// weight for the picked material). While the field still holds it, nobody has typed
    /// a weighed carat over it.
    pub(super) carat_shown: Option<String>,
    /// Cut fields (row, field number) whose text is not a number right now.
    pub(super) bad_fields: BTreeSet<(usize, i32)>,
    /// The azimuth typed for a face cut, by cut index. A vertical face cannot tell its
    /// azimuth, so the window keeps the typed one to show it and to turn back to when
    /// the elevation leaves the pole. Cleared whenever the rows are rebuilt from the
    /// model (undo, redo, load, adding or removing a cut), which may renumber the cuts.
    pub(super) face_azimuths: BTreeMap<usize, f64>,
    /// The library filter the design counts in the window were last asked for; the
    /// counts are asked again when the filter differs.
    pub(super) counted_filter: Option<FilterSnapshot>,
    /// The view's state.
    pub(super) view: ViewState,
    /// The plan run's state.
    pub(super) run: RunState,
    /// The saved plans' state.
    pub(super) saved: SavedState,
    /// The background mesh job (an OBJ import or a mesh scaling) the window waits for.
    pub(super) mesh_jobs: MeshJobs,
}

impl Default for Session {
    fn default() -> Self {
        let model = blank_model();
        Self {
            committed: model.clone(),
            model,
            undo: Vec::new(),
            redo: Vec::new(),
            revision: 0,
            choices: Vec::new(),
            cancel: None,
            ids: Arc::default(),
            excluded: BTreeMap::new(),
            model_mesh: None,
            model_measure: None,
            carat_shown: None,
            bad_fields: BTreeSet::new(),
            face_azimuths: BTreeMap::new(),
            counted_filter: None,
            view: ViewState::default(),
            run: RunState::default(),
            saved: SavedState::default(),
            mesh_jobs: MeshJobs::default(),
        }
    }
}

impl Session {
    /// Makes the current model an undo step if it differs from the last committed one.
    ///
    /// Committing an unchanged model (Enter followed by the focus-out that ends the same
    /// edit) does nothing. Returns whether a step was recorded.
    pub(super) fn commit(&mut self) -> bool {
        if self.model == self.committed {
            return false;
        }
        if self.undo.len() >= UNDO_CAP {
            self.undo.remove(0);
        }
        let previous = std::mem::replace(&mut self.committed, self.model.clone());
        self.undo.push(previous);
        self.redo.clear();
        true
    }

    /// Steps back to the previous committed model (committing pending edits first).
    /// Returns whether the model changed.
    pub(super) fn undo_step(&mut self) -> bool {
        self.commit();
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        if self.redo.len() >= UNDO_CAP {
            self.redo.remove(0);
        }
        let current = std::mem::replace(&mut self.committed, previous.clone());
        self.redo.push(current);
        self.model = previous;
        self.face_azimuths.clear();
        self.revision += 1;
        true
    }

    /// Steps forward to the model that was undone. Returns whether the model changed.
    ///
    /// Refused while a live edit is pending: redoing would silently throw that edit away,
    /// so the edit has to be committed (or undone) first.
    pub(super) fn redo_step(&mut self) -> bool {
        if self.model != self.committed {
            return false;
        }
        let Some(next) = self.redo.pop() else {
            return false;
        };
        if self.undo.len() >= UNDO_CAP {
            self.undo.remove(0);
        }
        let current = std::mem::replace(&mut self.committed, next.clone());
        self.undo.push(current);
        self.model = next;
        self.face_azimuths.clear();
        self.revision += 1;
        true
    }

    /// Replaces the model (a load or a reset) as one undo step.
    pub(super) fn replace_model(&mut self, model: RoughModel) {
        self.model = model;
        self.bad_fields.clear();
        self.face_azimuths.clear();
        self.revision += 1;
        self.commit();
    }

    /// Whether the window's Undo has anything to undo.
    #[must_use]
    pub(super) fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.model != self.committed
    }

    /// Whether the window's Redo has anything to redo (not while a live edit is pending,
    /// see [`Session::redo_step`]).
    #[must_use]
    pub(super) fn can_redo(&self) -> bool {
        !self.redo.is_empty() && self.model == self.committed
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{BoxFace, RoughBase, RoughCut};

    fn block(x: f64) -> RoughModel {
        RoughModel::new(
            RoughBase::Block {
                x_mm: x,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            Vec::new(),
        )
    }

    fn edit(session: &mut Session, x: f64) {
        session.model = block(x);
        session.commit();
    }

    #[test]
    fn committing_the_same_model_twice_records_one_step() {
        let mut session = Session::default();
        assert!(session.undo.is_empty() && !session.can_undo());
        session.model = block(10.0);
        assert!(session.commit());
        // Enter and then the focus-out that follows it commit the same edit.
        assert!(!session.commit());
        assert_eq!(session.undo.len(), 1);
        assert_eq!(session.undo[0], blank_model());
    }

    #[test]
    fn a_live_edit_is_not_a_step_until_it_is_committed() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        session.model = block(11.0);
        session.model = block(12.0);
        assert_eq!(session.undo.len(), 1, "typing alone records nothing");
        assert!(session.can_undo());
        assert!(session.commit());
        assert_eq!(session.undo.len(), 2);
        assert_eq!(session.undo[1], block(10.0));
    }

    #[test]
    fn undo_and_redo_walk_the_committed_models() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        edit(&mut session, 11.0);
        edit(&mut session, 12.0);

        assert!(session.undo_step());
        assert_eq!(session.model, block(11.0));
        assert!(session.undo_step());
        assert_eq!(session.model, block(10.0));
        assert!(session.can_redo());
        assert!(session.redo_step());
        assert_eq!(session.model, block(11.0));
        assert!(session.redo_step());
        assert_eq!(session.model, block(12.0));
        assert!(!session.redo_step(), "nothing left to redo");
        assert!(!session.can_redo());
    }

    #[test]
    fn undo_first_commits_the_pending_edit_and_a_new_edit_clears_redo() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        session.model = block(11.0);
        assert!(session.undo_step());
        assert!(
            session.model == block(10.0),
            "undo returns to before the pending edit"
        );
        assert!(session.can_redo());
        edit(&mut session, 13.0);
        assert!(!session.can_redo(), "a new edit ends the redo branch");
    }

    #[test]
    fn a_pending_live_edit_blocks_redo_and_keeps_the_redo_stack() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        edit(&mut session, 11.0);
        assert!(session.undo_step());
        assert!(session.can_redo());
        // A live edit that is not committed yet.
        session.model = block(20.0);
        assert!(!session.can_redo(), "redo would discard the live edit");
        assert!(!session.redo_step());
        assert_eq!(session.model, block(20.0), "the live edit survives");
        assert_eq!(session.redo.len(), 1, "the redo stack is untouched");
        // Back to the committed model, redo works again.
        session.model = block(10.0);
        assert!(session.can_redo());
        assert!(session.redo_step());
        assert_eq!(session.model, block(11.0));
    }

    #[test]
    fn undo_on_an_empty_history_does_nothing() {
        let mut session = Session::default();
        assert!(!session.undo_step());
        assert!(!session.can_undo());
        assert_eq!(session.model, blank_model());
    }

    #[test]
    fn the_history_keeps_at_most_the_cap_and_drops_the_oldest() {
        let mut session = Session::default();
        for i in 1..=150_u32 {
            edit(&mut session, f64::from(i));
        }
        assert_eq!(session.undo.len(), UNDO_CAP);
        // The oldest kept model is the one 100 edits back: x = 150 - 100 = 50.
        assert_eq!(session.undo[0], block(50.0));
        assert_eq!(session.undo[UNDO_CAP - 1], block(149.0));
        for _ in 0..UNDO_CAP {
            assert!(session.undo_step());
        }
        assert_eq!(session.model, block(50.0));
        assert!(!session.undo_step());
        assert_eq!(session.redo.len(), UNDO_CAP);
    }

    #[test]
    fn replacing_the_model_is_one_undo_step_and_bumps_the_revision() {
        let mut session = Session::default();
        edit(&mut session, 10.0);
        let before = session.revision;
        let shaped = RoughModel::new(
            RoughBase::Block {
                x_mm: 20.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            vec![RoughCut::Edge {
                faces: [BoxFace::Top, BoxFace::Front],
                setbacks_mm: [1.0, 1.0],
            }],
        );
        session.bad_fields.insert((0, 1));
        session.face_azimuths.insert(0, 45.0);
        session.replace_model(shaped.clone());
        assert!(session.revision > before);
        assert!(session.bad_fields.is_empty());
        assert!(
            session.face_azimuths.is_empty(),
            "typed azimuths belong to the old model's cut numbers"
        );
        assert_eq!(session.undo.len(), 2);
        assert!(session.undo_step());
        assert_eq!(session.model, block(10.0));
        assert!(session.redo_step());
        assert_eq!(session.model, shaped);
    }
}
