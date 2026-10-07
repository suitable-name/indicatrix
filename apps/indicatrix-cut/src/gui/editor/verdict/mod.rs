//! The overall verdict on the open design: the Good / Check / Problem badge in the Edit
//! tab's status strip, its popover of reasons and the one-click fixes.
//!
//! What a verdict IS (the thresholds, the reasons, the wording, the fixes) is
//! `indicatrix_editor::verdict`, shared with the web app. This group is the desktop's glue:
//!
//! - [`on_solved`] is the one entry point. It is called wherever a solve's numbers reach the
//!   panel ([`super::view::push_proportion_verdicts_from_solved`]: the synchronous Solve, the
//!   background solve's completion and a finished solid-preview replan), so the verdict is
//!   recomputed exactly when a solve lands and never on its own. It costs a copy of the design
//!   and its masts; the work (the geometry checks and one fast optical measurement) runs on a
//!   worker thread after a short debounce, so a handle drag that replans many times a second
//!   starts one worker, not one per frame.
//! - [`mark_stale`] is called from the stale-panel refresh every edit ends in: the verdict on
//!   screen is dimmed and its Fix buttons wait until the next solve lands.
//! - [`work`] is the worker, [`present`] writes `VerdictModel`, [`apply`] answers the Fix
//!   buttons (plan on a worker, apply as one undo step, toast).
//!
//! # State
//!
//! A `thread_local!` [`SLOT`], like the Retarget check's: Slint's event loop is
//! single-threaded and the workers only hand a result back through the event loop. Every
//! request and every edit bumps [`Slot::serial`], so a result for an older design is dropped.
//! [`Slot::tiers`] remembers the tiers the verdict describes, which is how a Fix refuses to
//! run on a design that changed since (a reason names tiers by position).

mod apply;
mod present;
#[cfg(test)]
mod tests;
mod work;

use self::work::Job;
use super::state::EditorState;
use crate::{
    EditorModel, MainWindow, VerdictModel, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
};
use indicatrix::{geometry::meet_solver::SolvedTier, optics::LightingPreset};
use indicatrix_cut_core::{ConstraintTier, Design};
use indicatrix_editor::verdict::{FixAction, SolveFacts, Verdict, VerdictInputs, evaluate};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex, TryLockError},
    time::Duration,
};

/// How long a request waits for a newer one before its worker starts: long enough to skip the
/// replans of a handle drag, short enough not to be felt after a click.
const REQUEST_DEBOUNCE: Duration = Duration::from_millis(150);

/// Said when a Fix is pressed while the verdict on screen no longer describes the design.
const OUT_OF_DATE: &str =
    "The design changed since this was checked. The verdict updates when the solve finishes.";

/// Said when a Fix is pressed while another is still running.
const FIX_RUNNING: &str = "Another fix is still running.";

/// The handles the Fix callback needs, kept for the life of the window.
struct Services {
    editor: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    preview_state: Arc<SolidPreviewState>,
    solid_last_solved: SolidLastSolved,
}

/// What the UI thread remembers of the latest verdict.
#[derive(Default)]
struct Slot {
    /// Bumped by every request and every edit. A worker result with another serial is dropped.
    serial: u64,
    /// The verdict on screen, if any.
    verdict: Option<Verdict>,
    /// The tiers of the design that verdict describes.
    tiers: Vec<ConstraintTier>,
    /// The refractive index the verdict was worked out for (the windowing fix needs it).
    n_d: f64,
    /// `true` when the verdict on screen describes the design as it stands: no edit and no
    /// request since it landed.
    current: bool,
    /// A fix is being planned or applied.
    busy: bool,
}

thread_local! {
    /// The window's handles, set once by [`setup_verdict`].
    static SERVICES: RefCell<Option<Rc<Services>>> = const { RefCell::new(None) };
    static SLOT: RefCell<Slot> = RefCell::new(Slot::default());
    /// Holds the next worker back until requests stop arriving for [`REQUEST_DEBOUNCE`].
    static DEBOUNCE: Timer = Timer::default();
}

fn services() -> Option<Rc<Services>> {
    SERVICES.with(|cell| cell.borrow().clone())
}

/// Registers `VerdictModel`'s Fix callback and keeps the handles it needs. The badge stays
/// hidden until the first solve lands.
pub(super) fn setup_verdict(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    SERVICES.with(|cell| {
        *cell.borrow_mut() = Some(Rc::new(Services {
            editor: Rc::clone(state),
            render_ctx: Arc::clone(render_ctx),
            preview_state: Arc::clone(preview_state),
            solid_last_solved: Arc::clone(solid_last_solved),
        }));
    });
    let ui_weak = ui.as_weak();
    ui.global::<VerdictModel>().on_fix_reason(move |index| {
        if let Some(ui) = ui_weak.upgrade() {
            apply::fix_reason(&ui, index);
        }
    });
}

/// A solve's numbers reached the panel: asks for a new verdict on `design`.
///
/// `solved` is the masts of its solve (`None` when it did not solve) and `n_d` its effective
/// refractive index.
///
/// Cheap on the UI thread (it copies the design and the masts and restarts a timer); the work
/// happens on a worker once requests pause for [`REQUEST_DEBOUNCE`]. Until that worker lands
/// the verdict on screen is shown as out of date.
pub(super) fn on_solved(ui: &MainWindow, design: &Design, solved: Option<&[SolvedTier]>, n_d: f64) {
    let Some(services) = services() else {
        return;
    };
    if design.tiers.is_empty() {
        reset(ui);
        return;
    }
    let serial = SLOT.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.serial += 1;
        slot.current = false;
        slot.tiers.clone_from(&design.tiers);
        slot.n_d = n_d;
        slot.serial
    });
    present::set_stale(ui, true);

    // The custom materials, only to find the design's material for the optical figures. A
    // render thread holding the context just means this verdict comes without them: the UI
    // thread never waits on it.
    let custom = match services.render_ctx.try_lock() {
        Ok(ctx) => Some(Arc::clone(&ctx.custom_materials)),
        Err(TryLockError::Poisoned(poisoned)) => {
            Some(Arc::clone(&poisoned.into_inner().custom_materials))
        }
        Err(TryLockError::WouldBlock) => None,
    };
    let job = RefCell::new(Some(Job {
        design: design.clone(),
        solved: solved.map(<[SolvedTier]>::to_vec),
        n_d,
        lighting: LightingPreset::from_index(
            ui.global::<ViewportModel>().get_selected_lighting_index(),
        ),
        custom,
        serial,
        ui_weak: ui.as_weak(),
    }));
    DEBOUNCE.with(|timer| {
        timer.start(TimerMode::SingleShot, REQUEST_DEBOUNCE, move || {
            let pending = job.borrow_mut().take();
            if let Some(job) = pending {
                work::spawn(job);
            }
        });
    });
}

/// An edit landed: the verdict on screen no longer describes the design. It stays visible,
/// dimmed, with its Fix buttons waiting; a worker still running for the older design is
/// dropped. The next solve brings the new verdict through [`on_solved`].
pub(super) fn mark_stale(ui: &MainWindow) {
    SLOT.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.serial += 1;
        slot.current = false;
    });
    DEBOUNCE.with(Timer::stop);
    present::set_stale(ui, true);
}

/// Forgets the verdict and hides the badge (the design has no tiers).
fn reset(ui: &MainWindow) {
    SLOT.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.serial += 1;
        slot.verdict = None;
        slot.tiers.clear();
        slot.current = false;
    });
    DEBOUNCE.with(Timer::stop);
    present::push_hidden(ui);
}

/// The sentence for a design that did not solve, from the editor's own status line.
///
/// Only when that line says the solve failed: `None` while it is the "not solved yet" marker
/// or empty.
fn failure_text(solve_state: &str, status_text: &str) -> Option<String> {
    let text = status_text.trim();
    (solve_state == "failed" && !text.is_empty()).then(|| text.to_string())
}

/// A worker's result, back on the UI thread. Dropped when `serial` is no longer current.
///
/// `had_solution` says whether the worker was given a mast list; without one the failure
/// sentence is the editor's status line, which the background solve sets AFTER it pushes the
/// numbers (the reason this is read here and not when the request is made).
fn finish(ui: &MainWindow, serial: u64, had_solution: bool, inputs: Option<VerdictInputs>) {
    if SLOT.with(|cell| cell.borrow().serial) != serial {
        return;
    }
    // The worker stopped unexpectedly: the verdict on screen stays, marked out of date.
    let Some(mut inputs) = inputs else {
        return;
    };
    if inputs.is_empty() {
        reset(ui);
        return;
    }
    if !had_solution && let SolveFacts::DoesNotSolve(why) = &mut inputs.solve {
        let model = ui.global::<EditorModel>();
        if let Some(text) = failure_text(
            model.get_solve_state().as_str(),
            model.get_status_text().as_str(),
        ) {
            *why = text;
        }
    }
    let verdict = evaluate(&inputs);
    SLOT.with(|cell| {
        let mut slot = cell.borrow_mut();
        slot.verdict = Some(verdict.clone());
        slot.current = true;
    });
    present::push_verdict(ui, &verdict);
}

/// Whether a Fix may run against a design with `tiers`: the verdict on screen must describe
/// it, and no other fix may be running.
///
/// # Errors
///
/// The sentence to toast.
fn ensure_current(tiers: &[ConstraintTier]) -> Result<(), String> {
    SLOT.with(|cell| {
        let slot = cell.borrow();
        if slot.busy {
            Err(FIX_RUNNING.to_string())
        } else if !slot.current || slot.tiers != tiers {
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
/// The sentence to toast: the verdict is out of date for the design with `tiers`, another fix
/// is running, or the reason has no fix.
fn pending_fix(
    index: usize,
    tiers: &[ConstraintTier],
) -> Result<(FixAction, Option<String>), String> {
    ensure_current(tiers)?;
    SLOT.with(|cell| {
        cell.borrow()
            .verdict
            .as_ref()
            .and_then(|verdict| verdict.reasons.get(index))
            .and_then(|reason| reason.fix.clone().map(|fix| (fix, reason.confirm.clone())))
            .ok_or_else(|| "There is no automatic fix for this one.".to_string())
    })
}
