//! The walks of the Solve, auto-solve, status, verdict, Fix, Preform and yield lessons, and the
//! premises they state about the design.

use super::{RICH, STANDARD, Sim, act, events, guide_named, material, read, walk};
use crate::{
    guide::StartingState,
    verdict::{FixAction, Level, ReasonKind, Verdict, evaluate, gather, plan_fix},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_cut_core::{Design, Edit};

/// The verdict of a design that solves, as the badge works it out (without the measured optics).
fn verdict_of(design: &Design) -> Verdict {
    let solved = design.solve().expect("the design solves");
    let n_d = design.effective_refractive_index();
    evaluate(&gather(design, Some(&solved), n_d, None))
}

/// The row of the tier called `name` in `design`.
fn row_in(design: &Design, name: &str) -> usize {
    design
        .tiers
        .iter()
        .position(|tier| tier.name == name)
        .unwrap_or_else(|| panic!("no tier is called {name:?}"))
}

#[test]
fn the_solve_lesson_can_be_played() {
    walk(
        "solving-solve",
        vec![
            read(),
            act(|sim| sim.raise(events::AUTO_SOLVE_OFF)),
            act(|sim| sim.set_angle("Crown Main", "36")),
            act(Sim::solve),
            read(),
            read(),
        ],
    );
}

#[test]
fn the_auto_solve_lesson_can_be_played() {
    walk(
        "solving-auto-solve",
        vec![
            read(),
            act(|sim| sim.raise(events::AUTO_SOLVE_OFF)),
            act(|sim| sim.set_angle("Crown Main", "36")),
            act(|sim| sim.raise(events::AUTO_SOLVE_ON)),
            act(|sim| {
                sim.set_angle("Crown Main", "34.5");
                // Auto-solve: nobody presses Solve, the solve lands by itself.
                sim.settle();
            }),
            read(),
        ],
    );
}

#[test]
fn the_status_lesson_can_be_played() {
    walk(
        "solving-status",
        vec![
            act(Sim::solve),
            read(),
            act(|sim| {
                let girdle = sim.row("Girdle");
                sim.apply(Edit::SetConstraint {
                    index: girdle,
                    constraint: MeetConstraint::MeetExisting,
                });
            }),
            act(Sim::solve),
            act(|sim| {
                let girdle = sim.row("Girdle");
                sim.apply(Edit::SetConstraint {
                    index: girdle,
                    constraint: MeetConstraint::ScaleReference(1.0),
                });
                sim.solve();
            }),
            read(),
        ],
    );
}

#[test]
fn a_girdle_with_no_anchor_does_not_solve_and_says_so() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    let girdle = sim.row("Girdle");
    sim.apply(Edit::SetConstraint {
        index: girdle,
        constraint: MeetConstraint::MeetExisting,
    });
    let error = sim
        .session
        .design
        .solve()
        .expect_err("the girdle has no anchor");
    assert!(
        error.to_string().contains("Girdle has no anchor"),
        "the strip's sentence: {error}"
    );
    sim.solve();
    assert!(!sim.solved, "a design that does not solve is not solved");
}

#[test]
fn the_verdict_lesson_can_be_played() {
    walk(
        "solving-verdict",
        vec![
            act(Sim::solve),
            read(),
            act(|sim| sim.raise(events::VERDICT_OPENED)),
            read(),
            act(|sim| sim.set_material("Opal")),
            act(|sim| {
                sim.solve();
                sim.raise(events::VERDICT_OPENED);
            }),
            read(),
        ],
    );
}

#[test]
fn a_lower_index_material_gives_the_standard_brilliant_a_windowing_reason() {
    let mut sim = Sim::start(&StartingState::Template(STANDARD));
    sim.set_material("Opal");
    let verdict = verdict_of(&sim.session.design);
    assert_ne!(verdict.level, Level::Good, "{}", verdict.headline);
    let windowing = verdict
        .reasons
        .iter()
        .find(|reason| reason.kind == ReasonKind::Windowing)
        .expect("a pavilion facet is below the critical angle of opal");
    assert!(windowing.tier.is_some(), "Show has a tier to select");
}

/// F5-18: the Standard Round Brilliant's culet is a pin-prick, a facet too small to polish, so
/// the verdict lesson's stone reads Check from its first solve, never Good, and Opal adds the
/// windowing reason on top. The lesson's text has to say what the verdict function says at each
/// of those moments.
#[test]
fn the_verdict_lesson_describes_the_badge_the_stone_really_shows() {
    let guide = guide_named("solving-verdict");
    let step_text = |at: usize| {
        let step = &guide.steps[at];
        format!("{} {}", step.actions.join(" "), step.check)
    };
    let mut sim = Sim::start(&guide.starting_state);

    // After the first solve: Check, because the culet is too small to polish.
    let first = verdict_of(&sim.session.design);
    assert_eq!(first.level, Level::Check, "{}", first.headline);
    let culet = row_in(&sim.session.design, "Culet");
    let small = first
        .reasons
        .iter()
        .find(|reason| reason.kind == ReasonKind::Undersized && reason.tier == Some(culet))
        .expect("the culet is too small to polish");
    assert!(small.text.contains("too small to polish"), "{}", small.text);
    assert_eq!(small.fix, None, "a small facet has Show but no Fix");
    let badge = step_text(1);
    assert!(
        badge.contains(&format!("This stone reads {}", first.level.word())),
        "the badge step names the word the badge shows: {badge}"
    );
    assert!(badge.contains("too small to polish"), "{badge}");
    assert!(
        !badge.contains("for example: Looks good"),
        "the sentence this stone shows is not the Good one: {badge}"
    );
    let reasons = step_text(2);
    assert!(
        reasons.contains("Culet") && reasons.contains("Show but no Fix"),
        "the reasons step names the culet's reason: {reasons}"
    );

    // After Opal: still Check, with the windowing reason added to the culet's.
    sim.set_material("Opal");
    let second = verdict_of(&sim.session.design);
    assert_eq!(second.level, Level::Check, "{}", second.headline);
    assert!(
        second
            .reasons
            .iter()
            .any(|reason| reason.kind == ReasonKind::Windowing),
        "opal lets light out of the pavilion"
    );
    assert!(
        second
            .reasons
            .iter()
            .any(|reason| reason.kind == ReasonKind::Undersized),
        "the culet is still too small"
    );
    let again = step_text(5);
    assert!(
        again.contains(&format!("still says {}", second.level.word()))
            && again.contains("besides the culet's"),
        "the last look says what the badge shows: {again}"
    );
    assert!(
        !guide
            .steps
            .iter()
            .any(|step| step.check.contains("no longer says Good")),
        "the stone never said Good"
    );
}

#[test]
fn the_verdict_fixes_lesson_can_be_played() {
    walk(
        "solving-verdict-fixes",
        vec![
            act(Sim::solve),
            act(|sim| {
                let table = sim.row("Table");
                sim.apply(Edit::RemoveTier { index: table });
            }),
            // A solve lands (button or auto-solve): the stone still closes without a table.
            act(Sim::settle),
            act(|sim| {
                let design = sim.session.design.clone();
                let n_d = design.effective_refractive_index();
                let plan =
                    plan_fix(&design, &FixAction::AddTable, n_d).expect("a table can be added");
                sim.apply(plan.edit);
            }),
            act(|sim| {
                let crown = sim.row("Crown Main");
                let mut tier = sim.session.design.tiers[crown].clone();
                let at = tier
                    .indices
                    .iter()
                    .position(|index| (*index - 12.0).abs() < 1e-9)
                    .expect("the crown mains include position 12");
                tier.indices[at] = 12.5;
                sim.apply(Edit::SetIndices {
                    index: crown,
                    indices: tier.indices,
                    detached: tier.detached,
                });
            }),
            act(|sim| {
                sim.settle();
                let design = sim.session.design.clone();
                let tier = row_in(&design, "Crown Main");
                let n_d = design.effective_refractive_index();
                let plan = plan_fix(&design, &FixAction::SnapToTeeth { tier }, n_d)
                    .expect("the index can be snapped");
                sim.apply(plan.edit);
            }),
            read(),
        ],
    );
}

#[test]
fn a_crown_with_no_table_is_a_reason_with_an_add_a_table_fix() {
    let mut sim = Sim::start(&StartingState::Template(STANDARD));
    let table = sim.row("Table");
    sim.apply(Edit::RemoveTier { index: table });
    let verdict = verdict_of(&sim.session.design);
    let reason = verdict
        .reasons
        .iter()
        .find(|reason| reason.kind == ReasonKind::MissingTable)
        .expect("the verdict notices the crown has no table");
    assert_eq!(reason.fix, Some(FixAction::AddTable));
    let n_d = sim.session.design.effective_refractive_index();
    let plan = plan_fix(&sim.session.design, &FixAction::AddTable, n_d).expect("a table fits");
    sim.apply(plan.edit);
    assert!(
        sim.session
            .design
            .tiers
            .iter()
            .any(|tier| tier.name == "Table")
    );
}

#[test]
fn an_index_between_two_teeth_is_a_reason_with_a_snap_to_teeth_fix() {
    let mut sim = Sim::start(&StartingState::Template(STANDARD));
    let crown = sim.row("Crown Main");
    let mut indices = sim.session.design.tiers[crown].indices.clone();
    let at = indices
        .iter()
        .position(|index| (*index - 12.0).abs() < 1e-9)
        .expect("the crown mains include position 12");
    indices[at] = 12.5;
    sim.apply(Edit::SetIndices {
        index: crown,
        indices,
        detached: Vec::new(),
    });
    let verdict = verdict_of(&sim.session.design);
    let reason = verdict
        .reasons
        .iter()
        .find(|reason| reason.kind == ReasonKind::OffGear)
        .expect("the verdict notices the index between two teeth");
    assert_eq!(reason.fix, Some(FixAction::SnapToTeeth { tier: crown }));
}

#[test]
fn the_preform_lesson_can_be_played() {
    walk(
        "solving-preform",
        vec![
            act(|sim| sim.open_tab(1)),
            read(),
            act(|sim| {
                let mut preform = sim.session.design.preform;
                assert!(
                    (preform.half_width - 1.5).abs() < 1e-9,
                    "the lesson says the rough starts at 1.5"
                );
                preform.half_width = 1.8;
                sim.apply(Edit::SetPreform { preform });
            }),
            act(Sim::settle),
            read(),
        ],
    );
}

#[test]
fn the_yield_lesson_can_be_played() {
    walk(
        "solving-yield-carat",
        vec![
            act(|sim| sim.set_material("Quartz")),
            act(|sim| sim.open_tab(1)),
            act(|sim| {
                sim.apply(Edit::SetGirdleDiameterMm {
                    girdle_diameter_mm: Some(8.0),
                });
            }),
            act(Sim::settle),
            read(),
        ],
    );
}

#[test]
fn the_materials_the_lessons_name_are_known_and_in_the_order_the_text_says() {
    // Opal has the lowest index (the verdict lesson), quartz moves to topaz (Retarget: Shift) and
    // sapphire is a bigger step still (Retarget: Optimize).
    let index = |name: &str| {
        material(name)
            .resolve(&indicatrix_cut_core::BuiltinMaterials)
            .n_d
    };
    let (opal, quartz, topaz, sapphire) = (
        index("Opal"),
        index("Quartz"),
        index("Topaz"),
        index("Sapphire"),
    );
    assert!(opal < quartz && quartz < topaz && topaz < sapphire);
    assert!(
        (opal - 1.45).abs() < 0.01,
        "the lesson says opal is 1.45, not {opal}"
    );
}
