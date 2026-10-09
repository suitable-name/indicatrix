//! "Locate inclusion from photos..." (the Rough planner) and the camera rigs it needs.
//!
//! Two windows, both opened from the planner and both hidden (never destroyed) when closed,
//! like the planner itself:
//!
//! - [`window`]: the locate window. Photos of a mesh rough on a fixed rig, the alignment of the
//!   mesh to the rig, the marks, the solve with its checks, and Accept, which adds the point as
//!   a shell through the planner's inclusion hook (one undo step).
//! - [`rig_window`]: the rig editor and the beam-splitter-cube calibration.
//!
//! The decisions that are not drawing (parsing a rig form, the clicks, the words of a report,
//! the worker pipelines) are in [`crate::locate_io`], where their tests run. What is here is
//! the Slint wiring: every heavy step (decoding a photo, calibrating, aligning, solving) runs
//! on a worker thread, and no `RefCell` borrow is held across a file dialog or a job.

mod canvas;
mod jobs;
mod photo;
mod rig_window;
mod window;

use super::host::Host;
use crate::{RoughPlanModel, locate_io::rig_form::default_stone_n, settings::SettingsPersister};
use indicatrix_cut_core::rough_plan::locate::RigProfile;
use slint::ComponentHandle;
use std::rc::Rc;

/// Opens the locate window (the planner's "Locate inclusion from photos..." button).
pub(super) fn open_locate(planner: &Rc<Host>) {
    window::open(planner);
}

/// The locate window's alignment and photos (`zoning` feature: the Rough colour wizard reads
/// them), or `None` when the window was never opened or nothing is aligned.
#[cfg(feature = "zoning")]
pub(super) fn alignment_snapshot()
-> Option<crate::gui::rough_colour::wizard::context::AlignmentSnapshot> {
    window::alignment_snapshot()
}

/// Closes both windows for good, with the planner. Called wherever the main window hides.
pub(super) fn close_windows() {
    window::close();
    rig_window::close();
}

/// The camera rigs stored in the settings file.
fn stored_rigs() -> Vec<RigProfile> {
    SettingsPersister::installed_for_this_thread()
        .map(|persister| persister.snapshot().rig_profiles)
        .unwrap_or_default()
}

/// Changes the stored rigs through the settings persister (the only writer of the file).
/// Returns whether there was a persister to write through.
fn update_rigs(change: impl FnOnce(&mut Vec<RigProfile>)) -> bool {
    SettingsPersister::installed_for_this_thread().is_some_and(|persister| {
        persister.update(|file| change(&mut file.rig_profiles));
        true
    })
}

/// The refractive index of the planner's current material, the default stone index of a rig.
fn planner_stone_n(planner: &Host) -> f64 {
    let index = planner
        .window
        .global::<RoughPlanModel>()
        .get_material_index();
    let name = usize::try_from(index).ok().and_then(|i| {
        planner
            .session
            .borrow()
            .choices
            .get(i)
            .map(|c| c.name.clone())
    });
    name.map_or_else(|| default_stone_n(""), |name| default_stone_n(&name))
}
