//! The registry: ids, validity of every built-in guide, the worked example built from the
//! shared `STEPS`, and the catalogue of generated guides.

use super::{
    EIGHT_FOLD_INDICES, EVENTS, Goal, GoalContext, Guide, GuideCatalog, GuideCategory, GuideStep,
    HIGHLIGHT_TARGETS, MANUAL, NEW_DESIGN_CREATED, STEPS, WELCOME_TOUR_ID, WORKED_EXAMPLE_ID,
    goal_from_completion, goal_met, goal_reached, is_valid_guide_id, static_guides,
    tests::{design_with, tier},
    worked_example_guide,
};
use indicatrix_cut_core::Design;

#[test]
fn built_in_guide_ids_are_unique_and_well_formed() {
    let guides = static_guides();
    for (index, guide) in guides.iter().enumerate() {
        assert!(is_valid_guide_id(&guide.id), "{:?}", guide.id);
        assert!(
            guides[..index].iter().all(|other| other.id != guide.id),
            "two built-in guides share the id {:?}",
            guide.id
        );
    }
    assert!(guides.iter().any(|guide| guide.id == WORKED_EXAMPLE_ID));
    assert!(guides.iter().any(|guide| guide.id == WELCOME_TOUR_ID));
}

#[test]
fn every_built_in_guide_is_fit_to_run() {
    for guide in static_guides() {
        assert_eq!(guide.problem(), None, "guide {:?}", guide.id);
    }
}

#[test]
fn the_lists_of_known_names_have_no_duplicates() {
    for list in [HIGHLIGHT_TARGETS, EVENTS] {
        for (index, name) in list.iter().enumerate() {
            assert!(!list[..index].contains(name), "{name:?} is listed twice");
        }
    }
    assert!(
        HIGHLIGHT_TARGETS.contains(&""),
        "no outline is a valid choice"
    );
    assert!(EVENTS.contains(&NEW_DESIGN_CREATED));
}

#[test]
fn guide_ids_are_lower_case_words_joined_by_hyphens() {
    for good in ["welcome-tour", "a", "build-12", "new-design-walkthrough"] {
        assert!(is_valid_guide_id(good), "{good}");
    }
    for bad in ["", "Welcome", "a b", "-a", "a-", "a--b", "a_b", "caf\u{e9}"] {
        assert!(!is_valid_guide_id(bad), "{bad:?}");
    }
}

#[test]
fn the_worked_example_is_the_shared_steps_word_for_word() {
    let guide = worked_example_guide();
    assert_eq!(guide.id, "new-design-walkthrough");
    assert_eq!(guide.category, GuideCategory::GettingStarted);
    assert_eq!(guide.steps.len(), STEPS.len());
    for (built, shared) in guide.steps.iter().zip(STEPS) {
        assert_eq!(built.title, shared.title);
        assert_eq!(built.intro, shared.intro);
        assert_eq!(built.check, shared.check);
        assert_eq!(built.why, shared.why);
        assert_eq!(built.waiting, shared.waiting);
        assert_eq!(built.highlight_target, shared.highlight_target);
        assert_eq!(built.allow, shared.allow);
        assert_eq!(
            built.actions.iter().map(String::as_str).collect::<Vec<_>>(),
            shared.actions
        );
        assert_eq!(built.is_manual(), shared.completion == MANUAL);
    }
}

#[test]
fn every_shared_completion_key_becomes_a_real_goal() {
    for step in STEPS {
        assert!(
            goal_from_completion(step.completion).is_some(),
            "{:?} has the unmapped key {:?}",
            step.title,
            step.completion
        );
    }
    assert!(goal_from_completion("tier_named:X9").is_none());
    assert!(goal_from_completion("no_such_goal").is_none());
}

/// A spread of designs: empty, each tier added in turn, with a material, with a yield.
fn designs() -> Vec<Design> {
    let g1 = tier("G1", 90.0, &EIGHT_FOLD_INDICES);
    let p1 = tier("P1", -40.0, &EIGHT_FOLD_INDICES);
    let c1 = tier("C1", 34.5, &EIGHT_FOLD_INDICES);
    let table = tier("T", 0.0, &[]);
    let mut all = vec![
        design_with(&[]),
        design_with(std::slice::from_ref(&g1)),
        design_with(&[g1.clone(), p1.clone()]),
        design_with(&[g1.clone(), p1.clone(), c1.clone()]),
        design_with(&[g1.clone(), p1.clone(), c1.clone(), table.clone()]),
        design_with(&[tier("G1", 0.0, &EIGHT_FOLD_INDICES)]),
        design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES[..7])]),
        design_with(&[tier("T", 0.0, &[12.0])]),
        design_with(&[tier("t", 0.04, &[0.0])]),
    ];
    let mut diamond = design_with(&[g1, p1, c1, table]);
    diamond.material.name = Some("Diamond".to_owned());
    all.push(diamond.clone());
    diamond.girdle_diameter_mm = Some(6.5);
    all.push(diamond);
    all
}

/// The guide the desktop runs decides exactly what the shared `goal_reached` decides, for
/// every step, design and solve verdict -- so the web app and the desktop never disagree
/// about when a step is done.
#[test]
fn the_worked_example_goals_agree_with_the_shared_predicates() {
    let guide = worked_example_guide();
    for design in designs() {
        for solved_closed in [false, true] {
            let ctx = GoalContext::new(&design).solved_closed(solved_closed);
            for (built, shared) in guide.steps.iter().zip(STEPS) {
                assert_eq!(
                    goal_met(&built.goal, &ctx),
                    goal_reached(shared.completion, &design, solved_closed),
                    "step {:?}, {} tiers, solved {solved_closed}",
                    shared.title,
                    design.tiers.len()
                );
            }
        }
    }
}

#[test]
fn the_new_design_step_waits_for_the_event_and_nothing_else() {
    let guide = worked_example_guide();
    let first = &guide.steps[0];
    let design = design_with(&[]);
    assert!(!goal_met(&first.goal, &GoalContext::new(&design)));
    let seen = [NEW_DESIGN_CREATED.to_owned()];
    assert!(goal_met(
        &first.goal,
        &GoalContext::new(&design).events(&seen)
    ));
}

#[test]
fn a_steps_completion_key_names_it_unless_it_is_a_reading_step() {
    let guide = worked_example_guide();
    let keys: Vec<String> = guide
        .steps
        .iter()
        .enumerate()
        .map(|(index, step)| step.completion_key(index))
        .collect();
    assert_eq!(keys[0], "goal:0");
    assert_eq!(keys[7], MANUAL, "Check the orbits is a reading step");
    assert_eq!(keys[9], MANUAL);
    // Every key is distinct among a guide's own steps, so a report for one step can never
    // complete another.
    let automatic: Vec<&String> = keys.iter().filter(|key| *key != MANUAL).collect();
    for (index, key) in automatic.iter().enumerate() {
        assert!(!automatic[..index].contains(key), "{key} is used twice");
    }
}

#[test]
fn the_worked_example_has_the_two_reading_steps_it_always_had() {
    let guide = worked_example_guide();
    let reading: Vec<&str> = guide
        .steps
        .iter()
        .filter(|step| step.is_manual())
        .map(|step| step.title.as_str())
        .collect();
    assert_eq!(reading, ["Check the orbits", "You have a working design"]);
}

fn small_guide(id: &str) -> Guide {
    Guide::new(
        id,
        "A small lesson",
        "Shows the catalogue a generated guide.",
        GuideCategory::BuildThisDesign,
    )
    .step(
        GuideStep::new("Add the girdle", "Build the first tier.")
            .actions(["Click + Add Tier."])
            .check("a G1 row in the tier table.")
            .goal(Goal::tier("G1", 90.0), "tier G1 at 90.0")
            .highlight("tier_table"),
    )
}

#[test]
fn a_catalogue_starts_with_the_built_in_guides_only() {
    let catalog = GuideCatalog::new();
    assert_eq!(catalog.all().len(), static_guides().len());
    assert!(catalog.get(WORKED_EXAMPLE_ID).is_some());
    assert!(catalog.get("no-such-guide").is_none());
    assert!(!catalog.is_generated(WORKED_EXAMPLE_ID));
}

#[test]
fn a_generated_guide_joins_the_catalogue_and_a_second_one_with_its_id_replaces_it() {
    let mut catalog = GuideCatalog::new();
    let built_in = catalog.all().len();
    catalog
        .add_generated(small_guide("build-a"))
        .expect("fit to run");
    assert_eq!(catalog.all().len(), built_in + 1);
    assert!(catalog.is_generated("build-a"));

    let mut renamed = small_guide("build-a");
    renamed.title = "A rebuilt lesson".to_owned();
    catalog.add_generated(renamed).expect("fit to run");
    assert_eq!(
        catalog.all().len(),
        built_in + 1,
        "same id: replaced, not added"
    );
    assert_eq!(
        catalog.get("build-a").map(|g| g.title.as_str()),
        Some("A rebuilt lesson")
    );

    catalog
        .add_generated(small_guide("build-b"))
        .expect("fit to run");
    assert_eq!(catalog.all().len(), built_in + 2);
    // Built-in guides stay first.
    assert_eq!(catalog.all()[0].id, static_guides()[0].id);
}

#[test]
fn a_generated_guide_may_not_take_a_built_in_id() {
    let mut catalog = GuideCatalog::new();
    let before = catalog.all().len();
    let error = catalog
        .add_generated(small_guide(WORKED_EXAMPLE_ID))
        .expect_err("built-in id");
    assert!(error.contains(WORKED_EXAMPLE_ID), "{error}");
    assert_eq!(catalog.all().len(), before);
}

#[test]
fn a_malformed_generated_guide_is_refused_with_the_reason() {
    let mut catalog = GuideCatalog::new();
    let before = catalog.all().len();

    assert!(catalog.add_generated(small_guide("Not Valid")).is_err());

    let mut no_steps = small_guide("build-c");
    no_steps.steps.clear();
    assert!(catalog.add_generated(no_steps).is_err());

    let typo = Guide::new("build-d", "Lesson", "Has a typo.", GuideCategory::Tiers).step(
        GuideStep::new("Step", "Intro.")
            .actions(["Do it."])
            .check("it.")
            .goal(Goal::Event("typo_event".into()), "the typo")
            .highlight("tier_tabel"),
    );
    let error = catalog.add_generated(typo).expect_err("typo'd highlight");
    assert!(error.contains("tier_tabel"), "{error}");

    let silent = Guide::new(
        "build-e",
        "Lesson",
        "Never says what it waits for.",
        GuideCategory::Tiers,
    )
    .step(GuideStep::new("Step", "Intro.").goal(Goal::SolvedClosed, ""));
    assert!(catalog.add_generated(silent).is_err());

    assert_eq!(catalog.all().len(), before, "nothing malformed got in");
}

#[test]
fn a_guide_whose_steps_share_a_title_is_refused() {
    let twice = small_guide("build-f").step(
        GuideStep::new("Add the girdle", "Again.")
            .actions(["Click + Add Tier."])
            .check("another row.")
            .goal(Goal::tier("G2", 90.0), "tier G2"),
    );
    assert!(
        twice
            .problem()
            .is_some_and(|p| p.contains("Add the girdle"))
    );
}
