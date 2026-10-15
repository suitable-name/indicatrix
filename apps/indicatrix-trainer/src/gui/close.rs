//! Closing the window without ever blocking the UI thread.
//!
//! A close request while training or with unsaved weights raises `close_prompt`. Closing for
//! real sends Pause and Shutdown, shows "Stopping" and keeps the window until the worker thread
//! has ended (the timer polls it); a second close request while stopping quits at once.

use std::rc::Rc;

use slint::{CloseRequestResponse, ComponentHandle};

use super::{
    Gui,
    worker_handle::{CloseAction, ClosePhase, close_action},
};
use crate::AppWindow;

impl Gui {
    pub(super) fn install_close_handler(self: &Rc<Self>, window: &AppWindow) {
        let gui = self.clone();
        window
            .window()
            .on_close_requested(move || gui.on_close_requested());
    }

    fn on_close_requested(&self) -> CloseRequestResponse {
        let (training, dirty) = self
            .with_c(|c| (c.get_is_training(), c.get_weights_dirty()))
            .unwrap_or((false, false));
        let finished = self.worker.borrow().is_finished();
        match close_action(training, dirty, finished, self.phase.get()) {
            CloseAction::Hide => {
                self.remember_window_size();
                CloseRequestResponse::HideWindow
            }
            CloseAction::Prompt => {
                self.with_c(|c| c.set_close_prompt(true));
                CloseRequestResponse::KeepWindowShown
            }
            CloseAction::BeginStop => {
                self.begin_stop();
                CloseRequestResponse::KeepWindowShown
            }
            CloseAction::Wait => CloseRequestResponse::KeepWindowShown,
        }
    }

    /// Asks the worker to stop; the window closes from the timer when its thread has ended.
    pub(super) fn begin_stop(&self) {
        self.phase.set(ClosePhase::Stopping);
        self.remember_window_size();
        self.worker.borrow().begin_shutdown();
        self.with_c(|c| {
            c.set_close_prompt(false);
            c.set_is_training(false);
            c.set_status("Stopping".into());
            c.set_busy_text(
                "Stopping the trainer… (close the window again to quit at once)".into(),
            );
        });
    }

    /// "Save and Close": choose a file, write the checkpoint, then stop.
    pub(super) fn close_save(self: &Rc<Self>) {
        self.with_c(|c| c.set_close_prompt(false));
        self.save_weights(true);
    }

    pub(super) fn close_discard(&self) {
        self.begin_stop();
    }

    pub(super) fn close_cancel(&self) {
        self.with_c(|c| c.set_close_prompt(false));
    }
}
