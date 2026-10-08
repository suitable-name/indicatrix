//! The callbacks behind the tutorials: the browser's list, the welcome dialog's three
//! answers, and the running guide's own callbacks (start, step entered, UI event, poll,
//! perform, launch tick, finished).
//!
//! The Slint side is `ui/models/tutorials.slint` (`TutorialsModel`) and
//! `ui/models/guide.slint` (`GuideModel`). Which guides exist, how they are grouped and
//! searched, and the "Done" bookkeeping are pure functions in `indicatrix_editor::guide`;
//! this module only turns them into Slint rows and saves the result through the settings
//! persister.

use super::{build_design, launch, perform, progress, runtime};
use crate::{
    EditorModel, GuideModel, MainWindow, PreferencesModel, TutorialRowData, TutorialsModel,
    gui::editor::state::EditorState,
    settings::{SettingsPersister, model::AppSettings},
};
use indicatrix_editor::guide::{
    BrowserRow, GuideCatalog, WELCOME_TOUR_ID, browser_rows, completed_count, progress_text,
    record_completion, start_label, steps_text,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, collections::BTreeSet, rc::Rc};

/// The event the browser reports when it opens (a tutorial may wait for it).
const TUTORIALS_OPENED: &str = "tutorials_opened";

/// One [`BrowserRow`] as the Slint row the browser draws.
fn row_data(row: &BrowserRow) -> TutorialRowData {
    TutorialRowData {
        id: row.id.as_str().into(),
        title: row.title.as_str().into(),
        summary: row.summary.as_str().into(),
        category: row.category.label().into(),
        first_in_category: row.first_in_category,
        done: row.done,
        start_label: start_label(row.done).into(),
        blocked_reason: row.blocked.unwrap_or_default().into(),
        step_text: steps_text(row.step_count).into(),
    }
}

/// The rows the browser shows for `query`, and the line under them.
fn listing(
    catalog: &GuideCatalog,
    completed: &BTreeSet<String>,
    has_design: bool,
    query: &str,
) -> (Vec<TutorialRowData>, String) {
    let rows = browser_rows(catalog.all(), completed, has_design, query)
        .iter()
        .map(row_data)
        .collect();
    let line = progress_text(
        completed_count(catalog.all(), completed),
        catalog.all().len(),
    );
    (rows, line)
}

/// The ids of the tutorials finished so far, from the saved settings.
fn completed_tutorials() -> BTreeSet<String> {
    SettingsPersister::installed_for_this_thread()
        .map(|store| store.snapshot().settings.tutorials_completed)
        .unwrap_or_default()
}

/// Fills the browser's list for `query`.
fn fill_browser(ui: &MainWindow, query: &str) {
    let completed = completed_tutorials();
    let has_design = ui.global::<EditorModel>().get_has_design();
    let (rows, line) =
        runtime::with_catalog(|catalog| listing(catalog, &completed, has_design, query));
    let model = ui.global::<TutorialsModel>();
    model.set_rows(ModelRc::new(VecModel::from(rows)));
    model.set_progress_text(line.into());
}

/// Notes in `settings` that guide `id` was finished. Finishing the welcome tour also counts
/// as having seen the welcome.
fn apply_finished(settings: &mut AppSettings, id: &str) {
    record_completion(&mut settings.tutorials_completed, id);
    if id == WELCOME_TOUR_ID {
        settings.first_run_tour_done = true;
    }
}

/// Saves that guide `id` was finished.
fn record_finished(ui: &MainWindow, id: &str) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|file| apply_finished(&mut file.settings, id));
    }
    if id == WELCOME_TOUR_ID {
        ui.global::<PreferencesModel>()
            .set_first_run_tour_pending(false);
    }
}

/// Saves that the welcome dialog has been seen, and hides it. Every one of its answers
/// does this, so it never comes back on its own.
fn welcome_seen(ui: &MainWindow) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|file| file.settings.first_run_tour_done = true);
    }
    ui.global::<PreferencesModel>()
        .set_first_run_tour_pending(false);
}

/// Wires every tutorial callback. Called once from `setup_guide`.
pub(super) fn setup(ui: &MainWindow, state: &Rc<RefCell<EditorState>>) {
    runtime::install_state(state);
    setup_guide_callbacks(ui);
    setup_browser_callbacks(ui);
    setup_welcome_callbacks(ui);
}

/// The running guide's callbacks.
fn setup_guide_callbacks(ui: &MainWindow) {
    let guide = ui.global::<GuideModel>();

    let ui_weak = ui.as_weak();
    guide.on_start_guide(move |id| {
        if let Some(ui) = ui_weak.upgrade() {
            launch::start_guide(&ui, id.as_str());
        }
    });

    // Every time a step is entered going forward its goal is checked straight away against
    // the current design, so a goal that is already met (auto-solve finished before the user
    // got there, say) completes without waiting for another edit. Events belong to the step
    // they happened in, so the new step starts without any. The compare step of a "Build this
    // design" lesson also holds the original design as the snapshot Compare measures against.
    let ui_weak = ui.as_weak();
    guide.on_step_entered(move || {
        if let Some(ui) = ui_weak.upgrade() {
            runtime::clear_events();
            build_design::step_entered(&ui);
            progress::check_now(&ui);
        }
    });

    let ui_weak = ui.as_weak();
    guide.on_event(move |name| {
        if let Some(ui) = ui_weak.upgrade() {
            progress::guide_event(&ui, name.as_str());
        }
    });

    let ui_weak = ui.as_weak();
    guide.on_poll(move || {
        if let Some(ui) = ui_weak.upgrade() {
            progress::check_now(&ui);
        }
    });

    // Next on an unfinished step does the step: the recipe runs, then the goal is judged as
    // for any other route to it. While a recipe that started a solve has no result yet, the
    // overlay layer's timer asks for ticks.
    let ui_weak = ui.as_weak();
    guide.on_perform(move |index| {
        if let Some(ui) = ui_weak.upgrade() {
            perform::start(&ui, index);
        }
    });

    let ui_weak = ui.as_weak();
    guide.on_perform_tick(move || {
        if let Some(ui) = ui_weak.upgrade() {
            perform::tick(&ui);
        }
    });

    let ui_weak = ui.as_weak();
    guide.on_launch_tick(move || {
        if let Some(ui) = ui_weak.upgrade() {
            launch::evaluate_launch(&ui);
        }
    });

    let ui_weak = ui.as_weak();
    guide.on_finished(move |id| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        record_finished(&ui, id.as_str());
        let tutorials = ui.global::<TutorialsModel>();
        if tutorials.get_browser_open() {
            fill_browser(&ui, tutorials.get_query().as_str());
        }
    });
}

/// The tutorial browser's callbacks.
fn setup_browser_callbacks(ui: &MainWindow) {
    let tutorials = ui.global::<TutorialsModel>();

    let ui_weak = ui.as_weak();
    tutorials.on_opened(move || {
        if let Some(ui) = ui_weak.upgrade() {
            fill_browser(&ui, ui.global::<TutorialsModel>().get_query().as_str());
            progress::guide_event(&ui, TUTORIALS_OPENED);
        }
    });

    let ui_weak = ui.as_weak();
    tutorials.on_query_changed(move |query| {
        if let Some(ui) = ui_weak.upgrade() {
            fill_browser(&ui, query.as_str());
        }
    });

    let ui_weak = ui.as_weak();
    tutorials.on_refresh(move || {
        if let Some(ui) = ui_weak.upgrade() {
            fill_browser(&ui, ui.global::<TutorialsModel>().get_query().as_str());
        }
    });

    // The browser closes first, so a question the start asks (unsaved changes) is the only
    // thing on screen.
    let ui_weak = ui.as_weak();
    tutorials.on_start(move |id| {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<TutorialsModel>().set_browser_open(false);
            launch::start_guide(&ui, id.as_str());
        }
    });
}

/// The welcome dialog's three answers.
fn setup_welcome_callbacks(ui: &MainWindow) {
    let tutorials = ui.global::<TutorialsModel>();

    let ui_weak = ui.as_weak();
    tutorials.on_tour_start(move || {
        if let Some(ui) = ui_weak.upgrade() {
            welcome_seen(&ui);
            launch::start_guide(&ui, WELCOME_TOUR_ID);
        }
    });

    let ui_weak = ui.as_weak();
    tutorials.on_tour_templates(move || {
        if let Some(ui) = ui_weak.upgrade() {
            welcome_seen(&ui);
            ui.global::<EditorModel>().set_new_dialog_open(true);
        }
    });

    let ui_weak = ui.as_weak();
    tutorials.on_tour_dismiss(move || {
        if let Some(ui) = ui_weak.upgrade() {
            welcome_seen(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::guide::{
        Goal, Guide, GuideCategory, GuideStep, NEEDS_A_DESIGN, StartingState, WORKED_EXAMPLE_ID,
    };

    fn needs_a_design() -> Guide {
        Guide::new(
            "slice-a-facet",
            "Slicing a facet",
            "Cut a facet with the Slice tool.",
            GuideCategory::Tiers,
        )
        .starting(StartingState::RequiresOpenDesign)
        .step(
            GuideStep::new("Add the girdle", "Build the first tier.")
                .actions(["Click + Add Tier."])
                .check("a G1 row in the tier table.")
                .goal(Goal::tier("G1", 90.0), "tier G1 at 90.0")
                .highlight("tier_table"),
        )
    }

    fn catalog_with_lesson() -> GuideCatalog {
        let mut catalog = GuideCatalog::new();
        catalog
            .add_generated(needs_a_design())
            .expect("a fit lesson");
        catalog
    }

    #[test]
    fn finishing_a_guide_records_its_id_once() {
        let mut settings = AppSettings::default();
        apply_finished(&mut settings, WORKED_EXAMPLE_ID);
        apply_finished(&mut settings, WORKED_EXAMPLE_ID);
        assert_eq!(settings.tutorials_completed.len(), 1);
        assert!(settings.tutorials_completed.contains(WORKED_EXAMPLE_ID));
    }

    #[test]
    fn finishing_an_ordinary_guide_leaves_the_welcome_pending() {
        let mut settings = AppSettings::default();
        assert!(!settings.first_run_tour_done);
        apply_finished(&mut settings, WORKED_EXAMPLE_ID);
        assert!(!settings.first_run_tour_done);
    }

    #[test]
    fn finishing_the_welcome_tour_counts_as_having_seen_the_welcome() {
        let mut settings = AppSettings::default();
        apply_finished(&mut settings, WELCOME_TOUR_ID);
        assert!(settings.first_run_tour_done);
        assert!(settings.tutorials_completed.contains(WELCOME_TOUR_ID));
    }

    #[test]
    fn resetting_progress_makes_every_row_startable_again() {
        let catalog = GuideCatalog::new();
        let mut settings = AppSettings::default();
        apply_finished(&mut settings, WORKED_EXAMPLE_ID);
        let (rows, _) = listing(&catalog, &settings.tutorials_completed, true, "");
        assert!(
            rows.iter()
                .any(|row| row.done && row.start_label == "Restart")
        );

        settings.reset_tutorials();
        let (rows, _) = listing(&catalog, &settings.tutorials_completed, true, "");
        assert!(
            rows.iter()
                .all(|row| !row.done && row.start_label == "Start")
        );
    }

    #[test]
    fn the_list_marks_finished_tutorials_and_counts_them() {
        let catalog = GuideCatalog::new();
        let total = catalog.all().len();
        let mut completed = BTreeSet::new();
        let (_, line) = listing(&catalog, &completed, true, "");
        assert_eq!(
            line,
            format!("No tutorials finished yet ({total} available).")
        );

        completed.insert(WORKED_EXAMPLE_ID.to_owned());
        let (rows, line) = listing(&catalog, &completed, true, "");
        assert_eq!(line, format!("1 of {total} tutorials finished."));
        let finished: Vec<&str> = rows
            .iter()
            .filter(|row| row.done)
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(finished, [WORKED_EXAMPLE_ID]);
    }

    #[test]
    fn a_search_narrows_the_list_but_not_the_count_under_it() {
        let catalog = GuideCatalog::new();
        let total = catalog.all().len();
        let (rows, line) = listing(&catalog, &BTreeSet::new(), true, "walkthrough");
        assert!(rows.len() < total);
        assert!(rows.iter().any(|row| row.id.as_str() == WORKED_EXAMPLE_ID));
        assert!(line.contains(&total.to_string()), "{line}");

        let (rows, _) = listing(
            &catalog,
            &BTreeSet::new(),
            true,
            "no such tutorial anywhere",
        );
        assert_eq!(rows.len(), 0, "no tutorial matches the search");
    }

    #[test]
    fn a_tutorial_that_needs_a_design_says_why_it_cannot_start() {
        let catalog = catalog_with_lesson();
        let (rows, _) = listing(&catalog, &BTreeSet::new(), false, "");
        let lesson = rows
            .iter()
            .find(|row| row.id.as_str() == "slice-a-facet")
            .expect("listed");
        assert_eq!(lesson.blocked_reason.as_str(), NEEDS_A_DESIGN);

        let (rows, _) = listing(&catalog, &BTreeSet::new(), true, "");
        assert!(rows.iter().all(|row| row.blocked_reason.is_empty()));
    }

    #[test]
    fn every_section_heading_sits_on_the_first_row_of_its_section_only() {
        let catalog = catalog_with_lesson();
        let (rows, _) = listing(&catalog, &BTreeSet::new(), true, "");
        let mut last_category = String::new();
        for row in &rows {
            let new_section = row.category.as_str() != last_category;
            assert_eq!(row.first_in_category, new_section, "{}", row.id);
            last_category = row.category.to_string();
        }
    }

    #[test]
    fn a_row_carries_the_texts_the_browser_draws() {
        let catalog = GuideCatalog::new();
        let (rows, _) = listing(&catalog, &BTreeSet::new(), true, "walkthrough");
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == WORKED_EXAMPLE_ID)
            .expect("listed");
        assert_eq!(row.title.as_str(), "New design walkthrough");
        assert_eq!(row.category.as_str(), GuideCategory::GettingStarted.label());
        assert!(row.step_text.ends_with("steps"), "{}", row.step_text);
        assert!(!row.summary.is_empty());
    }
}
