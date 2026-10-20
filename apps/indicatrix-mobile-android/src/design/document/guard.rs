//! The unsaved-changes guard: replacing a design that has unsaved changes (a new design,
//! Open, a file from another app, Close Design) first asks what to do with them.
//!
//! The shell's dialog has two buttons, so the three answers (Save, Discard, Cancel) are two
//! questions in a row, and every way out of the first one except "Save" leads to the
//! second, where only an explicit "Discard" loses the changes:
//!
//! 1. "Save Changes?" -- *Save* saves (through the platform's save dialog) and goes on when
//!    the file was written; *Don't Save* (also the Back key) asks the second question.
//! 2. "Discard Changes?" -- *Discard* goes on; *Cancel* (also Back) stays where you are.

use std::cell::RefCell;

use indicatrix_mobile_core::{
    document::{self, NewDesignForm},
    platform::PickedFile,
};
use slint::ComponentHandle;

use super::{
    close_gallery,
    files::{self, Action},
    handle::toast,
    library, open,
};
use crate::{AppWindow, Shell, design::Shared};

/// The dialog id of "Save Changes?".
pub(super) const DIALOG_SAVE: i32 = 1001;
/// The dialog id of "Discard Changes?".
pub(super) const DIALOG_DISCARD: i32 = 1002;

/// What waits behind the question.
pub enum Pending {
    /// Create the design of a template card (its session index).
    CreateTemplate(usize),
    /// Create a blank design from the Empty form.
    CreateForm(NewDesignForm),
    /// Show the file picker and open what was picked.
    Open,
    /// Open files another app sent.
    Incoming(Vec<PickedFile>),
    /// Open a library entry (its id).
    Library(i64),
    /// Close the design and return to the empty "Start a Design" state.
    Close,
}

thread_local! {
    static PENDING: RefCell<Option<Pending>> = const { RefCell::new(None) };
}

/// Runs `pending` now when the design has no unsaved changes, else asks first.
pub fn confirm_discard(window: &AppWindow, state: &Shared, pending: Pending) {
    if !state.borrow().session.is_dirty() {
        run(window, state, pending);
        return;
    }
    PENDING.with(|slot| *slot.borrow_mut() = Some(pending));
    let title = document::display_title(&state.borrow());
    show_dialog(
        window,
        DIALOG_SAVE,
        "Save Changes?",
        &format!("\"{title}\" has unsaved changes. Save them before you continue?"),
        "Save",
        "Don't Save",
    );
}

/// Handles the answer to one of this module's dialogs. Returns false for any other
/// dialog id, so the owner of the shell's single `dialog-result` handler can pass it on.
pub fn handle_dialog_result(window: &AppWindow, state: &Shared, id: i32, confirmed: bool) -> bool {
    match id {
        DIALOG_SAVE => {
            if confirmed {
                files::start(window, state, Action::Save, true);
            } else {
                let title = document::display_title(&state.borrow());
                show_dialog(
                    window,
                    DIALOG_DISCARD,
                    "Discard Changes?",
                    &format!("The changes to \"{title}\" will be lost."),
                    "Discard",
                    "Cancel",
                );
            }
            true
        }
        DIALOG_DISCARD => {
            let pending = take_pending();
            if let (true, Some(pending)) = (confirmed, pending) {
                run(window, state, pending);
            }
            true
        }
        _ => false,
    }
}

/// Takes the action that waits behind the question, if any.
pub(super) fn take_pending() -> Option<Pending> {
    PENDING.with(|slot| slot.borrow_mut().take())
}

/// Forgets the waiting action (the user said Cancel, or the save did not happen).
pub(super) fn clear_pending() {
    drop(take_pending());
}

/// Runs the waiting action after a save succeeded.
pub(super) fn run_pending(window: &AppWindow, state: &Shared) {
    if let Some(pending) = take_pending() {
        run(window, state, pending);
    }
}

/// Does what `pending` says. The design is replaced; unsaved changes were settled.
fn run(window: &AppWindow, state: &Shared, pending: Pending) {
    match pending {
        Pending::CreateTemplate(index) => {
            let result = document::new_from_template(&mut state.borrow_mut(), index);
            created(window, state, result);
        }
        Pending::CreateForm(form) => {
            let result = document::new_from_form(&mut state.borrow_mut(), &form);
            created(window, state, result);
        }
        Pending::Open => open::pick_and_open(window, state),
        Pending::Incoming(received) => open::open_files(window, state, received),
        Pending::Library(id) => library::load_and_open(window, state, id),
        Pending::Close => closed(window, state),
    }
}

/// Finishes a close: the core drops the design (and cancels its jobs), then every surface
/// is pushed again, so the Design page offers "Start a Design" and Cut shows its own
/// no-design state.
fn closed(window: &AppWindow, state: &Shared) {
    document::close_design(&mut state.borrow_mut());
    close_gallery(window);
    crate::design::after_edit(window, state);
    toast(window, "Design closed");
}

/// Finishes a creation: close the gallery and refresh, or say why it failed.
fn created(window: &AppWindow, state: &Shared, result: Result<(), String>) {
    match result {
        Ok(()) => {
            close_gallery(window);
            crate::design::after_edit(window, state);
            let title = document::display_title(&state.borrow());
            toast(window, &format!("New design: {title}"));
        }
        Err(message) => toast(window, &message),
    }
}

/// Shows the shell's dialog.
fn show_dialog(window: &AppWindow, id: i32, title: &str, body: &str, confirm: &str, cancel: &str) {
    window.global::<Shell>().invoke_show_dialog(
        id,
        title.into(),
        body.into(),
        confirm.into(),
        cancel.into(),
    );
}
