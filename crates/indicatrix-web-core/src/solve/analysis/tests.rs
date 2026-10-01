//! Optimize and Retarget through the solve handler, against the direct desktop calls.
//!
//! Every built-in template pins all of its tiers, so the quick tests here cover what a
//! template does through the handlers (Optimize has nothing free; Retarget's Shift mode
//! is identical to `build_proposal`). The two tests that run a real search cost a
//! `Full`-fidelity tilt sweep each (over a minute unoptimized), so they are `#[ignore]`d
//! like the equivalent tests in `indicatrix-cut-core`: run them with
//! `cargo test -p indicatrix-web-core -- --ignored`.

use std::cell::RefCell;

use super::*;
use crate::{
    protocol::{FromWorker, ToWorker, WorkerRole},
    solve::{SolveRequest, design_to_toml, handle_solve},
    worker::WorkerHandler,
};
use indicatrix::geometry::meet_solver::{
    Block, MeetConstraint, classify_blocks, meet_tier_inputs_from_asc,
};
use indicatrix_cut_core::{
    ConstraintTier, FreshDesignSpec, PreformSpec, ScheduleMeta, templates::TEMPLATES,
};
use indicatrix_editor::{
    EditorSession,
    retarget::view::{resolved_material_from_selection, retarget_view},
};

/// "RBC-445" (PC 13.156): a small, genuinely meet-derived design, the same fixture the
/// editor's and the cut core's own tests use.
const RBC_445: &str = "GemCad 5.0\ng 96 0.0\ny 3 y\nI 1.54\n\
     H PC 13.156  RBC-445\n\
     H Richard B Conley, Facets, Oct 2013 p10\n\
     a -50.400000 0.61624001 92 n 1 68 60 36 28 4 G TCP\n\
     a -48.900000 0.63552822 88 n 2 72 56 40 24 8 G TCP\n\
     a -90.000000 0.91252100 92 n 3 68 60 36 28 4\n\
     a -90.000000 0.96225045 88 n 4 72 56 40 24 8\n\
     a -47.200000 0.59908304 95 n 5 65 63 33 31 1\n\
     a -43.000000 0.64485570 86 n 6 74 54 42 22 10 G PCP\n\
     a -47.266965 0.63949242 87 n 7 73 55 41 23 9\n\
     a 31.000000 0.62509296 4 n A 28 36 60 68 92\n\
     a 29.000000 0.62477630 8 n B 24 40 56 72 88\n\
     a 28.100000 0.60078994 2 n C 30 34 62 66 94\n\
     a 20.940747 0.56272062 14 n D 18 46 50 78 82\n\
     a 0.000000 0.40674031 96 n E\n";

/// RBC-445 with each block's first tier bootstrapped to a `ScaleReference` and every
/// other tier keeping its file's meet -- so three tiers are pinned and the rest free.
fn rbc_445() -> Design {
    let schedule = indicatrix_formats::asc::parse_asc(RBC_445).expect("fixture parses");
    let mut inputs = meet_tier_inputs_from_asc(&schedule);
    let blocks = classify_blocks(&inputs);
    for block in [Block::Crown, Block::Pavilion, Block::Girdle] {
        let anchored = inputs
            .iter()
            .zip(&blocks)
            .any(|(t, &b)| b == block && matches!(t.constraint, MeetConstraint::ScaleReference(_)));
        if anchored {
            continue;
        }
        if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
            inputs[i].constraint = MeetConstraint::ScaleReference(schedule.tiers[i].mast);
        }
    }
    let tiers = inputs
        .into_iter()
        .zip(&schedule.tiers)
        .map(|(input, original)| ConstraintTier {
            angle_deg: input.angle_deg,
            name: original.name.clone(),
            indices: input.indices,
            constraint: input.constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        })
        .collect();
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gemcad_version: schedule.gemcad_version.clone(),
            gear_teeth: schedule.gear_teeth,
            gear_reference_angle: schedule.gear_reference_angle,
            symmetry_order: schedule.symmetry_order,
            mirror: schedule.mirror,
            refractive_index: schedule.refractive_index,
            headers: schedule.headers.clone(),
            footnotes: schedule.footnotes,
        },
        tiers,
    )
}

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

fn quartz() -> MaterialSelectionData {
    MaterialSelectionData {
        name: Some("Quartz".to_string()),
        ..MaterialSelectionData::default()
    }
}

fn shift_params(crown_fraction: f64, scale_crown_by_ratio: bool) -> RetargetParams {
    RetargetParams {
        target: quartz(),
        crown_fraction,
        scale_crown_by_ratio,
        mode: RetargetModeData::Shift,
        optimize: OptimizeParams::default(),
    }
}

fn no_hooks<R>(f: impl FnOnce(&SolveHooks<'_>) -> R) -> R {
    let cancel = AtomicBool::new(false);
    f(&SolveHooks {
        cancel: &cancel,
        on_progress: &|_| {},
        on_sweep: &|_| {},
    })
}

fn retargeted(response: SolveResponse) -> RetargetResultData {
    match response {
        SolveResponse::Retargeted(result) => result,
        other => panic!("expected a retarget answer, got {other:?}"),
    }
}

#[test]
fn the_default_params_are_the_desktops_default_config() {
    assert_eq!(
        OptimizeParams::default().config(),
        OptimizeConfig::default()
    );
    let mut params = OptimizeParams {
        seed: 9,
        max_evaluations: 12,
        polish: false,
        ..OptimizeParams::default()
    };
    let config = params.config();
    assert_eq!((config.seed, config.max_evaluations), (9, 12));
    assert_eq!(config.polish_start_step_deg, None);
    params.polish = true;
    assert_eq!(
        params.config().polish_start_step_deg,
        OptimizeConfig::default().polish_start_step_deg
    );
}

#[test]
fn an_outcome_round_trips_through_its_plain_data() {
    let outcome = OptimizeOutcome {
        before: ObjectiveComponents {
            windowing_pct: 12.5,
            extinction_pct: 8.0,
            tilt_brilliance_pct: 60.0,
        },
        before_score: 20.0,
        before_yield_loss_pct: 30.0,
        after: ObjectiveComponents {
            windowing_pct: 9.25,
            extinction_pct: 11.0,
            tilt_brilliance_pct: 65.0,
        },
        after_score: 15.0,
        after_yield_loss_pct: 25.0,
        evaluations: 42,
        changes: vec![AngleChange {
            index: 3,
            from_deg: 40.75,
            to_deg: 41.5,
        }],
        cancelled: true,
        polish_evaluations: 7,
        polish_improvement: 0.5,
    };
    let data = OptimizeResultData::from_outcome(&outcome, Some(1.54));
    assert_eq!(data.defaulted_ri, Some(1.54));
    assert_eq!(data.to_outcome(), outcome);
}

#[test]
fn every_template_pins_all_its_tiers_so_optimize_says_nothing_is_free() {
    for (i, template) in TEMPLATES.iter().enumerate() {
        let design = template_design(i as i32 + 1);
        let toml = design_to_toml(&design).expect("encodes");
        let response = handle_solve(
            &toml,
            &SolveRequest::Optimize {
                params: OptimizeParams::default(),
                custom_materials: Vec::new(),
            },
        );
        match response {
            SolveResponse::AnalysisFailed {
                message,
                missing_anchor,
            } => {
                assert!(
                    message.contains("nothing free"),
                    "{}: {message}",
                    template.name
                );
                assert!(!missing_anchor);
            }
            other => panic!("{}: expected a refusal, got {other:?}", template.name),
        }
    }
}

#[test]
fn shift_retarget_through_the_handler_equals_build_proposal_on_every_template() {
    for (i, template) in TEMPLATES.iter().enumerate() {
        let design = template_design(i as i32 + 1);
        let toml = design_to_toml(&design).expect("encodes");
        for (fraction, ratio) in [(0.0, false), (0.5, false), (0.0, true)] {
            let params = shift_params(fraction, ratio);
            let got = retargeted(handle_solve(
                &toml,
                &SolveRequest::Retarget {
                    params: params.clone(),
                    custom_materials: Vec::new(),
                },
            ));

            let target = resolved_material_from_selection(&params.target.into(), &[]);
            let crown = CrownShift {
                fraction,
                scale_by_ratio: ratio,
            };
            let direct =
                retarget::build_proposal(&design, &target, crown, RetargetMode::Shift, &[])
                    .expect("Shift never fails");
            let direct_rows: Vec<RetargetRowData> =
                direct.rows.iter().map(RetargetRowData::from).collect();
            assert_eq!(got.rows, direct_rows, "{} rows", template.name);
            assert_eq!(got.notes, direct.notes, "{} notes", template.name);
            assert!(got.anchored_errors.is_empty() && got.solve_error.is_empty());

            // The page rebuilds the proposal from the rows: same edit as the direct one.
            let rebuilt = got
                .to_proposal(target)
                .expect("a template has pavilion and crown tiers");
            assert_eq!(rebuilt.rows, direct.rows, "{} rebuilt rows", template.name);
            assert_eq!(
                retarget::apply(&design, &rebuilt),
                retarget::apply(&design, &direct),
                "{} edit",
                template.name
            );
        }
    }
}

#[test]
fn optimize_mode_retarget_refuses_anchored_tiers_exactly_like_the_desktop_view() {
    let design = rbc_445();
    let params = RetargetParams {
        mode: RetargetModeData::Optimize,
        ..shift_params(0.0, false)
    };
    let got = retargeted(no_hooks(|hooks| run_retarget(&design, &params, &[], hooks)));

    let target = resolved_material_from_selection(&params.target.clone().into(), &[]);
    let (view, proposal) = retarget_view(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Optimize(params.optimize.config()),
        &[],
    );
    assert!(proposal.is_none());
    assert_eq!(got.anchored_errors, view.anchored_errors);
    assert_eq!(got.anchored_errors.len(), 2);
    assert!(got.rows.is_empty() && got.solve_error.is_empty());
}

#[test]
fn only_the_named_tiers_stay_free_and_the_rest_are_pinned_at_their_masts() {
    let design = rbc_445();
    let solved = design.solve().expect("the fixture solves");
    let free_before = free_tier_indices(&design);
    assert!(free_before.len() >= 2, "{free_before:?}");
    let keep = free_before[0];
    let params = OptimizeParams {
        only_tiers: Some(vec![keep as u32]),
        ..OptimizeParams::default()
    };
    let restricted = restricted_design(&design, &params).expect("solves");
    assert_eq!(free_tier_indices(&restricted), vec![keep]);
    for &i in &free_before[1..] {
        assert!(matches!(
            restricted.tiers[i].constraint,
            MeetConstraint::ScaleReference(mast) if mast.to_bits() == solved[i].mast.to_bits()
        ));
    }
    // No restriction, or an empty one, leaves the design alone.
    let plain = OptimizeParams::default();
    assert_eq!(
        restricted_design(&design, &plain).expect("ok").tiers,
        design.tiers
    );
    let empty = OptimizeParams {
        only_tiers: Some(Vec::new()),
        ..OptimizeParams::default()
    };
    assert_eq!(
        restricted_design(&design, &empty).expect("ok").tiers,
        design.tiers
    );
}

/// A real search through a Worker handler: streams progress, and its result equals the
/// direct `optimize_design` call the desktop makes (same seed, same material).
#[test]
#[ignore = "runs Full-fidelity tilt sweeps: over a minute unoptimized"]
fn an_optimize_search_through_the_worker_equals_a_direct_call() {
    let design = rbc_445();
    let params = OptimizeParams {
        seed: 3,
        max_evaluations: 6,
        polish: false,
        ..OptimizeParams::default()
    };
    let toml = design_to_toml(&design).expect("encodes");
    let mut worker = WorkerHandler::new();
    worker.handle(
        ToWorker::Init {
            protocol_version: crate::protocol::PROTOCOL_VERSION,
            role: WorkerRole::Solve,
            worker_index: 0,
        },
        &|| 0.0,
    );
    let streamed = RefCell::new(Vec::new());
    let clock = std::cell::Cell::new(0.0);
    let ticking = || {
        clock.set(clock.get() + 250.0);
        clock.get()
    };
    let reply = worker.handle_streaming(
        ToWorker::Solve {
            job_id: 1,
            design_toml: toml,
            request: SolveRequest::Optimize {
                params: params.clone(),
                custom_materials: Vec::new(),
            },
        },
        &ticking,
        &|message| streamed.borrow_mut().push(message),
    );
    let Some(FromWorker::SolveResult {
        response: SolveResponse::Optimized(got),
        ..
    }) = reply
    else {
        panic!("expected an optimize result, got {reply:?}");
    };

    let mut selection = design.material.clone();
    let defaulted = default_optimize_material_ri(&design, &mut selection, &[]);
    let material = resolved_gem_material(&selection, &EditorMaterialLookup::new(&[]));
    let direct = optimize_design(
        &design,
        &material,
        &params.config(),
        &SearchHooks::default(),
    )
    .expect("the fixture solves");
    assert_eq!(got, OptimizeResultData::from_outcome(&direct, defaulted));

    let streamed = streamed.into_inner();
    assert!(streamed.len() >= 2, "{streamed:?}");
    assert!(
        streamed
            .iter()
            .all(|m| matches!(m, FromWorker::Progress { job_id: 1, .. }))
    );
}

/// Retarget's Optimize mode on a design with nothing anchored in scope (girdle only):
/// the search runs (empty free set) and the answer equals `build_proposal`'s.
#[test]
#[ignore = "runs a Full-fidelity tilt sweep: over a minute unoptimized"]
fn an_optimize_mode_retarget_equals_build_proposal() {
    let design = Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::default(),
        vec![ConstraintTier {
            angle_deg: 90.0,
            name: "Girdle".to_string(),
            indices: vec![],
            constraint: MeetConstraint::ScaleReference(1.0),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }],
    );
    let params = RetargetParams {
        mode: RetargetModeData::Optimize,
        optimize: OptimizeParams {
            max_evaluations: 4,
            ..OptimizeParams::default()
        },
        ..shift_params(0.0, false)
    };
    let got = retargeted(no_hooks(|hooks| run_retarget(&design, &params, &[], hooks)));
    let target = resolved_material_from_selection(&params.target.clone().into(), &[]);
    let direct = retarget::build_proposal(
        &design,
        &target,
        CrownShift::default(),
        RetargetMode::Optimize(params.optimize.config()),
        &[],
    )
    .expect("nothing anchored in scope");
    assert_eq!(got.rows.len(), direct.rows.len());
    assert_eq!(got.notes, direct.notes);
}
