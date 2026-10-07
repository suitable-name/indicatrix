//! Plays every viewing, output, library and app tutorial in a real [`EditorSession`].
//!
//! A tutorial is data, so the only way to know a step can be finished is to finish it. Each test
//! here walks one lesson: for every step the learner's action is made the way the program makes
//! it. A tier moved by a drag handle is the session call the handle ends in (`set_tier_angle`,
//! `pin_tier_mast`, `rotate_tier_indices`), a kept slice is an added tier, and what happens only
//! on screen (a facet clicked, a file exported, the glossary opened) is the event the desktop
//! reports. Before the action the step's goal must be unmet, so no step can complete by itself,
//! and after it the goal must be met. A reading step has no action and must be a manual one.
//!
//! The submodules hold the walks, one file for each file of lessons.

mod app;
mod library;
mod output;
mod render;
mod solid;

use super::{viewing, viewing_events};
use crate::{
    EditorSession,
    guide::{
        EVENTS, Goal, GoalContext, Group, Guide, GuideCategory, GuideStep, HIGHLIGHT_TARGETS,
        StartingState, goal_met, start, static_guides,
    },
    loading::{TierFormFields, parse_tier_form},
    templates::template_spec,
    tier_save::other_tier_names_excluding,
};
use indicatrix_cut_core::Edit;
use std::{collections::BTreeSet, time::Duration};

/// Every viewing, output, library and app tutorial, in the order the browser lists them. A new
/// lesson must be added here and given a walk in the submodule of its file.
const IDS: [&str; 29] = [
    "viewing-solid-picking",
    "viewing-drag-handles",
    "viewing-slice",
    "viewing-diagram",
    "viewing-cut-slider",
    "viewing-live-render",
    "viewing-design-lighting",
    "viewing-custom-material",
    "viewing-material-coefficients",
    "output-cutting-mode",
    "output-export-asc",
    "output-export-gcs",
    "output-cutting-sheet",
    "output-diagram-png",
    "output-save",
    "output-open",
    "library-import",
    "library-search",
    "library-filters",
    "library-load-design",
    "library-rough-planner",
    "prefs-interface-mode",
    "prefs-high-contrast",
    "prefs-interface-scale",
    "prefs-larger-handles",
    "app-command-palette",
    "app-keyboard-shortcuts",
    "app-help-viewer",
    "app-glossary",
];

/// The editor as a learner has it: the session, the UI events reported during the current step,
/// the Solid viewport's view mode, and a clock that moves on with every drag.
struct Sim {
    session: EditorSession,
    events: Vec<String>,
    view_mode: Option<i32>,
    clock: Duration,
}

/// What the learner does in one step: nothing for a reading step, otherwise the action.
enum Move {
    /// A reading step: the learner presses Next.
    Read,
    /// An action step that leaves no trace in the design: the learner does it and the desktop
    /// reports this UI event.
    Event(&'static str),
    /// An action step: the learner does this.
    Act(fn(&mut Sim)),
}

/// A reading step: the learner presses Next.
const fn read() -> Move {
    Move::Read
}

/// An action step the desktop reports as the UI event `name`.
const fn event(name: &'static str) -> Move {
    Move::Event(name)
}

/// An action step: the learner does this.
const fn act(play: fn(&mut Sim)) -> Move {
    Move::Act(play)
}

/// A session over teaching template `card`, built from the start plan the desktop's guide route
/// follows, so the stone is the one the learner sees.
fn template_session(card: usize) -> EditorSession {
    let state = StartingState::Template(card);
    start::started_session(&state).unwrap_or_else(|| panic!("{state:?} creates no design"))
}

impl Sim {
    /// The editor a lesson starts from.
    fn start(state: &StartingState) -> Self {
        let session = match state {
            StartingState::Template(card) => template_session(*card),
            // A lesson that works with whatever is open is played over the Rich Teaching Design.
            StartingState::CurrentDesign | StartingState::RequiresOpenDesign => template_session(5),
            other => panic!("the viewing tutorials do not start from {other:?}"),
        };
        Self {
            session,
            events: Vec::new(),
            view_mode: None,
            clock: Duration::ZERO,
        }
    }

    /// Whether `goal` holds in the editor as it is now.
    fn met(&self, goal: &Goal) -> bool {
        let mut context = GoalContext::new(&self.session.design).events(&self.events);
        if let Some(mode) = self.view_mode {
            context = context.view_mode(mode);
        }
        goal_met(goal, &context)
    }

    /// The desktop reports UI event `name`.
    fn fire(&mut self, name: &str) {
        self.events.push(name.to_owned());
    }

    /// The Solid viewport is in view mode `mode`.
    fn show(&mut self, mode: i32) {
        self.view_mode = Some(mode);
    }

    /// The row of the tier called `name`.
    fn row(&self, name: &str) -> usize {
        self.session
            .design
            .tiers
            .iter()
            .position(|tier| tier.names().iter().any(|known| known.trim() == name))
            .unwrap_or_else(|| panic!("no tier is called {name:?}"))
    }

    /// The time of the next drag step: a tenth of a second after the last.
    fn tick(&mut self) -> Duration {
        self.clock += Duration::from_millis(100);
        self.clock
    }

    /// Drags the angle handle of the tier called `name` to `degrees`.
    fn drag_angle(&mut self, name: &str, degrees: f64) {
        let row = self.row(name);
        let now = self.tick();
        self.session
            .set_tier_angle(row, degrees, now)
            .expect("the drag applies")
            .expect("the angle changed");
    }

    /// Drags the depth handle of the tier called `name` to `mast`.
    fn drag_depth(&mut self, name: &str, mast: f64) {
        let row = self.row(name);
        let now = self.tick();
        self.session
            .pin_tier_mast(row, mast, now)
            .expect("the drag applies")
            .expect("the mast changed");
    }

    /// Drags the index handle of the tier called `name` round the wheel by `teeth`.
    fn turn(&mut self, name: &str, teeth: i64) {
        let row = self.row(name);
        let now = self.tick();
        self.session
            .rotate_tier_indices(row, teeth, now)
            .expect("the drag applies")
            .expect("the tier turned");
    }

    /// Keeps a sliced facet: adds a tier called `name` meeting the Girdle at `degrees`.
    fn keep_slice(&mut self, name: &str, degrees: f64) {
        let design = &self.session.design;
        let tier = parse_tier_form(TierFormFields {
            angle: &degrees.to_string(),
            constraint_kind: 1,
            constraint_text: "Girdle",
            name,
            indices: "0, 12, 24, 36, 48, 60, 72, 84",
            gear_teeth_abs: design.meta.gear_teeth_abs(),
            imported_meet: None,
            original_notes: None,
            other_tier_names: other_tier_names_excluding(design, -1),
        })
        .unwrap_or_else(|message| panic!("the tier form is refused: {message}"));
        let index = design.tiers.len();
        self.session
            .apply(Edit::AddTier { index, tier })
            .expect("the tier is added");
    }

    /// Ctrl+Z.
    fn undo(&mut self) {
        self.session
            .undo()
            .expect("undo works")
            .expect("there was something to undo");
    }
}

/// The tutorial with this id.
fn guide_named(id: &str) -> Guide {
    viewing::guides()
        .into_iter()
        .find(|guide| guide.id == id)
        .unwrap_or_else(|| panic!("there is no viewing tutorial called {id:?}"))
}

/// Plays the tutorial `id`: one move for each of its steps. Returns the editor as the lesson
/// leaves it, for a test that wants to look at the result.
fn walk(id: &str, moves: Vec<Move>) -> Sim {
    let guide = guide_named(id);
    assert_eq!(guide.problem(), None, "{id} is not fit to run");
    assert_eq!(
        moves.len(),
        guide.steps.len(),
        "{id} needs one move for each of its steps"
    );
    let mut sim = Sim::start(&guide.starting_state);
    for (number, (step, play)) in guide.steps.iter().zip(moves).enumerate() {
        let at = number + 1;
        // Events belong to the step they happened in.
        sim.events.clear();
        match play {
            Move::Read => assert!(
                step.is_manual(),
                "{id}, step {at} ({:?}) waits for something, but the walk does nothing",
                step.title
            ),
            Move::Event(name) => play_action(id, at, step, &mut sim, |sim| sim.fire(name)),
            Move::Act(play) => play_action(id, at, step, &mut sim, play),
        }
    }
    sim
}

/// One action step: the goal is unmet, the learner acts, the goal is met.
fn play_action(id: &str, at: usize, step: &GuideStep, sim: &mut Sim, play: impl FnOnce(&mut Sim)) {
    assert!(
        !step.is_manual(),
        "{id}, step {at} ({:?}) is a reading step",
        step.title
    );
    assert!(
        !sim.met(&step.goal),
        "{id}, step {at} ({:?}) is already met before the learner does anything",
        step.title
    );
    play(sim);
    assert!(
        sim.met(&step.goal),
        "{id}, step {at} ({:?}) is not met after the learner's action",
        step.title
    );
}

/// The event names every goal of `guides` waits for.
fn waited_for(guides: &[Guide]) -> BTreeSet<String> {
    guides
        .iter()
        .flat_map(|guide| &guide.steps)
        .flat_map(|step| step.goal.events().into_iter().map(str::to_owned))
        .collect()
}

#[test]
fn the_viewing_tutorials_are_registered_in_order() {
    let guides = viewing::guides();
    let ids: Vec<&str> = guides.iter().map(|guide| guide.id.as_str()).collect();
    assert_eq!(ids, IDS, "the lessons, in browser order");
    let catalogue = static_guides();
    for guide in &guides {
        assert_eq!(guide.problem(), None, "{}", guide.id);
        assert!(
            (3..=8).contains(&guide.steps.len()),
            "{} has {} steps; a lesson teaches one function in three to eight",
            guide.id,
            guide.steps.len()
        );
        assert_eq!(
            catalogue
                .iter()
                .filter(|other| other.id == guide.id)
                .count(),
            1,
            "{} is in the catalogue exactly once",
            guide.id
        );
    }
}

#[test]
fn each_area_is_listed_under_its_own_section() {
    for guide in viewing::guides() {
        let expected = match guide.id.split('-').next() {
            Some("viewing") => GuideCategory::Viewing,
            Some("output") => GuideCategory::Output,
            Some("library") => GuideCategory::Library,
            Some("prefs") => GuideCategory::Preferences,
            Some("app") => GuideCategory::GettingStarted,
            other => panic!("{} starts with an unknown area {other:?}", guide.id),
        };
        assert_eq!(guide.category, expected, "{}", guide.id);
    }
}

#[test]
fn every_event_is_known_to_the_catalogue_and_waited_for_by_some_lesson() {
    for name in viewing_events::ALL {
        assert!(
            EVENTS.contains(name),
            "{name} is missing from catalog::EVENTS"
        );
    }
    let waited = waited_for(&viewing::guides());
    for name in viewing_events::ALL {
        assert!(
            waited.contains(*name),
            "no viewing lesson waits for {name}: the desktop would report it to nobody"
        );
    }
    for name in &waited {
        assert!(
            EVENTS.contains(&name.as_str()),
            "a lesson waits for {name}, which the catalogue does not know"
        );
    }
}

#[test]
fn every_highlight_a_lesson_uses_is_a_known_target() {
    for guide in viewing::guides() {
        for step in &guide.steps {
            assert!(
                HIGHLIGHT_TARGETS.contains(&step.highlight_target.as_str()),
                "{}, {:?}: {:?} is not a known highlight target",
                guide.id,
                step.title,
                step.highlight_target
            );
        }
    }
}

#[test]
fn undo_is_never_locked_in_a_lesson_that_locks_anything() {
    for guide in viewing::guides() {
        for step in &guide.steps {
            assert!(
                step.allow.contains(&Group::History),
                "{}, {:?}: a slip could not be undone",
                guide.id,
                step.title
            );
        }
    }
}

#[test]
fn a_step_that_needs_an_advanced_control_unlocks_the_advanced_group() {
    // Cutting mode, the Gem Cut Studio export, Preferences and the Simple | Advanced pill are
    // advanced controls: Simple mode hides them unless the step unlocks the group.
    let advanced_events = [
        viewing_events::CUTTING_STEP_MARKED,
        viewing_events::GCS_EXPORTED,
        viewing_events::INTERFACE_MODE_CHANGED,
        viewing_events::HIGH_CONTRAST_CHANGED,
        viewing_events::UI_SCALE_CHANGED,
        viewing_events::LARGE_HANDLES_CHANGED,
    ];
    let mut checked = 0;
    for guide in viewing::guides() {
        for step in &guide.steps {
            if step
                .goal
                .events()
                .iter()
                .any(|name| advanced_events.contains(name))
            {
                checked += 1;
                assert!(
                    step.allow.contains(&Group::Advanced),
                    "{}, {:?}: Simple mode would hide the control this step needs",
                    guide.id,
                    step.title
                );
            }
        }
    }
    assert_eq!(checked, 7, "the steps that wait for an advanced control");
}

#[test]
fn the_lessons_that_start_from_a_card_use_the_rich_teaching_design() {
    for guide in viewing::guides() {
        if let StartingState::Template(card) = guide.starting_state {
            assert_eq!(card, 5, "{} starts from card {card}", guide.id);
        }
    }
    let spec = template_spec(5).expect("the template exists");
    assert_eq!(spec.name, "Rich Teaching Design");
}
