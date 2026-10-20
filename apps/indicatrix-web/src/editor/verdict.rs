//! The overall verdict badge (Good / Check / Problem), its popover of reasons and the
//! one-edit Fix, over the shared `indicatrix_editor::verdict` -- the desktop's
//! `gui::editor::verdict`, minus the worker threads.
//!
//! What a verdict IS (thresholds, reasons, wording, fixes) is `indicatrix_editor::verdict`.
//! This module is the glue:
//!
//! - [`refresh`] works out the verdict from the app's state: `gather` reads the design and
//!   the solve that goes with it (no extra solve: the masts are the cached ones), `evaluate`
//!   turns the inputs into a [`Verdict`], and the result is written to `VerdictModel`. It runs
//!   from a 250 ms poll and compares a small key first, so a quiet app costs a few
//!   comparisons and a changed design costs one `gather` (a few milliseconds of geometry).
//!   While the design has no current solve, the verdict on screen stays, dimmed, with its Fix
//!   buttons waiting; with no design or no tiers the badge is hidden.
//! - A Fix press ([`fix_reason`]) asks for the question the verdict carries (a removal), then
//!   plans the edit with `plan_fix`, applies it through the session as ONE undo step and ends
//!   in [`finish_edit`] (refresh, persist, auto-solve). `plan_fix` solves candidates on the
//!   page thread: it runs one event-loop turn after the press, so "Working on the fix..."
//!   paints first, but a very large design can hold the page for a moment (the desktop plans
//!   on a worker thread).
//! - The measured optical figures (windowing, extinction, brilliance) are not gathered: the
//!   desktop measures them on a worker thread, and the page would have to trace on its own
//!   thread. The structural, windowing-by-angle and proportion findings are all here.
//!
//! The reasons name tiers by position, so a Fix runs only while the verdict on screen
//! describes the design as it is now (its generation), exactly the desktop's rule.

use crate::{
    AppWindow, InspectorModel, VerdictModel, VerdictReasonData,
    app::{
        Ctx,
        push::{MessageKind, show_message},
        state::{SolveState, WebApp},
    },
    editor::{
        edit::{Dirty, finish_edit, set_selection},
        selection,
        unsaved::confirm_action,
    },
};
use indicatrix_editor::verdict::{FixAction, FixPlan, Verdict, evaluate, gather, plan_fix};
use slint::{ComponentHandle, ModelRc, Timer, TimerMode, VecModel};
use std::{cell::RefCell, time::Duration};

/// How often the badge checks the app for a design or solve it has not judged yet.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// One event-loop turn: lets "Working on the fix..." paint before the planner holds the page.
const PLAN_DELAY: Duration = Duration::from_millis(30);

/// Said when a Fix is pressed while the verdict on screen no longer describes the design.
const OUT_OF_DATE: &str =
    "The design changed since this was checked. The verdict updates when the solve finishes.";

/// Said when a Fix is pressed while another is still running.
const FIX_RUNNING: &str = "Another fix is still running.";

/// The confirmation dialog's heading for a fix that removes facets.
const CONFIRM_HEADING: &str = "Remove What Is Cut Away?";

/// The confirmation dialog's button for it.
const CONFIRM_BUTTON: &str = "Remove";

/// What the verdict was worked out from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Basis {
    /// No solve of the current design (yet).
    Unsolved,
    /// A solve of the current design that succeeded.
    Solved,
    /// A solve of the current design that failed.
    Failed,
}

/// Everything a verdict is a function of; a changed key means a new verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Key {
    generation: u64,
    basis: Basis,
    n_d_bits: u64,
    custom_materials: usize,
}

/// What the page remembers of the latest verdict.
#[derive(Default)]
struct Slot {
    /// The key last handled, `None` while there is nothing to judge.
    key: Option<Key>,
    /// The verdict on screen.
    verdict: Option<Verdict>,
    /// The design generation that verdict describes.
    generation: Option<u64>,
    /// The refractive index it was worked out for (the windowing fix needs it).
    n_d: f64,
    /// A fix is being planned or applied.
    busy: bool,
}

thread_local! {
    static SLOT: RefCell<Slot> = RefCell::new(Slot::default());
    /// The poll timer (kept alive for the page's lifetime).
    static POLL: Timer = Timer::default();
}

/// The key for the app's current state; `None` when there is nothing to judge (no design, or
/// no tiers yet).
fn current_key(app: &WebApp) -> Option<Key> {
    let state = app.design.as_ref()?;
    let design = &state.session.design;
    if design.tiers.is_empty() {
        return None;
    }
    let generation = state.session.current_generation();
    let basis = match &app.solve {
        SolveState::Solved { generation: g, .. } if *g == generation => Basis::Solved,
        SolveState::Failed { generation: g, .. } if *g == generation => Basis::Failed,
        _ => Basis::Unsolved,
    };
    Some(Key {
        generation,
        basis,
        n_d_bits: design
            .effective_refractive_index_with(&app.custom_materials)
            .to_bits(),
        custom_materials: app.custom_materials.len(),
    })
}

/// The verdict for the app's current solve and the refractive index it used, `None` without a
/// current solve (solved or failed).
fn compute(app: &WebApp) -> Option<(Verdict, f64)> {
    let state = app.design.as_ref()?;
    let design = &state.session.design;
    let generation = state.session.current_generation();
    let n_d = design.effective_refractive_index_with(&app.custom_materials);
    let (solved, failure) = match &app.solve {
        SolveState::Solved {
            generation: g,
            tiers,
            ..
        } if *g == generation => (Some(tiers.as_slice()), None),
        SolveState::Failed {
            generation: g,
            message,
        } if *g == generation => (None, Some(message.as_str())),
        _ => return None,
    };
    let inputs = gather(design, solved, n_d, failure);
    Some((evaluate(&inputs), n_d))
}

/// One reason as the popover shows it.
fn reason_rows(verdict: &Verdict) -> Vec<VerdictReasonData> {
    verdict
        .reasons
        .iter()
        .map(|reason| VerdictReasonData {
            level: reason.level.code(),
            text: reason.text.as_str().into(),
            tier: reason
                .tier
                .and_then(|tier| i32::try_from(tier).ok())
                .unwrap_or(-1),
            fix_label: reason
                .fix
                .as_ref()
                .map(FixAction::label)
                .unwrap_or_default()
                .into(),
            fix_hint: reason
                .fix
                .as_ref()
                .map(FixAction::hint)
                .unwrap_or_default()
                .into(),
        })
        .collect()
}

/// Shows `verdict` (and clears the out-of-date mark).
fn push_verdict(ui: &AppWindow, verdict: &Verdict) {
    let model = ui.global::<VerdictModel>();
    model.set_level(verdict.level.code());
    model.set_word(verdict.level.word().into());
    model.set_headline(verdict.headline.as_str().into());
    model.set_reasons(ModelRc::new(VecModel::from(reason_rows(verdict))));
    model.set_stale(false);
}

/// Hides the badge: there is nothing to judge.
fn push_hidden(ui: &AppWindow) {
    let model = ui.global::<VerdictModel>();
    model.set_level(-1);
    model.set_word("".into());
    model.set_headline("".into());
    model.set_reasons(ModelRc::new(
        VecModel::from(Vec::<VerdictReasonData>::new()),
    ));
    model.set_stale(false);
    model.set_busy(false);
    model.set_open(false);
}

/// Brings the badge up to date with the app: a new verdict when a solve of the current design
/// is there to judge, the old one dimmed while it is not, nothing when there is no design or
/// no tiers. Cheap when nothing changed. Call with no `RefCell` borrow of the app held.
pub fn refresh(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let key = current_key(&ctx.state.borrow());
    if SLOT.with(|cell| cell.borrow().key == key) {
        return;
    }
    let Some(key) = key else {
        SLOT.with(|cell| {
            let busy = cell.borrow().busy;
            *cell.borrow_mut() = Slot {
                busy,
                ..Slot::default()
            };
        });
        push_hidden(&ui);
        return;
    };
    let judged = (key.basis != Basis::Unsolved)
        .then(|| compute(&ctx.state.borrow()))
        .flatten();
    if let Some((verdict, n_d)) = judged {
        push_verdict(&ui, &verdict);
        SLOT.with(|cell| {
            let mut slot = cell.borrow_mut();
            slot.key = Some(key);
            slot.verdict = Some(verdict);
            slot.generation = Some(key.generation);
            slot.n_d = n_d;
        });
    } else {
        // No current solve: the verdict on screen (if any) describes an older design.
        SLOT.with(|cell| cell.borrow_mut().key = Some(key));
        ui.global::<VerdictModel>().set_stale(true);
    }
}

/// Whether a Fix may run against the design as it is now.
///
/// # Errors
///
/// The sentence to toast: another fix is running, or the verdict on screen does not describe
/// the current design.
fn ensure_current(ctx: &Ctx) -> Result<(), String> {
    let current = ctx.state.borrow().generation();
    SLOT.with(|cell| {
        let slot = cell.borrow();
        if slot.busy {
            Err(FIX_RUNNING.to_string())
        } else if slot.generation.is_none() || slot.generation != current {
            Err(OUT_OF_DATE.to_string())
        } else {
            Ok(())
        }
    })
}

/// The fix behind reason `index` of the verdict on screen, and the question to ask before
/// applying it (`Some` where it removes something).
///
/// # Errors
///
/// The sentence to toast: the verdict is out of date, another fix is running, or the reason has
/// no fix.
fn pending_fix(ctx: &Ctx, index: i32) -> Result<(FixAction, Option<String>), String> {
    ensure_current(ctx)?;
    SLOT.with(|cell| {
        usize::try_from(index)
            .ok()
            .and_then(|index| {
                cell.borrow()
                    .verdict
                    .as_ref()
                    .and_then(|verdict| verdict.reasons.get(index))
                    .and_then(|reason| reason.fix.clone().map(|fix| (fix, reason.confirm.clone())))
            })
            .ok_or_else(|| "There is no automatic fix for this one.".to_string())
    })
}

/// Marks a fix as running (or done) on the slot and the model.
fn set_busy(ctx: &Ctx, busy: bool) {
    SLOT.with(|cell| cell.borrow_mut().busy = busy);
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<VerdictModel>().set_busy(busy);
    }
}

/// `VerdictModel.fix-reason(index)`: the Fix button of reason `index` was pressed.
fn fix_reason(ctx: &Ctx, index: i32) {
    match pending_fix(ctx, index) {
        Ok((action, Some(question))) => {
            let c = ctx.clone();
            confirm_action(ctx, CONFIRM_HEADING, &question, CONFIRM_BUTTON, move || {
                start_fix(&c, action);
            });
        }
        Ok((action, None)) => start_fix(ctx, action),
        Err(message) => show_message(ctx, MessageKind::Info, &message),
    }
}

/// Plans `action` one event-loop turn from now and applies the plan.
fn start_fix(ctx: &Ctx, action: FixAction) {
    // The confirmation dialog may have been open a while: check again.
    if let Err(message) = ensure_current(ctx) {
        show_message(ctx, MessageKind::Info, &message);
        return;
    }
    set_busy(ctx, true);
    show_message(ctx, MessageKind::Info, "Working on the fix...");
    let c = ctx.clone();
    Timer::single_shot(PLAN_DELAY, move || run_fix(&c, &action));
}

/// Plans `action` against the design as it is and applies the plan when it still fits.
fn run_fix(ctx: &Ctx, action: &FixAction) {
    let n_d = SLOT.with(|cell| cell.borrow().n_d);
    let planned = {
        let app = ctx.state.borrow();
        app.design.as_ref().map(|state| {
            (
                state.session.current_generation(),
                plan_fix(&state.session.design, action, n_d),
            )
        })
    };
    set_busy(ctx, false);
    match planned {
        None => {}
        Some((generation, Ok(plan))) => apply_plan(ctx, generation, plan),
        Some((_, Err(message))) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Applies `plan` (made for design generation `generation`) as one undo step.
fn apply_plan(ctx: &Ctx, generation: u64, plan: FixPlan) {
    let FixPlan {
        edit,
        message,
        select,
    } = plan;
    let outcome = {
        let mut guard = ctx.state.borrow_mut();
        let app = &mut *guard;
        let Some(state) = app.design.as_mut() else {
            return;
        };
        if state.session.current_generation() == generation {
            let flat_before = state.session.design.tiers.len();
            match state.session.try_apply_mapped(edit, None) {
                Ok((_, renumbering)) => {
                    let design = &state.session.design;
                    let followed = app.selected_tier.and_then(|row| {
                        renumbering.map_table_row(
                            row,
                            flat_before,
                            design.tiers.len(),
                            design.concave_tiers.len(),
                        )
                    });
                    set_selection(app, select.or(followed));
                    Ok(())
                }
                Err(error) => Err(error.to_string()),
            }
        } else {
            Err("The design changed while the fix was being planned. Nothing was changed.".into())
        }
    };
    match outcome {
        Ok(()) => {
            finish_edit(ctx, Dirty::All);
            show_message(
                ctx,
                MessageKind::Success,
                &format!("{message} Undo (Ctrl+Z) puts it back."),
            );
        }
        Err(text) => show_message(ctx, MessageKind::Error, &text),
    }
}

/// The popover's "Show": selects the tier row and opens the inspector's Tier tab on it.
fn show_tier(ctx: &Ctx, tier: i32) {
    selection::select(ctx, tier);
    if let Some(ui) = ctx.ui.upgrade() {
        let inspector = ui.global::<InspectorModel>();
        inspector.set_tab(0);
        inspector.set_collapsed(false);
    }
}

/// Wires the verdict badge's callbacks and starts its poll.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<VerdictModel>();
    let c = ctx.clone();
    model.on_fix_reason(move |index| fix_reason(&c, index));
    let c = ctx.clone();
    model.on_show_tier(move |tier| show_tier(&c, tier));
    let poll = ctx.clone();
    POLL.with(|timer| {
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || refresh(&poll));
    });
}
