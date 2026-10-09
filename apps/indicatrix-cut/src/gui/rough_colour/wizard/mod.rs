//! The "Rough colour..." wizard (`zoning` feature): photos of a mesh rough on a backlit rig become
//! the colour of the stone, and of colour zones inside it.
//!
//! Eight steps, in a strip like the cutting mode's: [`steps::Step`]. The window is `RoughColourWindow`
//! (`ui/rough_colour_window.slint`), created on the first open and hidden when closed so its
//! photos and results survive.
//!
//! * Window-free logic with tests: [`steps`] (which step can open), [`brush`] (the mask brush),
//!   [`filters`] (the reference filter set file), [`checklist`], [`progress`] (stage weights and the
//!   ETA line), [`raster`] (the pictures), [`compare`] (difference map, tables, colour chips),
//!   [`report`] (Markdown and PNG export) and [`zone_rows`] (the zones panel as data, the
//!   refinement iterations).
//! * Workers: [`work`] (prepare a view, calibrate) and [`fitwork`] (fit, refine, suggest, zones from
//!   marks); they run on threads and return plain data.
//! * The window: [`host`] (creation, jobs), [`state`], [`view_model`] (push), [`actions`]
//!   (callbacks), [`context`] (what the planner and the locate window give).
//!
//! It lives outside `gui::rough_plan` on purpose: the planner's host is private to that module, so
//! `gui::rough_plan::colour_link` builds a [`context::PlannerLink`] and hands it to [`open`].

pub mod actions;
pub mod brush;
pub mod checklist;
pub mod compare;
pub mod context;
pub mod filters;
pub mod fitwork;
pub mod host;
pub mod progress;
pub mod raster;
pub mod report;
pub mod state;
pub mod steps;
pub mod view_model;
pub mod work;
pub mod zone_rows;

pub use host::{close, open};
