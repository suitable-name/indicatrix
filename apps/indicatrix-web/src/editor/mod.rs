//! The CAD editing panels of the web app: the Design dock's tier table
//! and command bar, tier editing, and the unsaved-changes dialog.
//!
//! # Layout
//!
//! - [`table`]: builds the tier-table rows (`indicatrix_editor::view_model::rows`), the
//!   solve status and the warning list from [`WebApp`](crate::app::state::WebApp), keeps
//!   `TierTableModel` in step with it (a 100 ms poll plus an immediate [`table::sync`]
//!   after every edit), and wires the table's own callbacks.
//! - [`selection`]: selecting tiers (plain, Ctrl/Shift multi-select, arrow keys, warning
//!   jumps) and keeping `WebApp::selected_tier` and `EditorSession::multi_selected` in
//!   step with the Solid and Diagram views.
//! - [`edit`]: the editing actions (delete, duplicate, move, detach, adopt, pin, steps,
//!   mirror, Solve, auto-solve) and [`edit::finish_edit`], the one path every edit ends in
//!   (refresh, persist, auto-solve); [`edit_add`] is quick-add.
//! - [`nudge`]: the inline angle cell's commit and the coalesced nudge.
//! - [`orbit_tools`]: Complete orbit and Mirror indices.
//! - [`unsaved`]: the Save / Discard / Cancel dialog in front of every action that would
//!   replace a design with unsaved changes ([`unsaved::confirm_discard`]).
//!
//! The inspector, design settings, New Design dialog, Optimize, Retarget and snapshot
//! panels, and the Solid view's mouse manipulation (`crate::views::manip`), live in their
//! own modules, declared below:
//!
//! - [`inspector`]: the Tier / Preform / Optimize / Schedule tabs and the refresh of every
//!   one of these panels ([`inspector::refresh`]);
//! - [`settings`]: the Design settings dialog (material, gear, symmetry, details, printed
//!   proportions);
//! - [`new_design`]: the full New Design dialog over the template gallery;
//! - [`optimize`] and [`retarget`]: the searches that run in the solve Worker;
//! - [`snapshot`]: Snapshot design / Compare to snapshot.
//!
//! The guided walkthrough is [`guide`]: the shared step content, the automatic
//! advance and the controls it locks.

pub mod edit;
pub mod edit_add;
pub mod guide;
pub mod inspector;
pub mod new_design;
pub mod nudge;
pub mod optimize;
pub mod orbit_tools;
pub mod retarget;
pub mod selection;
pub mod settings;
pub mod snapshot;
pub mod table;
pub mod unsaved;

use crate::{AppWindow, app::Ctx};

/// Wires the editor's callbacks. Call once from `app::main`, after `app::callbacks::wire`.
/// Other editor modules (the inspector, design settings, Optimize, ...) add their own
/// `wire` calls here.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    unsaved::wire(ui, ctx);
    table::wire(ui, ctx);
    inspector::wire(ui, ctx);
    settings::wire(ui, ctx);
    optimize::wire(ui, ctx);
    retarget::wire(ui, ctx);
    new_design::wire(ui, ctx);
    snapshot::wire(ui, ctx);
    guide::wire(ui, ctx);
}
