//! Plays every solving, optimizing and comparing tutorial in a real [`EditorSession`].
//!
//! A tutorial is data, so the only way to know a step can be finished is to finish it. Each test
//! here walks one lesson: for every step the learner's action is made through the same session
//! calls the desktop editor makes (the inline angle cell, an edit, Undo, a jump through the
//! history, a Fix planned by the verdict, a retarget checked and applied, a sweep angle used, the
//! text of Edit as Text applied), or, for what leaves no trace in the design (the Solve button
//! was pressed, a result list appeared), the UI event the desktop reports. Before the action the
//! step's goal must be unmet, so no step can complete by itself, and after it the goal must be
//! met. A reading step has no action and must be a manual one.
//!
//! Beside the walks, the premises a lesson states about the design are asserted: a girdle with no
//! anchor does not solve and says so, the verdict offers the Fix a lesson presses, a retarget to
//! topaz is valid, a search for sapphire offers an option, a sweep can reach the pavilion.
//!
//! The submodules hold the walks, one file for each file of lessons.

mod compare;
mod optimize;
mod solve;

use super::{solving, solving_events as events};
use crate::{
    EditorSession,
    guide::{
        EVENTS, Goal, GoalContext, Group, Guide, GuideCategory, StartingState, goal_met, start,
        static_guides,
    },
    retarget::validity::analyze,
};
use indicatrix_cut_core::{Design, Edit, MaterialSelection};

/// The session template index of the Standard Round Brilliant.
const STANDARD: usize = 1;

/// The session template index of the Rich Teaching Design.
const RICH: usize = 5;

/// Every solving tutorial, in the order the browser lists them. A new lesson must be added here
/// and given a walk in the submodule of its file.
const IDS: [&str; 16] = [
    "solving-solve",
    "solving-auto-solve",
    "solving-status",
    "solving-verdict",
    "solving-verdict-fixes",
    "solving-preform",
    "solving-yield-carat",
    "optimizing-deep-solve",
    "optimizing-optimize",
    "optimizing-retarget-shift",
    "optimizing-retarget-optimize",
    "optimizing-sweep",
    "solving-history",
    "solving-variants",
    "solving-snapshot-compare",
    "solving-raw-text",
];

/// How many of [`IDS`] are listed under Solving; the rest are listed under Optimizing.
const SOLVING_COUNT: usize = 7;

/// The editor as a learner has it: the session, the UI events reported during the current step,
/// whether the last solve closed the stone, the inspector tab, and the variants kept so far.
struct Sim {
    session: EditorSession,
    events: Vec<String>,
    solved: bool,
    tab: Option<i32>,
    variants: Vec<Design>,
    /// The Design Settings panel's Material list holds a pick that Apply Material has not
    /// written to the design yet.
    pending_material: Option<String>,
}

/// What the learner does in one step: nothing for a reading step, otherwise the action.
enum Move {
    /// A reading step: the learner presses Next.
    Read,
    /// An action step: the learner does this.
    Act(fn(&mut Sim)),
}

/// A reading step: the learner presses Next.
const fn read() -> Move {
    Move::Read
}

/// An action step: the learner does this.
const fn act(play: fn(&mut Sim)) -> Move {
    Move::Act(play)
}

/// A material choice by name, as the Design Settings panel makes it.
fn material(name: &str) -> MaterialSelection {
    MaterialSelection {
        name: Some(name.to_owned()),
        specific_gravity_override: None,
        refractive_index_override: None,
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
    }
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
            // The Deep Solve lesson only reads: any open design will do.
            StartingState::RequiresOpenDesign => template_session(RICH),
            other => panic!("the solving tutorials do not start from {other:?}"),
        };
        Self {
            session,
            events: Vec::new(),
            solved: false,
            tab: None,
            variants: Vec::new(),
            pending_material: None,
        }
    }

    /// Whether `goal` holds in the editor as it is now.
    fn met(&self, goal: &Goal) -> bool {
        let mut context = GoalContext::new(&self.session.design)
            .solved_closed(self.solved)
            .events(&self.events);
        if let Some(tab) = self.tab {
            context = context.inspector_tab(tab);
        }
        goal_met(goal, &context)
    }

    /// The row of the tier called `name`.
    fn row(&self, name: &str) -> usize {
        self.session
            .design
            .tiers
            .iter()
            .position(|tier| {
                tier.names()
                    .iter()
                    .any(|known| known.trim().eq_ignore_ascii_case(name))
            })
            .unwrap_or_else(|| panic!("no tier is called {name:?}"))
    }

    /// Applies an edit the way the editor does. An edit marks the design not solved.
    fn apply(&mut self, edit: Edit) {
        self.session.apply(edit).expect("the edit applies");
        self.solved = false;
    }

    /// The inline angle cell: double-click, type `text`, press Enter.
    fn set_angle(&mut self, name: &str, text: &str) {
        let row = self.row(name);
        self.session
            .set_tier_angle_from_text(row, text)
            .expect("the angle reads");
        self.solved = false;
    }

    /// Picks `name` in the Design Settings panel's Material list. The panel only marks the pick
    /// pending (the button reads "Apply Material *"); the design keeps its material until
    /// [`Self::apply_material`].
    fn pick_material(&mut self, name: &str) {
        self.pending_material = Some(name.to_owned());
    }

    /// The Apply Material button: writes the pending pick to the design.
    fn apply_material(&mut self) {
        let name = self
            .pending_material
            .take()
            .expect("a material was picked before Apply Material");
        self.apply(Edit::SetMaterial {
            material: material(&name),
        });
    }

    /// What the lessons tell the learner to do: pick `name` in the Material list, then click
    /// Apply Material.
    fn set_material(&mut self, name: &str) {
        self.pick_material(name);
        self.apply_material();
    }

    /// Ctrl+Z.
    fn undo(&mut self) {
        self.session
            .undo()
            .expect("undo works")
            .expect("there was something to undo");
        self.solved = false;
    }

    /// A click on a row of the History tab: the design goes to `position` (0 is Start).
    fn jump(&mut self, position: usize) {
        self.session.jump_to(position).expect("the jump works");
        self.solved = false;
    }

    /// The UI reports event `name`.
    fn raise(&mut self, name: &str) {
        self.events.push(name.to_owned());
    }

    /// A solve lands: the verdict says whether the stone closes.
    fn settle(&mut self) {
        self.solved = analyze(&self.session.design, false).is_ok();
    }

    /// The Solve button (or F5): the solve runs, then the desktop reports the press.
    fn solve(&mut self) {
        self.settle();
        self.raise(events::SOLVE_REQUESTED);
    }

    /// The inspector shows tab `tab`.
    fn open_tab(&mut self, tab: i32) {
        self.tab = Some(tab);
    }
}

/// Every tutorial of the solving files.
fn all_guides() -> Vec<Guide> {
    solving::guides()
}

/// The tutorial with this id.
fn guide_named(id: &str) -> Guide {
    all_guides()
        .into_iter()
        .find(|guide| guide.id == id)
        .unwrap_or_else(|| panic!("there is no solving tutorial called {id:?}"))
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
        sim.events.clear();
        match play {
            Move::Read => assert!(
                step.is_manual(),
                "{id}, step {at} ({:?}) waits for something, but the walk does nothing",
                step.title
            ),
            Move::Act(play) => {
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
                play(&mut sim);
                assert!(
                    sim.met(&step.goal),
                    "{id}, step {at} ({:?}) is not met after the learner's action",
                    step.title
                );
            }
        }
    }
    sim
}

#[test]
fn the_solving_tutorials_are_registered_in_order() {
    let guides = all_guides();
    let ids: Vec<&str> = guides.iter().map(|guide| guide.id.as_str()).collect();
    assert_eq!(ids, IDS, "the lessons, in browser order");
    for (position, guide) in guides.iter().enumerate() {
        let wanted = if position < SOLVING_COUNT {
            GuideCategory::Solving
        } else {
            GuideCategory::Optimizing
        };
        assert_eq!(
            guide.category, wanted,
            "{} is listed under the wrong heading",
            guide.id
        );
        assert_eq!(guide.problem(), None, "{}", guide.id);
        assert!(!guide.steps.is_empty(), "{} has no steps", guide.id);
    }
    let catalogue = static_guides();
    for id in IDS {
        assert_eq!(
            catalogue.iter().filter(|guide| guide.id == id).count(),
            1,
            "{id} is in the catalogue exactly once"
        );
    }
}

#[test]
fn the_catalogue_knows_every_event_the_lessons_report_and_wait_for() {
    for name in events::ALL {
        assert!(
            EVENTS.contains(name),
            "{name} is missing from catalog::EVENTS"
        );
    }
    let waited_for: Vec<String> = all_guides()
        .iter()
        .flat_map(|guide| guide.steps.iter())
        .flat_map(|step| step.goal.events().into_iter().map(str::to_owned))
        .collect();
    for name in &waited_for {
        assert!(
            events::ALL.contains(&name.as_str()),
            "a lesson waits for {name}, which solving_events does not list"
        );
    }
    for name in events::ALL {
        assert!(
            waited_for.iter().any(|waited| waited == name),
            "no lesson waits for {name}: drop it or use it"
        );
    }
}

#[test]
fn the_lessons_use_only_plain_characters() {
    // Basic Latin and the degree sign (Latin-1) only: no emoji, no arrows, no typographic quotes.
    let plain = |text: &str| text.chars().all(|c| c.is_ascii() || c == '\u{b0}');
    for guide in all_guides() {
        assert!(plain(&guide.title) && plain(&guide.summary), "{}", guide.id);
        for step in &guide.steps {
            let texts = [&step.title, &step.intro, &step.check, &step.why];
            assert!(
                texts.iter().all(|text| plain(text)) && step.actions.iter().all(|a| plain(a)),
                "{}, step {:?} has a character outside plain text",
                guide.id,
                step.title
            );
        }
    }
}

/// The Material list only marks a pick as pending; the design's material is written by Apply
/// Material. So a step that waits for a material must say "click Apply Material", keep the
/// Design Settings panel usable, and stay open after the pick alone.
#[test]
fn a_material_step_waits_for_apply_material_and_says_so() {
    let mut checked = 0;
    for guide in all_guides() {
        for step in &guide.steps {
            let Goal::Material(name) = &step.goal else {
                continue;
            };
            checked += 1;
            let at = format!("{}, {:?}", guide.id, step.title);
            assert!(
                step.actions
                    .iter()
                    .any(|action| action.contains("Click Apply Material")),
                "{at}: the actions never tell the learner to click Apply Material"
            );
            assert!(
                step.check.contains("Apply Material"),
                "{at}: the look-for line does not mention Apply Material: {:?}",
                step.check
            );
            assert!(
                step.allow.contains(&Group::DesignSettings),
                "{at}: the Apply Material button is locked"
            );
            let mut sim = Sim::start(&guide.starting_state);
            assert!(!sim.met(&step.goal), "{at}: already met");
            sim.pick_material(name);
            assert!(
                !sim.met(&step.goal),
                "{at}: the pick alone must not finish the step"
            );
            sim.apply_material();
            assert!(sim.met(&step.goal), "{at}: Apply Material must finish it");
        }
    }
    assert!(
        checked >= 5,
        "the five Set Material steps were not all found"
    );
}

#[test]
fn every_step_of_every_lesson_says_what_it_does() {
    for guide in all_guides() {
        for step in &guide.steps {
            assert!(
                !step.intro.trim().is_empty(),
                "{}: {}",
                guide.id,
                step.title
            );
            if step.is_manual() {
                assert!(
                    !step.actions.is_empty(),
                    "{}: the reading step {:?} has nothing to read",
                    guide.id,
                    step.title
                );
            } else {
                assert!(
                    !step.check.trim().is_empty() || !step.actions.is_empty(),
                    "{}: the action step {:?} says nothing",
                    guide.id,
                    step.title
                );
            }
        }
    }
}
