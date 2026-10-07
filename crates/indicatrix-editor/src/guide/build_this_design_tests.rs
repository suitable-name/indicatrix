//! Tests of the "Build this design" generator: the lesson it writes for a library design,
//! followed step by step in a real [`EditorSession`] the way a learner would.

use super::{
    ALL_GROUPS, BUILD_ID_PREFIX, BuildGuideError, BuildPlan, COMPARE_STEP_TITLE,
    EIGHT_FOLD_INDICES, Goal, GoalContext, Group, Guide, GuideCatalog, GuideCategory, GuideStep,
    LARGE_DESIGN_TIERS, MAX_TIERS_PER_STEP, MeetKind, NEW_DESIGN_CREATED, REBUILD_ANGLE_TOL_DEG,
    SOLVE_STEP_TITLE, START_STEP_TITLE, StartPlan, StartingState, TierRecipe, browser_rows,
    build_guide_id,
    build_text::{
        Role, angle_text, indices_phrase, indices_text, infer_roles, mast_text, number_text,
        plain_number_text, teaching_names,
    },
    build_this_design_guide, build_this_design_plan, goal_met, is_build_guide_id, is_compare_step,
    is_valid_guide_id, original_label, plan_start, record_completion, reference_design,
    static_guides,
    tests::tier,
    worked_example_guide,
};
use crate::{EditorSession, loading::parse_tier_form};
use indicatrix::geometry::meet_solver::{
    MeetConstraint, SolveStrategy, SolvedTier, classify_blocks,
};
use indicatrix_cut_core::{
    ConstraintTier, Design, Edit, FreshDesignSpec, MaterialSelection, PreformSpec, ScheduleMeta,
    design::{ConcaveTier, ConcaveTool, ToolMotion},
};
use std::collections::BTreeSet;

/// The library key (a UUID) the fixtures' lessons are made for.
const KEY: &str = "5f0e7a52-1c3d-4c1e-9a55-0d6f3c2b7e11";

/// The title the fixtures' lessons are made for.
const TITLE: &str = "Standard Round Brilliant";

// ---------------------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------------------

/// A 96-tooth, 8-fold, mirrored design with these tiers and no solve behind it.
fn design_of(tiers: &[ConstraintTier]) -> Design {
    Design::new(
        PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        ScheduleMeta::standard_round_brilliant(),
        tiers.to_vec(),
    )
}

/// The standard round brilliant: eight tiers, each pinned to its depth.
fn rbc() -> Design {
    design_of(&ConstraintTier::standard_round_brilliant())
}

/// The round brilliant as a `.asc` import leaves it: every tier pinned, with what the file's
/// notes said each tier meets kept aside.
fn imported_rbc() -> Design {
    let mut design = rbc();
    for tier in &mut design.tiers {
        tier.imported_meet = match tier.name.as_str() {
            "Lower Girdle" => Some(MeetConstraint::MeetNamed(vec!["Pavilion Main".to_owned()])),
            "Culet" => Some(MeetConstraint::MeetExisting),
            "Crown Main" => Some(MeetConstraint::MeetNamed(vec!["Star".to_owned()])),
            "Upper Girdle" => Some(MeetConstraint::MeetNamed(vec!["Crown Main".to_owned()])),
            _ => None,
        };
    }
    design
}

/// Tier names the tier form cannot take as they are: a repeat, a blank, a comma, a slash.
fn awkward_names() -> Design {
    design_of(&[
        tier("P1", -41.0, &EIGHT_FOLD_INDICES),
        tier("p1", -43.0, &EIGHT_FOLD_INDICES),
        tier("", 34.5, &EIGHT_FOLD_INDICES),
        tier("X,Y", 40.0, &EIGHT_FOLD_INDICES),
        tier("A/B", -45.0, &EIGHT_FOLD_INDICES),
        tier(" T ", 0.0, &[]),
        tier("G1", 90.0, &EIGHT_FOLD_INDICES),
    ])
}

/// A girdle, `pavilion` pavilion tiers, `crown` crown tiers and a table, all pinned.
fn ladder(pavilion: u32, crown: u32) -> Design {
    let mut tiers = vec![tier("G1", 90.0, &EIGHT_FOLD_INDICES)];
    tiers.extend((0..pavilion).map(|k| {
        tier(
            &format!("P{}", k + 1),
            -(30.0 + f64::from(k)),
            &EIGHT_FOLD_INDICES,
        )
    }));
    tiers.extend((0..crown).map(|k| {
        tier(
            &format!("C{}", k + 1),
            10.0 + f64::from(k),
            &EIGHT_FOLD_INDICES,
        )
    }));
    tiers.push(tier("T", 0.0, &[]));
    design_of(&tiers)
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

fn rbc_with_groove() -> Design {
    let mut design = rbc();
    design.concave_tiers.push(groove());
    design
}

fn plan_of(design: &Design) -> BuildPlan {
    build_this_design_plan(design, None, TITLE, KEY).expect("a lesson for this design")
}

fn guide_of(design: &Design) -> Guide {
    plan_of(design).guide
}

fn titles(guide: &Guide) -> Vec<&str> {
    guide.steps.iter().map(|step| step.title.as_str()).collect()
}

fn step_titled<'a>(guide: &'a Guide, title: &str) -> &'a GuideStep {
    guide
        .steps
        .iter()
        .find(|step| step.title == title)
        .unwrap_or_else(|| panic!("no step titled {title:?}"))
}

fn holds(goal: &Goal, design: &Design) -> bool {
    goal_met(goal, &GoalContext::new(design))
}

fn solved_with(masts: &[f64]) -> Vec<SolvedTier> {
    masts
        .iter()
        .map(|&mast| SolvedTier {
            mast,
            strategy: SolveStrategy::ScaleReference,
            detail: String::new(),
        })
        .collect()
}

// ---------------------------------------------------------------------------------------
// Following a lesson in a real editor session
// ---------------------------------------------------------------------------------------

/// The design the New Design dialog makes for `target`'s gear, symmetry, mirror and preform.
fn dialog_design(target: &Design) -> EditorSession {
    EditorSession::from_spec(FreshDesignSpec {
        gear_teeth: target.meta.gear_teeth.abs(),
        symmetry_order: target.meta.symmetry_order,
        mirror: target.meta.mirror,
        material: MaterialSelection::none(),
        preform: target.preform,
    })
}

/// Types `recipe` into the tier form of `session` and adds the tier it makes.
fn type_recipe(session: &mut EditorSession, recipe: &TierRecipe) {
    let gear = session.design.meta.gear_teeth_abs();
    let others: Vec<String> = session
        .design
        .tiers
        .iter()
        .map(|tier| tier.name.clone())
        .collect();
    let tier = parse_tier_form(recipe.fields(gear, others))
        .unwrap_or_else(|error| panic!("the form refused {}: {error}", recipe.name));
    let index = session.design.tiers.len();
    session
        .apply(Edit::AddTier { index, tier })
        .expect("add tier");
}

/// Plays the lesson from its first step: every step waits until its tiers are in, and is
/// done once they are. Returns the finished session.
fn follow_lesson(target: &Design, plan: &BuildPlan) -> EditorSession {
    let steps = &plan.guide.steps;
    let mut session = dialog_design(target);
    let events = vec![NEW_DESIGN_CREATED.to_owned()];
    let start = &steps[0];
    assert!(
        !goal_met(&start.goal, &GoalContext::new(&session.design)),
        "the start step waits for the New Design event"
    );
    assert!(
        goal_met(
            &start.goal,
            &GoalContext::new(&session.design).events(&events)
        ),
        "a design the dialog made for the original's gear completes the start step"
    );
    let mut recipes = plan.recipes.iter();
    for step in &steps[1..steps.len() - 3] {
        assert!(
            !holds(&step.goal, &session.design),
            "{:?} must wait for its tiers",
            step.title
        );
        if matches!(step.goal, Goal::ConcaveTiersAtLeast(_)) {
            let next = target.concave_tiers[session.design.concave_tiers.len()].clone();
            session.design.concave_tiers.push(next);
        } else {
            let mut added = 0;
            while !holds(&step.goal, &session.design) {
                let recipe = recipes
                    .next()
                    .unwrap_or_else(|| panic!("{:?} never completes", step.title));
                type_recipe(&mut session, recipe);
                added += 1;
            }
            assert!(
                added <= MAX_TIERS_PER_STEP,
                "{:?} takes {added} tiers",
                step.title
            );
        }
        assert!(
            holds(&step.goal, &session.design),
            "{:?} is done once its tiers are in",
            step.title
        );
    }
    assert!(recipes.next().is_none(), "every recipe belongs to a step");
    session
}

/// The solved depth of every tier of `design` the lesson's original has under the same name.
fn masts_from_original(guide: &Guide, design: &Design) -> Vec<f64> {
    let original = reference_design(guide).expect("a build lesson holds its original");
    design
        .tiers
        .iter()
        .map(|tier| {
            let at = original
                .design
                .tiers
                .iter()
                .position(|candidate| candidate.name == tier.name)
                .unwrap_or_else(|| panic!("the original has no tier {:?}", tier.name));
            original.masts[at]
        })
        .collect()
}

/// The compare step holds for `design` once it is solved to the original's depths, and not
/// before.
fn assert_compare_met(plan: &BuildPlan, design: &Design) {
    let compare = step_titled(&plan.guide, COMPARE_STEP_TITLE);
    let masts = masts_from_original(&plan.guide, design);
    let ctx = GoalContext::new(design)
        .solved_closed(true)
        .solved_masts(&masts);
    assert!(
        goal_met(&compare.goal, &ctx),
        "a faithful rebuild completes the compare step"
    );
    assert!(
        !holds(&compare.goal, design),
        "the compare step waits for the solve"
    );
}

/// Follows the lesson for `target` through the solve and compare steps.
fn follow_to_the_end(target: &Design) {
    let plan = plan_of(target);
    let session = follow_lesson(target, &plan);
    assert_eq!(session.design.tiers.len(), target.tiers.len());
    let solve = step_titled(&plan.guide, SOLVE_STEP_TITLE);
    assert!(!holds(&solve.goal, &session.design));
    assert!(goal_met(
        &solve.goal,
        &GoalContext::new(&session.design).solved_closed(true)
    ));
    assert_compare_met(&plan, &session.design);
}

#[test]
fn the_round_brilliant_lesson_can_be_followed_to_the_end() {
    follow_to_the_end(&rbc());
}

#[test]
fn an_imported_lesson_can_be_followed_to_the_end() {
    follow_to_the_end(&imported_rbc());
}

#[test]
fn awkward_tier_names_are_replaced_and_the_lesson_still_completes() {
    follow_to_the_end(&awkward_names());
}

#[test]
fn a_large_lesson_can_be_followed_to_the_end() {
    follow_to_the_end(&ladder(22, 22));
}

#[test]
fn a_lesson_with_a_concave_tier_can_be_followed_to_the_end() {
    follow_to_the_end(&rbc_with_groove());
}

#[test]
fn a_target_with_a_negative_gear_is_rebuilt_on_the_positive_one() {
    let mut target = rbc();
    target.meta.gear_teeth = -96;
    follow_to_the_end(&target);
}

#[test]
fn a_target_with_another_gear_and_no_mirror_can_be_followed() {
    let mut target = rbc();
    target.meta.gear_teeth = 100;
    target.meta.mirror = false;
    follow_to_the_end(&target);
}

#[test]
fn a_lesson_for_one_tier_is_five_steps() {
    let target = design_of(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    let plan = plan_of(&target);
    assert_eq!(
        titles(&plan.guide),
        [
            START_STEP_TITLE,
            "Add G1 (girdle)",
            SOLVE_STEP_TITLE,
            COMPARE_STEP_TITLE,
            "You rebuilt Standard Round Brilliant",
        ]
    );
    assert!(plan.guide.summary.contains("1 tier in cutting order"));
    follow_to_the_end(&target);
}

// ---------------------------------------------------------------------------------------
// The round brilliant lesson
// ---------------------------------------------------------------------------------------

#[test]
fn the_round_brilliant_is_taught_in_cutting_order() {
    let plan = plan_of(&rbc());
    assert_eq!(
        titles(&plan.guide),
        [
            "Start a new design",
            "Add G1 Girdle (girdle)",
            "Add P1 Pavilion Main (pavilion main)",
            "Add P2 Lower Girdle (pavilion break)",
            "Add Culet (culet)",
            "Add C1 Star (star)",
            "Add C2 Crown Main (crown main)",
            "Add C3 Upper Girdle (upper break)",
            "Add T Table (table)",
            "Solve and check",
            "Compare with the original",
            "You rebuilt Standard Round Brilliant",
        ]
    );
    assert_eq!(plan.original_positions, [4, 5, 6, 7, 1, 2, 3, 0]);
    assert!(plan.guide.problem().is_none());
}

/// A step is titled with the code the finished sheet prints for the tier, then the tier's own
/// name when it says more: an old-style name (`1`, `A`, `G`) is the code alone.
#[test]
fn a_step_title_leads_with_the_tier_code() {
    let target = design_of(&[
        tier("A", 35.0, &EIGHT_FOLD_INDICES),
        tier("1", -41.0, &EIGHT_FOLD_INDICES),
        tier("G", 90.0, &EIGHT_FOLD_INDICES),
        tier("Table", 0.0, &[]),
    ]);
    let plan = plan_of(&target);
    // Cutting order: the pavilion section in stored order (1, G), then A, then the table.
    assert_eq!(plan.original_positions, [1, 2, 0, 3]);
    let step_titles = titles(&plan.guide);
    assert!(step_titles[1].starts_with("Add P1 ("), "{}", step_titles[1]);
    assert!(step_titles[2].starts_with("Add G1 ("), "{}", step_titles[2]);
    assert!(step_titles[3].starts_with("Add C1 ("), "{}", step_titles[3]);
    assert!(
        step_titles[4].starts_with("Add T Table ("),
        "{}",
        step_titles[4]
    );
    // The learner still types the tier's own name.
    let table = &plan.guide.steps[4];
    assert!(table.actions.contains(&"Name: Table".to_owned()));
    assert_eq!(table.check, "a Table row in the tier table.");
}

#[test]
fn the_lesson_is_a_build_guide_under_the_designs_key() {
    let guide = guide_of(&rbc());
    assert_eq!(guide.id, format!("build:{KEY}"));
    assert_eq!(guide.id, build_guide_id(KEY));
    assert_eq!(guide.title, "Build Standard Round Brilliant");
    assert_eq!(guide.category, GuideCategory::BuildThisDesign);
    assert_eq!(guide.starting_state, StartingState::CurrentDesign);
    assert!(guide.summary.contains("8 tiers in cutting order"));
    assert_eq!(plan_start(&guide.starting_state, false), StartPlan::Begin);
}

#[test]
fn the_recipes_are_the_exact_form_entries_of_each_tier() {
    let recipes = plan_of(&rbc()).recipes;
    let recipe = |name: &str, angle: &str, indices: &str, text: &str| TierRecipe {
        name: name.to_owned(),
        angle: angle.to_owned(),
        indices: indices.to_owned(),
        constraint_kind: 2,
        constraint_text: text.to_owned(),
    };
    assert_eq!(recipes[0], recipe("Girdle", "90.0", "0 x16", "1.0"));
    assert_eq!(recipes[1], recipe("Pavilion Main", "-41.0", "0 x8", "0.67"));
    assert_eq!(
        recipes[2],
        recipe(
            "Lower Girdle",
            "-42.5",
            "95, 1, 11, 13, 23, 25, 35, 37, 47, 49, 59, 61, 71, 73, 83, 85",
            "0.68"
        )
    );
    assert_eq!(recipes[3], recipe("Culet", "-0", "", "0.88"));
    assert_eq!(recipes[4], recipe("Star", "15.0", "6 x8", "0.45"));
    assert_eq!(recipes[5], recipe("Crown Main", "34.5", "0 x8", "0.59"));
    assert_eq!(recipes[7], recipe("Table", "0.0", "", "0.32"));
}

#[test]
fn a_tier_step_lists_what_to_type_and_waits_for_the_tier() {
    let guide = guide_of(&rbc());
    let girdle = step_titled(&guide, "Add G1 Girdle (girdle)");
    assert!(girdle.intro.starts_with("Tier 1 of 8."));
    assert_eq!(girdle.actions[0], "Click + Add Tier.");
    assert_eq!(
        girdle.actions[1],
        "Angle (deg): 90.0 (or click Girdle Facet Preset, which also sets Meets)"
    );
    assert_eq!(girdle.actions[2], "Meets: Exact scale value -- 1.0");
    assert_eq!(girdle.actions[3], "Name: Girdle");
    assert!(
        girdle.actions[4].starts_with("Indices: 0 x16 (that is 0, 6, 12, 18"),
        "{}",
        girdle.actions[4]
    );
    assert_eq!(girdle.actions[5], "Click Add Tier.");
    assert_eq!(girdle.check, "a Girdle row in the tier table.");
    assert_eq!(girdle.waiting, "tier Girdle at 90.0 with 16 indices");
    assert_eq!(girdle.highlight_target, "inspector_tier");
    assert_eq!(
        girdle.allow,
        [
            Group::TierForm,
            Group::TierTable,
            Group::Advanced,
            Group::History
        ],
        "an Exact scale value step unlocks the Advanced controls, which show that Meets entry"
    );
    let Goal::TierMatches {
        name,
        angle_deg,
        tol_deg,
        indices,
        constraint_kind,
    } = &girdle.goal
    else {
        panic!("a tier step waits for its tier");
    };
    assert_eq!(name, "Girdle");
    assert_eq!(*angle_deg, 90.0);
    assert_eq!(*tol_deg, REBUILD_ANGLE_TOL_DEG);
    assert_eq!(indices.as_ref().map(Vec::len), Some(16));
    assert_eq!(*constraint_kind, Some(MeetKind::ExactScale));
}

#[test]
fn the_culet_is_typed_with_a_minus_zero_and_blank_indices() {
    let guide = guide_of(&rbc());
    let culet = step_titled(&guide, "Add Culet (culet)");
    assert!(culet.actions[1].starts_with("Angle (deg): -0 ("));
    assert!(culet.actions[1].contains("minus"));
    assert_eq!(culet.actions[4], "Indices: leave blank");
    assert_eq!(culet.waiting, "tier Culet at -0 with no indices");
    assert!(culet.why.contains("minus zero"));
    let table = step_titled(&guide, "Add T Table (table)");
    assert_eq!(table.actions[1], "Angle (deg): 0.0");
    assert_eq!(table.actions[4], "Indices: leave blank");
}

#[test]
fn the_first_tier_of_each_block_explains_why_it_states_its_depth() {
    let guide = guide_of(&rbc());
    for title in [
        "Add G1 Girdle (girdle)",
        "Add P1 Pavilion Main (pavilion main)",
        "Add C1 Star (star)",
    ] {
        let why = &step_titled(&guide, title).why;
        assert!(why.contains("sets that block's size"), "{title}: {why}");
    }
    let other = &step_titled(&guide, "Add C2 Crown Main (crown main)").why;
    assert!(!other.contains("sets that block's size"));
    assert!(other.contains("does not say which facets this tier meets"));
}

#[test]
fn the_start_step_describes_the_new_design_dialog() {
    let guide = guide_of(&rbc());
    let start = &guide.steps[0];
    assert_eq!(start.title, START_STEP_TITLE);
    assert!(start.intro.contains("in 12 steps"), "{}", start.intro);
    assert_eq!(start.actions[1], "Start From: Empty.");
    assert_eq!(
        start.actions[2],
        "Preform Shape: Cylinder -- Half-Width 1.5, Length / Width 1.0, Depth 1.5."
    );
    assert_eq!(
        start.actions[3],
        "Index Gear: 96. Symmetry Order: 8. Mirror: on."
    );
    assert_eq!(start.highlight_target, "new_design_dialog");
    assert_eq!(start.allow, [Group::NewDesign]);
    assert_eq!(start.goal.events(), [NEW_DESIGN_CREATED]);
}

#[test]
fn the_start_step_names_a_custom_gear_and_the_mirror_setting() {
    let mut target = rbc();
    target.meta.gear_teeth = 100;
    target.meta.mirror = false;
    let guide = guide_of(&target);
    assert_eq!(
        guide.steps[0].actions[3],
        "Index Gear: Custom, then Custom Teeth: 100. Symmetry Order: 8. Mirror: off."
    );
    assert!(guide.steps[0].why.contains("Mirror off"));
}

#[test]
fn the_start_step_wants_a_fresh_design_with_the_originals_gear() {
    let guide = guide_of(&rbc());
    let start = &guide.steps[0];
    let events = vec![NEW_DESIGN_CREATED.to_owned()];
    let fresh = EditorSession::fresh().design;
    assert!(goal_met(
        &start.goal,
        &GoalContext::new(&fresh).events(&events)
    ));
    let mut other_gear = rbc();
    other_gear.meta.gear_teeth = 80;
    other_gear.tiers.clear();
    assert!(!goal_met(
        &start.goal,
        &GoalContext::new(&other_gear).events(&events)
    ));
    // The tiers of the original are not a fresh start.
    assert!(!goal_met(
        &start.goal,
        &GoalContext::new(&rbc()).events(&events)
    ));
}

#[test]
fn the_solve_and_compare_steps_unlock_what_they_need() {
    let guide = guide_of(&rbc());
    let solve = step_titled(&guide, SOLVE_STEP_TITLE);
    assert_eq!(solve.highlight_target, "solve_button");
    assert!(solve.allow.contains(&Group::Solve));
    let compare = step_titled(&guide, COMPARE_STEP_TITLE);
    assert!(compare.allow.contains(&Group::Advanced));
    assert!(compare.allow.contains(&Group::Solve));
    assert!(compare.goal.wants_solved_masts());
    assert!(!solve.goal.wants_solved_masts());
    assert!(
        compare.goal.watches_ui(),
        "the depths arrive after the design check, so the step is polled"
    );
    assert!(!solve.goal.watches_ui());
    let last = guide.steps.last().expect("a closing step");
    assert!(last.is_manual());
    assert_eq!(last.allow.as_slice(), ALL_GROUPS);
}

#[test]
fn the_closing_step_mentions_the_material_only_when_the_original_has_one() {
    let plain = guide_of(&rbc());
    assert_eq!(plain.steps.last().expect("last").actions.len(), 2);
    let mut target = rbc();
    target.material.name = Some("Diamond".to_owned());
    let guide = guide_of(&target);
    let actions = &guide.steps.last().expect("last").actions;
    assert_eq!(actions.len(), 3);
    assert!(actions[2].contains("cut in Diamond"));
}

#[test]
fn a_blank_title_reads_as_this_design() {
    let guide = build_this_design_guide(&rbc(), None, "   ", KEY).expect("a lesson");
    assert_eq!(guide.title, "Build this design");
    assert_eq!(
        guide.steps.last().expect("last").title,
        "You rebuilt this design"
    );
}

// ---------------------------------------------------------------------------------------
// The compare step
// ---------------------------------------------------------------------------------------

#[test]
fn the_compare_step_judges_the_cut_and_the_solved_depths() {
    let target = rbc();
    let plan = plan_of(&target);
    let session = follow_lesson(&target, &plan);
    let compare = step_titled(&plan.guide, COMPARE_STEP_TITLE);
    let solved = session.design.solve().expect("the rebuilt design solves");
    let masts: Vec<f64> = solved.iter().map(|tier| tier.mast).collect();
    let judge = |masts: &[f64]| {
        goal_met(
            &compare.goal,
            &GoalContext::new(&session.design)
                .solved_closed(true)
                .solved_masts(masts),
        )
    };
    assert!(judge(&masts), "the solved rebuild matches");
    let mut near = masts.clone();
    near[0] *= 1.005;
    assert!(judge(&near), "half a percent is within the tolerance");
    let mut far = masts;
    far[0] *= 1.03;
    assert!(!judge(&far), "three percent is not");
    assert!(
        !goal_met(&compare.goal, &GoalContext::new(&session.design)),
        "an unsolved design never matches"
    );
}

#[test]
fn a_wrong_angle_or_meets_kind_does_not_complete_the_tier_step() {
    let target = rbc();
    let plan = plan_of(&target);
    let mut session = follow_lesson(&target, &plan);
    let step = step_titled(&plan.guide, "Add P1 Pavilion Main (pavilion main)");
    let at = session
        .design
        .tiers
        .iter()
        .position(|tier| tier.name == "Pavilion Main")
        .expect("the tier is in");
    assert!(holds(&step.goal, &session.design));
    session.design.tiers[at].angle_deg -= 0.5;
    assert!(!holds(&step.goal, &session.design), "half a degree off");
    assert_compare_unmet(&plan, &session.design);
    session.design.tiers[at].angle_deg += 0.5;
    session.design.tiers[at].constraint = MeetConstraint::MeetExisting;
    assert!(!holds(&step.goal, &session.design), "another Meets kind");
}

/// The compare step does not hold for `design` even when it is solved.
fn assert_compare_unmet(plan: &BuildPlan, design: &Design) {
    let compare = step_titled(&plan.guide, COMPARE_STEP_TITLE);
    let masts = vec![1.0; design.tiers.len()];
    let ctx = GoalContext::new(design)
        .solved_closed(true)
        .solved_masts(&masts);
    assert!(!goal_met(&compare.goal, &ctx));
}

#[test]
fn a_missing_concave_tier_keeps_the_compare_step_open() {
    let target = rbc_with_groove();
    let plan = plan_of(&target);
    let mut session = follow_lesson(&target, &plan);
    assert_compare_met(&plan, &session.design);
    session.design.concave_tiers.clear();
    let compare = step_titled(&plan.guide, COMPARE_STEP_TITLE);
    let masts = masts_from_original(&plan.guide, &session.design);
    let ctx = GoalContext::new(&session.design)
        .solved_closed(true)
        .solved_masts(&masts);
    assert!(!goal_met(&compare.goal, &ctx));
}

#[test]
fn the_original_is_held_by_the_lesson_with_the_lesson_names_and_pinned_depths() {
    let guide = guide_of(&awkward_names());
    let original = reference_design(&guide).expect("the lesson holds its original");
    let names: Vec<&str> = original
        .design
        .tiers
        .iter()
        .map(|tier| tier.name.as_str())
        .collect();
    assert_eq!(names, ["P1", "P2", "C1", "C2", "P3", "T", "G1"]);
    assert_eq!(original.masts.len(), 7);
    assert!(original.design.tiers.iter().all(|tier| matches!(
        tier.constraint,
        MeetConstraint::ScaleReference(_)
    ) && tier.imported_meet.is_none()));
}

#[test]
fn only_a_build_lesson_has_an_original_and_a_compare_step() {
    let guide = guide_of(&rbc());
    assert!(reference_design(&worked_example_guide()).is_none());
    assert!(is_compare_step(&guide, 10));
    for index in [0, 1, 9, 11, 99] {
        assert!(!is_compare_step(&guide, index), "step {index}");
    }
    assert!(!is_compare_step(&worked_example_guide(), 10));
    assert_eq!(original_label(&guide), "Original: Standard Round Brilliant");
}

// ---------------------------------------------------------------------------------------
// Meets
// ---------------------------------------------------------------------------------------

#[test]
fn an_imported_lesson_states_the_anchors_and_names_only_earlier_tiers() {
    let plan = plan_of(&imported_rbc());
    let kind = |name: &str| {
        plan.recipes
            .iter()
            .find(|recipe| recipe.name == name)
            .unwrap_or_else(|| panic!("no recipe {name}"))
            .constraint_kind
    };
    for anchor in ["Girdle", "Pavilion Main", "Star"] {
        assert_eq!(kind(anchor), 2, "{anchor} anchors its block");
    }
    for (at, recipe) in plan.recipes.iter().enumerate() {
        if recipe.constraint_kind != 1 {
            continue;
        }
        for named in recipe.constraint_text.split(',').map(str::trim) {
            assert!(
                plan.recipes[..at]
                    .iter()
                    .any(|earlier| earlier.name == named),
                "{} names {named}, which is not cut before it",
                recipe.name
            );
        }
    }
}

#[test]
fn an_imported_lesson_solves_to_the_originals_depths() {
    let target = imported_rbc();
    let plan = plan_of(&target);
    let session = follow_lesson(&target, &plan);
    let solved = session.design.solve().expect("the lesson's design solves");
    let masts: Vec<f64> = solved.iter().map(|tier| tier.mast).collect();
    let compare = step_titled(&plan.guide, COMPARE_STEP_TITLE);
    let ctx = GoalContext::new(&session.design)
        .solved_closed(true)
        .solved_masts(&masts);
    assert!(
        goal_met(&compare.goal, &ctx),
        "whatever mix of meets the lesson teaches reproduces the original's depths"
    );
}

#[test]
fn a_meet_with_a_tier_cut_later_is_stated_as_an_exact_depth() {
    let mut target = rbc();
    for tier in &mut target.tiers {
        if tier.name == "Crown Main" {
            tier.imported_meet = Some(MeetConstraint::MeetNamed(vec!["Table".to_owned()]));
        }
    }
    let plan = plan_of(&target);
    let step = step_titled(&plan.guide, "Add C2 Crown Main (crown main)");
    assert!(step.why.contains("cut later in the lesson"), "{}", step.why);
    assert_eq!(step.actions[2], "Meets: Exact scale value -- 0.59");
}

#[test]
fn a_design_of_many_tiers_states_every_depth_directly() {
    let mut target = ladder(30, 30);
    for tier in &mut target.tiers {
        tier.imported_meet = Some(MeetConstraint::MeetExisting);
    }
    let plan = plan_of(&target);
    assert_eq!(plan.recipes.len(), 62);
    assert!(
        plan.recipes
            .iter()
            .all(|recipe| recipe.constraint_kind == 2)
    );
    follow_to_the_end(&target);
}

// ---------------------------------------------------------------------------------------
// The Simple interface
// ---------------------------------------------------------------------------------------

/// Whether `goal` needs a tier whose Meets kind is "Exact scale value".
fn needs_exact_scale(goal: &Goal) -> bool {
    match goal {
        Goal::TierMatches {
            constraint_kind, ..
        } => *constraint_kind == Some(MeetKind::ExactScale),
        Goal::All(goals) | Goal::Any(goals) => goals.iter().any(needs_exact_scale),
        _ => false,
    }
}

/// Whether `goal` is the goal of a tier step: one tier, or a list of tiers.
fn is_tier_goal(goal: &Goal) -> bool {
    match goal {
        Goal::TierMatches { .. } => true,
        Goal::All(goals) => goals.iter().all(is_tier_goal),
        _ => false,
    }
}

/// Whether an action line tells the learner to choose the Meets entry "Exact scale value",
/// which the Simple interface leaves out of the Meets list of a new tier.
fn names_the_exact_entry(action: &str) -> bool {
    let lower = action.to_ascii_lowercase();
    lower.contains("meets") && lower.contains("exact scale value")
}

#[test]
fn a_build_step_that_asks_for_exact_scale_value_unlocks_the_advanced_controls() {
    let lessons = [
        guide_of(&rbc()),
        guide_of(&imported_rbc()),
        guide_of(&awkward_names()),
        guide_of(&ladder(22, 22)),
        guide_of(&rbc_with_groove()),
    ];
    for guide in &lessons {
        let mut exact_steps = 0;
        for step in guide.steps.iter().filter(|step| is_tier_goal(&step.goal)) {
            let exact = needs_exact_scale(&step.goal);
            assert_eq!(
                step.actions
                    .iter()
                    .any(|action| names_the_exact_entry(action)),
                exact,
                "{:?}: the text and the goal disagree about Exact scale value",
                step.title
            );
            assert_eq!(
                step.allow.contains(&Group::Advanced),
                exact,
                "{:?}: the Advanced controls are unlocked exactly when Exact scale value is asked for",
                step.title
            );
            assert_eq!(
                step.why.contains("shows the Advanced controls"),
                exact,
                "{:?}: the explanation says why the Advanced controls show",
                step.title
            );
            for group in [Group::TierForm, Group::TierTable, Group::History] {
                assert!(
                    step.allow.contains(&group),
                    "{:?} locks {group:?}",
                    step.title
                );
            }
            exact_steps += usize::from(exact);
        }
        assert!(
            exact_steps > 0,
            "{}: every lesson has its anchors",
            guide.id
        );
    }
}

/// Steps outside the Build lessons that tell the learner to pick "Exact scale value" and need
/// no Advanced unlock, with the reason.
const EXACT_ENTRY_ALREADY_LISTED: &[(&str, &str)] = &[
    // The Rich Teaching Design pins every depth, so its Table already uses the entry and the
    // Simple interface lists it for that tier.
    ("tiers-arithmetic", "Calculate a scale value"),
];

/// Every step of every built-in tutorial and of generated Build lessons (the round brilliant,
/// an imported `.asc` design and a large design) that names the Meets entry "Exact scale
/// value" can be done in the Simple interface: it unlocks the Advanced group, or offers the
/// Girdle Facet Preset, which sets that entry itself, or is a listed step on a tier that
/// already uses it.
#[test]
fn every_step_that_names_the_exact_scale_entry_can_be_done_in_simple_mode() {
    let mut guides = static_guides();
    guides.extend([rbc(), imported_rbc(), ladder(22, 22)].iter().map(guide_of));
    let mut checked = 0;
    for guide in &guides {
        for step in &guide.steps {
            if !step
                .actions
                .iter()
                .any(|action| names_the_exact_entry(action))
            {
                continue;
            }
            checked += 1;
            let preset = step
                .actions
                .iter()
                .any(|action| action.contains("Girdle Facet Preset"));
            let listed =
                EXACT_ENTRY_ALREADY_LISTED.contains(&(guide.id.as_str(), step.title.as_str()));
            assert!(
                step.allow.contains(&Group::Advanced) || preset || listed,
                "{}, {:?}: Simple mode does not list Exact scale value, so this step needs the \
                 Advanced group, the Girdle Facet Preset, or a listed exception",
                guide.id,
                step.title
            );
        }
    }
    assert!(checked >= 4, "only {checked} steps name the entry");
}

#[test]
fn the_girdle_step_names_the_preset_that_fills_in_angle_and_meets() {
    let guide = guide_of(&rbc());
    let girdle = step_titled(&guide, "Add G1 Girdle (girdle)");
    assert!(girdle.actions[1].contains("Girdle Facet Preset"));
    assert!(
        girdle
            .why
            .contains("Girdle Facet Preset fills in Angle 90 and Meets Exact scale value 1"),
        "{}",
        girdle.why
    );
    assert!(girdle.why.contains("only Name and Indices are left"));
    let pavilion = step_titled(&guide, "Add P1 Pavilion Main (pavilion main)");
    assert!(
        pavilion
            .actions
            .iter()
            .chain([&pavilion.why])
            .all(|text| !text.contains("Girdle Facet Preset")),
        "only the girdle has a preset"
    );

    // A girdle that is not half a unit wide: the preset's scale value has to be changed.
    let mut wide = design_of(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    wide.tiers[0].constraint = MeetConstraint::ScaleReference(0.8);
    let guide = guide_of(&wide);
    let girdle = step_titled(&guide, "Add G1 (girdle)");
    assert!(
        girdle.why.contains("change the Scale value to 0.8"),
        "{}",
        girdle.why
    );
}

// ---------------------------------------------------------------------------------------
// Large designs
// ---------------------------------------------------------------------------------------

#[test]
fn a_large_design_groups_alike_tiers_into_checklist_steps() {
    let plan = plan_of(&ladder(22, 22));
    assert!(plan.recipes.len() > LARGE_DESIGN_TIERS);
    assert_eq!(
        titles(&plan.guide),
        [
            "Start a new design",
            "Add G1 (girdle)",
            "Add P1 to P6 (pavilion facets)",
            "Add P7 to P12 (pavilion facets)",
            "Add P13 to P18 (pavilion facets)",
            "Add P19 to P22 (pavilion facets)",
            "Add C1 to C6 (crown facets)",
            "Add C7 to C12 (crown facets)",
            "Add C13 to C18 (crown facets)",
            "Add C19 to C22 (crown facets)",
            "Add T (table)",
            "Solve and check",
            "Compare with the original",
            "You rebuilt Standard Round Brilliant",
        ]
    );
    assert!(plan.guide.steps[0].intro.contains("in 14 steps"));
    for step in &plan.guide.steps[2..=9] {
        assert!(
            matches!(&step.goal, Goal::All(goals) if (4..=6).contains(&goals.len())),
            "{}",
            step.title
        );
        assert!(step.actions.len() <= MAX_TIERS_PER_STEP + 1);
    }
}

#[test]
fn a_grouped_step_is_a_checklist_that_explains_the_anchor() {
    let guide = guide_of(&ladder(22, 22));
    let first = step_titled(&guide, "Add P1 to P6 (pavilion facets)");
    assert!(
        first.intro.starts_with("Tiers 2 to 7 of 46."),
        "{}",
        first.intro
    );
    assert_eq!(first.actions.len(), 7);
    assert!(
        first.actions[1].starts_with("P1: angle -30.0, indices 0 x8"),
        "{}",
        first.actions[1]
    );
    assert!(first.why.contains("these 6 tiers share one step"));
    assert!(first.why.contains("sets that block's size"));
    let second = step_titled(&guide, "Add P7 to P12 (pavilion facets)");
    assert!(!second.why.contains("sets that block's size"));
    let last = step_titled(&guide, "Add P19 to P22 (pavilion facets)");
    assert_eq!(last.actions.len(), 5);
    assert!(last.why.contains("these 4 tiers share one step"));
}

#[test]
fn a_grouped_step_waits_for_every_tier_in_it() {
    let target = ladder(22, 22);
    let plan = plan_of(&target);
    let step = step_titled(&plan.guide, "Add P1 to P6 (pavilion facets)");
    let mut session = dialog_design(&target);
    // The girdle first, then five of the six tiers: not done until the sixth.
    for recipe in &plan.recipes[..6] {
        type_recipe(&mut session, recipe);
    }
    assert!(!holds(&step.goal, &session.design));
    type_recipe(&mut session, &plan.recipes[6]);
    assert!(holds(&step.goal, &session.design));
}

// ---------------------------------------------------------------------------------------
// Names
// ---------------------------------------------------------------------------------------

#[test]
fn names_the_form_cannot_take_are_replaced_by_unique_ones() {
    let target = awkward_names();
    let blocks = classify_blocks(&target.meet_tier_inputs());
    assert_eq!(
        teaching_names(&target.tiers, &blocks),
        ["P1", "P2", "C1", "C2", "P3", "T", "G1"]
    );
    let plan = plan_of(&target);
    let order: Vec<&str> = plan.recipes.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(order, ["P1", "P2", "P3", "G1", "C1", "C2", "T"]);
    assert_eq!(plan.original_positions, [0, 1, 4, 6, 2, 3, 5]);
}

#[test]
fn roles_follow_the_names_and_then_the_index_counts() {
    let eight = EIGHT_FOLD_INDICES;
    let sixteen: Vec<f64> = (0..16).map(|k| f64::from(k).mul_add(6.0, 3.0)).collect();
    let design = design_of(&[
        tier("U", -0.0, &[]),
        tier("P1", -41.0, &eight),
        tier("P2", -42.5, &sixteen),
        tier("C1", 15.0, &eight),
        tier("C2", 34.5, &eight),
        tier("C3", 41.0, &sixteen),
        tier("T", 0.0, &[]),
        tier("G", 90.0, &eight),
    ]);
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let names = teaching_names(&design.tiers, &blocks);
    assert_eq!(
        infer_roles(&design.tiers, &names, &blocks),
        [
            Role::Culet,
            Role::PavilionMain,
            Role::PavilionBreak,
            Role::Star,
            Role::CrownMain,
            Role::UpperBreak,
            Role::Table,
            Role::Girdle,
        ]
    );
}

#[test]
fn a_concave_tier_has_its_own_step_between_the_pavilion_and_the_crown() {
    let plan = plan_of(&rbc_with_groove());
    let guide = &plan.guide;
    assert_eq!(plan.recipes.len(), 8, "only flat tiers have recipes");
    assert_eq!(guide.steps.len(), 13);
    let concave = &guide.steps[5];
    assert_eq!(
        concave.title, "Add P3 Groove (concave tier 1)",
        "the groove continues the P count of the two flat pavilion tiers"
    );
    assert_eq!(guide.steps[4].title, "Add Culet (culet)");
    assert_eq!(guide.steps[6].title, "Add C1 Star (star)");
    assert!(matches!(concave.goal, Goal::ConcaveTiersAtLeast(1)));
    assert_eq!(concave.highlight_target, "tier_table");
    // The button the step names must be one the Simple interface shows too: the tier
    // table's, not the command bar's "+ Concave".
    assert_eq!(
        concave.actions[0],
        "Click + Add Concave Tier in the tier table's toolbar."
    );
    assert!(
        concave
            .actions
            .iter()
            .all(|action| !action.contains("command bar"))
    );
    assert!(concave.actions.contains(&"Name: Groove".to_owned()));
    assert!(concave.actions.contains(&"Indices: 0, 12".to_owned()));
    assert!(
        concave
            .actions
            .iter()
            .any(|action| action.starts_with("Tool: Cylinder ("))
    );
    assert!(concave.actions.contains(&"D/W: 0.25".to_owned()));
    assert!(
        concave
            .actions
            .contains(&"Theta (deg): 0.0; X: 0.0, Y: 0.15, Z: 0.03".to_owned())
    );
    assert!(
        concave
            .actions
            .contains(&"Reciprocating: ticked".to_owned())
    );
    assert!(concave.why.contains("cutting sheet"));
    let compare = step_titled(guide, COMPARE_STEP_TITLE);
    assert!(matches!(&compare.goal, Goal::All(goals) if goals.len() == 2));
}

// ---------------------------------------------------------------------------------------
// Depths from the caller
// ---------------------------------------------------------------------------------------

#[test]
fn supplied_depths_are_used_instead_of_solving() {
    let target = rbc();
    let solved = solved_with(&[0.5; 8]);
    let plan = build_this_design_plan(&target, Some(&solved), TITLE, KEY).expect("a lesson");
    let original = reference_design(&plan.guide).expect("an original");
    assert_eq!(original.masts, [0.5; 8]);
    assert_eq!(plan.recipes[0].constraint_text, "0.5");
}

#[test]
fn a_depth_list_of_the_wrong_length_is_ignored() {
    let solved = solved_with(&[0.5; 3]);
    let plan = build_this_design_plan(&rbc(), Some(&solved), TITLE, KEY).expect("a lesson");
    let original = reference_design(&plan.guide).expect("an original");
    assert_eq!(original.masts[4], 1.0, "the pinned girdle depth is used");
}

#[test]
fn a_solved_list_with_a_failed_tier_is_refused() {
    let solved: Vec<SolvedTier> = (0..8)
        .map(|_| SolvedTier {
            mast: 1.0,
            strategy: SolveStrategy::Failed,
            detail: String::new(),
        })
        .collect();
    let error = build_this_design_guide(&rbc(), Some(&solved), TITLE, KEY).expect_err("refused");
    assert!(matches!(error, BuildGuideError::DoesNotSolve(_)), "{error}");
}

// ---------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------

#[test]
fn a_design_without_tiers_has_no_lesson() {
    let error = build_this_design_guide(&design_of(&[]), None, TITLE, KEY).expect_err("no tiers");
    assert_eq!(error, BuildGuideError::NoTiers);
}

#[test]
fn a_design_with_only_zero_depths_has_no_lesson() {
    let mut target = rbc();
    for tier in &mut target.tiers {
        tier.constraint = MeetConstraint::ScaleReference(0.0);
    }
    let error = build_this_design_guide(&target, None, TITLE, KEY).expect_err("placeholders");
    assert_eq!(error, BuildGuideError::NoUsableMasts);
}

#[test]
fn a_design_that_does_not_solve_has_no_lesson() {
    let mut lonely = tier("P1", -41.0, &EIGHT_FOLD_INDICES);
    lonely.constraint = MeetConstraint::MeetExisting;
    let error =
        build_this_design_guide(&design_of(&[lonely]), None, TITLE, KEY).expect_err("no anchor");
    assert!(
        matches!(&error, BuildGuideError::DoesNotSolve(reason) if reason.contains("no anchor")),
        "{error}"
    );
}

#[test]
fn a_key_that_cannot_be_part_of_an_id_is_refused() {
    for key in ["", "has space", "caf\u{e9}"] {
        let error = build_this_design_guide(&rbc(), None, TITLE, key).expect_err("bad key");
        assert_eq!(error, BuildGuideError::BadKey, "key {key:?}");
    }
}

#[test]
fn every_error_reads_as_a_plain_sentence() {
    let errors = [
        BuildGuideError::NoTiers,
        BuildGuideError::NoUsableMasts,
        BuildGuideError::DoesNotSolve("Pavilion has no anchor.".to_owned()),
        BuildGuideError::BadKey,
        BuildGuideError::CannotTeach("P1: bad".to_owned()),
        BuildGuideError::NotFit("two steps".to_owned()),
    ];
    for error in errors {
        let text = error.to_string();
        assert!(text.is_ascii(), "{text}");
        assert!(text.len() > 20, "{text}");
        assert!(!text.contains("  "), "{text}");
    }
    assert!(
        BuildGuideError::DoesNotSolve("Pavilion has no anchor.".to_owned())
            .to_string()
            .ends_with("Pavilion has no anchor.")
    );
}

// ---------------------------------------------------------------------------------------
// Ids, the catalogue and the browser
// ---------------------------------------------------------------------------------------

#[test]
fn a_build_id_is_the_prefix_and_a_visible_key() {
    assert!(build_guide_id(KEY).starts_with(BUILD_ID_PREFIX));
    assert!(is_build_guide_id(&build_guide_id(KEY)));
    assert!(!is_build_guide_id("worked-example"));
    for good in [
        format!("build:{KEY}"),
        "build:https://example.org/designs/A.asc".to_owned(),
        "build:A".to_owned(),
        format!("build:{}", "x".repeat(300)),
        "worked-example".to_owned(),
    ] {
        assert!(is_valid_guide_id(&good), "{good}");
    }
    for bad in [
        "build:".to_owned(),
        "build:a b".to_owned(),
        "Build:abc".to_owned(),
        "build:caf\u{e9}".to_owned(),
        format!("build:{}", "x".repeat(301)),
        "Worked-Example".to_owned(),
    ] {
        assert!(!is_valid_guide_id(&bad), "{bad}");
    }
}

#[test]
fn the_browser_lists_a_lesson_under_built_from_the_library_and_marks_it_done() {
    assert_eq!(
        GuideCategory::BuildThisDesign.label(),
        "Built from the library"
    );
    let mut catalog = GuideCatalog::new();
    let guide = guide_of(&rbc());
    catalog.add_generated(guide.clone()).expect("a fit guide");
    assert!(catalog.is_generated(&guide.id));
    // Generating the lesson again keeps one entry.
    catalog.add_generated(guide.clone()).expect("replaced");
    assert_eq!(
        catalog
            .all()
            .iter()
            .filter(|known| known.id == guide.id)
            .count(),
        1
    );

    let mut done = BTreeSet::new();
    let rows = browser_rows(catalog.all(), &done, false, "");
    let row = rows
        .iter()
        .find(|row| row.id == guide.id)
        .expect("the lesson has a row");
    assert_eq!(row.category, GuideCategory::BuildThisDesign);
    assert!(row.first_in_category);
    assert!(!row.done);
    assert!(row.blocked.is_none());
    assert_eq!(row.step_count, 12);

    assert!(record_completion(&mut done, &guide.id));
    let rows = browser_rows(catalog.all(), &done, false, "built from the library");
    assert!(rows.iter().any(|row| row.id == guide.id && row.done));
}

// ---------------------------------------------------------------------------------------
// The words and numbers behind the steps
// ---------------------------------------------------------------------------------------

#[test]
fn angles_keep_a_culet_minus_zero_and_tiny_angles() {
    assert_eq!(angle_text(90.0), "90.0");
    assert_eq!(angle_text(-41.0), "-41.0");
    assert_eq!(angle_text(34.5), "34.5");
    assert_eq!(angle_text(41.12), "41.12");
    assert_eq!(angle_text(-0.0), "-0");
    assert_eq!(angle_text(0.0), "0.0");
    assert_eq!(angle_text(0.00001), "0.00001");
}

#[test]
fn depths_and_plain_numbers_are_short() {
    assert_eq!(mast_text(1.0), "1.0");
    assert_eq!(mast_text(0.5), "0.5");
    assert_eq!(mast_text(0.67), "0.67");
    assert_eq!(mast_text(0.123_456), "0.1235");
    assert_eq!(number_text(-0.00001, 3), "0.0");
    assert_eq!(number_text(12.0, 2), "12.0");
    assert_eq!(plain_number_text(12.0), "12");
    assert_eq!(plain_number_text(12.5), "12.5");
    assert_eq!(plain_number_text(-0.0), "0");
    assert_eq!(plain_number_text(0.0), "0");
}

#[test]
fn index_lists_use_the_shortest_shorthand_the_form_reads_back() {
    let typed = |indices: &[f64], angle: f64| indices_text(indices, 96, angle).typed;
    assert_eq!(typed(&EIGHT_FOLD_INDICES, 34.5), "0 x8");
    assert_eq!(
        typed(&[6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0], 15.0),
        "6 x8"
    );
    // An orbit that wraps past the end of the gear.
    assert_eq!(typed(&[95.0, 23.0, 47.0, 71.0], 30.0), "23 x4");
    // An even run that is not an orbit of the gear.
    assert_eq!(typed(&[10.0, 20.0, 30.0, 40.0], 30.0), "10:10:50");
    // Short or uneven lists stay as they are.
    assert_eq!(typed(&[3.0, 9.0, 15.0], 30.0), "3, 9, 15");
    assert_eq!(typed(&[0.0, 1.0, 5.0, 40.0], 30.0), "0, 1, 5, 40");
}

#[test]
fn a_lone_zero_is_blank_only_for_a_table_a_culet_and_a_girdle() {
    for angle in [0.0, -0.0, 90.0] {
        let text = indices_text(&[0.0], 96, angle);
        assert!(text.blank, "angle {angle}");
        assert_eq!(text.typed, "");
    }
    assert!(indices_text(&[], 96, 30.0).blank);
    let lone = indices_text(&[0.0], 96, 30.0);
    assert!(!lone.blank);
    assert_eq!(lone.typed, "0");
}

#[test]
fn a_goal_describes_its_indices_in_a_few_words() {
    let eight = indices_text(&EIGHT_FOLD_INDICES, 96, 34.5);
    assert_eq!(indices_phrase(&eight, &EIGHT_FOLD_INDICES), "8 indices");
    let none = indices_text(&[], 96, 0.0);
    assert_eq!(indices_phrase(&none, &[]), "no indices");
    let one = indices_text(&[5.0], 96, 30.0);
    assert_eq!(indices_phrase(&one, &[5.0]), "1 index");
}
