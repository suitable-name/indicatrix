//! The Optimize tab (the desktop's `solve_actions::{optimize_run,optimize_outcome}`,
//! `view::optimize_apply` and `optimize_solve.rs`): a coordinate search over the free tier
//! angles for windowing, extinction and tilt brilliance, run in the solve Worker with live
//! progress and applied as one undoable edit.
//!
//! # Flow
//!
//! Run sends the design (native TOML) and the settings to the solve Worker as
//! `SolveRequest::Optimize`; the Worker streams its stage line and evaluation count back
//! while it computes. The result is held until Apply (`EditorSession::apply_optimize_outcome`,
//! one `ModifyTier` per changed tier) and only while it is current: an edit since the run
//! started makes it stale, shown for reference but not appliable.
//!
//! # What differs from the desktop
//!
//! - Cancel is cooperative, as on the desktop: the page revokes the job's cancel URL, the
//!   Worker sees that between tier decisions (`SolveClient::request_cancel`) and answers
//!   with the best result found so far, which is shown and can be applied. The Worker needs
//!   ~10 s to do that (one more evaluation, then the closing full-fidelity score), and twice
//!   that when the cancel came during the starting-point score, so the wait is
//!   `GRACEFUL_CANCEL_MS` (30 s); "Stop now" (a second press) or a search that does not stop
//!   in time is terminated, and then keeps no partial result.
//! - The Worker serves solves too: an edit's auto-solve supersedes a running search (the
//!   search then reports "the design changed"), and Run waits for a running solve.
//! - There is no viewport "Preview" of a pending result or "Compare" window.

mod push;

pub use push::{clear, push};

use crate::{
    AppModel, AppWindow, InspectorModel, OptimizeModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        solve::is_solving,
    },
    editor::{edit::Dirty, inspector::finish},
    workers,
};
use indicatrix_cut_core::{Design, OptimizeOutcome, free_tier_indices};
use indicatrix_editor::optimize_view::{
    default_optimize_material_ri, optimize_change_rows, optimize_result_rows,
    optimize_start_status, optimize_status_text, parse_optimize_weights,
};
use indicatrix_web_core::{
    solve::{OptimizeParams, SolveRequest, SolveResponse, design_to_toml},
    solve_error::SolveError,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

/// A finished result waiting for Apply.
struct Pending {
    outcome: OptimizeOutcome,
    /// The design generation the search ran against.
    generation: u64,
    /// The result table's status line (Apply prefixes it "Applied:").
    status: String,
}

#[derive(Default)]
struct Runtime {
    /// Bumped by every Run and Cancel; a result of an older run is ignored.
    epoch: u64,
    pending: Option<Pending>,
    /// The design generation the result on screen was computed against (applied or not):
    /// once the design moves on, the result is badged "Stale: design changed".
    shown: Option<u64>,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

/// The coordinate-stage evaluation budget the fields would run with now (the desktop's
/// rule: blank, unparseable or zero means 200).
fn configured_budget(model: &OptimizeModel<'_>) -> usize {
    model
        .get_budget_text()
        .trim()
        .parse::<usize>()
        .ok()
        .filter(|&v| v > 0)
        .unwrap_or(200)
}

/// The Worker request for the fields, or why they cannot run.
fn build_params(
    model: &OptimizeModel<'_>,
    multi_selected: &[usize],
    lighting_preset_index: i32,
) -> Result<OptimizeParams, String> {
    let weights = parse_optimize_weights(
        &model.get_weight_windowing(),
        &model.get_weight_extinction(),
        &model.get_weight_tilt_brilliance(),
        model.get_weight_yield(),
    )?;
    Ok(OptimizeParams {
        windowing: weights.windowing,
        extinction: weights.extinction,
        tilt_brilliance: weights.tilt_brilliance,
        yield_weight: weights.yield_weight,
        seed: model.get_seed_text().trim().parse::<u64>().unwrap_or(0),
        max_evaluations: u32::try_from(configured_budget(model)).unwrap_or(u32::MAX),
        polish: model.get_polish_enabled(),
        only_tiers: (model.get_only_selected() && !multi_selected.is_empty())
            .then(|| multi_selected.iter().map(|&i| i as u32).collect()),
        lighting_preset_index,
    })
}

/// The Optimize tab's WEIGHTS as Worker settings with everything else at its default --
/// what Retarget's Optimize mode searches with (the desktop's `optimize_config_from_ui`).
/// Best effort: a malformed weight field is not this caller's form to validate, so the
/// default weights stand in for it.
pub fn params_or_default(ui: &AppWindow) -> OptimizeParams {
    let model = ui.global::<OptimizeModel>();
    let lighting_preset_index = ui.global::<AppModel>().get_lighting_index();
    parse_optimize_weights(
        &model.get_weight_windowing(),
        &model.get_weight_extinction(),
        &model.get_weight_tilt_brilliance(),
        model.get_weight_yield(),
    )
    .map_or_else(
        |_| OptimizeParams {
            lighting_preset_index,
            ..OptimizeParams::default()
        },
        |weights| OptimizeParams {
            windowing: weights.windowing,
            extinction: weights.extinction,
            tilt_brilliance: weights.tilt_brilliance,
            yield_weight: weights.yield_weight,
            lighting_preset_index,
            ..OptimizeParams::default()
        },
    )
}

/// What Run captured from the app.
struct Start {
    toml: String,
    design: Design,
    generation: u64,
    custom: Vec<indicatrix::optics::materials::GemMaterial>,
    multi_selected: Vec<usize>,
}

fn capture(ctx: &Ctx) -> Result<Start, String> {
    let app = ctx.state.borrow();
    let design_state = app.design.as_ref().ok_or("No design loaded.")?;
    let design = design_state.session.design.clone();
    Ok(Start {
        toml: design_to_toml(&design)?,
        design,
        generation: design_state.session.current_generation(),
        custom: app.custom_materials.clone(),
        multi_selected: design_state
            .session
            .multi_selected
            .iter()
            .copied()
            .collect(),
    })
}

/// Run: sends the search to the Worker.
fn run(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<OptimizeModel>();
    if model.get_running() {
        return;
    }
    if is_solving(ctx) {
        show_message(
            ctx,
            MessageKind::Info,
            "The design is still solving -- start Optimize when it has finished.",
        );
        return;
    }
    let start = match capture(ctx) {
        Ok(start) => start,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    if free_tier_indices(&start.design).is_empty() {
        show_message(
            ctx,
            MessageKind::Error,
            "Optimize has nothing free to move on this design right now.",
        );
        return;
    }
    let lighting_preset_index = ui.global::<AppModel>().get_lighting_index();
    let params = match build_params(&model, &start.multi_selected, lighting_preset_index) {
        Ok(params) => params,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let pool = match workers::pool() {
        Ok(pool) => pool,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    // The RI a design that names no material is scored against, named from the first frame.
    let mut selection = start.design.material.clone();
    let defaulted_ri = default_optimize_material_ri(&start.design, &mut selection, &start.custom);
    let epoch = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.epoch += 1;
        rt.pending = None;
        rt.shown = None;
        rt.epoch
    });
    model.set_result_stale(false);
    model.set_running(true);
    model.set_status(optimize_start_status(defaulted_ri).into());
    model.set_status_is_problem(false);
    model.set_progress(-1.0);
    model.set_can_apply(false);
    model.set_result_rows(ModelRc::new(VecModel::default()));
    model.set_change_rows(ModelRc::new(VecModel::default()));

    let request = SolveRequest::Optimize {
        params,
        custom_materials: start.custom,
    };
    let weak = ctx.ui.clone();
    let future = pool
        .solve()
        .solve_with_progress(start.toml, request, move |progress| {
            if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                let model = ui.global::<OptimizeModel>();
                model.set_status(progress.message.into());
                model.set_progress(progress.fraction.unwrap_or(-1.0));
            }
        });
    let ctx = ctx.clone();
    let (design, generation) = (start.design, start.generation);
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        finish_run(&ctx, epoch, &design, generation, result);
    });
}

/// Puts a finished result on screen (`handle_optimize_outcome` on the desktop).
fn finish_run(
    ctx: &Ctx,
    epoch: u64,
    design: &Design,
    generation: u64,
    result: Result<SolveResponse, SolveError>,
) {
    if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
        return;
    }
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<OptimizeModel>();
    model.set_running(false);
    model.set_cancelling(false);
    model.set_progress(-1.0);
    match result {
        Ok(SolveResponse::Optimized(data)) => {
            show_outcome(ctx, &model, design, generation, data.to_outcome());
        }
        Ok(SolveResponse::AnalysisFailed {
            message,
            missing_anchor,
        }) => {
            let status = if missing_anchor {
                format!(
                    "Optimize failed: {message} -- add an Exact scale value tier there and Solve first."
                )
            } else if message.ends_with('.') {
                message
            } else {
                format!("Optimize failed: {message}.")
            };
            model.set_status(status.as_str().into());
            model.set_status_is_problem(true);
            model.set_can_apply(false);
            show_message(ctx, MessageKind::Error, &status);
        }
        Ok(SolveResponse::InvalidDesign { message }) => {
            let status = format!("The solve worker could not read the design: {message}");
            model.set_status(status.as_str().into());
            model.set_status_is_problem(true);
            show_message(ctx, MessageKind::Error, &status);
        }
        Ok(SolveResponse::Cancelled) => {
            model.set_status("Optimize cancelled before it had a result.".into());
            model.set_status_is_problem(false);
        }
        Ok(_) => {
            model.set_status("Optimize was interrupted.".into());
            model.set_status_is_problem(false);
        }
        // The Worker did not stop within the grace period and was terminated.
        Err(SolveError::Cancelled) => {
            model.set_status(
                "Optimize cancelled -- the search did not stop in time, so it keeps no partial result."
                    .into(),
            );
            model.set_status_is_problem(false);
            crate::app::solve::auto_solve(ctx);
        }
        Err(SolveError::Superseded) => {
            model.set_status(
                "Optimize stopped: the design changed while it was running. Run it again.".into(),
            );
            model.set_status_is_problem(false);
        }
        Err(error) => {
            let status = format!("Optimize failed: {error}");
            model.set_status(status.as_str().into());
            model.set_status_is_problem(true);
            show_message(ctx, MessageKind::Error, &status);
        }
    }
}

/// The result table, the change rows and Apply for a finished search.
fn show_outcome(
    ctx: &Ctx,
    model: &OptimizeModel<'_>,
    design: &Design,
    generation: u64,
    outcome: OptimizeOutcome,
) {
    let stale = ctx.state.borrow().generation() != Some(generation);
    let no_net_improvement =
        !outcome.changes.is_empty() && outcome.after_score >= outcome.before_score;
    let status = if no_net_improvement {
        format!(
            "No net improvement at full tilt fidelity -- not recommended ({})",
            optimize_status_text(&outcome)
        )
    } else {
        optimize_status_text(&outcome)
    };
    model.set_status(status.as_str().into());
    model.set_status_is_problem(no_net_improvement);
    model.set_result_rows(ModelRc::new(VecModel::from(
        optimize_result_rows(&outcome)
            .into_iter()
            .map(|row| crate::OptimizeResultRow {
                label: row.label.into(),
                before: row.before.into(),
                after: row.after.into(),
                direction: row.direction,
            })
            .collect::<Vec<_>>(),
    )));
    model.set_change_rows(ModelRc::new(VecModel::from(
        optimize_change_rows(&outcome, design)
            .into_iter()
            .map(|row| crate::OptimizeChangeRow {
                tier_number: row.tier_number.into(),
                name: row.name.into(),
                from_angle: row.from_angle.into(),
                to_angle: row.to_angle.into(),
            })
            .collect::<Vec<_>>(),
    )));
    let can_apply = !outcome.changes.is_empty() && !stale;
    model.set_can_apply(can_apply);
    model.set_result_stale(stale);
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.shown = Some(generation);
        rt.pending = can_apply.then_some(Pending {
            outcome,
            generation,
            status: status.clone(),
        });
    });
    if stale {
        show_message(
            ctx,
            MessageKind::Info,
            "The design changed while Optimize was running -- its result is shown for \
             reference but can no longer be applied. Re-run Optimize on the current design.",
        );
    } else {
        show_message(
            ctx,
            if no_net_improvement {
                MessageKind::Info
            } else {
                MessageKind::Success
            },
            &format!("Optimize finished: {status}"),
        );
        if can_apply && let Some(ui) = ctx.ui.upgrade() {
            // Bring the tab into view: completion is easy to miss on another tab.
            let inspector = ui.global::<InspectorModel>();
            inspector.set_tab(2);
            inspector.set_collapsed(false);
        }
    }
}

/// Cancel: asks the Worker to stop and keep its best result so far (see the module doc
/// comment). The result arrives through [`finish_run`] like a finished search's; only
/// when the pool cannot ask, or the Worker does not stop in time, is the search terminated
/// and its partial result lost.
fn cancel(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<OptimizeModel>();
    if !model.get_running() {
        return;
    }
    if model.get_cancelling() {
        // A second press while the Worker is still winding down ("Stop now"): give up
        // the partial result rather than wait for it.
        abandon(ctx, &model);
        return;
    }
    let asked = workers::pool().and_then(|pool| pool.solve().request_cancel());
    if asked.is_ok() {
        model.set_cancelling(true);
        model.set_status(
            "Cancelling -- keeping the best result found so far (scoring it can take \
             up to ~20 s; Stop now gives it up)..."
                .into(),
        );
        model.set_status_is_problem(false);
        return;
    }
    abandon(ctx, &model);
}

/// The hard cancel: the search's result is dropped, the page is free again.
fn abandon(ctx: &Ctx, model: &OptimizeModel<'_>) {
    RUNTIME.with(|rt| rt.borrow_mut().epoch += 1);
    if let Ok(pool) = workers::pool() {
        // Terminating the Worker discards the search; the pool starts a fresh one.
        let _ = pool.solve().cancel();
    }
    model.set_running(false);
    model.set_cancelling(false);
    model.set_progress(-1.0);
    model.set_status(
        "Optimize cancelled -- the search did not stop in time, so it keeps no partial result."
            .into(),
    );
    model.set_status_is_problem(false);
    // A solve that was queued behind the search has to run again.
    crate::app::solve::auto_solve(ctx);
}

/// Apply: commits the held result as one undoable edit, if it still describes the design.
fn apply(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<OptimizeModel>();
    let held = RUNTIME.with(|rt| {
        rt.borrow()
            .pending
            .as_ref()
            .map(|p| (p.generation, p.outcome.clone(), p.status.clone()))
    });
    let Some((generation, outcome, status)) = held else {
        return;
    };
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        if design_state.session.current_generation() == generation {
            design_state
                .session
                .apply_optimize_outcome(&outcome)
                .map_err(Some)
        } else {
            Err(None)
        }
    };
    model.set_can_apply(false);
    match result {
        Ok(applied) => {
            RUNTIME.with(|rt| {
                let mut rt = rt.borrow_mut();
                rt.pending = None;
                rt.shown = None;
            });
            model.set_result_stale(false);
            model.set_status(format!("Applied: {status}").into());
            finish(ctx, Dirty::All);
            show_message(
                ctx,
                MessageKind::Success,
                &format!("Applied {applied} tier change(s) from Optimize."),
            );
        }
        Err(None) => show_message(
            ctx,
            MessageKind::Warning,
            "Result discarded: the design changed while Optimize was running. Re-run Optimize.",
        ),
        Err(Some(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Registers the Optimize tab's callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<OptimizeModel>();
    let c = ctx.clone();
    model.on_run(move || run(&c));
    let c = ctx.clone();
    model.on_cancel(move || cancel(&c));
    let c = ctx.clone();
    model.on_apply(move || apply(&c));
}
