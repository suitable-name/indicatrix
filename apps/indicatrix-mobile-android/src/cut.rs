//! The Cut destination's Rust side: the instruction list and its progress card, sharing
//! and printing the cutting sheet, sharing the diagram, resetting the done marks, and the
//! hand-over to Cutting Mode. The logic lives in `indicatrix_mobile_core::cutting`; this
//! module ties it to the `CutModel` global and the platform.
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
    if id != DIALOG_RESET {
        return false;
    }
    if confirmed {
        state.borrow().cutting.progress.reset();
        toast::show(window, "Progress reset");
        design::refresh(window, state);
    }
    true
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

/// Asks before the done marks are cleared.
fn ask_reset(window: &AppWindow, state: &Shared) {
    let (done, title) = {
        let app = state.borrow();
        (
            cutting::summary(&app).map_or(0, |summary| summary.done),
            document::display_title(&app),
        )
    };
    if done == 0 {
        toast::show(window, "Nothing is marked done yet");
        return;
    }
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_RESET,
        "Reset Progress?".into(),
        format!("All {done} done marks of \"{title}\" will be cleared. This cannot be undone.")
            .into(),
        "Reset".into(),
        "Cancel".into(),
    );
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
