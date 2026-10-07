//! The step content's own invariants, and the goal predicates behind the
//! automatic advance.

use super::{
    EIGHT_FOLD_INDICES, Group, HIGHLIGHT_TARGETS, MANUAL, NEW_DESIGN_CREATED, STEPS, Step,
    goal_reached, reached_completion, same_index_set,
    steps::{TIER_STEP, TIER_STEP_EXACT},
};
use crate::{
    EditorSession,
    view_model::{rows::tier_items_stale, solid_status::status_text_and_is_problem},
};
use indicatrix::geometry::meet_solver::{Block, MeetConstraint, classify_blocks};
use indicatrix_cut_core::{
    ConstraintTier, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, Edit, ManufacturabilityWarning,
    Risk, check_manufacturability, critical_angle_deg, exceeds_preform, windowing_risk,
};

/// Every completion key a step may use: [`MANUAL`], [`NEW_DESIGN_CREATED`] (an
/// event), and the state goals `progress::goal_reached` evaluates. A typo in a
/// step's key would otherwise silently mean "never completes".
const COMPLETION_KEYS: &[&str] = &[
    MANUAL,
    NEW_DESIGN_CREATED,
    "tier_named:G1",
    "tier_named:P1",
    "tier_named:C1",
    "tier_named:T",
    "material:Diamond",
    "solved_closed",
    "yield_applied",
];

#[test]
fn every_step_uses_a_known_completion_key_and_a_recognized_highlight_target() {
    for step in STEPS {
        assert!(
            COMPLETION_KEYS.contains(&step.completion),
            "step {:?} has an unknown completion key {:?}",
            step.title,
            step.completion
        );
        assert!(
            HIGHLIGHT_TARGETS.contains(&step.highlight_target),
            "step {:?} has an unrecognized highlight_target {:?}",
            step.title,
            step.highlight_target
        );
    }
}

#[test]
fn every_action_step_says_what_to_do_and_what_it_waits_for() {
    for step in STEPS {
        assert_ne!(step.title, "");
        assert_ne!(step.intro, "");
        if step.completion != MANUAL {
            assert!(
                !step.actions.is_empty(),
                "step {:?} has no actions",
                step.title
            );
            assert_ne!(
                step.waiting, "",
                "step {:?} has no waiting text",
                step.title
            );
            assert_ne!(step.check, "", "step {:?} has no check line", step.title);
        }
        for action in step.actions {
            assert_ne!(*action, "", "step {:?} has a blank action line", step.title);
        }
    }
}

#[test]
fn step_titles_are_unique() {
    for (i, step) in STEPS.iter().enumerate() {
        for other in &STEPS[i + 1..] {
            assert_ne!(step.title, other.title, "duplicate guide step title");
        }
    }
}

/// The walkthrough as the manual's chapter 7 lays it out: ten steps, the two
/// reading steps (8, "Check the orbits", and 10, the closing note) wait for Next,
/// and the closing step locks nothing.
#[test]
fn the_walkthrough_is_ten_steps_and_ends_unlocked() {
    assert_eq!(STEPS.len(), 10);
    let manual: Vec<&str> = STEPS
        .iter()
        .filter(|step| step.completion == MANUAL)
        .map(|step| step.title)
        .collect();
    assert_eq!(manual, ["Check the orbits", "You have a working design"]);
    let last = STEPS.last().expect("ten steps");
    for group in [
        Group::NewDesign,
        Group::TierForm,
        Group::TierTable,
        Group::DesignSettings,
        Group::Solve,
        Group::PreformTab,
        Group::Advanced,
        Group::ViewTabs,
        Group::FileOps,
        Group::History,
    ] {
        assert!(last.allow.contains(&group), "closing step locks {group:?}");
    }
}

/// Each action step unlocks the control its own actions name.
#[test]
fn each_action_step_unlocks_its_own_control() {
    let unlocks = |completion: &str, group: Group| {
        STEPS
            .iter()
            .find(|step| step.completion == completion)
            .is_some_and(|step| step.allow.contains(&group))
    };
    assert!(unlocks(NEW_DESIGN_CREATED, Group::NewDesign));
    for tier in ["G1", "P1", "C1", "T"] {
        assert!(unlocks(&format!("tier_named:{tier}"), Group::TierForm));
    }
    assert!(unlocks("material:Diamond", Group::DesignSettings));
    assert!(unlocks("solved_closed", Group::Solve));
    assert!(unlocks("yield_applied", Group::PreformTab));
    // The first step must not leave file actions open: a Load Selected there would
    // replace the design the walkthrough is about to build.
    assert!(!unlocks(NEW_DESIGN_CREATED, Group::FileOps));
}

/// A tier with an exact-scale meet. The one place the guide tests build a
/// `ConstraintTier` literal (`goal_tests` and `catalog_tests` reuse it).
pub(super) fn tier(name: &str, angle_deg: f64, indices: &[f64]) -> ConstraintTier {
    ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: indices.to_vec(),
        constraint: MeetConstraint::ScaleReference(1.0),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

/// A fresh 96-tooth, 8-fold design (`EditorSession::fresh`) with `tiers` added
/// through `History`, the way the tier form adds them.
pub(super) fn design_with(tiers: &[ConstraintTier]) -> Design {
    let mut state = EditorSession::fresh();
    for (index, tier) in tiers.iter().enumerate() {
        state
            .apply(Edit::AddTier {
                index,
                tier: tier.clone(),
            })
            .expect("add tier");
    }
    state.design
}

#[test]
fn tier_goals_need_the_right_name_angle_and_indices() {
    let good = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    assert!(goal_reached("tier_named:G1", &good, false));
    // Case and surrounding whitespace in the typed name do not matter.
    assert!(goal_reached(
        "tier_named:G1",
        &design_with(&[tier(" g1", 90.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    // The classic mistake the step warns about: 0.0 instead of 90.0.
    assert!(!goal_reached(
        "tier_named:G1",
        &design_with(&[tier("G1", 0.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    // One index short is not the girdle the step asked for.
    assert!(!goal_reached(
        "tier_named:G1",
        &design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES[..7])]),
        false
    ));
    // Right tier, wrong name.
    assert!(!goal_reached(
        "tier_named:G1",
        &design_with(&[tier("Girdle", 90.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    // A different step's tier does not complete this one.
    assert!(!goal_reached("tier_named:P1", &good, false));
    assert!(goal_reached(
        "tier_named:P1",
        &design_with(&[tier("P1", -40.0, &EIGHT_FOLD_INDICES)]),
        false
    ));
    assert!(goal_reached(
        "tier_named:C1",
        &design_with(&[tier("C1", 34.5, &EIGHT_FOLD_INDICES)]),
        false
    ));
    assert!(goal_reached(
        "tier_named:T",
        &design_with(&[tier("T", 0.0, &[])]),
        false
    ));
    assert!(!goal_reached(
        "tier_named:T",
        &design_with(&[tier("T", 0.0, &[12.0])]),
        false
    ));
}

#[test]
fn index_sets_compare_as_gear_positions() {
    assert!(same_index_set(&[84.0, 0.0, 12.0], &[0.0, 12.0, 84.0], 96.0));
    assert!(same_index_set(&[96.0], &[0.0], 96.0));
    assert!(same_index_set(&[], &[], 96.0));
    assert!(same_index_set(&[0.0], &[], 96.0));
    assert!(!same_index_set(&[0.0, 12.0], &[0.0, 24.0], 96.0));
    assert!(!same_index_set(&[0.0, 0.0], &[0.0, 12.0], 96.0));
}

#[test]
fn material_solve_and_yield_goals_read_the_design() {
    let mut design = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    assert!(!goal_reached("material:Diamond", &design, false));
    design.material.name = Some("Diamond".to_string());
    assert!(goal_reached("material:Diamond", &design, false));

    assert!(goal_reached("solved_closed", &design, true));
    assert!(!goal_reached("solved_closed", &design, false));
    // A zero-tier design "solves" to its bare preform, which is not the stone.
    assert!(!goal_reached("solved_closed", &design_with(&[]), true));

    assert!(!goal_reached("yield_applied", &design, false));
    design.girdle_diameter_mm = Some(6.5);
    assert!(goal_reached("yield_applied", &design, false));
}

/// `reached_completion` is `goal_reached` on the step's own key: what the two UIs call
/// after each change to the design.
#[test]
fn reached_completion_names_the_step_key_only_once_its_goal_holds() {
    let empty = design_with(&[]);
    let girdle = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    let step_of = |completion: &str| {
        STEPS
            .iter()
            .position(|step| step.completion == completion)
            .expect("a step with that key")
    };
    let g1 = step_of("tier_named:G1");
    assert_eq!(reached_completion(g1, &empty, false), None);
    assert_eq!(
        reached_completion(g1, &girdle, false),
        Some("tier_named:G1")
    );
    // A later step's goal does not complete an earlier one, nor the reverse.
    assert_eq!(
        reached_completion(step_of("tier_named:P1"), &girdle, false),
        None
    );
    let solve = step_of("solved_closed");
    assert_eq!(reached_completion(solve, &girdle, false), None);
    assert_eq!(
        reached_completion(solve, &girdle, true),
        Some("solved_closed")
    );
    // Reading steps, the new-design event and an index past the end never complete.
    assert_eq!(reached_completion(step_of(MANUAL), &girdle, true), None);
    assert_eq!(
        reached_completion(step_of(NEW_DESIGN_CREATED), &girdle, true),
        None
    );
    assert_eq!(reached_completion(STEPS.len(), &girdle, true), None);
}

/// Neither a reading step nor the new-design event can be completed by a design
/// predicate: the first only advances on Next, the second only through the
/// explicit `notify` in `do_new_design_create`'s success path, so a rejected form
/// (which never reaches it) can never advance.
#[test]
fn manual_and_event_keys_never_complete_from_state() {
    let design = design_with(&[tier("G1", 90.0, &EIGHT_FOLD_INDICES)]);
    assert!(!goal_reached(MANUAL, &design, true));
    assert!(!goal_reached(NEW_DESIGN_CREATED, &design, true));
    assert!(!goal_reached("tier_named:X9", &design, true));
}

/// The Meets setting a step's "Meets: ..." line tells the learner to choose.
fn typed_meets(text: &str) -> MeetConstraint {
    if text == "Unspecified vertex" {
        return MeetConstraint::MeetExisting;
    }
    if let Some(value) = text.strip_prefix("Exact scale value \u{2014} ") {
        return MeetConstraint::ScaleReference(value.trim().parse().expect("a scale value"));
    }
    if let Some(names) = text.strip_prefix("Named facet(s) \u{2014} ") {
        return MeetConstraint::MeetNamed(
            names
                .split(',')
                .map(|name| name.trim().to_owned())
                .collect(),
        );
    }
    panic!("a Meets line this test does not know: {text:?}");
}

/// The tier a tier-authoring step tells the learner to add, read back from the step's own
/// action lines (the text the guide panel shows), so the tests below check the numbers a
/// learner really types.
fn typed_tier(step: &Step) -> ConstraintTier {
    let (mut angle, mut meets, mut name, mut indices) = (None, None, None, None);
    for action in step.actions {
        if let Some(rest) = action.strip_prefix("Angle (deg): ") {
            // The girdle's line goes on to mention the preset: only the number counts.
            angle = rest
                .split_whitespace()
                .next()
                .and_then(|number| number.parse::<f64>().ok());
        } else if let Some(rest) = action.strip_prefix("Meets: ") {
            meets = Some(typed_meets(rest));
        } else if let Some(rest) = action.strip_prefix("Name: ") {
            name = Some(rest.trim().to_owned());
        } else if let Some(rest) = action.strip_prefix("Indices: ") {
            indices = Some(if rest == "leave blank" {
                Vec::new()
            } else {
                rest.split(',')
                    .map(|index| index.trim().parse::<f64>().expect("an index"))
                    .collect()
            });
        }
    }
    let name = name.unwrap_or_else(|| panic!("step {:?} has no Name line", step.title));
    let mut typed = tier(
        &name,
        angle.unwrap_or_else(|| panic!("step {:?} has no Angle line", step.title)),
        &indices.unwrap_or_else(|| panic!("step {:?} has no Indices line", step.title)),
    );
    typed.constraint = meets.unwrap_or_else(|| panic!("step {:?} has no Meets line", step.title));
    typed
}

/// The steps that add a tier, in order.
fn tier_steps() -> impl Iterator<Item = &'static Step> {
    STEPS
        .iter()
        .filter(|step| step.completion.starts_with("tier_named:"))
}

/// The tiers the four tier steps tell the learner to add.
fn walkthrough_tiers() -> Vec<ConstraintTier> {
    tier_steps().map(typed_tier).collect()
}

/// The value of a tier that states its scale.
fn scale_of(tier: &ConstraintTier) -> f64 {
    let MeetConstraint::ScaleReference(value) = tier.constraint else {
        panic!("tier {:?} does not state a scale value", tier.name);
    };
    value
}

/// The walkthrough has to reach its own "Solve and check" step: the numbers it tells the
/// learner to type give a design that solves to a closed stone with a girdle band and a
/// table. The solver needs an exact scale value in each block (crown, pavilion and girdle),
/// so a design with a Named facet(s) meet as the only crown and pavilion tiers never solves.
#[test]
fn the_walkthroughs_typed_values_solve_to_a_closed_stone_with_a_girdle_and_a_table() {
    let tiers = walkthrough_tiers();
    let names: Vec<&str> = tiers.iter().map(|tier| tier.name.as_str()).collect();
    assert_eq!(names, ["G1", "P1", "C1", "T"]);
    let design = design_with(&tiers);

    // What the learner types is what each step's own goal asks for.
    for step in tier_steps() {
        assert!(
            goal_reached(step.completion, &design, false),
            "the values typed in step {:?} do not meet its goal",
            step.title
        );
    }

    // Every block has an exact scale value of its own.
    let inputs = design.meet_tier_inputs();
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        assert!(blocks.contains(&block), "no {block:?} tier");
        assert!(
            inputs.iter().zip(&blocks).any(|(input, &of)| {
                of == block && matches!(input.constraint, MeetConstraint::ScaleReference(_))
            }),
            "the {block:?} block has no exact scale value"
        );
    }
    let solved = design
        .solve()
        .expect("every block is anchored, so the design solves");

    // The Solve step's own verdict: a closed solid, not a problem.
    let (status, is_problem) = status_text_and_is_problem(&design);
    assert!(
        !is_problem && status.starts_with("Closed solid"),
        "{status}"
    );

    // A girdle band thicker than nothing but thin, and a table.
    let proportions = design
        .stone_proportions(&solved)
        .expect("a closed solid has proportions");
    assert!(
        proportions.girdle_thickness.is_some_and(|band| band > 0.0),
        "the girdle band has no thickness"
    );
    let girdle = proportions.girdle_to_width_percent.expect("a girdle band");
    assert!(
        (1.0..=9.0).contains(&girdle),
        "girdle {girdle:.2} % of width"
    );
    let table = proportions.table_percent.expect("a table facet");
    assert!(
        (35.0..=80.0).contains(&table),
        "table {table:.1} % of width"
    );

    // Every facet reaches the surface, and the blank the first step asks for is big enough:
    // no preform plane cuts the stone.
    let vanishing: Vec<String> =
        check_manufacturability(&design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .into_iter()
            .filter_map(|warning| match warning {
                ManufacturabilityWarning::VanishingFacet { tier_name, .. } => Some(tier_name),
                _ => None,
            })
            .collect();
    assert!(
        vanishing.is_empty(),
        "facets that never appear: {vanishing:?}"
    );
    assert!(
        exceeds_preform(&design, &solved).is_none(),
        "the stone is bigger than the blank"
    );
}

/// Why the table states its height too: an Unspecified vertex table on a crown of main facets
/// alone settles on the next vertex below the point where they meet, which is the girdle's top
/// edge, so the table would cut the whole crown away.
#[test]
fn an_unspecified_table_would_sink_to_the_girdle_and_cut_the_crown_away() {
    let mut tiers = walkthrough_tiers();
    let girdle = scale_of(&tiers[0]);
    let crown = scale_of(&tiers[2]);
    tiers[3].constraint = MeetConstraint::MeetExisting;
    let design = design_with(&tiers);
    let solved = design.solve().expect("the three blocks are still anchored");

    let crown_angle = tiers[2].angle_deg.to_radians();
    let girdle_top = girdle.mul_add(-crown_angle.sin(), crown) / crown_angle.cos();
    assert!(
        (solved[3].mast - girdle_top).abs() < 1e-6,
        "the table landed at {}, the girdle's top edge is at {girdle_top}",
        solved[3].mast
    );
    // The stated table is well above it.
    assert!(scale_of(&walkthrough_tiers()[3]) > girdle_top + 0.2);
}

/// The Simple interface leaves "Exact scale value" out of the Meets list for a new tier, so
/// each of the four steps that ask for it unlocks the Advanced controls and says why. The
/// girdle step also names the preset that fills the entry in, but a learner who types the
/// girdle by hand needs the full list as much as the others do.
#[test]
fn the_steps_that_ask_for_an_exact_scale_value_can_be_done_in_the_simple_interface() {
    for step in tier_steps() {
        let asks = step
            .actions
            .iter()
            .any(|action| action.starts_with("Meets: Exact scale value"));
        assert!(asks, "step {:?} should state its scale value", step.title);
        assert!(
            step.allow.contains(&Group::Advanced),
            "step {:?} asks for Exact scale value, which Simple hides, and unlocks nothing \
             that shows it",
            step.title
        );
        assert!(
            step.why.contains("shows the Advanced controls"),
            "step {:?}: the explanation says why the Advanced controls show",
            step.title
        );
    }
    // The girdle step keeps the shortcut and says what the one click sets.
    let girdle = step_with("tier_named:G1");
    assert!(
        girdle.actions.iter().any(|action| {
            action.contains("Girdle Facet Preset") && action.contains("Exact scale value 1.0")
        }),
        "the girdle step names the preset and what it sets"
    );
}

/// One definition of the two tier-step lock sets serves the walkthrough and the generated
/// lessons; the second is the first plus the Advanced controls.
#[test]
fn the_exact_scale_lock_set_is_the_plain_tier_lock_set_plus_advanced() {
    assert!(TIER_STEP_EXACT.contains(&Group::Advanced));
    assert!(!TIER_STEP.contains(&Group::Advanced));
    assert!(
        TIER_STEP_EXACT
            .iter()
            .filter(|group| **group != Group::Advanced)
            .eq(TIER_STEP.iter())
    );
}

/// The Solve step tells the learner to look at a tier's angle and Meets setting and offers the
/// Add Anchor button for a block with no anchor, so the tier form and the tier table must be
/// open on it.
#[test]
fn the_solve_step_leaves_open_what_its_explanation_sends_the_learner_to() {
    let solve = step_with("solved_closed");
    assert!(solve.allow.contains(&Group::Solve));
    assert!(solve.why.contains("tier form") && solve.why.contains("Add Anchor"));
    assert!(solve.allow.contains(&Group::TierForm));
    assert!(solve.allow.contains(&Group::TierTable));
}

/// The step that completes on `completion`.
fn step_with(completion: &str) -> &'static Step {
    STEPS
        .iter()
        .find(|step| step.completion == completion)
        .unwrap_or_else(|| panic!("no step completes on {completion:?}"))
}

/// What the learner sees in the P1 row's MARGIN cell: its text and the colour word its risk
/// level draws in (`tier_table_row.slint`: green safe, amber marginal, red windows).
fn p1_margin_cell(design: &Design) -> (String, &'static str) {
    let rows = tier_items_stale(design, design.effective_refractive_index());
    let row = rows
        .iter()
        .find(|row| row.name == "P1")
        .expect("the walkthrough's design has a P1 row");
    let colour = match row.risk_level {
        0 => "green",
        1 => "amber",
        2 => "red",
        other => panic!("risk level {other} draws no colour"),
    };
    (row.margin_text.clone(), colour)
}

/// The colour word the windowing rule gives a margin.
const fn colour_of(risk: Risk) -> &'static str {
    match risk {
        Risk::Safe => "green",
        Risk::Marginal => "amber",
        Risk::Windows => "red",
    }
}

/// What the pavilion step and the material step tell the learner to look for in the P1 row is
/// what the tier table shows, so the wording cannot drift from the windowing rule again: with
/// no material the design uses its default index 1.54 (critical angle 40.5 degrees) and a
/// -40 degree pavilion reads a few tenths of a degree below it, in red; Diamond turns it green.
#[test]
fn the_pavilion_and_material_steps_describe_the_margin_the_tier_table_shows() {
    let mut design = design_with(&walkthrough_tiers());
    let pavilion_angle = walkthrough_tiers()[1].angle_deg;

    // Step 3's state: no material has been picked.
    let pavilion = step_with("tier_named:P1");
    let default_ri = design.effective_refractive_index();
    let (text, colour) = p1_margin_cell(&design);
    assert_eq!(
        colour,
        colour_of(windowing_risk(pavilion_angle, default_ri)),
        "the row's colour follows the windowing rule"
    );
    assert!(
        pavilion.check.contains(&text) && pavilion.check.contains(colour),
        "step {:?} asks for a MARGIN cell that does not read {text} in {colour}: {}",
        pavilion.title,
        pavilion.check
    );
    assert!(
        !pavilion.check.contains("Safe"),
        "the first look at P1 is not Safe"
    );
    assert!(
        pavilion.why.contains(&format!("{default_ri}"))
            && pavilion
                .why
                .contains(&format!("{:.1} degrees", critical_angle_deg(default_ri))),
        "the explanation gives the default index and its critical angle: {}",
        pavilion.why
    );

    // Step 6's state: Diamond is applied.
    design.material.name = Some("Diamond".to_owned());
    let material = step_with("material:Diamond");
    let diamond_ri = design.effective_refractive_index();
    let (text, colour) = p1_margin_cell(&design);
    assert_eq!(
        colour,
        colour_of(windowing_risk(pavilion_angle, diamond_ri)),
        "the row's colour follows the windowing rule"
    );
    assert_eq!(colour, "green", "Diamond makes the walkthrough's P1 Safe");
    assert!(
        material.check.contains(&text) && material.check.contains(colour),
        "step {:?} asks for a MARGIN cell that does not read {text} in {colour}: {}",
        material.title,
        material.check
    );
    assert!(
        material
            .why
            .contains(&format!("{:.1} degrees", critical_angle_deg(diamond_ri)))
            && material
                .why
                .contains(&format!("{:.1} degrees", critical_angle_deg(default_ri))),
        "the explanation gives both critical angles: {}",
        material.why
    );
}
