//! What the results on screen were planned from, kept on the UI thread.

use crate::{
    RoughPlanModel, RoughPlanResult,
    gui::rough_plan::{
        host::Host,
        saved::{convert::CandidateSource, dto::DesignShape},
    },
};
use indicatrix_cut_core::rough_plan::{PlanSettings, RoughLayout, RoughModel};
use slint::{ModelRc, SharedString};
use std::{collections::BTreeMap, rc::Rc};

/// Whether a design of a loaded plan still is what the plan was saved with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui::rough_plan) enum DesignStatus {
    /// The design is in the library and unchanged.
    Unchanged,
    /// The design is in the library but its geometry changed since the plan was saved.
    Changed,
    /// The design is no longer in the library.
    Deleted,
    /// The saved entry is gone, but a design with the same title was found.
    MatchedByTitle {
        /// The entry the title resolved to.
        resolved_entry_id: i64,
    },
}

/// Where the results on screen came from.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(in crate::gui::rough_plan) enum ResultsSource {
    /// A plan just run in this window.
    #[default]
    Planned,
    /// A saved plan opened from the library.
    Loaded {
        /// The saved plan's id.
        plan_id: i64,
        /// Its name.
        name: String,
        /// When it was saved, in Unix seconds.
        created_at: i64,
    },
}

/// What the window showed about the results when a run pushed them off the screen, so a
/// run that ends without results can put the same rows back.
#[derive(Default)]
pub(in crate::gui::rough_plan) struct Screen {
    /// The result rows themselves (the same model, not a copy).
    pub rows: ModelRc<RoughPlanResult>,
    /// The summary line.
    pub summary: SharedString,
    /// Designs skipped by the run that made the rows.
    pub skipped: i32,
    /// The selected result row, `-1` for none.
    pub selected_result: i32,
    /// The selected design row of that result, `-1` for none.
    pub selected_group: i32,
    /// The result whose cut plan was open, `-1` for none.
    pub expanded_index: i32,
    /// How many rows had their keep box ticked.
    pub keep_count: i32,
    /// The banner above a loaded plan's rows, empty for a fresh plan.
    pub banner: SharedString,
}

impl Screen {
    /// What `model` shows of the results right now.
    pub(in crate::gui::rough_plan) fn capture(model: &RoughPlanModel<'_>) -> Self {
        Self {
            rows: model.get_results(),
            summary: model.get_summary(),
            skipped: model.get_skipped_count(),
            selected_result: model.get_selected_result(),
            selected_group: model.get_selected_group(),
            expanded_index: model.get_expanded_index(),
            keep_count: model.get_keep_count(),
            banner: model.get_loaded_banner(),
        }
    }

    /// Shows this screen again.
    pub(in crate::gui::rough_plan) fn show(self, model: &RoughPlanModel<'_>) {
        model.set_results(self.rows);
        model.set_summary(self.summary);
        model.set_skipped_count(self.skipped);
        model.set_selected_result(self.selected_result);
        model.set_selected_group(self.selected_group);
        model.set_expanded_index(self.expanded_index);
        model.set_keep_count(self.keep_count);
        model.set_loaded_banner(self.banner);
    }
}

/// The results a run that is still going pushed off the screen, kept whole until that run
/// publishes its own.
pub(in crate::gui::rough_plan) struct Replaced {
    /// The state behind the rows (its own `replaced` and `pending` are empty).
    state: RunState,
    /// The rows and the texts around them.
    screen: Screen,
}

/// A Plan request waiting for the answer to "replace the results that were not saved?".
pub(in crate::gui::rough_plan) type Proceed = Box<dyn FnOnce(&Rc<Host>)>;

/// The results on screen and the plan they came from. Saving uses this snapshot, never
/// the live-edited model.
#[derive(Default)]
pub(in crate::gui::rough_plan) struct RunState {
    /// The shown top list, in rank order (the index is the result row index).
    pub(in crate::gui::rough_plan) layouts: Vec<RoughLayout>,
    /// The model the shown layouts were planned for.
    pub(in crate::gui::rough_plan) plan_model: Option<RoughModel>,
    /// The settings the shown layouts were planned with.
    pub(in crate::gui::rough_plan) plan_settings: Option<PlanSettings>,
    /// The material of the plan (for saving and labels).
    pub(in crate::gui::rough_plan) material_name: String,
    /// The weighed carat of the plan, if one was entered.
    pub(in crate::gui::rough_plan) weighed_ct: Option<f64>,
    /// Where the plan's candidate designs came from.
    pub(in crate::gui::rough_plan) candidate_source: CandidateSource,
    /// The design titles of every entry id in `layouts`.
    pub(in crate::gui::rough_plan) titles: BTreeMap<i64, String>,
    /// Staleness by design; empty for a fresh plan.
    pub(in crate::gui::rough_plan) statuses: BTreeMap<i64, DesignStatus>,
    /// The shape each design had when the shown layouts were planned (a loaded plan: what
    /// its file stored), by the entry id the layouts use. A save writes these shapes
    /// rather than measuring the designs again, so the file describes the stones as
    /// planned.
    pub(in crate::gui::rough_plan) shapes: BTreeMap<i64, DesignShape>,
    /// The "keep" tick of every result row.
    pub(in crate::gui::rough_plan) keep: Vec<bool>,
    /// Where the shown results came from.
    pub(in crate::gui::rough_plan) source: ResultsSource,
    /// Whether the shown results are a fresh plan that no save has covered yet. Set when
    /// a plan's results are published; cleared when a save covers every result and when a
    /// saved plan is shown. The window mirrors it as `RoughPlanModel.results_unsaved`.
    pub(in crate::gui::rough_plan) results_unsaved: bool,
    /// The results a running plan pushed off the screen, put back if it ends without
    /// results and dropped when it publishes its own.
    pub(super) replaced: Option<Box<Replaced>>,
    /// The Plan request waiting for the answer to the replace question.
    pub(super) pending: Option<Proceed>,
    /// Bumped by every [`show_layouts`](super::show_layouts) and every new run, so a slower
    /// earlier push notices it was superseded.
    pub(super) generation: u64,
    /// The summary line of the run that just finished, for the push that follows it.
    pub(super) summary: Option<String>,
}

impl RunState {
    /// Forgets the shown results (the list is cleared) and supersedes any push in flight.
    pub(in crate::gui::rough_plan) fn clear(&mut self) {
        let generation = self.generation + 1;
        *self = Self {
            generation,
            ..Self::default()
        };
    }

    /// A new run starts: the shown results and what `screen` showed of them are kept
    /// aside until the run publishes its own, and any push in flight is superseded.
    pub(in crate::gui::rough_plan) fn begin_run(&mut self, screen: Screen) {
        let generation = self.generation + 1;
        let mut state = std::mem::take(self);
        state.replaced = None;
        state.pending = None;
        *self = Self {
            generation,
            replaced: Some(Box::new(Replaced { state, screen })),
            ..Self::default()
        };
    }

    /// The run ended without results: the state [`Self::begin_run`] set aside is the
    /// shown state again, and its screen is returned for the window to show. `None` when
    /// nothing was set aside (the run already published, or none began).
    pub(in crate::gui::rough_plan) fn restore_replaced(&mut self) -> Option<Screen> {
        let Replaced { state, screen } = *self.replaced.take()?;
        let generation = self.generation + 1;
        *self = Self {
            generation,
            ..state
        };
        Some(screen)
    }

    /// How many result rows have their keep box ticked.
    #[must_use]
    pub(in crate::gui::rough_plan) fn kept(&self) -> usize {
        self.keep.iter().filter(|&&kept| kept).count()
    }
}

/// The results to show: the fields of [`RunState`] except the keep ticks, which start
/// all clear.
pub(in crate::gui::rough_plan) struct ShownLayouts {
    /// The layouts in rank order.
    pub layouts: Vec<RoughLayout>,
    /// The model they were planned for.
    pub plan_model: Option<RoughModel>,
    /// The settings they were planned with.
    pub plan_settings: Option<PlanSettings>,
    /// The plan's material.
    pub material_name: String,
    /// The plan's weighed carat.
    pub weighed_ct: Option<f64>,
    /// Where the plan's candidate designs came from.
    pub candidate_source: CandidateSource,
    /// The design titles of every entry id in `layouts`.
    pub titles: BTreeMap<i64, String>,
    /// Staleness by design (empty for a fresh plan).
    pub statuses: BTreeMap<i64, DesignStatus>,
    /// The shape each design had when the shown layouts were planned (a loaded plan: what
    /// its file stored), by the entry id the layouts use. A save writes these shapes
    /// rather than measuring the designs again, so the file describes the stones as
    /// planned.
    pub shapes: BTreeMap<i64, DesignShape>,
    /// Where the results came from.
    pub source: ResultsSource,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clearing_forgets_everything_but_moves_the_generation_on() {
        let mut state = RunState {
            plan_settings: Some(PlanSettings::default()),
            material_name: "Quartz".to_string(),
            keep: vec![true, false, true],
            source: ResultsSource::Loaded {
                plan_id: 3,
                name: "Aqua".to_string(),
                created_at: 0,
            },
            results_unsaved: true,
            generation: 4,
            summary: Some("x".to_string()),
            ..RunState::default()
        };
        assert_eq!(state.kept(), 2);
        state.clear();
        assert_eq!(state.generation, 5);
        assert!(state.plan_settings.is_none() && state.summary.is_none());
        assert!(state.material_name.is_empty() && state.keep.is_empty());
        assert_eq!(state.source, ResultsSource::Planned);
        assert!(!state.results_unsaved);
        assert_eq!(state.kept(), 0);
    }

    #[test]
    fn a_run_keeps_the_previous_results_aside_and_a_cancel_brings_them_back() {
        let mut state = RunState {
            material_name: "Quartz".to_string(),
            keep: vec![true, false],
            results_unsaved: true,
            generation: 4,
            ..RunState::default()
        };
        let screen = Screen {
            skipped: 7,
            selected_result: 1,
            ..Screen::default()
        };

        state.begin_run(screen);
        assert_eq!(state.generation, 5, "a push in flight is superseded");
        assert!(state.material_name.is_empty() && state.keep.is_empty());
        assert!(!state.results_unsaved, "the run starts with nothing shown");
        assert!(state.replaced.is_some());

        let back = state.restore_replaced().expect("the old results were kept");
        assert_eq!((back.skipped, back.selected_result), (7, 1));
        assert_eq!(state.material_name, "Quartz");
        assert_eq!(state.keep, vec![true, false]);
        assert!(state.results_unsaved, "the old results were still unsaved");
        assert_eq!(state.generation, 6, "restoring supersedes pushes as well");
        assert!(state.replaced.is_none());
        assert!(
            state.restore_replaced().is_none(),
            "there is nothing left to put back"
        );
    }

    #[test]
    fn publishing_a_new_result_drops_the_previous_one_for_good() {
        let mut state = RunState {
            material_name: "Quartz".to_string(),
            ..RunState::default()
        };
        state.begin_run(Screen::default());
        // What `publish` does when the run's own results arrive.
        state.replaced = None;
        state.material_name = "Beryl".to_string();
        assert!(state.restore_replaced().is_none());
        assert_eq!(state.material_name, "Beryl");
    }

    #[test]
    fn beginning_a_run_twice_never_nests_the_kept_state() {
        let mut state = RunState {
            material_name: "Quartz".to_string(),
            ..RunState::default()
        };
        state.begin_run(Screen::default());
        state.material_name = "Beryl".to_string();
        state.begin_run(Screen::default());
        let _ = state.restore_replaced().expect("kept");
        assert_eq!(state.material_name, "Beryl");
        assert!(state.replaced.is_none(), "only the latest state is kept");
    }
}
