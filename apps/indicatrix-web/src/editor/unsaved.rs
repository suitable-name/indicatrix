//! The unsaved-changes guard: a Slint Save / Discard / Cancel dialog
//! (`ui/components/confirm_dialog.slint`, modelled on the desktop's
//! `confirm_action_dialog.slint`) in front of every action that would replace a design
//! that has unsaved edits (New Design, Open, drop). It replaces the browser's blocking
//! `window.confirm`.
//!
//! # Flow
//!
//! [`confirm_discard`] runs its continuation at once when there is nothing to lose,
//! otherwise it opens the dialog and keeps the continuation until the user chooses:
//!
//! - **Discard** runs it.
//! - **Cancel** (also Escape, the close button and a click on the backdrop) drops it.
//! - **Save** downloads the design as a native pair, exactly like File > Save native pair
//!   (the save may first have to solve the design, so it is asynchronous), and runs the
//!   continuation once the save has marked the design saved
//!   ([`design_saved`], called by `crate::io::save`). A save that fails leaves the design
//!   unsaved, so the continuation never runs; it is dropped after [`SAVE_WAIT`], or when
//!   the design changes before the save finishes.
//!
//! A second [`confirm_discard`] while a dialog is open replaces the first one's
//! continuation (the older request is cancelled).

use crate::{
    ConfirmModel,
    app::{
        Ctx,
        state::{DesignState, WebApp},
    },
    io,
};
use slint::{ComponentHandle, Timer, TimerMode};
use std::{cell::RefCell, time::Duration};

/// How long a "Save, then continue" waits for the save to finish before giving up.
const SAVE_WAIT: Duration = Duration::from_secs(60);

/// What to do once the user has decided.
type Continuation = Box<dyn FnOnce()>;

/// A continuation waiting for its Save to finish.
struct AfterSave {
    /// The design generation the Save was started for.
    generation: u64,
    on_proceed: Continuation,
}

thread_local! {
    /// The continuation behind the open dialog.
    static DIALOG: RefCell<Option<Continuation>> = const { RefCell::new(None) };
    /// The continuation waiting for a Save started from the dialog.
    static AFTER_SAVE: RefCell<Option<AfterSave>> = const { RefCell::new(None) };
    /// Drops [`AFTER_SAVE`] after [`SAVE_WAIT`].
    static SAVE_TIMEOUT: Timer = Timer::default();
}

/// The name of the design to lose when there are unsaved changes, `None` when there is
/// nothing to lose.
#[must_use]
fn unsaved_design_name(app: &WebApp) -> Option<String> {
    app.design
        .as_ref()
        .filter(|design| design.session.is_dirty())
        .map(DesignState::display_name)
}

/// Runs `on_ok` when there is nothing unsaved to lose, or once the user has chosen to
/// discard (or save) the unsaved changes -- see the module doc comment. No `RefCell`
/// borrow is held while it runs.
pub fn confirm_discard(ctx: &Ctx, on_ok: impl FnOnce() + 'static) {
    let name = unsaved_design_name(&ctx.state.borrow());
    let Some(name) = name else {
        on_ok();
        return;
    };
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    DIALOG.with(|cell| *cell.borrow_mut() = Some(Box::new(on_ok)));
    let model = ui.global::<ConfirmModel>();
    model.set_heading("Unsaved changes".into());
    model.set_message(
        format!(
            "\"{name}\" has unsaved changes. Save them (downloads the .asc and \
             .indicatrix.toml), discard them, or cancel?"
        )
        .into(),
    );
    model.set_primary_label("Save".into());
    model.set_secondary_label("Discard".into());
    model.set_open(true);
}

/// Opens the dialog with only a confirm button (`ok_label`, no Save) and Cancel; `on_ok`
/// runs when the user confirms and is dropped on Cancel, Escape or a backdrop click. A
/// second dialog request replaces the pending continuation.
pub fn confirm_action(
    ctx: &Ctx,
    heading: &str,
    message: &str,
    ok_label: &str,
    on_ok: impl FnOnce() + 'static,
) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    DIALOG.with(|cell| *cell.borrow_mut() = Some(Box::new(on_ok)));
    let model = ui.global::<ConfirmModel>();
    model.set_heading(heading.into());
    model.set_message(message.into());
    model.set_primary_label("".into());
    model.set_secondary_label(ok_label.into());
    model.set_open(true);
}

/// Called by `crate::io::save` when a save has marked the design saved: continues an
/// action that was waiting for it (see the module doc comment).
pub fn design_saved(ctx: &Ctx) {
    let waiting = AFTER_SAVE.with(|cell| cell.borrow_mut().take());
    let Some(waiting) = waiting else {
        return;
    };
    SAVE_TIMEOUT.with(Timer::stop);
    if ctx.state.borrow().generation() == Some(waiting.generation) {
        (waiting.on_proceed)();
    }
}

fn close_dialog(ctx: &Ctx) {
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<ConfirmModel>().set_open(false);
    }
}

/// The dialog's Save button.
fn save_then_proceed(ctx: &Ctx) {
    close_dialog(ctx);
    let Some(on_proceed) = DIALOG.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    let Some(generation) = ctx.state.borrow().generation() else {
        on_proceed();
        return;
    };
    AFTER_SAVE.with(|cell| {
        *cell.borrow_mut() = Some(AfterSave {
            generation,
            on_proceed,
        });
    });
    SAVE_TIMEOUT.with(|timer| {
        timer.start(TimerMode::SingleShot, SAVE_WAIT, || {
            AFTER_SAVE.with(|cell| cell.borrow_mut().take());
        });
    });
    io::save::save_native_pair(ctx);
}

/// The dialog's Discard button.
fn discard_and_proceed(ctx: &Ctx) {
    close_dialog(ctx);
    if let Some(on_proceed) = DIALOG.with(|cell| cell.borrow_mut().take()) {
        on_proceed();
    }
}

/// The dialog's Cancel button (and Escape, close, backdrop).
fn cancel(ctx: &Ctx) {
    close_dialog(ctx);
    DIALOG.with(|cell| cell.borrow_mut().take());
}

/// Wires the dialog's three buttons.
pub fn wire(ui: &crate::AppWindow, ctx: &Ctx) {
    let model = ui.global::<ConfirmModel>();
    let c = ctx.clone();
    model.on_primary(move || save_then_proceed(&c));
    let c = ctx.clone();
    model.on_secondary(move || discard_and_proceed(&c));
    let c = ctx.clone();
    model.on_cancel(move || cancel(&c));
}
