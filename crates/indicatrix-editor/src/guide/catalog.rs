//! The registry of guides: the built-in ones, any generated at run time, and the lists
//! that keep a guide's highlight targets and event names honest.
//!
//! **Adding a tutorial** is one function that returns a [`Guide`] (built with
//! [`Guide::new`] and [`GuideStep::new`]) and one line in [`BUILDERS`]. A test checks that
//! every built-in guide is fit to run: unique ids, known highlight targets and events,
//! steps that say what they wait for.

use super::{
    ANGLE_TOLERANCE_DEG, MANUAL, NEW_DESIGN_CREATED, expected_tier,
    goal::Goal,
    model::{Guide, GuideCategory, GuideStep, StartingState},
    perform::attach_performs,
    steps::{STEPS, Step},
    tour::welcome_tour_guide,
    tutorials,
};

/// The id of the worked example (Chapter 7): the guide `GuideModel.start()` opens.
pub const WORKED_EXAMPLE_ID: &str = "new-design-walkthrough";

/// Every `highlight_target` some `.slint` component actually checks for, or `""` for none.
///
/// A target missing here fails a guide's check; a target listed here that no component
/// draws silently means "no outline", so add a name in the same change that makes a
/// component compare `GuideModel.highlight_target` against it.
pub const HIGHLIGHT_TARGETS: &[&str] = &[
    "",
    // The New Design dialog, and the New Design... button and empty-state card while it
    // is closed.
    "new_design_dialog",
    // The inspector's Tier tab.
    "inspector_tier",
    // The Design Settings panel.
    "design_settings",
    // The Solve button on the command bar.
    "solve_button",
    // The tier table.
    "tier_table",
    // The inspector's Preform tab.
    "preform_tab",
    // The solving, optimizing and comparing tutorials (`tutorials::solving`).
    // The Auto-solve list of the command bar.
    "auto_solve",
    // The overall verdict badge.
    "verdict_badge",
    // The Deep Solve button of the command bar.
    "deep_solve_button",
    // The inspector's Optimize tab.
    "optimize_tab",
    // The Retarget... button of the command bar.
    "retarget_button",
    // The inspector's History tab.
    "history_tab",
    // The Snapshot button of the command bar.
    "snapshot_button",
    // The Compare (to the snapshot) button of the command bar.
    "compare_button",
    // The viewing, output, library and app tutorials (`tutorials::viewing`).
    // The Solid viewport picture.
    "solid_viewport",
    // The Solid | Path-traced | Both | Diagram pills of the viewport toolbar.
    "view_modes",
    // The Cut slider of the viewport toolbar.
    "cut_slider",
    // The Snap pill of the viewport toolbar.
    "snap_pill",
    // The Slice pill of the viewport toolbar.
    "slice_pill",
    // The Live Render tab above the picture.
    "live_render_tab",
    // The Lighting drop-down of the Live Render toolbar.
    "lighting_combo",
    // The Cut Mode button of the command bar.
    "cutting_mode_button",
    // The Export .asc button of the command bar.
    "export_asc_button",
    // The Export... split menu of the command bar.
    "export_menu",
    // The Save button of the command bar.
    "save_button",
    // The Open button of the command bar.
    "open_button",
    // The Load Selected button of the command bar.
    "load_button",
    // The library search box in the header.
    "library_search",
    // The Shape, Gear, Sort and Filters controls in the header.
    "library_filters",
    // The Import button in the header.
    "library_import",
    // The Simple | Advanced pill in the header.
    "mode_switch",
];

/// Every UI event name a [`Goal::Event`] may wait for.
///
/// A UI reports one with `GuideModel.event("name")` (Slint or Rust). Add a name here in the
/// same change that adds the call that reports it: an event nobody reports is a step that
/// never completes.
pub const EVENTS: &[&str] = &[
    // A design was created from the New Design dialog (or by a guide starting one).
    NEW_DESIGN_CREATED,
    // The command palette opened.
    "palette_opened",
    // The tutorial browser opened.
    "tutorials_opened",
    // Two or more tiers were ticked into the tier table's multi-select group.
    tutorials::TIERS_MULTI_SELECTED,
    // The solving, optimizing and comparing tutorials (`tutorials::solving_events`; each name
    // is documented there, and `gui::tutorial_events::raise` or a Slint call reports it).
    tutorials::solving_events::SOLVE_REQUESTED,
    tutorials::solving_events::AUTO_SOLVE_OFF,
    tutorials::solving_events::AUTO_SOLVE_ON,
    tutorials::solving_events::VERDICT_OPENED,
    tutorials::solving_events::DEEP_SOLVE_STARTED,
    tutorials::solving_events::DEEP_SOLVE_FINISHED,
    tutorials::solving_events::OPTIMIZE_PRESET_CHOSEN,
    tutorials::solving_events::OPTIMIZE_RANGES_OPENED,
    tutorials::solving_events::OPTIMIZE_FINISHED,
    tutorials::solving_events::OPTIMIZE_CANDIDATE_PICKED,
    tutorials::solving_events::VARIANTS_OPENED,
    tutorials::solving_events::VARIANT_SAVED,
    tutorials::solving_events::VARIANTS_COMPARED,
    tutorials::solving_events::SNAPSHOT_TAKEN,
    tutorials::solving_events::COMPARE_OPENED,
    tutorials::solving_events::COMPARE_WINDOW_OPENED,
    tutorials::solving_events::RAW_TEXT_DIFF_SHOWN,
    // The viewing, output, library and app tutorials (`tutorials::viewing_events`; each
    // name is documented there, and `gui::tutorial_events::raise` reports it).
    // Picking in the viewports and the tier table.
    tutorials::viewing_events::TIER_SELECTED,
    tutorials::viewing_events::FACET_PICKED,
    // The drag tools: Snap, Slice, the Cut slider and the Diagram panels.
    tutorials::viewing_events::SNAP_TOGGLED,
    tutorials::viewing_events::SLICE_STARTED,
    tutorials::viewing_events::SLICE_DRAWN,
    tutorials::viewing_events::SLICE_FLIPPED,
    tutorials::viewing_events::SLICE_CUTS_STONE,
    tutorials::viewing_events::SLICE_SYMMETRIC_TOGGLED,
    tutorials::viewing_events::CUT_SLIDER_ROUGH,
    tutorials::viewing_events::CUT_SLIDER_STEP,
    tutorials::viewing_events::CUT_SLIDER_FINISHED,
    tutorials::viewing_events::DIAGRAM_PANEL_ENLARGED,
    // Cutting mode, the four exports, Save, and a design replaced.
    tutorials::viewing_events::CUTTING_STEP_MARKED,
    tutorials::viewing_events::ASC_EXPORTED,
    tutorials::viewing_events::GCS_EXPORTED,
    tutorials::viewing_events::SHEET_EXPORTED,
    tutorials::viewing_events::DIAGRAM_EXPORTED,
    tutorials::viewing_events::DESIGN_SAVED,
    tutorials::viewing_events::DESIGN_REPLACED,
    // The library and the Rough Planner.
    tutorials::viewing_events::LIBRARY_IMPORTED,
    tutorials::viewing_events::LIBRARY_SEARCHED,
    tutorials::viewing_events::LIBRARY_SEARCH_CLEARED,
    tutorials::viewing_events::LIBRARY_FILTERED,
    tutorials::viewing_events::LIBRARY_FILTERS_RESET,
    tutorials::viewing_events::LIBRARY_DESIGN_SELECTED,
    tutorials::viewing_events::ROUGH_PLANNER_OPENED,
    tutorials::viewing_events::ROUGH_PLAN_FINISHED,
    // Materials, Live Render and a design's lighting.
    tutorials::viewing_events::CUSTOM_MATERIAL_SAVED,
    tutorials::viewing_events::COEFFICIENT_MATERIAL_SAVED,
    tutorials::viewing_events::LIVE_RENDER_OPENED,
    tutorials::viewing_events::LIGHTING_PRESET_CHOSEN,
    tutorials::viewing_events::DESIGN_LIGHTING_SAVED,
    tutorials::viewing_events::DESIGN_LIGHTING_FORGOTTEN,
    // Preferences, the shortcut list, the manual and the glossary.
    tutorials::viewing_events::INTERFACE_MODE_CHANGED,
    tutorials::viewing_events::HIGH_CONTRAST_CHANGED,
    tutorials::viewing_events::UI_SCALE_CHANGED,
    tutorials::viewing_events::LARGE_HANDLES_CHANGED,
    tutorials::viewing_events::SHORTCUTS_OPENED,
    tutorials::viewing_events::HELP_OPENED,
    tutorials::viewing_events::GLOSSARY_OPENED,
];

/// The goal a completion key of the shared [`STEPS`] stands for, or `None` for a key no
/// goal matches.
#[must_use]
pub fn goal_from_completion(key: &str) -> Option<Goal> {
    if key == MANUAL {
        return Some(Goal::Manual);
    }
    if key == NEW_DESIGN_CREATED {
        return Some(Goal::Event(NEW_DESIGN_CREATED.to_owned()));
    }
    match key {
        "solved_closed" => return Some(Goal::SolvedClosed),
        "yield_applied" => return Some(Goal::YieldApplied),
        _ => {}
    }
    if let Some(material) = key.strip_prefix("material:") {
        return Some(Goal::Material(material.to_owned()));
    }
    let name = key.strip_prefix("tier_named:")?;
    let (angle_deg, indices) = expected_tier(name)?;
    Some(Goal::TierMatches {
        name: name.to_owned(),
        angle_deg,
        tol_deg: ANGLE_TOLERANCE_DEG,
        indices: Some(indices.to_vec()),
        constraint_kind: None,
    })
}

/// One shared [`Step`] as a [`GuideStep`], word for word.
fn guide_step_from(step: &Step) -> GuideStep {
    GuideStep {
        title: step.title.to_owned(),
        intro: step.intro.to_owned(),
        actions: step.actions.iter().map(|&a| a.to_owned()).collect(),
        check: step.check.to_owned(),
        why: step.why.to_owned(),
        waiting: step.waiting.to_owned(),
        highlight_target: step.highlight_target.to_owned(),
        goal: goal_from_completion(step.completion).unwrap_or(Goal::Manual),
        allow: step.allow.to_vec(),
        perform: None,
    }
}

/// The worked example as a [`Guide`], built from the same [`STEPS`] the web app walks
/// through: same text, same locks, same goals.
#[must_use]
pub fn worked_example_guide() -> Guide {
    let mut guide = Guide::new(
        WORKED_EXAMPLE_ID,
        "New design walkthrough",
        "Build an 8-fold round brilliant from an empty design: girdle, pavilion, crown and table.",
        GuideCategory::GettingStarted,
    )
    .starting(StartingState::CurrentDesign);
    guide.steps = STEPS.iter().map(guide_step_from).collect();
    guide
}

/// The built-in guides, each made by one function. Add a tutorial by adding its function.
const BUILDERS: &[fn() -> Guide] = &[welcome_tour_guide, worked_example_guide];

/// Every built-in guide, in browser order: the ones in [`BUILDERS`], then the per-function
/// tutorials of [`tutorials::all`].
///
/// Every action step carries what Next does for the learner
/// ([`attach_performs`]), or nothing where the step cannot be done automatically.
#[must_use]
pub fn static_guides() -> Vec<Guide> {
    let mut guides: Vec<Guide> = BUILDERS.iter().map(|build| build()).collect();
    guides.extend(tutorials::all());
    attach_performs(&mut guides);
    guides
}

/// The guides a UI offers: the built-in ones plus any generated while it runs.
#[derive(Clone, Debug)]
pub struct GuideCatalog {
    guides: Vec<Guide>,
    /// How many entries of `guides` are built in; the rest were generated.
    built_in: usize,
}

impl Default for GuideCatalog {
    fn default() -> Self {
        Self::new()
    }
}

impl GuideCatalog {
    /// A catalogue holding the built-in guides.
    #[must_use]
    pub fn new() -> Self {
        let guides = static_guides();
        let built_in = guides.len();
        Self { guides, built_in }
    }

    /// Every guide, built-in first, then generated, each group in the order added.
    #[must_use]
    pub fn all(&self) -> &[Guide] {
        &self.guides
    }

    /// The guide with this id.
    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Guide> {
        self.guides.iter().find(|guide| guide.id == id)
    }

    /// Whether the guide with this id was generated at run time.
    #[must_use]
    pub fn is_generated(&self, id: &str) -> bool {
        self.guides
            .iter()
            .skip(self.built_in)
            .any(|guide| guide.id == id)
    }

    /// Adds a guide generated at run time, replacing an earlier generated guide with the
    /// same id (so rebuilding the lesson for a library design twice keeps one entry).
    ///
    /// # Errors
    ///
    /// A guide that is not fit to run ([`Guide::problem`]), or one whose id belongs to a
    /// built-in guide, is refused with the reason.
    pub fn add_generated(&mut self, guide: Guide) -> Result<(), String> {
        if let Some(problem) = guide.problem() {
            return Err(problem);
        }
        if self
            .guides
            .iter()
            .take(self.built_in)
            .any(|built_in| built_in.id == guide.id)
        {
            return Err(format!("{:?} is the id of a built-in guide", guide.id));
        }
        match self
            .guides
            .iter()
            .skip(self.built_in)
            .position(|existing| existing.id == guide.id)
        {
            Some(offset) => self.guides[self.built_in + offset] = guide,
            None => self.guides.push(guide),
        }
        Ok(())
    }
}
