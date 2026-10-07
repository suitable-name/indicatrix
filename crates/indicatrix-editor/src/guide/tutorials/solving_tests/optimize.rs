//! The walks of the Deep Solve, Optimize, Retarget and angle sweep lessons, and the premises they
//! state about the design.

use super::{RICH, STANDARD, Sim, act, events, material, read, walk};
use crate::{
    guide::StartingState,
    optimize_view::{default_vary_anchored, preset_labels},
    retarget::{
        CrownShift, apply_with_anchors, build_plan,
        check::{CheckInputs, check_retarget},
        search::{
            DEFAULT_RANGE_INDEX, EFFORT_CHOICES, RANGE_CHOICES_DEG, SearchInputs, SearchSettings,
            run_search,
        },
    },
    sweep::{SweepRange, apply_sweep_angle, plan_sweep, sweepable_tiers},
};
use indicatrix::optics::LightingPreset;
use indicatrix_cut_core::{BuiltinMaterials, Edit};
use std::sync::atomic::AtomicBool;

/// `edit` and then the material choice, as ONE batch, which is how Apply commits a retarget.
fn with_material(edit: Edit, name: &str) -> Edit {
    let mut edits = match edit {
        Edit::Batch(edits) => edits,
        single => vec![single],
    };
    edits.push(Edit::SetMaterial {
        material: material(name),
    });
    Edit::Batch(edits)
}

/// The Retarget dialog in Shift mode: the proposal for `target`, its verdict (which must allow
/// Apply), and Apply.
fn retarget_by_shift(sim: &mut Sim, target: &str) {
    let design = sim.session.design.clone();
    let resolved = material(target).resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &resolved, CrownShift::default(), &[]);
    let check = check_retarget(&CheckInputs {
        girdle: None,
        design: &design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    });
    assert!(
        check.allows_apply(),
        "the lesson applies a Shift to {target}: {}",
        check.validity.headline()
    );
    let edit = apply_with_anchors(&design, &plan.proposal(), &check.anchors);
    sim.apply(with_material(edit, target));
}

/// The Retarget dialog in Optimize mode: a search for `target` with a small budget, its best
/// option, and Apply.
fn retarget_by_search(sim: &mut Sim, target: &str) {
    let design = sim.session.design.clone();
    let resolved = material(target).resolve(&BuiltinMaterials);
    let plan = build_plan(&design, &resolved, CrownShift::default(), &[]);
    let settings = SearchSettings {
        evaluations: 24,
        keep: 2,
        ..SearchSettings::default()
    };
    let report = run_search(
        &SearchInputs {
            design: &design,
            plan: &plan,
            current_gem: None,
            lighting: LightingPreset::RingLights,
            settings: &settings,
        },
        &AtomicBool::new(false),
        &|_, _| {},
    )
    .expect("the search runs");
    let best = report
        .candidates
        .first()
        .expect("the search offers at least one option");
    let proposal = plan.with_angles(&design, &best.angles).proposal();
    let edit = apply_with_anchors(&design, &proposal, &best.anchors);
    sim.apply(with_material(edit, target));
}

#[test]
fn the_deep_solve_lesson_can_be_played() {
    walk(
        "optimizing-deep-solve",
        vec![
            read(),
            act(|sim| sim.raise(events::DEEP_SOLVE_STARTED)),
            act(|sim| sim.raise(events::DEEP_SOLVE_FINISHED)),
            read(),
        ],
    );
}

#[test]
fn the_optimize_lesson_can_be_played() {
    walk(
        "optimizing-optimize",
        vec![
            act(|sim| sim.set_material("Quartz")),
            act(|sim| sim.open_tab(2)),
            act(|sim| sim.raise(events::OPTIMIZE_PRESET_CHOSEN)),
            act(|sim| sim.raise(events::OPTIMIZE_RANGES_OPENED)),
            act(|sim| sim.raise(events::OPTIMIZE_FINISHED)),
            act(|sim| sim.raise(events::OPTIMIZE_CANDIDATE_PICKED)),
            // Apply this candidate: the picked candidate's angles go in as one edit.
            act(|sim| sim.set_angle("Crown Main", "35.5")),
            read(),
        ],
    );
}

#[test]
fn the_optimize_tab_says_what_the_lesson_says_it_says() {
    assert!(preset_labels().contains(&"Low windowing"));
    // "Vary anchored tiers" starts ticked on a design where every tier is pinned.
    let sim = Sim::start(&StartingState::Template(STANDARD));
    assert!(default_vary_anchored(&sim.session.design));
}

#[test]
fn the_retarget_lessons_say_what_the_dialog_starts_with() {
    // Range 6 degrees either side, Effort Quick (100 steps) is the first choice of the list.
    assert!((RANGE_CHOICES_DEG[DEFAULT_RANGE_INDEX] - 6.0).abs() < 1e-12);
    assert_eq!(EFFORT_CHOICES[0], 100);
    let settings = SearchSettings::default();
    assert!((settings.range_deg - 6.0).abs() < 1e-12);
    assert_eq!(settings.keep, 3, "up to three options are listed");
    // "How Shift works" says the crown follows the pavilion's stretch, which is the policy the
    // dialog starts with and `retarget_by_shift` / `retarget_by_search` use.
    let crown = CrownShift::default();
    assert!(crown.follow_pavilion && !crown.scale_by_ratio);
}

#[test]
fn the_retarget_shift_lesson_can_be_played() {
    walk(
        "optimizing-retarget-shift",
        vec![
            act(|sim| sim.set_material("Quartz")),
            read(),
            act(|sim| retarget_by_shift(sim, "Topaz")),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn a_shift_to_topaz_moves_the_pavilion_and_is_one_undo_step() {
    let mut sim = Sim::start(&StartingState::Template(RICH));
    sim.set_material("Quartz");
    let before = sim.session.design.clone();
    retarget_by_shift(&mut sim, "Topaz");
    let pavilion = sim.row("Pavilion Main");
    assert!(
        (sim.session.design.tiers[pavilion].angle_deg - before.tiers[pavilion].angle_deg).abs()
            > 0.05,
        "the pavilion main moves"
    );
    sim.undo();
    let angles = |design: &indicatrix_cut_core::Design| -> Vec<f64> {
        design.tiers.iter().map(|tier| tier.angle_deg).collect()
    };
    assert_eq!(
        angles(&sim.session.design),
        angles(&before),
        "one undo takes every angle back"
    );
    assert_eq!(
        sim.session.design.material.name, before.material.name,
        "and the material"
    );
}

#[test]
fn the_retarget_optimize_lesson_can_be_played() {
    walk(
        "optimizing-retarget-optimize",
        vec![
            act(|sim| sim.set_material("Quartz")),
            read(),
            act(|sim| retarget_by_search(sim, "Sapphire")),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn the_sweep_lesson_can_be_played() {
    walk(
        "optimizing-sweep",
        vec![
            read(),
            act(|sim| {
                let pavilion = sim.row("Pavilion Main");
                apply_sweep_angle(&mut sim.session, pavilion, -42.0).expect("the angle is used");
                sim.solved = false;
            }),
            act(Sim::undo),
            read(),
        ],
    );
}

#[test]
fn the_sweep_can_reach_the_pavilion_and_marks_the_current_angle() {
    let sim = Sim::start(&StartingState::Template(RICH));
    let pavilion = sim.row("Pavilion Main");
    let tiers = sweepable_tiers(&sim.session.design);
    assert!(tiers.contains(&pavilion), "the pavilion main can be swept");
    assert!(
        !tiers.contains(&sim.row("Table")) && !tiers.contains(&sim.row("Girdle")),
        "the table and the girdle cannot"
    );
    let plan = plan_sweep(
        &sim.session.design,
        pavilion,
        SweepRange {
            from_deg: -42.0,
            to_deg: -38.0,
            step_deg: 1.0,
        },
    )
    .expect("the range is fine");
    assert_eq!(plan.angles.len(), 5, "-42, -41, -40, -39 and -38");
    assert!((plan.angles[plan.current_row] + 40.0).abs() < 1e-9);
}
