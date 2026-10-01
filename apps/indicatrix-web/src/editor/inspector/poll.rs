//! The refresh: a fingerprint of everything the panels show, a 100 ms poll that pushes
//! when it changed, and [`refresh`], which pushes at once.

use super::{PushCtx, form, preform, readouts};
use crate::{
    DesignSettingsModel, InspectorModel,
    app::{
        Ctx,
        state::{SolveState, WebApp},
    },
    editor::{optimize, settings},
};
use indicatrix_editor::scratch::{PushedScratch, ScratchDelta};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, time::Duration};

/// How often the panels check the app state for changes they did not cause.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// What the design looked like at the last push.
#[derive(Debug, Clone, PartialEq, Eq)]
struct DesignPrint {
    generation: u64,
    dirty: bool,
    tiers: usize,
    can_undo: bool,
    can_redo: bool,
}

/// Which solve, if any, the app holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SolvePrint {
    None,
    Solved(u64),
    Failed(u64),
}

/// Everything a push depends on that can change without these panels causing it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Fingerprint {
    design: Option<DesignPrint>,
    selected: Option<usize>,
    solve: SolvePrint,
    custom_materials: usize,
    tab: i32,
    collapsed: bool,
    settings_open: bool,
}

/// The poll's memory.
#[derive(Default)]
struct Panels {
    last: Option<Fingerprint>,
    scratch: PushedScratch,
}

thread_local! {
    static PANELS: RefCell<Panels> = RefCell::new(Panels::default());
    static POLL: Timer = Timer::default();
}

const fn solve_print(app: &WebApp) -> SolvePrint {
    match &app.solve {
        SolveState::NotSolved => SolvePrint::None,
        SolveState::Solved { generation, .. } => SolvePrint::Solved(*generation),
        SolveState::Failed { generation, .. } => SolvePrint::Failed(*generation),
    }
}

fn fingerprint(ctx: &Ctx) -> Option<Fingerprint> {
    let ui = ctx.ui.upgrade()?;
    let app = ctx.state.borrow();
    let inspector = ui.global::<InspectorModel>();
    Some(Fingerprint {
        design: app.design.as_ref().map(|d| DesignPrint {
            generation: d.session.current_generation(),
            dirty: d.session.is_dirty(),
            tiers: d.session.design.tiers.len(),
            can_undo: d.session.history.can_undo(),
            can_redo: d.session.history.can_redo(),
        }),
        selected: app.selected_tier,
        solve: solve_print(&app),
        custom_materials: app.custom_materials.len(),
        tab: inspector.get_tab(),
        collapsed: inspector.get_collapsed(),
        settings_open: ui.global::<DesignSettingsModel>().get_open(),
    })
}

/// Starts the poll.
pub fn start(ctx: &Ctx) {
    let ctx = ctx.clone();
    POLL.with(|timer| {
        timer.start(TimerMode::Repeated, POLL_INTERVAL, move || poll(&ctx));
    });
}

fn poll(ctx: &Ctx) {
    let Some(now) = fingerprint(ctx) else {
        return;
    };
    let unchanged = PANELS.with(|p| p.borrow().last.as_ref() == Some(&now));
    if !unchanged {
        push(ctx, &now);
    }
}

/// Pushes every panel from the current state at once (the poll would get there within
/// 100 ms).
pub fn refresh(ctx: &Ctx) {
    if let Some(now) = fingerprint(ctx) {
        push(ctx, &now);
    }
}

/// Whether the design in `now` is a wholesale replacement of the one in `previous`: a
/// design with no undo history that reads clean and whose generation moved (New, Open and
/// restore continue the generation counter, mark the new session saved and start an empty
/// history; an edit leaves undo history, an undo leaves it dirty).
fn is_replacement(previous: Option<&Fingerprint>, now: &Fingerprint) -> bool {
    let Some(design) = &now.design else {
        return false;
    };
    let Some(before) = previous.and_then(|p| p.design.as_ref()) else {
        return true;
    };
    before.generation != design.generation && !design.can_undo && !design.can_redo && !design.dirty
}

fn push(ctx: &Ctx, now: &Fingerprint) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let previous = PANELS.with(|p| p.borrow_mut().last.replace(now.clone()));
    if ctx.state.borrow().design.is_none() {
        PANELS.with(|p| p.borrow_mut().scratch = PushedScratch::default());
        form::clear_all(&ui);
        settings::clear(&ui);
        optimize::clear(&ui);
        return;
    }
    with_pcx(ctx, previous.as_ref(), now, |pcx| {
        form::push(pcx);
        readouts::push(pcx);
        preform::push(pcx);
        settings::push(pcx);
        optimize::push(pcx);
    });
}

/// Runs `f` on the current state as a [`PushCtx`], for a callback that needs the rows or
/// the selection the way a push does (no "previous" push: nothing counts as changed).
/// `None` without a window or a design.
pub fn with_current<R>(ctx: &Ctx, f: impl FnOnce(&PushCtx<'_>) -> R) -> Option<R> {
    let now = fingerprint(ctx)?;
    build(ctx, None, &now, false, f)
}

fn with_pcx<R>(
    ctx: &Ctx,
    previous: Option<&Fingerprint>,
    now: &Fingerprint,
    f: impl FnOnce(&PushCtx<'_>) -> R,
) -> Option<R> {
    build(ctx, previous, now, true, f)
}

/// Builds the [`PushCtx`] for `now` (recording the scratch snapshot when `record`).
fn build<R>(
    ctx: &Ctx,
    previous: Option<&Fingerprint>,
    now: &Fingerprint,
    record: bool,
    f: impl FnOnce(&PushCtx<'_>) -> R,
) -> Option<R> {
    let ui = ctx.ui.upgrade()?;
    let app = ctx.state.borrow();
    let design_state = app.design.as_ref()?;
    let replaced = record && is_replacement(previous, now);
    let design = &design_state.session.design;
    let delta = if record {
        PANELS.with(|p| {
            let mut panels = p.borrow_mut();
            if replaced {
                panels.scratch = PushedScratch::default();
            }
            panels.scratch.record(design)
        })
    } else {
        // A callback's view: no group counts as changed.
        ScratchDelta {
            material: false,
            gear: false,
            symmetry: false,
            preform: false,
            girdle: false,
            meta: false,
        }
    };
    let previous_design = previous.and_then(|p| p.design.as_ref());
    let pcx = PushCtx {
        ui: &ui,
        app: &app,
        design_state,
        design,
        delta: &delta,
        solved: app.current_solved(),
        n_d: design.effective_refractive_index_with(&app.custom_materials),
        custom: &app.custom_materials,
        replaced,
        previous_selection: if replaced {
            None
        } else {
            previous.and_then(|p| p.selected)
        },
        previous_generation: if replaced {
            None
        } else {
            previous_design.map(|d| d.generation)
        },
        previous_tiers: previous_design.map_or(0, |d| d.tiers),
        tab: now.tab,
        collapsed: now.collapsed,
        rows_cell: std::cell::OnceCell::new(),
    };
    Some(f(&pcx))
}
