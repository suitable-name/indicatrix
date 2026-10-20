//! The Cut destination's Rust side: the instruction list and its progress card, sharing
//! and printing the cutting sheet, sharing the diagram, resetting the done marks of the open
//! design or deleting those of every design, and the hand-over to Cutting Mode. The logic
//! lives in `indicatrix_mobile_core::cutting`; this module ties it to the `CutModel` global
//! and the platform.
//!
//! | Submodule | Job |
//! |---|---|
//! | `rows` | pushes the list, the progress card and the empty states |
//! | `diagram` | draws the highlighted diagram on a worker thread (Medium and larger windows) |
//! | `export` | builds the sheet / picture off the UI thread and hands it to the platform |
//! | `mode` | Cutting Mode (its own lane) |
//!
//! Cutting Mode is opened here ([`start`]) and closes itself; when it ends,
//! `CutModel.mode-ended` re-pushes the page so the new done marks show.

mod diagram;
mod export;
pub mod mode;
mod rows;

use std::rc::Rc;

use indicatrix_mobile_core::{app_state::AppState, cutting, document};
use slint::{ComponentHandle, Weak};

use crate::{
    AppWindow, CutModel, Shell,
    design::{self, Shared},
    push::toast,
};

/// The dialog id of "Reset Progress?" (ids from 2000 up belong to the Cut destination).
const DIALOG_RESET: i32 = 2001;
/// The dialog id of "Delete All Cutting Progress?". The Cut page's menu opens that question
/// itself with this id (`ui/pages/cut.slint`); keep the two in step.
const DIALOG_DELETE_ALL: i32 = 2002;

/// Connects the Cut destination's callbacks.
pub fn wire(window: &AppWindow, state: &Shared) {
    let model = window.global::<CutModel>();
    let weak = window.as_weak();

    model.on_start_cutting(action(state, &weak, |window, state| {
        start(window, state, None);
    }));
    model.on_open_step({
        let (state, weak) = (Rc::clone(state), weak.clone());
        move |step| {
            if let Some(window) = weak.upgrade() {
                start(&window, &state, usize::try_from(step).ok());
            }
        }
    });
    model.on_select_step({
        let (state, weak) = (Rc::clone(state), weak.clone());
        move |step| {
            if let Some(window) = weak.upgrade() {
                rows::select(&window, &state.borrow(), step);
            }
        }
    });
    model.on_solve(action(state, &weak, design::solve::request_solve));
    model.on_share_sheet(action(state, &weak, |window, state| {
        export::start(window, state, export::Kind::Sheet);
    }));
    model.on_print_sheet(action(state, &weak, |window, state| {
        export::start(window, state, export::Kind::Print);
    }));
    model.on_share_diagram(action(state, &weak, |window, state| {
        export::start(window, state, export::Kind::Diagram);
    }));
    model.on_reset_progress(action(state, &weak, ask_reset));
    model.on_mode_ended(action(state, &weak, design::refresh));
    mode::wire(window, state);
}

/// Pushes the Cut destination from the state (Cutting Mode restarts or closes itself
/// when the design changed under it).
pub fn push(window: &AppWindow, state: &AppState) {
    rows::push(window, state);
    mode::push(window, state);
}

/// Answers the Cut destination's dialogs. Returns false for any other dialog id.
pub fn handle_dialog_result(window: &AppWindow, state: &Shared, id: i32, confirmed: bool) -> bool {
    match id {
        DIALOG_RESET => {
            if confirmed {
                state.borrow().cutting.progress.reset();
                toast::show(window, "Progress reset");
                design::refresh(window, state);
            }
            true
        }
        DIALOG_DELETE_ALL => {
            if confirmed {
                let removed = cutting::delete_all_progress(&state.borrow());
                toast::show(window, delete_all_text(removed));
                design::refresh(window, state);
            }
            true
        }
        _ => false,
    }
}

/// What the snackbar says after "Delete All Cutting Progress" removed `removed` designs'
/// files.
const fn delete_all_text(removed: usize) -> &'static str {
    if removed == 0 {
        "No cutting progress was stored"
    } else {
        "All cutting progress deleted"
    }
}

/// Opens Cutting Mode at `step`, or where the cutting left off. A design without a
/// current solve gets a message instead.
fn start(window: &AppWindow, state: &Shared, step: Option<usize>) {
    let resume = {
        let app = state.borrow();
        cutting::summary(&app).map(|summary| summary.next_step)
    };
    let target = match resume {
        Ok(next) => step.unwrap_or(next),
        Err(error) => {
            toast::show(window, &error.to_string());
            return;
        }
    };
    mode::open(window, state, i32::try_from(target).unwrap_or(0));
    if state.borrow().cutting.mode.is_some() {
        window.global::<CutModel>().set_mode_open(true);
    }
}

/// Asks before the marks of the open design are cleared. The question is asked whenever the
/// design has any mark stored, a changed one (its step was edited since) included, not
/// only when a step reads done.
fn ask_reset(window: &AppWindow, state: &Shared) {
    let (done, changed, has_marks, title) = {
        let app = state.borrow();
        // Binds the store to the open design, so the marks are its own.
        cutting::sync_progress(&app);
        let (done, changed) =
            cutting::summary(&app).map_or((0, 0), |summary| (summary.done, summary.changed));
        (
            done,
            changed,
            app.cutting.progress.has_marks(),
            document::display_title(&app),
        )
    };
    if !has_marks {
        toast::show(window, "Nothing is marked yet");
        return;
    }
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_RESET,
        "Reset Progress?".into(),
        reset_message(&title, done, changed).into(),
        "Reset".into(),
        "Cancel".into(),
    );
}

/// The body of the "Reset Progress?" question for `title`, whose marks stand at `done`
/// steps done and `changed` steps marked once but edited since.
fn reset_message(title: &str, done: usize, changed: usize) -> String {
    let standing = match (done, changed) {
        (0, 0) => String::new(),
        (done, 0) => format!(" ({done} done)"),
        (0, changed) => format!(" ({changed} changed since marked)"),
        (done, changed) => format!(" ({done} done, {changed} changed since marked)"),
    };
    format!("The cutting progress of \"{title}\"{standing} will be cleared. This cannot be undone.")
}

/// A callback closure that upgrades the window and runs `job` with it and the state.
fn action(
    state: &Shared,
    weak: &Weak<AppWindow>,
    job: fn(&AppWindow, &Shared),
) -> impl Fn() + 'static {
    let (state, weak) = (Rc::clone(state), weak.clone());
    move || {
        if let Some(window) = weak.upgrade() {
            job(&window, &state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_reset_question_says_what_is_stored() {
        assert_eq!(
            reset_message("Barion", 3, 0),
            "The cutting progress of \"Barion\" (3 done) will be cleared. This cannot be undone."
        );
        assert_eq!(
            reset_message("Barion", 0, 2),
            "The cutting progress of \"Barion\" (2 changed since marked) will be cleared. \
             This cannot be undone."
        );
        assert_eq!(
            reset_message("Barion", 4, 1),
            "The cutting progress of \"Barion\" (4 done, 1 changed since marked) will be \
             cleared. This cannot be undone."
        );
        // Only marks of steps the design no longer has: nothing to count, still asked.
        assert_eq!(
            reset_message("Barion", 0, 0),
            "The cutting progress of \"Barion\" will be cleared. This cannot be undone."
        );
    }

    #[test]
    fn deleting_everything_says_when_there_was_nothing() {
        assert_eq!(delete_all_text(0), "No cutting progress was stored");
        assert_eq!(delete_all_text(3), "All cutting progress deleted");
    }
}
