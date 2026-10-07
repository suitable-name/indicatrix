//! The UI events the viewing, output, library and app tutorials wait for.
//!
//! A tutorial step can read most of what it needs from the design (a tier moved, a tier
//! added) or from the view mode. The rest happens only on screen: a facet was clicked, the
//! Cut slider moved to the rough, a file was exported, the Rough Planner opened. The desktop
//! reports each of those once, by name, through `GuideModel.event`; a step then waits for the
//! name with `Goal::Event`.
//!
//! Every name here is listed in [`ALL`], which `catalog::EVENTS` includes, so a step that
//! waits for a name nobody reports fails the registry test. The desktop raises them through
//! `gui::tutorial_events::raise`, one call at the place the action succeeds, and the
//! desktop's own guard test (`tests/guide_highlights.rs`) checks that every name below has a
//! call site.

/// A tier became the selected one: a row was clicked in the tier table, or a facet was
/// clicked in the Solid or Diagram view.
pub const TIER_SELECTED: &str = "tier_selected";

/// A facet was clicked in the Solid, Path-traced, Both or Diagram view and it belongs to a
/// tier.
pub const FACET_PICKED: &str = "facet_picked";

/// The Snap pill was clicked.
pub const SNAP_TOGGLED: &str = "snap_toggled";

/// Slice mode was switched on (the Slice pill, or the S key).
pub const SLICE_STARTED: &str = "slice_started";

/// A line drawn across the stone became a provisional tier.
pub const SLICE_DRAWN: &str = "slice_drawn";

/// The provisional tier was flipped to cut the other side.
pub const SLICE_FLIPPED: &str = "slice_flipped";

/// The provisional tier now touches the stone (its depth handle was dragged inward), so
/// Keep will accept it.
pub const SLICE_CUTS_STONE: &str = "slice_cuts_stone";

/// The Symmetric pill of the Slice tool was clicked.
pub const SLICE_SYMMETRIC_TOGGLED: &str = "slice_symmetric_toggled";

/// The Cut slider was moved all the way left, to the rough.
pub const CUT_SLIDER_ROUGH: &str = "cut_slider_rough";

/// The Cut slider was moved to a position between the rough and the finished stone.
pub const CUT_SLIDER_STEP: &str = "cut_slider_step";

/// The Cut slider was moved all the way right, to the finished stone.
pub const CUT_SLIDER_FINISHED: &str = "cut_slider_finished";

/// One of the Diagram view's three panels was enlarged to fill the view.
pub const DIAGRAM_PANEL_ENLARGED: &str = "diagram_panel_enlarged";

/// A step was marked done in cutting mode.
pub const CUTTING_STEP_MARKED: &str = "cutting_step_marked";

/// An `.asc` file was exported from the editor.
pub const ASC_EXPORTED: &str = "asc_exported";

/// A Gem Cut Studio `.gcs` file was exported from the editor.
pub const GCS_EXPORTED: &str = "gcs_exported";

/// The printable HTML cutting sheet was exported.
pub const SHEET_EXPORTED: &str = "sheet_exported";

/// The diagram PNG was exported.
pub const DIAGRAM_EXPORTED: &str = "diagram_exported";

/// The design was saved to its `.indicatrix` file.
pub const DESIGN_SAVED: &str = "design_saved";

/// A different design replaced the open one: New Design, Load Selected or Open.
pub const DESIGN_REPLACED: &str = "design_replaced";

/// Designs were imported into the library.
pub const LIBRARY_IMPORTED: &str = "library_imported";

/// The library search box was edited and now holds text.
pub const LIBRARY_SEARCHED: &str = "library_searched";

/// The library search box was edited and is now empty: the learner cleared the search.
pub const LIBRARY_SEARCH_CLEARED: &str = "library_search_cleared";

/// A library filter changed: the shape or gear drop-down, a range slider or the sort order.
pub const LIBRARY_FILTERED: &str = "library_filtered";

/// The library had a search or a filter on, and a change took the last one off: nothing narrows
/// the list any more (the sort order does not count).
pub const LIBRARY_FILTERS_RESET: &str = "library_filters_reset";

/// A design was selected in the library list.
pub const LIBRARY_DESIGN_SELECTED: &str = "library_design_selected";

/// The Rough Planner window opened.
pub const ROUGH_PLANNER_OPENED: &str = "rough_planner_opened";

/// A rough plan finished and its layouts are on screen.
pub const ROUGH_PLAN_FINISHED: &str = "rough_plan_finished";

/// A custom material was saved in the Material Editor.
pub const CUSTOM_MATERIAL_SAVED: &str = "custom_material_saved";

/// A custom material was saved with its refractive index typed as Sellmeier or Cauchy
/// coefficients.
pub const COEFFICIENT_MATERIAL_SAVED: &str = "coefficient_material_saved";

/// The Live Render tab was shown.
pub const LIVE_RENDER_OPENED: &str = "live_render_opened";

/// A lighting preset was chosen in the Live Render toolbar.
pub const LIGHTING_PRESET_CHOSEN: &str = "lighting_preset_chosen";

/// The lighting on screen was saved for the open design.
pub const DESIGN_LIGHTING_SAVED: &str = "design_lighting_saved";

/// The open design's saved lighting was forgotten.
pub const DESIGN_LIGHTING_FORGOTTEN: &str = "design_lighting_forgotten";

/// The Simple | Advanced switch was used.
pub const INTERFACE_MODE_CHANGED: &str = "interface_mode_changed";

/// The High contrast switch in Preferences was used.
pub const HIGH_CONTRAST_CHANGED: &str = "high_contrast_changed";

/// The Interface scale in Preferences was changed.
pub const UI_SCALE_CHANGED: &str = "ui_scale_changed";

/// The Larger handles switch in Preferences was used.
pub const LARGE_HANDLES_CHANGED: &str = "large_handles_changed";

/// The keyboard shortcuts list opened (Help menu, or the ? key).
pub const SHORTCUTS_OPENED: &str = "shortcuts_opened";

/// The manual opened in the help window (from the Help menu, F1 or a round ? button).
pub const HELP_OPENED: &str = "help_opened";

/// The glossary opened.
pub const GLOSSARY_OPENED: &str = "glossary_opened";

/// Every event above, for `catalog::EVENTS` and for the desktop's guard test.
pub const ALL: &[&str] = &[
    TIER_SELECTED,
    FACET_PICKED,
    SNAP_TOGGLED,
    SLICE_STARTED,
    SLICE_DRAWN,
    SLICE_FLIPPED,
    SLICE_CUTS_STONE,
    SLICE_SYMMETRIC_TOGGLED,
    CUT_SLIDER_ROUGH,
    CUT_SLIDER_STEP,
    CUT_SLIDER_FINISHED,
    DIAGRAM_PANEL_ENLARGED,
    CUTTING_STEP_MARKED,
    ASC_EXPORTED,
    GCS_EXPORTED,
    SHEET_EXPORTED,
    DIAGRAM_EXPORTED,
    DESIGN_SAVED,
    DESIGN_REPLACED,
    LIBRARY_IMPORTED,
    LIBRARY_SEARCHED,
    LIBRARY_SEARCH_CLEARED,
    LIBRARY_FILTERED,
    LIBRARY_FILTERS_RESET,
    LIBRARY_DESIGN_SELECTED,
    ROUGH_PLANNER_OPENED,
    ROUGH_PLAN_FINISHED,
    CUSTOM_MATERIAL_SAVED,
    COEFFICIENT_MATERIAL_SAVED,
    LIVE_RENDER_OPENED,
    LIGHTING_PRESET_CHOSEN,
    DESIGN_LIGHTING_SAVED,
    DESIGN_LIGHTING_FORGOTTEN,
    INTERFACE_MODE_CHANGED,
    HIGH_CONTRAST_CHANGED,
    UI_SCALE_CHANGED,
    LARGE_HANDLES_CHANGED,
    SHORTCUTS_OPENED,
    HELP_OPENED,
    GLOSSARY_OPENED,
];

/// The cut slider event for a `SolidPreviewModel.tier_cutoff` value: `-2` is the rough,
/// `-1` the finished stone and `0` or more a position after that many steps.
#[must_use]
pub const fn cut_slider_event(cutoff: i32) -> &'static str {
    match cutoff {
        -2 => CUT_SLIDER_ROUGH,
        -1 => CUT_SLIDER_FINISHED,
        _ => CUT_SLIDER_STEP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_listed_once_and_is_lower_case_words() {
        for (index, name) in ALL.iter().enumerate() {
            assert!(!ALL[..index].contains(name), "{name} is listed twice");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not lower_snake_case"
            );
        }
    }

    #[test]
    fn the_cut_slider_events_follow_the_cutoff() {
        assert_eq!(cut_slider_event(-2), CUT_SLIDER_ROUGH);
        assert_eq!(cut_slider_event(-1), CUT_SLIDER_FINISHED);
        assert_eq!(cut_slider_event(0), CUT_SLIDER_STEP);
        assert_eq!(cut_slider_event(7), CUT_SLIDER_STEP);
    }
}
