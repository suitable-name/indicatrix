//! Cutting mode on the desktop: the full-window, one-step-per-page view of the cutting
//! instructions, with a done mark and index ticks per design.
//!
//! The pure model -- the steps in sheet order, their keys and fingerprints, the marks and how
//! they count, the page texts and the index wheel -- is `indicatrix_editor::cutting_mode`. This
//! module is the window side:
//!
//! - **opening** (`CuttingModeModel.open_cutting_mode`, from the command bar, Edit > Cutting
//!   Mode... and the palette): the design is solved and its steps built on a worker thread, then
//!   the marks are read from the library and the page opens at the first step not done;
//! - **stepping** (arrow keys, buttons): moves the page, and [`viewport`] shows the stone after
//!   that step and highlights its tier;
//! - **marking** (Space or D, the button, a click on an index chip): stores the change in the
//!   library ([`store`]) before the page shows it, so what the page shows is what is stored;
//! - **design changes** (Undo, Open, an edit made while the screen is up): a timer compares the
//!   design's identity and generation with the ones the steps were built from and rebuilds, staying
//!   on the same tier when it still exists;
//! - **closing** puts the Cut slider, the selection and the view mode back.
//!
//! Everything runs on the UI thread except the solve. The state lives in a thread-local session
//! (the same way the editor's other per-window state does), and no `EditorState` borrow is held
//! across a call into Slint.

mod push;
mod store;
mod viewport;

use super::state::EditorState;
use crate::{
    CuttingModeModel, MainWindow,
    gui::{show_toast, tutorial_events::raise},
};
use indicatrix_cut_core::Design;
use indicatrix_editor::{
    cutting_mode::{
        CuttingPlan, build_plan,
        progress::{MarkChange, Progress, next_position, previous_position},
    },
    guide::viewing_events as events,
};
use indicatrix_vault::{db::sqlite::Database, model::design_key::normalize_design_uuid};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{
    cell::RefCell,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};

/// How often the open screen checks that the design is still the one its steps came from.
const WATCH_INTERVAL: Duration = Duration::from_millis(400);

/// What names the design the steps were built from: if any part differs, the steps are stale.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Watch {
    /// The design's library UUID (normalised).
    uuid: String,
    /// `EditorSession::current_generation`: bumped by every edit, undo and redo.
    generation: u64,
    /// `EditorState::design_epoch`: bumped when a different design replaces this one.
    epoch: u64,
}

/// The editor state and the library, as the callbacks and the timer reach them.
type EditorHandles = (Rc<RefCell<EditorState>>, Arc<Mutex<Database>>);

/// The handles the callbacks and the timer work through.
struct Handles {
    state: Rc<RefCell<EditorState>>,
    db: Arc<Mutex<Database>>,
}

/// What this window remembers while cutting mode is open.
#[derive(Default)]
struct Session {
    handles: Option<Handles>,
    /// Counts the preparations started; a result for an older one is dropped.
    ticket: u64,
    /// A preparation is running on the worker thread.
    preparing: bool,
    /// The steps, once built.
    plan: Option<CuttingPlan>,
    /// The marks of the design, as stored.
    progress: Progress,
    /// The step showing.
    position: usize,
    /// What the steps were built from.
    watch: Option<Watch>,
    /// The step key to stay on when the steps are rebuilt (a refresh), else the page opens at
    /// the first step not done.
    resume_key: Option<String>,
    /// The viewport as it was before cutting mode, and the design epoch and UUID it was for.
    saved: Option<(viewport::Saved, String, u64)>,
    /// The timer that watches for design changes; alive while the screen is open.
    timer: Option<Timer>,
}

thread_local! {
    /// The window's session. Slint callbacks and the editor run on one thread, so a
    /// thread-local needs no lock.
    static SESSION: RefCell<Session> = RefCell::new(Session::default());
}

fn with_session<R>(f: impl FnOnce(&mut Session) -> R) -> R {
    SESSION.with(|cell| f(&mut cell.borrow_mut()))
}

fn handles() -> Option<EditorHandles> {
    with_session(|session| {
        session
            .handles
            .as_ref()
            .map(|h| (Rc::clone(&h.state), Arc::clone(&h.db)))
    })
}

/// Wires the cutting mode screen's callbacks and remembers the handles it works through.
/// Called once, when the editor is set up.
pub(super) fn setup_cutting_mode(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    db: &Arc<Mutex<Database>>,
) {
    with_session(|session| {
        session.handles = Some(Handles {
            state: Rc::clone(state),
            db: Arc::clone(db),
        });
    });
    let model = ui.global::<CuttingModeModel>();

    let weak = ui.as_weak();
    model.on_open_requested(move || {
        if let Some(ui) = weak.upgrade() {
            open(&ui);
        }
    });
    let weak = ui.as_weak();
    model.on_closed(move || {
        if let Some(ui) = weak.upgrade() {
            closed(&ui);
        }
    });
    let weak = ui.as_weak();
    model.on_previous(move || {
        if let Some(ui) = weak.upgrade() {
            let target = with_session(|session| previous_position(session.position));
            go_to(&ui, target);
        }
    });
    let weak = ui.as_weak();
    model.on_next(move || {
        if let Some(ui) = weak.upgrade() {
            go_to(&ui, next_step());
        }
    });
    let weak = ui.as_weak();
    model.on_jump_to(move |step| {
        if let (Some(ui), Ok(step)) = (weak.upgrade(), usize::try_from(step)) {
            let last = with_session(|session| session.plan.as_ref().map(|plan| plan.steps.len()));
            if last.is_some_and(|count| step < count) {
                go_to(&ui, Some(step));
            }
        }
    });
    let weak = ui.as_weak();
    model.on_toggle_done(move || {
        if let Some(ui) = weak.upgrade() {
            let before = done_count();
            let stored = change_marks(&ui, |plan, progress, position| {
                plan.steps
                    .get(position)
                    .map_or_else(Vec::new, |step| progress.toggle_done(step))
            });
            if stored {
                show_current(&ui);
                report_marked(&ui, before);
            }
        }
    });
    let weak = ui.as_weak();
    model.on_mark_done_and_next(move || {
        if let Some(ui) = weak.upgrade() {
            let before = done_count();
            let stored = change_marks(&ui, |plan, progress, position| {
                plan.steps
                    .get(position)
                    .map_or_else(Vec::new, |step| progress.mark_done(step))
            });
            if stored {
                if let Some(target) = next_step() {
                    with_session(|session| session.position = target);
                }
                show_current(&ui);
                report_marked(&ui, before);
            }
        }
    });
    let weak = ui.as_weak();
    model.on_chip_clicked(move |chip| {
        let (Some(ui), Ok(chip)) = (weak.upgrade(), usize::try_from(chip)) else {
            return;
        };
        let before = done_count();
        let stored = change_marks(&ui, |plan, progress, position| {
            plan.steps
                .get(position)
                .map_or_else(Vec::new, |step| progress.toggle_chip(step, chip))
        });
        if stored {
            show_current(&ui);
            report_marked(&ui, before);
        }
    });
    let weak = ui.as_weak();
    model.on_reset_confirmed(move || {
        if let Some(ui) = weak.upgrade() {
            reset(&ui);
        }
    });
}

// ---------------------------------------------------------------------------------------
// Opening and rebuilding.
// ---------------------------------------------------------------------------------------

/// The design as the editor holds it right now, or why it cannot be read.
enum Snapshot {
    /// The editor state is in use by something else this instant.
    Busy,
    /// No design is open.
    NoDesign,
    /// The design has no library identity to file marks under.
    Unidentified,
    /// A copy of the design and what names it.
    Ready(Box<Design>, Watch),
}

fn snapshot(state: &RefCell<EditorState>) -> Snapshot {
    let Ok(st) = state.try_borrow() else {
        return Snapshot::Busy;
    };
    if !st.has_design {
        return Snapshot::NoDesign;
    }
    let Some(uuid) = normalize_design_uuid(st.design_uuid()) else {
        return Snapshot::Unidentified;
    };
    let watch = Watch {
        uuid,
        generation: st.current_generation(),
        epoch: st.design_epoch.load(Ordering::Relaxed),
    };
    Snapshot::Ready(Box::new(st.design.clone()), watch)
}

/// What names the design now, or `None` while the editor state is in use or there is no
/// identified design -- a tick of the watcher then simply waits for the next one.
fn current_watch(state: &RefCell<EditorState>) -> Option<Watch> {
    let st = state.try_borrow().ok()?;
    if !st.has_design {
        return None;
    }
    Some(Watch {
        uuid: normalize_design_uuid(st.design_uuid())?,
        generation: st.current_generation(),
        epoch: st.design_epoch.load(Ordering::Relaxed),
    })
}

/// Shows `message` on the screen in place of a page.
fn fail(ui: &MainWindow, message: &str) {
    let model = ui.global::<CuttingModeModel>();
    model.set_error(message.into());
    model.set_busy(false);
}

/// The screen has just opened for a design that can be stepped through: take note of the
/// viewport, then build the steps.
fn open(ui: &MainWindow) {
    let Some((state, _)) = handles() else {
        fail(ui, "Cutting mode is not ready yet. Try again in a moment.");
        return;
    };
    if let Some(watch) = current_watch(&state) {
        let saved = viewport::capture(ui);
        with_session(|session| {
            session.saved = Some((saved, watch.uuid, watch.epoch));
            session.resume_key = None;
            session.plan = None;
            session.progress = Progress::default();
            session.position = 0;
            session.watch = None;
        });
    }
    prepare_steps(ui);
}

/// Builds the steps of the design now open, on a worker thread, and shows the page when they
/// are ready. A result that a newer request or the closing screen has overtaken is dropped.
fn prepare_steps(ui: &MainWindow) {
    let Some((state, _)) = handles() else {
        fail(ui, "Cutting mode is not ready yet. Try again in a moment.");
        return;
    };
    let (design, watch) = match snapshot(&state) {
        Snapshot::Ready(design, watch) => (design, watch),
        Snapshot::Busy => {
            fail(
                ui,
                "The editor is busy right now. Close this and try again in a moment.",
            );
            return;
        }
        Snapshot::NoDesign => {
            fail(ui, "Open or create a design first.");
            return;
        }
        Snapshot::Unidentified => {
            fail(
                ui,
                "This design has no library identity yet, so its progress could not be kept. Save it once and try again.",
            );
            return;
        }
    };
    let ticket = with_session(|session| {
        session.ticket += 1;
        session.preparing = true;
        session.ticket
    });
    let weak = ui.as_weak();
    let spawned = std::thread::Builder::new()
        .name("cutting-mode-steps".to_owned())
        .spawn(move || {
            let outcome =
                catch_unwind(AssertUnwindSafe(|| prepare(&design))).unwrap_or_else(|_| {
                    Err("Something went wrong while preparing the cutting steps.".to_owned())
                });
            // The window may be gone by now; then there is nobody to tell.
            let _ = weak.upgrade_in_event_loop(move |ui| prepared(&ui, ticket, watch, outcome));
        });
    if spawned.is_err() {
        with_session(|session| session.preparing = false);
        fail(ui, "The cutting steps could not be prepared.");
    }
}

/// Solves `design` and builds its steps. Runs on the worker thread: the solve is the slow part.
fn prepare(design: &Design) -> Result<CuttingPlan, String> {
    let solved = design.solve().map_err(|err| {
        format!(
            "This design does not solve yet ({err}). Fix the problem shown in the status bar first."
        )
    })?;
    build_plan(design, &solved, &[])
        .ok_or_else(|| "This design has no tiers to cut yet.".to_owned())
}

/// The worker's answer: shows the page, or why there is none.
fn prepared(ui: &MainWindow, ticket: u64, watch: Watch, outcome: Result<CuttingPlan, String>) {
    let current = with_session(|session| {
        let current = session.ticket == ticket;
        if current {
            session.preparing = false;
        }
        current
    });
    if !current || !ui.global::<CuttingModeModel>().get_open() {
        return;
    }
    let plan = match outcome {
        Ok(plan) => plan,
        Err(message) => {
            fail(ui, &message);
            return;
        }
    };
    let progress = handles().map_or_else(Progress::default, |(_, db)| {
        store::load(&db, &watch.uuid).unwrap_or_else(|err| {
            show_toast(
                ui,
                &format!("Could not read your cutting progress: {err}"),
                "warning",
            );
            Progress::default()
        })
    });
    with_session(|session| {
        let kept = session
            .resume_key
            .take()
            .and_then(|key| plan.steps.iter().position(|step| step.key == key));
        session.position = kept.unwrap_or_else(|| progress.resume_position(&plan.steps));
        session.plan = Some(plan);
        session.progress = progress;
        session.watch = Some(watch);
    });
    let model = ui.global::<CuttingModeModel>();
    model.set_error("".into());
    model.set_busy(false);
    show_current(ui);
    start_watcher(ui);
}

/// Starts the timer that notices the design changing under the open screen, unless it runs.
fn start_watcher(ui: &MainWindow) {
    if with_session(|session| session.timer.is_some()) {
        return;
    }
    let weak = ui.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, WATCH_INTERVAL, move || {
        if let Some(ui) = weak.upgrade() {
            check_for_change(&ui);
        }
    });
    with_session(|session| session.timer = Some(timer));
}

/// One tick of the watcher: when the design is not the one the steps were built from (an
/// Undo, an Open, an edit), builds them again, staying on the same tier when it still exists.
fn check_for_change(ui: &MainWindow) {
    if !ui.global::<CuttingModeModel>().get_open() {
        return;
    }
    let (preparing, built_from, key) = with_session(|session| {
        (
            session.preparing,
            session.watch.clone(),
            session
                .plan
                .as_ref()
                .and_then(|plan| plan.steps.get(session.position))
                .map(|step| step.key.clone()),
        )
    });
    if preparing {
        return;
    }
    let Some((state, _)) = handles() else {
        return;
    };
    let Some(now) = current_watch(&state) else {
        return;
    };
    if built_from.as_ref() != Some(&now) {
        with_session(|session| session.resume_key = key);
        prepare_steps(ui);
    }
}

// ---------------------------------------------------------------------------------------
// Stepping and marking.
// ---------------------------------------------------------------------------------------

/// How many steps are marked done: what the page's progress line counts.
fn done_count() -> usize {
    with_session(|session| {
        session
            .plan
            .as_ref()
            .map_or(0, |plan| session.progress.done_count(&plan.steps))
    })
}

/// Tells an open tutorial that a step was marked done, when the last change raised the number
/// of done steps above `before`. Taking a mark back or ticking one index of several does not.
fn report_marked(ui: &MainWindow, before: usize) {
    if done_count() > before {
        raise(ui, events::CUTTING_STEP_MARKED);
    }
}

/// The step after the one showing, if there is one.
fn next_step() -> Option<usize> {
    with_session(|session| {
        session
            .plan
            .as_ref()
            .and_then(|plan| next_position(session.position, plan.steps.len()))
    })
}

/// Moves to step `target` (when there is one) and shows it.
fn go_to(ui: &MainWindow, target: Option<usize>) {
    if let Some(target) = target {
        with_session(|session| session.position = target);
        show_current(ui);
    }
}

/// Pushes the page of the step showing and draws its stone.
fn show_current(ui: &MainWindow) {
    let name = ui.get_loaded_design_name();
    SESSION.with(|cell| {
        let session = cell.borrow();
        let Some(plan) = &session.plan else {
            return;
        };
        push::push_page(ui, plan, &session.progress, session.position, name.as_str());
        if let Some(step) = plan.steps.get(session.position) {
            viewport::show_step(ui, step, session.position, plan.steps.len());
        }
    });
}

/// Stores the changes `decide` asks for -- given the steps, the marks and the step showing --
/// and then applies them to the marks the page reads. Nothing changes on the page when
/// the library refuses them. Returns whether they were stored (an empty list counts).
fn change_marks(
    ui: &MainWindow,
    decide: impl FnOnce(&CuttingPlan, &Progress, usize) -> Vec<MarkChange>,
) -> bool {
    let Some((_, db)) = handles() else {
        return false;
    };
    let planned = with_session(|session| {
        let plan = session.plan.as_ref()?;
        let uuid = session.watch.as_ref()?.uuid.clone();
        Some((uuid, decide(plan, &session.progress, session.position)))
    });
    let Some((uuid, changes)) = planned else {
        return false;
    };
    if changes.is_empty() {
        return true;
    }
    if let Err(err) = store::save(&db, &uuid, &changes, store::unix_now()) {
        show_toast(
            ui,
            &format!("Could not save your cutting progress: {err}"),
            "error",
        );
        return false;
    }
    with_session(|session| session.progress.apply(&changes));
    true
}

/// "Reset progress" confirmed: every mark of the design is removed and the page goes back to
/// the first step.
fn reset(ui: &MainWindow) {
    let Some((_, db)) = handles() else {
        return;
    };
    let Some(uuid) = with_session(|session| session.watch.as_ref().map(|w| w.uuid.clone())) else {
        return;
    };
    if let Err(err) = store::clear(&db, &uuid) {
        show_toast(
            ui,
            &format!("Could not reset your cutting progress: {err}"),
            "error",
        );
        return;
    }
    with_session(|session| {
        session.progress.clear();
        session.position = 0;
    });
    show_current(ui);
    show_toast(ui, "Every mark for this design was removed.", "info");
}

// ---------------------------------------------------------------------------------------
// Closing.
// ---------------------------------------------------------------------------------------

/// The screen closed (Esc, the Close button, the menu): stops the watcher, drops the steps and
/// puts the viewport back. A design that replaced the one cutting mode opened for comes back
/// finished and unselected, because the old slider position means nothing for it.
fn closed(ui: &MainWindow) {
    let (saved, state) = with_session(|session| {
        session.ticket += 1;
        session.preparing = false;
        session.plan = None;
        session.progress = Progress::default();
        session.position = 0;
        session.watch = None;
        session.resume_key = None;
        if let Some(timer) = session.timer.take() {
            timer.stop();
        }
        (
            session.saved.take(),
            session.handles.as_ref().map(|h| Rc::clone(&h.state)),
        )
    });
    push::clear_page(ui);
    let model = ui.global::<CuttingModeModel>();
    model.set_error("".into());
    model.set_busy(false);
    let Some((saved, uuid, epoch)) = saved else {
        return;
    };
    let replaced = state
        .and_then(|state| current_watch(&state))
        .is_some_and(|now| now.uuid != uuid || now.epoch != epoch);
    viewport::restore(
        ui,
        if replaced {
            saved.after_replacement()
        } else {
            saved
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    #[test]
    fn a_solvable_design_is_prepared_into_its_steps() {
        let design = Design::concave_fixture();
        let plan = prepare(&design).expect("the fixture solves");
        assert_eq!(plan.steps.len(), design.preview_step_count());
        assert!(plan.steps.iter().all(|step| !step.key.is_empty()));
    }

    #[test]
    fn a_design_that_does_not_solve_is_refused_with_a_reason() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            vec![ConstraintTier {
                angle_deg: 30.0,
                name: "C1".to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }],
        );
        let message = prepare(&design).expect_err("nothing to meet");
        assert!(
            message.starts_with("This design does not solve yet"),
            "{message}"
        );
        assert!(message.ends_with("status bar first."), "{message}");
    }

    #[test]
    fn a_design_without_tiers_is_refused_with_a_reason() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        assert!(
            prepare(&design).is_err(),
            "there is nothing to step through"
        );
    }

    #[test]
    fn what_names_a_design_compares_part_by_part() {
        let built_from = Watch {
            uuid: "0b9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10".to_owned(),
            generation: 4,
            epoch: 1,
        };
        assert_eq!(built_from, built_from.clone());
        for changed in [
            Watch {
                generation: 5,
                ..built_from.clone()
            },
            Watch {
                epoch: 2,
                ..built_from.clone()
            },
            Watch {
                uuid: "7c3d2f19-8a64-4e0b-b1d5-9f2a6c4e8d73".to_owned(),
                ..built_from.clone()
            },
        ] {
            assert_ne!(built_from, changed);
        }
    }
}
