//! What the wizard needs from the Rough planner and the locate window, as plain data and
//! closures.
//!
//! The planner's host and the locate window's state are private to `gui::rough_plan`;
//! that module builds a [`PlannerLink`] (see its `colour_link`) and this module never reaches
//! into it.

use crate::MainWindow;
use indicatrix_cut_core::rough_plan::{locate::Rigid, shape::RoughMesh};
use indicatrix_vault::db::sqlite::Database;
use slint::Weak;
use std::{
    path::PathBuf,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// The rough the planner holds.
#[derive(Debug, Clone)]
pub struct PlannerContext {
    /// The registry id of the rough's mesh.
    pub mesh_id: u64,
    /// The mesh of the rough (with its inclusions).
    pub mesh: Arc<RoughMesh>,
    /// The saved plan's name, or "Rough" for a plan not saved yet.
    pub rough_name: String,
    /// The saved plan's id, if the shown results are a saved plan: the rough colour is stored
    /// against it.
    pub plan_id: Option<i64>,
    /// The planner's material choice (the host of the colour).
    pub host_material: String,
}

/// The alignment of the mesh to the rig that the locate window holds, with its photos.
#[derive(Debug, Clone)]
pub struct AlignmentSnapshot {
    /// The rig the alignment was made with.
    pub rig_name: String,
    /// Mesh to rig.
    pub transform: Rigid,
    /// The registry id of the mesh it was fitted to.
    pub mesh_id: u64,
    /// The stone photos loaded in the locate window, by view.
    pub photos: Vec<Option<PathBuf>>,
}

/// The planner as the wizard sees it.
#[derive(Clone)]
pub struct PlannerLink {
    /// The main window (file pickers need it).
    pub main: Weak<MainWindow>,
    /// The vault.
    pub db: Arc<Mutex<Database>>,
    /// The planner's rough now, or `None` when it has no mesh or the planner is gone.
    pub context: Rc<dyn Fn() -> Option<PlannerContext>>,
    /// The locate window's alignment now, or `None` when there is none.
    pub alignment: Rc<dyn Fn() -> Option<AlignmentSnapshot>>,
    /// Opens the locate window.
    pub open_locate: Rc<dyn Fn()>,
}
