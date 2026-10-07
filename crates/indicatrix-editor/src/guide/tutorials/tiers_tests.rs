//! Plays every tiers tutorial in a real [`EditorSession`].
//!
//! A tutorial is data, so the only way to know a step can be finished is to finish it. Each
//! test here walks one lesson: for every step the learner's edit is made through the same
//! session calls the desktop editor makes (the tier form's parse and save, the inline angle
//! cell, the orbit edits behind the facet chips, the table's duplicate, move and remove, the
//! step ladder, the relations). Before the edit the step's goal must be unmet, so no step
//! can complete by itself, and after it the goal must be met. A reading step has no edit and
//! must be a manual one.
//!
//! The submodules hold the walks, one file for each file of lessons.

mod basics;
mod concave;
mod editing;
mod meets;
mod series;

use super::{TIERS_MULTI_SELECTED, tiers};
use crate::{
    EditorSession,
    guide::{
        EVENTS, Goal, GoalContext, Guide, GuideCategory, StartingState, goal_met, start,
        static_guides,
    },
    loading::{TierFormFields, parse_tier_form, parse_tier_target},
    tier_save::{other_tier_names_excluding, tier_save_edit_with_target},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{ConstraintTier, Design, Edit, EditError};
use std::time::Duration;

/// Every tiers tutorial, in the order the browser lists them. A new lesson must be added here
/// and given a walk in the submodule of its file.
const IDS: [&str; 23] = [
    "tiers-add-a-tier",
    "tiers-edit-a-tier",
    "tiers-index-shorthands",
    "tiers-facet-chips",
    "tiers-quick-add",
    "tiers-inline-angle",
    "tiers-meets-named-facets",
    "tiers-meets-mm-targets",
    "tiers-adopt-imported-meets",
    "tiers-pin-to-mast",
    "tiers-step-series",
    "tiers-linked-series",
    "tiers-mirror-to-other-block",
    "tiers-relations",
    "tiers-arithmetic",
    "tiers-cheater-offset",
    "tiers-tier-notes",
    "tiers-duplicate",
    "tiers-move",
    "tiers-delete",
    "tiers-multi-select",
    "tiers-concave-cylinder-cone",
    "tiers-concave-sphere-disc",
];

/// The editor as a learner has it: the session, the UI events reported during the current
/// step, and a clock that moves on with every nudge so a run of nudges coalesces.
struct Sim {
    session: EditorSession,
    events: Vec<String>,
    clock: Duration,
}

/// The tier form's fields, as typed.
#[derive(Clone, Debug)]
struct Form {
    angle: String,
    /// The Meets drop-down: 0 unspecified, 1 named, 2 exact scale, 3 depth (mm), 4 girdle
    /// thickness (mm), 5 table width (mm).
    kind: i32,
    text: String,
    name: String,
    indices: String,
}

impl Form {
    /// A form with these fields typed in.
    fn new(angle: &str, kind: i32, text: &str, name: &str, indices: &str) -> Self {
        Self {
            angle: angle.to_owned(),
            kind,
            text: text.to_owned(),
            name: name.to_owned(),
            indices: indices.to_owned(),
        }
    }

    /// The form as the inspector fills it when a row is picked.
    fn of(tier: &ConstraintTier) -> Self {
        let (kind, text) = match &tier.constraint {
            MeetConstraint::MeetExisting => (0, String::new()),
            MeetConstraint::MeetNamed(names) => (1, names.join(", ")),
            MeetConstraint::ScaleReference(value) => (2, value.to_string()),
        };
        Self {
            angle: tier.angle_deg.to_string(),
            kind,
            text,
            name: tier.name.clone(),
            indices: tier
                .indices
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        }
    }
}

/// What the learner does in one step: nothing for a reading step, otherwise the edit.
enum Move {
    /// A reading step: the learner presses Next.
    Read,
    /// An action step: the learner makes this edit.
    Act(fn(&mut Sim)),
}

/// A reading step: the learner presses Next.
const fn read() -> Move {
    Move::Read
}

/// An action step: the learner makes this edit.
const fn act(play: fn(&mut Sim)) -> Move {
    Move::Act(play)
}

/// The session a lesson that creates its own design starts in: built from the start plan the
/// desktop's guide route follows, so the stone is the one the learner sees.
fn started_session(state: &StartingState) -> EditorSession {
    start::started_session(state).unwrap_or_else(|| panic!("{state:?} creates no design"))
}

impl Sim {
    /// The editor a lesson starts from.
    fn start(state: &StartingState) -> Self {
        let session = match state {
            StartingState::NewEmpty | StartingState::Template(_) => started_session(state),
            // A lesson about an imported design is played over the Rich Teaching Design;
            // its first move makes the design an imported one.
            StartingState::RequiresOpenDesign => started_session(&StartingState::Template(5)),
            other => panic!("the tiers tutorials do not start from {other:?}"),
        };
        Self {
            session,
            events: Vec::new(),
            clock: Duration::ZERO,
        }
    }

    /// Whether `goal` holds in the editor as it is now.
    fn met(&self, goal: &Goal) -> bool {
        let context = GoalContext::new(&self.session.design).events(&self.events);
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

    /// The time of the next nudge: a tenth of a second after the last.
    fn tick(&mut self) -> Duration {
        self.clock += Duration::from_millis(100);
        self.clock
    }

    /// Applies an edit the way the editor does.
    fn apply(&mut self, edit: Edit) {
        self.session.apply(edit).expect("the edit applies");
    }

    /// Applies an edit an orbit or index call has just built.
    fn apply_built(&mut self, built: Result<Edit, EditError>) {
        self.apply(built.expect("the edit builds"));
    }

    /// Ctrl+Z.
    fn undo(&mut self) {
        self.session
            .undo()
            .expect("undo works")
            .expect("there was something to undo");
    }

    /// The tier form's Save Tier on row `index` (or Add Tier for `-1`).
    fn save(&mut self, index: i32, form: &Form) {
        let design = &self.session.design;
        let current = usize::try_from(index)
            .ok()
            .and_then(|at| design.tiers.get(at));
        let tier = parse_tier_form(TierFormFields {
            angle: &form.angle,
            constraint_kind: form.kind,
            constraint_text: &form.text,
            name: &form.name,
            indices: &form.indices,
            gear_teeth_abs: design.meta.gear_teeth_abs(),
            imported_meet: current.and_then(|found| found.imported_meet.clone()),
            original_notes: current.and_then(|found| found.original_notes.clone()),
            other_tier_names: other_tier_names_excluding(design, index),
        })
        .unwrap_or_else(|message| panic!("the tier form is refused: {message}"));
        let target = parse_tier_target(form.kind, &form.text).expect("the target reads");
        let (_, edit) = tier_save_edit_with_target(design, index, tier, target, None);
        self.apply(edit);
    }

    /// The tier form's Add Tier.
    fn add(&mut self, form: &Form) {
        self.save(-1, form);
    }

    /// Picks the row called `name`, changes the form, and clicks Save Tier.
    fn edit_tier(&mut self, name: &str, change: impl FnOnce(&mut Form)) {
        let row = self.row(name);
        let mut form = Form::of(&self.session.design.tiers[row]);
        change(&mut form);
        self.save(i32::try_from(row).expect("a small row"), &form);
    }

    /// An orbit edit of the tier called `name`, as the facet chips make it.
    fn orbit(&mut self, name: &str, build: impl FnOnce(&Design, usize) -> Result<Edit, EditError>) {
        let row = self.row(name);
        let built = build(&self.session.design, row);
        self.apply_built(built);
    }

    /// The inline angle cell: double-click, type `text`, press Enter.
    fn set_angle(&mut self, name: &str, text: &str) {
        let row = self.row(name);
        self.session
            .set_tier_angle_from_text(row, text)
            .expect("the angle reads");
    }

    /// Presses Up (`delta` above zero) or Down in the angle cell of the tier called `name`.
    fn nudge(&mut self, name: &str, delta: f64) {
        let row = self.row(name);
        let now = self.tick();
        self.session
            .nudge_angles(&[row], delta, now)
            .expect("the nudge applies");
    }
}

/// The tutorial with this id.
fn guide_named(id: &str) -> Guide {
    tiers::guides()
        .into_iter()
        .find(|guide| guide.id == id)
        .unwrap_or_else(|| panic!("there is no tiers tutorial called {id:?}"))
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
                    "{id}, step {at} ({:?}) is not met after the learner's edit",
                    step.title
                );
            }
        }
    }
    sim
}

#[test]
fn the_tiers_tutorials_are_registered_in_order() {
    let guides = tiers::guides();
    let ids: Vec<&str> = guides.iter().map(|guide| guide.id.as_str()).collect();
    assert_eq!(ids, IDS, "the lessons, in browser order");
    for guide in &guides {
        assert!(guide.id.starts_with("tiers-"), "{}", guide.id);
        assert!(
            matches!(guide.category, GuideCategory::Tiers),
            "{} is not listed under Tiers",
            guide.id
        );
        assert_eq!(guide.problem(), None, "{}", guide.id);
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
fn the_multi_select_event_is_one_the_catalogue_knows() {
    assert!(EVENTS.contains(&TIERS_MULTI_SELECTED));
    let waiting: Vec<String> = guide_named("tiers-multi-select")
        .steps
        .iter()
        .flat_map(|step| step.goal.events().into_iter().map(str::to_owned))
        .collect();
    assert_eq!(waiting, [TIERS_MULTI_SELECTED]);
}

#[test]
fn a_check_goal_is_judged_by_its_test_and_needs_a_label() {
    let design = EditorSession::fresh().design;
    let context = GoalContext::new(&design);
    let empty = Goal::Check {
        label: "the design has no tiers",
        test: |ctx| ctx.design.tiers.is_empty(),
    };
    let never = Goal::Check {
        label: "never",
        test: |_| false,
    };
    assert!(goal_met(&empty, &context));
    assert!(!goal_met(&never, &context));
    assert!(goal_met(&Goal::All(vec![empty.clone()]), &context));
    assert!(!goal_met(&Goal::All(vec![empty.clone(), never]), &context));
    assert_eq!(empty.problem(), None);
    let unlabelled = Goal::Check {
        label: "  ",
        test: |_| true,
    };
    assert!(unlabelled.problem().is_some());
}
