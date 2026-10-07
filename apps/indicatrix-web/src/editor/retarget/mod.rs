//! "Retarget for material" (the desktop's `callbacks/retarget_actions` and
//! `retarget_dialog.slint`): proposes new facet angles for another material and applies
//! them, together with the material, as one undoable edit.
//!
//! # Two algorithms
//!
//! Shift mode is pure angle arithmetic and is built right here on the page
//! (`indicatrix_editor::retarget::retarget_view`), so dragging the crown slider is live.
//! Optimize mode then searches the free tiers for the target material; that search runs
//! in the solve Worker as `SolveRequest::Retarget`, with the same progress and cancel as
//! the Optimize tab, and its rows come back as plain data that the page turns back into a
//! `RetargetProposal`. A tier still pinned by a scale reference cannot be searched, so
//! Optimize mode refuses up front and names the tiers, exactly as the desktop does.
//!
//! # What differs from the desktop
//!
//! There is no viewport ghost preview or visual before/after compare window; a cancelled
//! Optimize-mode search keeps no partial result (see `editor::optimize`).

mod view;

use crate::{
    AppWindow, RetargetModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        solve::is_solving,
    },
    editor::{edit::Dirty, inspector::finish, optimize},
    workers,
};
use indicatrix_cut_core::{Edit, MaterialSelection, ResolvedMaterial};
use indicatrix_editor::{
    material::design_material_options,
    retarget::{
        self, CrownShift, RetargetMode, RetargetProposal,
        view::{
            RetargetView, initial_target_index, resolve_target_selection,
            resolved_material_from_selection, retarget_view, target_display_name,
            target_material_selection,
        },
    },
};
use indicatrix_web_core::{
    solve::{RetargetModeData, RetargetParams, SolveRequest, SolveResponse, design_to_toml},
    solve_error::SolveError,
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::cell::RefCell;

/// The proposal waiting for Apply, with the generation it was built against.
struct Held {
    proposal: RetargetProposal,
    generation: u64,
}

#[derive(Default)]
struct Runtime {
    /// Bumped by every rebuild, Cancel and Close; a search of an older epoch is ignored.
    epoch: u64,
    held: Option<Held>,
    /// A search is running in the Worker.
    searching: bool,
}

thread_local! {
    static RUNTIME: RefCell<Runtime> = RefCell::new(Runtime::default());
}

/// Starts a new epoch, stopping a running search first.
fn next_epoch() -> u64 {
    let was_searching = RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        rt.epoch += 1;
        rt.held = None;
        std::mem::take(&mut rt.searching)
    });
    if was_searching && let Ok(pool) = workers::pool() {
        // Terminating the Worker discards the search; the pool starts a fresh one.
        let _ = pool.solve().cancel();
    }
    RUNTIME.with(|rt| rt.borrow().epoch)
}

/// The dialog was asked to open: seeds the target from the design's own material and
/// builds the first (Shift) proposal.
fn open(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RetargetModel>();
    next_epoch();
    model.set_mode_index(0);
    model.set_crown_fraction(0.0);
    model.set_scale_crown_by_ratio(false);
    model.set_crown_follows_pavilion(true);
    model.set_is_busy(false);
    model.set_progress(-1.0);
    {
        let app = ctx.state.borrow();
        let Some(design_state) = app.design.as_ref() else {
            return;
        };
        let material = &design_state.session.design.material;
        let options = design_material_options(&app.custom_materials);
        model.set_target_material_index(initial_target_index(material, &options));
        model.set_target_ri_override_text(
            material
                .refractive_index_override
                .map_or_else(String::new, |v| format!("{v}"))
                .into(),
        );
        model.set_material_options(ModelRc::new(VecModel::from(
            options
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    }
    model.set_is_open(true);
    rebuild(ctx);
}

/// The target, mode or crown controls changed: rebuilds the proposal.
fn rebuild(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RetargetModel>();
    let epoch = next_epoch();
    model.set_is_busy(false);
    model.set_progress(-1.0);
    let crown = CrownShift {
        fraction: f64::from(model.get_crown_fraction()),
        scale_by_ratio: model.get_scale_crown_by_ratio(),
        follow_pavilion: model.get_crown_follows_pavilion(),
    };
    let combo_index = model.get_target_material_index();
    let ri_text = model.get_target_ri_override_text();
    let mode_optimize = model.get_mode_index() == 1;

    let prepared = {
        let app = ctx.state.borrow();
        let Some(design_state) = app.design.as_ref() else {
            return;
        };
        let design = &design_state.session.design;
        // An unparseable override is reported, not silently resolved against the
        // design's own material.
        resolve_target_selection(design, &app.custom_materials, combo_index, &ri_text).map(
            |selection| {
                let target = resolved_material_from_selection(&selection, &app.custom_materials);
                let (scope, _blocks) = retarget::retarget_scope(design);
                let anchored = retarget::anchored_tiers_in(design, &scope);
                (
                    selection,
                    target,
                    design_state.session.current_generation(),
                    anchored,
                )
            },
        )
    };
    let (selection, target, generation, anchored) = match prepared {
        Ok(prepared) => prepared,
        Err(message) => {
            view::push_target_error(&model, &message);
            return;
        }
    };
    view::push_target_readout(&model, &selection, &target);
    if mode_optimize {
        if anchored.is_empty() {
            start_search(ctx, epoch, generation, &selection, target, crown);
        } else {
            let refusal = RetargetView {
                anchored_errors: anchored
                    .iter()
                    .map(|(index, name)| format!("#{index} \"{name}\""))
                    .collect(),
                ..RetargetView::default()
            };
            view::push_view(&model, &refusal);
            model.set_can_apply(false);
        }
        return;
    }
    let (built, proposal) = {
        let app = ctx.state.borrow();
        let Some(design_state) = app.design.as_ref() else {
            return;
        };
        retarget_view(
            &design_state.session.design,
            &target,
            crown,
            RetargetMode::Shift,
            &app.custom_materials,
        )
    };
    view::push_view(&model, &built);
    hold(&model, epoch, proposal, generation);
}

/// Keeps `proposal` for Apply (when it is still this epoch's).
fn hold(
    model: &RetargetModel<'_>,
    epoch: u64,
    proposal: Option<RetargetProposal>,
    generation: u64,
) {
    let has = proposal.is_some();
    RUNTIME.with(|rt| {
        let mut rt = rt.borrow_mut();
        if rt.epoch == epoch {
            rt.held = proposal.map(|proposal| Held {
                proposal,
                generation,
            });
        }
    });
    model.set_can_apply(has);
}

/// Sends the Optimize-mode search to the Worker.
fn start_search(
    ctx: &Ctx,
    epoch: u64,
    generation: u64,
    selection: &MaterialSelection,
    target: ResolvedMaterial,
    crown: CrownShift,
) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RetargetModel>();
    if is_solving(ctx) {
        view::push_view(
            &model,
            &RetargetView {
                notes: vec![
                    "The design is still solving -- change any control to start the search when it has finished."
                        .to_string(),
                ],
                ..RetargetView::default()
            },
        );
        model.set_can_apply(false);
        return;
    }
    let (toml, custom) = {
        let app = ctx.state.borrow();
        let Some(design_state) = app.design.as_ref() else {
            return;
        };
        (
            design_to_toml(&design_state.session.design),
            app.custom_materials.clone(),
        )
    };
    let toml = match toml {
        Ok(toml) => toml,
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
    let request = SolveRequest::Retarget {
        params: RetargetParams {
            target: selection.into(),
            crown_fraction: crown.fraction,
            scale_crown_by_ratio: crown.scale_by_ratio,
            crown_follows_pavilion: crown.follow_pavilion,
            mode: RetargetModeData::Optimize,
            optimize: optimize::params_or_default(&ui),
        },
        custom_materials: custom,
    };
    RUNTIME.with(|rt| rt.borrow_mut().searching = true);
    model.set_is_busy(true);
    model.set_can_apply(false);
    model.set_progress_text("Seeding from the critical-angle shift, then searching...".into());
    view::push_view(&model, &RetargetView::default());
    let weak = ctx.ui.clone();
    let future = pool
        .solve()
        .solve_with_progress(toml, request, move |progress| {
            if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
                return;
            }
            if let Some(ui) = weak.upgrade() {
                let model = ui.global::<RetargetModel>();
                model.set_progress_text(progress.message.into());
                model.set_progress(progress.fraction.unwrap_or(-1.0));
            }
        });
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let result = future.await;
        finish_search(&ctx, epoch, generation, &target, result);
    });
}

/// Puts a finished search on screen.
fn finish_search(
    ctx: &Ctx,
    epoch: u64,
    generation: u64,
    target: &ResolvedMaterial,
    result: Result<SolveResponse, SolveError>,
) {
    if RUNTIME.with(|rt| rt.borrow().epoch) != epoch {
        return;
    }
    RUNTIME.with(|rt| rt.borrow_mut().searching = false);
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RetargetModel>();
    model.set_is_busy(false);
    model.set_progress(-1.0);
    match result {
        Ok(SolveResponse::Retargeted(data)) => {
            let proposal = data.to_proposal(target.clone());
            let shown = RetargetView {
                rows: proposal
                    .iter()
                    .flat_map(|p| p.rows.iter().map(retarget::view::row_view))
                    .collect(),
                notes: data.notes,
                anchored_errors: data.anchored_errors,
                solve_error: data.solve_error,
            };
            view::push_view(&model, &shown);
            hold(&model, epoch, proposal, generation);
        }
        Ok(SolveResponse::InvalidDesign { message }) => {
            show_message(
                ctx,
                MessageKind::Error,
                &format!("The solve worker could not read the design: {message}"),
            );
        }
        Ok(_) => {}
        Err(SolveError::Superseded) => {
            view::push_view(
                &model,
                &RetargetView {
                    notes: vec![
                        "The design changed while the search was running -- change any control to search again."
                            .to_string(),
                    ],
                    ..RetargetView::default()
                },
            );
        }
        Err(error) => show_message(
            ctx,
            MessageKind::Error,
            &format!("Retarget search failed: {error}"),
        ),
    }
}

/// Cancel search.
fn cancel_search(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RetargetModel>();
    next_epoch();
    model.set_is_busy(false);
    model.set_progress(-1.0);
    model.set_can_apply(false);
    view::push_view(
        &model,
        &RetargetView {
            notes: vec!["Optimize cancelled.".to_string()],
            ..RetargetView::default()
        },
    );
    // A solve that was queued behind the search has to run again.
    crate::app::solve::auto_solve(ctx);
}

/// Close: drops the proposal and any running search.
fn close(ctx: &Ctx) {
    next_epoch();
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<RetargetModel>();
        model.set_is_open(false);
        model.set_is_busy(false);
    }
}

/// Apply: the angle retarget and the target material together, one undo step -- moving
/// the angles without the material would leave critical-angle-shifted tiers reading as
/// windows against the OLD material until the material was applied separately.
fn apply(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<RetargetModel>();
    let Some(held) = RUNTIME.with(|rt| rt.borrow_mut().held.take()) else {
        return;
    };
    let combo_index = model.get_target_material_index();
    let ri_text = model.get_target_ri_override_text();
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let custom = app.custom_materials.clone();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        if design_state.session.current_generation() == held.generation {
            let design = &design_state.session.design;
            let selection = target_material_selection(design, &custom, combo_index, &ri_text);
            let name = target_display_name(&selection);
            let material_change = (selection != design.material).then_some(selection);
            let angle_edit = retarget::apply(design, &held.proposal);
            let edit = match material_change {
                Some(material) => Edit::Batch(vec![angle_edit, Edit::SetMaterial { material }]),
                None => angle_edit,
            };
            design_state
                .session
                .apply(edit)
                .map(|_| (held.proposal.rows.len(), name))
                .map_err(Some)
        } else {
            Err(None)
        }
    };
    match outcome {
        Ok((applied, name)) => {
            next_epoch();
            model.set_is_open(false);
            finish(ctx, Dirty::All);
            show_message(
                ctx,
                MessageKind::Success,
                &format!("Retargeted {applied} tier(s) for {name}."),
            );
        }
        Err(None) => show_message(
            ctx,
            MessageKind::Error,
            "The design changed since this retarget proposal was built -- open Retarget for material again.",
        ),
        Err(Some(e)) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Registers the dialog's callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<RetargetModel>();
    let c = ctx.clone();
    model.on_open(move || open(&c));
    let c = ctx.clone();
    model.on_close(move || close(&c));
    let c = ctx.clone();
    model.on_proposal_changed(move || rebuild(&c));
    let c = ctx.clone();
    model.on_cancel_optimize(move || cancel_search(&c));
    let c = ctx.clone();
    model.on_apply(move || apply(&c));
}
