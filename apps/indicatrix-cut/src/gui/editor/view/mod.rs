//! Pushing [`EditorState`](super::state::EditorState) into `EditorView`/the shared
//! viewport ([`refresh_editor_panel`]/`refresh_viewport`/[`refresh_all`]/
//! [`refresh_editor_panel_stale`]), and the Deep Solve/Optimize hint and result-
//! formatting helpers. See this group's own `mod.rs` doc comment for the "Solve on
//! explicit action, not on every edit" reasoning [`refresh_editor_panel_stale`] exists
//! to honour.
//!
//! Split into sibling files by responsibility: [`panel`] (the "real solve" panel
//! refresh and its `_from_solved` proportion/yield mirrors), [`panel_stale`] (the
//! no-solve counterpart, plus the Deep Solve/Optimize availability pushes both
//! paths share), [`inspector`] (the design-settings panel, the viewport material
//! link, and the Tier tab's facet-chip row), [`viewport`] (feeding the shared GPU/
//! solid-preview viewport, and the paint-first `refresh_all_now`/`refresh_all`
//! entry points), [`solve_results`] (Deep Solve/Optimize availability hints and
//! per-tier result tables), and [`optimize_apply`] (the Optimize result summary
//! table, its ghost-preview candidate, and the weight-form parser). This file only
//! re-declares the modules and re-exports what the rest of `gui::editor` reaches
//! through `view::*`.

mod inspector;
mod optimize_apply;
mod panel;
mod panel_stale;
mod solve_results;
mod viewport;

use super::state;

// Re-exported so sibling `callbacks::*` modules can spell this `view::SolidLastSolved`
// (matching how they already reach every other `view::*` helper) rather than reaching
// past this module into `solid_preview::preview_state` directly.
pub(in crate::gui::editor) use crate::gui::solid_preview::preview_state::SolidLastSolved;

pub(in crate::gui::editor) use inspector::{push_selected_tier_chips, sync_viewport_material_link};
pub(in crate::gui::editor) use optimize_apply::{
    build_optimize_preview_design, optimize_result_rows, optimize_status_text,
    parse_optimize_weights, submit_design_ghost_preview,
};
pub(in crate::gui::editor) use panel::{
    girdle_and_ratio_texts_from_solved, preform_mm_texts_from_solved,
    proportions_texts_from_solved, push_proportion_verdicts_from_solved, refresh_editor_panel,
};
pub(in crate::gui::editor) use panel_stale::{
    configured_optimize_max_evaluations, refresh_editor_panel_stale,
};
pub(in crate::gui::editor) use solve_results::{
    deep_solve_tier_rows, facet_count_from_solved, format_deep_solve_report, optimize_change_rows,
};
pub(in crate::gui::editor) use viewport::{
    ReplanSource, push_has_design, refresh_all, refresh_all_now, scaled_viewport_size,
    submit_preview_replan, submit_preview_replan_chained, submit_preview_replan_for,
};
// `push_solved_preview` is the one push function `editor::apply_matching_preview_frame`
// (this group's other public entry point) also needs -- `push_stale_content` stays
// `pub(super)` in `panel_stale`, reachable only from within `view` itself.
pub(in crate::gui::editor) use viewport::push_solved_preview;
