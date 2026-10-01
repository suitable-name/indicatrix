//! `handle_solve` against a direct `Design::solve` on every built-in template.

use super::*;
use indicatrix_cut_core::{FreshDesignSpec, MaterialSelection, PreformSpec, templates::TEMPLATES};
use indicatrix_editor::{EditorSession, solve_policy::design_to_gpu_planes_from_solved};

fn template_design(template_index: i32) -> Design {
    EditorSession::from_template(
        FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: MaterialSelection::none(),
            preform: PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        },
        template_index,
    )
    .design
}

fn outcome(response: SolveResponse) -> SolveOutcome {
    match response {
        SolveResponse::Solved(outcome) => outcome,
        other => panic!("expected a solve outcome, got {other:?}"),
    }
}

#[test]
fn every_template_solved_through_the_worker_equals_a_direct_solve() {
    for (i, template) in TEMPLATES.iter().enumerate() {
        let design = template_design(i as i32 + 1);
        let direct = design.solve().expect("every template solves");
        let toml = design_to_toml(&design).expect("encodes");
        let got = outcome(handle_solve(&toml, &SolveRequest::Solve));

        let solved = got.solved.as_ref().expect("solved through the worker");
        assert_eq!(solved.len(), direct.len(), "{}", template.name);
        for (a, b) in solved.iter().zip(&direct) {
            assert_eq!(a.mast.to_bits(), b.mast.to_bits(), "{} mast", template.name);
            assert_eq!(
                SolveStrategy::from(a.strategy),
                b.strategy,
                "{}",
                template.name
            );
            assert_eq!(a.detail, b.detail, "{}", template.name);
        }
        let direct_planes = planes_to_data(&design_to_gpu_planes_from_solved(&design, &direct));
        assert_eq!(got.planes, direct_planes, "{} planes", template.name);
        let (status, problem) = status_text_and_is_problem_from_solved(&design, &direct);
        assert_eq!(
            (got.status_text.as_str(), got.status_is_problem),
            (status.as_str(), problem)
        );
        assert_eq!(got.error, None);
        assert_eq!(got.tier_count as usize, design.tiers.len());
        assert!(!got.too_many_planes);
        // The round trip back to `SolvedTier` is lossless.
        let back = to_solved_tiers(solved);
        assert_eq!(back.len(), direct.len());
    }
}

#[test]
fn the_transport_keeps_every_solve_input() {
    let design = template_design(1);
    let loaded = design_from_toml(&design_to_toml(&design).expect("encodes")).expect("loads");
    assert_eq!(loaded.tiers, design.tiers);
    assert_eq!(loaded.meta.gear_teeth, design.meta.gear_teeth);
    assert_eq!(loaded.meta.symmetry_order, design.meta.symmetry_order);
    assert_eq!(loaded.preform, design.preform);
}

#[test]
fn an_empty_design_solves_and_bad_toml_is_reported() {
    let empty = template_design(0);
    let got = outcome(handle_solve(
        &design_to_toml(&empty).expect("encodes"),
        &SolveRequest::Solve,
    ));
    assert_eq!(got.tier_count, 0);
    assert!(got.solved.is_some());

    assert!(matches!(
        handle_solve("this is not a design", &SolveRequest::Solve),
        SolveResponse::InvalidDesign { .. }
    ));
}

#[test]
fn a_set_cancel_flag_cancels() {
    let design = template_design(1);
    assert_eq!(
        solve_design(&design, &AtomicBool::new(true)),
        SolveResponse::Cancelled
    );
}
