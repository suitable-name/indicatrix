//! The window-free half of "Locate inclusion from photos" and "Calibrate rig": everything the
//! two windows decide that is not drawing, so that its tests run with the rest of the
//! workspace (the planner window's own tests are never run).
//!
//! The numbers come from the core (`indicatrix_cut_core::rough_plan::locate`); this module
//! turns what the user types and clicks into the core's inputs and the core's results into
//! lines of words:
//!
//! - [`rig_form`]: a rig as text fields, parsed with the field named in every message;
//! - [`rig_store`]: the named rigs kept in the settings file;
//! - [`axes`]: the coarse axis choice and nudges that start the mesh-to-rig alignment;
//! - [`marks`] and [`calib_marks`]: the clicks on the inclusion photos and on the cube photos;
//! - [`pipeline`]: the alignment and the solve, as run on a worker thread;
//! - [`overlay`]: what the photo canvas draws, and the click-to-pixel mapping;
//! - [`report`]: the lines the windows show;
//! - [`record`]: a located inclusion with its marks and rig, for solving it again.

pub mod axes;
pub mod calib_marks;
pub mod marks;
pub mod overlay;
pub mod pipeline;
pub mod record;
pub mod report;
pub mod rig_form;
pub mod rig_store;
