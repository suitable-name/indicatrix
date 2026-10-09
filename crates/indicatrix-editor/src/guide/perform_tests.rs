//! Plays the recipes Next performs in a real [`EditorSession`].
//!
//! A recipe is data, so the only way to know Next really finishes a step is to carry it out. The
//! `Sim` here does what the desktop does for each [`Perform`]: the Tier form's parse and save
//! (with the side rule of a new pavilion tier), the concave form, the Steps panel, the mirror, the
//! relation and cheater edits, the palette commands that change the design, and, for what leaves
//! no trace in the design (the Solve press, a panel opening), the UI event the desktop reports.
//! After a recipe, the step's goal must hold.
//!
//! The walks cover the worked example, a "Build this design" lesson (plain and with a concave
//! tier) and every static tutorial. A walk stops where a step has no recipe, or where a recipe is
//! a pure UI action this simulation cannot play; the steps that only skip are printed by
//! `the_steps_that_only_skip_are_listed` (run it with `--nocapture`).

use super::{
    CMD_ADOPT_ALL, CMD_COMPARE, CMD_DELETE, CMD_DUPLICATE, CMD_MOVE_DOWN, CMD_MOVE_UP, CMD_SOLVE,
    CMD_UNDO, COMPARE_STEP_TITLE, Goal, GoalContext, Guide, MeetKind, NEW_DESIGN_CREATED,
    PERFORM_COMMANDS, Perform, StartingState, TierEntry, WORKED_EXAMPLE_ID, build_this_design_plan,
    concave_position, default_perform, goal_met,
    perform_table::{self, Action},
    resolve_tier,
    solving_events::{SNAPSHOT_TAKEN, SOLVE_REQUESTED},
    start, static_guides, tier_position,
    viewing_events::{
        HELP_OPENED, LIVE_RENDER_OPENED, SHORTCUTS_OPENED, SLICE_STARTED, SNAP_TOGGLED,
    },
};
use crate::{
    EditorSession,
    loading::{TierFormFields, eval_number, parse_tier_form, parse_tier_target},
    retarget::validity::analyze,
    tier_save::{concave_tier_save_edit, other_tier_names_excluding, tier_save_edit_with_target},
};
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, MaterialSelection, PreformSpec, ScheduleMeta,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
    name_indicates_pavilion,
};
use std::collections::BTreeSet;

/// The library key (a UUID) the fixture lessons are made for.
const KEY: &str = "5f0e7a52-1c3d-4c1e-9a55-0d6f3c2b7e11";

/// The title the fixture lessons are made for.
const TITLE: &str = "Standard Round Brilliant";

/// The editor as a learner has it: the session, the UI events reported during the current step,
/// whether the last solve closed the stone, the row picked in the tier table, the view mode and
/// the inspector tab.
struct Sim {
    session: EditorSession,
    events: Vec<String>,
    solved: bool,
    selected: Option<String>,
    view_mode: Option<i32>,
    tab: Option<i32>,
}

/// Where a walk of a guide ended.
struct Walked {
    /// How many steps had their recipe played and their goal checked.
    verified: usize,
    /// Why the walk stopped before the last step, `None` when it reached it.
    stopped: Option<String>,
}

fn debug(error: impl std::fmt::Debug) -> String {
    format!("{error:?}")
}

impl Sim {
    /// The editor a guide starts from, `None` for a guide that needs a library design.
    fn start(state: &StartingState) -> Option<Self> {
        let session = match state {
            StartingState::NewEmpty | StartingState::Template(_) => start::started_session(state)?,
            // A guide about the open design is played over the Rich Teaching Design.
            StartingState::CurrentDesign | StartingState::RequiresOpenDesign => {
                start::started_session(&StartingState::Template(5))?
            }
            StartingState::LibraryDesign(_) => return None,
        };
        Some(Self {
            session,
            events: Vec::new(),
            solved: false,
            selected: None,
            view_mode: None,
            tab: None,
        })
    }

    /// Whether `goal` holds in the editor as it is now.
    fn met(&self, goal: &Goal) -> bool {
        let mut context = GoalContext::new(&self.session.design)
            .solved_closed(self.solved)
            .events(&self.events);
        if let Some(mode) = self.view_mode {
            context = context.view_mode(mode);
        }
        if let Some(tab) = self.tab {
            context = context.inspector_tab(tab);
        }
        goal_met(goal, &context)
    }

    /// The row of the tier called `name`.
    fn row(&self, name: &str) -> Result<usize, String> {
        tier_position(&self.session.design, name)
            .ok_or_else(|| format!("there is no tier called {name:?}"))
    }

    /// The row picked in the tier table.
    fn picked(&self) -> Result<usize, String> {
        let name = self
            .selected
            .as_deref()
            .ok_or_else(|| "no row is picked".to_owned())?;
        self.row(name)
    }

    /// Applies an edit the way the editor does. An edit marks the design not solved.
    fn apply(&mut self, edit: Edit) -> Result<(), String> {
        self.session.apply(edit).map_err(debug)?;
        self.solved = false;
        Ok(())
    }

    /// The Tier form's Add Tier or Save Tier, with the side rule the desktop's save applies.
    fn tier(&mut self, entry: &TierEntry) -> Result<(), String> {
        let save = resolve_tier(&self.session.design, entry)?;
        if save.angle.trim_start().starts_with('=') {
            let row = usize::try_from(save.index)
                .map_err(|_| "a relation needs an existing tier".to_owned())?;
            self.session
                .set_tier_relation(row, &save.angle)
                .map_err(debug)?;
            self.solved = false;
            return Ok(());
        }
        let design = &self.session.design;
        let current = usize::try_from(save.index)
            .ok()
            .and_then(|at| design.tiers.get(at));
        let mut tier = parse_tier_form(TierFormFields {
            angle: &save.angle,
            constraint_kind: save.constraint_kind,
            constraint_text: &save.constraint_text,
            name: &save.name,
            indices: &save.indices,
            gear_teeth_abs: design.meta.gear_teeth_abs(),
            imported_meet: current.and_then(|found| found.imported_meet.clone()),
            original_notes: current.and_then(|found| found.original_notes.clone()),
            other_tier_names: other_tier_names_excluding(design, save.index),
        })?;
        if let Some(found) = current {
            tier.detached.clone_from(&found.detached);
            if found.angle_deg.is_sign_negative() {
                tier.angle_deg = -tier.angle_deg.abs();
            }
        } else if name_indicates_pavilion(&tier.name) {
            tier.angle_deg = -tier.angle_deg.abs();
        }
        let target = parse_tier_target(save.constraint_kind, &save.constraint_text)?;
        let (_, edit) = tier_save_edit_with_target(design, save.index, tier, target, None);
        self.apply(edit)
    }

    /// A palette command. `Ok(false)` for one this simulation cannot play.
    fn command(&mut self, id: &str) -> Result<bool, String> {
        match id {
            CMD_SOLVE => {
                self.solved = analyze(&self.session.design, false).is_ok();
                self.events.push(SOLVE_REQUESTED.to_owned());
            }
            CMD_UNDO => {
                self.session
                    .undo()
                    .map_err(debug)?
                    .ok_or("there is nothing to undo")?;
                self.solved = false;
            }
            CMD_DUPLICATE => {
                let row = self.picked()?;
                self.session
                    .duplicate_tier(row)
                    .map_err(debug)?
                    .ok_or("the tier does not exist")?;
                self.solved = false;
            }
            CMD_DELETE => {
                let row = self.picked()?;
                self.session.remove_tier(row).map_err(debug)?;
                self.selected = None;
                self.solved = false;
            }
            CMD_MOVE_UP | CMD_MOVE_DOWN => {
                let row = self.picked()?;
                let direction = if id == CMD_MOVE_UP { -1 } else { 1 };
                // A tier at the end of the table stays where it is.
                self.session.move_tier(row, direction).map_err(debug)?;
                self.solved = false;
            }
            other => {
                let Some(event) = event_of(other) else {
                    return Ok(false);
                };
                self.events.push(event.to_owned());
            }
        }
        Ok(true)
    }

    /// Carries out `perform`. `Ok(false)` when part of it is a UI action this simulation cannot
    /// play.
    fn play(&mut self, perform: &Perform) -> Result<bool, String> {
        match perform {
            Perform::NewDesign {
                template_index,
                spec,
            } => {
                self.session = EditorSession::from_template((**spec).clone(), *template_index);
                self.solved = false;
                self.selected = None;
                self.events.push(NEW_DESIGN_CREATED.to_owned());
            }
            Perform::Tier(entry) => self.tier(entry)?,
            Perform::ConcaveTier { edit, tier } => {
                let at = match edit {
                    Some(name) => Some(
                        concave_position(&self.session.design, name)
                            .ok_or_else(|| format!("there is no concave tier {name:?}"))?,
                    ),
                    None => None,
                };
                let edit = concave_tier_save_edit(&self.session.design, at, (**tier).clone());
                self.apply(edit)?;
            }
            Perform::SelectTier(name) => {
                self.row(name)?;
                self.selected = Some(name.clone());
            }
            Perform::Steps(series) => {
                let made = if series.linked {
                    self.session.generate_step_series_linked(
                        &series.name,
                        &series.start,
                        &series.step,
                        series.count,
                        &series.indices,
                        &series.anchor,
                    )
                } else {
                    self.session.generate_step_series(
                        &series.name,
                        &series.start,
                        &series.step,
                        series.count,
                        &series.indices,
                        &series.anchor,
                    )
                };
                made.map(|_| ())?;
                self.solved = false;
            }
            Perform::MirrorTier { tier, suffix } => {
                let row = self.row(tier)?;
                self.session
                    .mirror_tier_to_other_block(row, suffix)
                    .map_err(debug)?
                    .ok_or("the tier does not exist")?;
                self.solved = false;
            }
            Perform::ClearRelation(tier) => {
                let row = self.row(tier)?;
                self.session.clear_tier_relation(row).map_err(debug)?;
                self.solved = false;
            }
            Perform::TierNote { tier, text } => {
                let index = self.row(tier)?;
                let note = (!text.trim().is_empty()).then(|| text.clone());
                self.apply(Edit::SetTierNote { index, note })?;
            }
            Perform::CheaterOffset { tier, text } => {
                let index = self.row(tier)?;
                let offset_deg = if text.trim().is_empty() {
                    None
                } else {
                    Some(eval_number(text, None).map_err(|error| error.to_string())?)
                };
                self.apply(Edit::SetCheaterOffset { index, offset_deg })?;
            }
            Perform::Material(name) => {
                let mut material = MaterialSelection::none();
                material.name = Some(name.clone());
                self.apply(Edit::SetMaterial { material })?;
            }
            Perform::Yield { girdle_diameter_mm } => {
                self.apply(Edit::SetGirdleDiameterMm {
                    girdle_diameter_mm: Some(*girdle_diameter_mm),
                })?;
            }
            Perform::ViewMode(mode) => self.view_mode = Some(*mode),
            Perform::InspectorTab(tab) => self.tab = Some(*tab),
            Perform::Command(id) => return self.command(id),
            Perform::Sequence(parts) => {
                let mut played = true;
                for part in parts {
                    played &= self.play(part)?;
                }
                return Ok(played);
            }
        }
        Ok(true)
    }
}

/// The UI event the desktop reports when palette command `id` has run, for a command that
/// changes nothing in the design.
fn event_of(id: &str) -> Option<&'static str> {
    match id {
        "solve.snapshot" => Some(SNAPSHOT_TAKEN),
        "view.live_render" => Some(LIVE_RENDER_OPENED),
        "view.snap" => Some(SNAP_TOGGLED),
        "view.slice" => Some(SLICE_STARTED),
        "help.shortcuts" => Some(SHORTCUTS_OPENED),
        "help.manual" => Some(HELP_OPENED),
        _ => None,
    }
}

/// Plays the guide's steps in order: each unfinished action step gets its recipe, and its goal
/// must hold afterwards.
///
/// # Errors
///
/// A sentence naming the step whose recipe was refused or left its goal unmet.
fn walk(guide: &Guide) -> Result<Walked, String> {
    let stopped = |verified: usize, why: String| {
        Ok(Walked {
            verified,
            stopped: Some(why),
        })
    };
    let Some(mut sim) = Sim::start(&guide.starting_state) else {
        return stopped(0, "it starts from a library design".to_owned());
    };
    let mut verified = 0;
    for step in &guide.steps {
        if step.is_manual() {
            continue;
        }
        sim.events.clear();
        if sim.met(&step.goal) {
            continue;
        }
        if step.goal.wants_solved_masts() {
            return stopped(
                verified,
                format!("{:?} is judged on solved depths", step.title),
            );
        }
        let Some(perform) = &step.perform else {
            return stopped(verified, format!("{:?} has no recipe", step.title));
        };
        match sim.play(perform) {
            Ok(true) => {}
            Ok(false) => {
                return stopped(verified, format!("{:?} is a UI action", step.title));
            }
            Err(message) => {
                return Err(format!(
                    "{}, {:?}: the recipe is refused: {message}",
                    guide.id, step.title
                ));
            }
        }
        if !sim.met(&step.goal) {
            return Err(format!(
                "{}, {:?}: the goal ({}) does not hold after the recipe",
                guide.id, step.title, step.waiting
            ));
        }
        verified += 1;
    }
    Ok(Walked {
        verified,
        stopped: None,
    })
}

/// The built-in guide with this id.
fn guide_named(id: &str) -> Guide {
    static_guides()
        .into_iter()
        .find(|guide| guide.id == id)
        .unwrap_or_else(|| panic!("there is no guide called {id:?}"))
}

/// The standard round brilliant, every tier pinned.
fn rbc() -> Design {
    Design::new(
        PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

/// A concave tier on the pavilion side.
fn groove() -> ConcaveTier {
    ConcaveTier {
        name: "Groove".to_owned(),
        angle_deg: -42.0,
        indices: vec![0.0, 12.0],
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: 0.0,
        displacement: [0.0, 0.15, 0.03],
        diameter_ratio: 0.25,
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

fn build_guide(design: &Design) -> Guide {
    build_this_design_plan(design, None, TITLE, KEY)
        .expect("a lesson for this design")
        .guide
}

#[test]
fn every_recipe_is_fit_to_run() {
    for guide in static_guides() {
        assert_eq!(guide.problem(), None, "{}", guide.id);
        for step in &guide.steps {
            if step.is_manual() {
                assert!(
                    step.perform.is_none(),
                    "{}, {:?} is a reading step",
                    guide.id,
                    step.title
                );
            }
            if let Some(perform) = &step.perform {
                assert_eq!(perform.problem(), None, "{}, {:?}", guide.id, step.title);
            }
        }
    }
}

#[test]
fn the_recipe_table_names_real_steps_once() {
    let guides = static_guides();
    let mut seen = BTreeSet::new();
    for entry in perform_table::entries() {
        assert!(
            seen.insert((entry.guide, entry.step)),
            "{} / {:?} is listed twice",
            entry.guide,
            entry.step
        );
        let guide = guides
            .iter()
            .find(|guide| guide.id == entry.guide)
            .unwrap_or_else(|| {
                panic!(
                    "the table names a guide that does not exist: {}",
                    entry.guide
                )
            });
        let step = guide
            .steps
            .iter()
            .find(|step| step.title == entry.step)
            .unwrap_or_else(|| panic!("{} has no step {:?}", entry.guide, entry.step));
        assert!(
            !step.is_manual(),
            "{} / {:?} is a reading step",
            entry.guide,
            entry.step
        );
        match entry.action {
            Action::Do(perform) => assert_eq!(
                step.perform,
                Some(perform),
                "{} / {:?} did not get its listed recipe",
                entry.guide,
                entry.step
            ),
            Action::SkipOnly => assert_eq!(
                step.perform, None,
                "{} / {:?} is listed as skip-only",
                entry.guide, entry.step
            ),
        }
    }
}

#[test]
fn the_command_list_has_every_named_command() {
    for id in [
        CMD_SOLVE,
        CMD_UNDO,
        CMD_DUPLICATE,
        CMD_DELETE,
        CMD_MOVE_UP,
        CMD_MOVE_DOWN,
        CMD_ADOPT_ALL,
        CMD_COMPARE,
    ] {
        assert!(PERFORM_COMMANDS.contains(&id), "{id}");
    }
    let unique: BTreeSet<&&str> = PERFORM_COMMANDS.iter().collect();
    assert_eq!(
        unique.len(),
        PERFORM_COMMANDS.len(),
        "a command is listed twice"
    );
}

#[test]
fn a_goal_that_says_everything_implies_its_recipe() {
    assert_eq!(
        default_perform(&Goal::Material("Diamond".to_owned())),
        Some(Perform::Material("Diamond".to_owned()))
    );
    assert_eq!(
        default_perform(&Goal::ViewMode(1)),
        Some(Perform::ViewMode(1))
    );
    assert_eq!(
        default_perform(&Goal::InspectorTab(2)),
        Some(Perform::InspectorTab(2))
    );
    assert_eq!(default_perform(&Goal::SolvedClosed), Some(Perform::solve()));
    assert_eq!(
        default_perform(&Goal::Event("solve_requested".to_owned())),
        Some(Perform::solve())
    );
    assert_eq!(
        default_perform(&Goal::tier("Crown Main", 36.0)),
        Some(Perform::Tier(TierEntry::of("Crown Main").angle("36")))
    );
    assert!(matches!(
        default_perform(&Goal::FreshDesign {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
        }),
        Some(Perform::NewDesign { .. })
    ));
}

#[test]
fn a_goal_that_leaves_values_open_implies_no_recipe() {
    assert_eq!(default_perform(&Goal::Manual), None);
    assert_eq!(default_perform(&Goal::YieldApplied), None);
    assert_eq!(default_perform(&Goal::TierExists("Table".to_owned())), None);
    assert_eq!(default_perform(&Goal::ConcaveTiersAtLeast(1)), None);
    assert_eq!(
        default_perform(&Goal::tier("G1", 90.0).with_meet(MeetKind::ExactScale)),
        None
    );
    assert_eq!(
        default_perform(&Goal::tier("P1", -41.0).with_meet(MeetKind::Named)),
        None
    );
    assert_eq!(
        default_perform(&Goal::Event("tier_selected".to_owned())),
        None
    );
}

#[test]
fn a_tier_recipe_is_one_undo_step() {
    let mut sim = Sim::start(&StartingState::NewEmpty).expect("an empty start");
    let before = sim.session.design.tiers.len();
    let added = Perform::add_tier("90", 2, "1", "G1", "0:12:96");
    assert!(sim.play(&added).expect("the tier is added"));
    assert_eq!(sim.session.design.tiers.len(), before + 1);
    assert!(sim.play(&Perform::undo()).expect("the tier is taken back"));
    assert_eq!(sim.session.design.tiers.len(), before);
}

#[test]
fn a_refused_entry_changes_nothing_and_says_why() {
    let mut sim = Sim::start(&StartingState::NewEmpty).expect("an empty start");
    let bad_indices = Perform::add_tier("90", 2, "1", "G1", "0, 12, 500");
    assert!(
        sim.play(&bad_indices).is_err(),
        "an index off the gear is refused"
    );
    assert_eq!(
        sim.session.design.tiers,
        [] as [indicatrix_cut_core::ConstraintTier; 0]
    );
    let missing = Perform::Tier(TierEntry::of("Nothing").angle("41"));
    assert!(sim.play(&missing).is_err(), "there is no such row to edit");
}

#[test]
fn the_worked_example_is_performed_up_to_its_optional_yield_step() {
    let guide = guide_named(WORKED_EXAMPLE_ID);
    let walked = walk(&guide).unwrap_or_else(|message| panic!("{message}"));
    // The new design, four tiers, the material and the solve.
    assert_eq!(walked.verified, 7);
    let stopped = walked.stopped.expect("the optional yield step only skips");
    assert!(stopped.contains("Yield and rendering"), "{stopped}");
}

#[test]
fn a_build_lesson_is_performed_up_to_the_compare() {
    let guide = build_guide(&rbc());
    let walked = walk(&guide).unwrap_or_else(|message| panic!("{message}"));
    let stopped = walked
        .stopped
        .expect("the compare step is judged on depths");
    assert!(stopped.contains(COMPARE_STEP_TITLE), "{stopped}");
    // Every step but the compare and the closing one.
    assert_eq!(walked.verified, guide.steps.len() - 2);
    let compare = guide
        .steps
        .iter()
        .find(|step| step.title == COMPARE_STEP_TITLE)
        .expect("a compare step");
    assert_eq!(compare.perform, Some(Perform::command(CMD_COMPARE)));
}

#[test]
fn a_build_lesson_with_a_concave_tier_is_performed_too() {
    let mut design = rbc();
    design.concave_tiers.push(groove());
    let guide = build_guide(&design);
    assert!(
        guide
            .steps
            .iter()
            .any(|step| matches!(step.perform, Some(Perform::ConcaveTier { .. }))),
        "the concave step has a recipe"
    );
    let walked = walk(&guide).unwrap_or_else(|message| panic!("{message}"));
    assert_eq!(walked.verified, guide.steps.len() - 2);
}

#[test]
fn every_tutorial_is_performed_as_far_as_its_recipes_go() {
    let mut failures = Vec::new();
    for guide in static_guides() {
        if let Err(message) = walk(&guide) {
            failures.push(message);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Prints every step that only skips, by guide id and step title. Run with `--nocapture`.
#[test]
fn the_steps_that_only_skip_are_listed() {
    let mut skipped = 0;
    let mut total = 0;
    for guide in static_guides() {
        for step in &guide.steps {
            if step.is_manual() {
                continue;
            }
            total += 1;
            if step.perform.is_none() {
                skipped += 1;
                println!("skip only: {} / {}", guide.id, step.title);
            }
        }
    }
    println!("{skipped} of {total} action steps only skip");
    assert!(
        skipped < total,
        "most action steps have a recipe: {skipped} of {total} only skip"
    );
}
