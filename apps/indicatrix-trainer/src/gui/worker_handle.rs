//! The training worker as the UI thread sees it: channels, the active flag and the thread handle.
//!
//! The UI thread never joins a running worker: closing sends Pause and Shutdown, shows
//! "Stopping" and polls [`WorkerHandle::is_finished`] on the timer.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    thread::JoinHandle,
};

use crate::{
    trainer_worker::{TrainerCommand, spawn_trainer_worker},
    ui_state::WorkerEvent,
};

/// Channels and thread of one worker.
pub struct WorkerHandle {
    cmd_tx: Sender<TrainerCommand>,
    event_rx: Receiver<WorkerEvent>,
    is_active: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl WorkerHandle {
    /// Starts a fresh worker thread (it loads the data set itself).
    pub fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = channel::<TrainerCommand>();
        let (event_tx, event_rx) = channel::<WorkerEvent>();
        let is_active = Arc::new(AtomicBool::new(false));
        let join = spawn_trainer_worker(cmd_rx, event_tx, is_active.clone());
        Self {
            cmd_tx,
            event_rx,
            is_active,
            join: Some(join),
        }
    }

    /// Sends a command; false when the worker has ended.
    pub fn send(&self, command: TrainerCommand) -> bool {
        self.cmd_tx.send(command).is_ok()
    }

    /// The next pending event, if any.
    pub fn try_event(&self) -> Result<WorkerEvent, TryRecvError> {
        self.event_rx.try_recv()
    }

    /// Asks the worker to stop: the rollout ends within one environment step, then the thread
    /// exits after the update in flight.
    pub fn begin_shutdown(&self) {
        self.is_active.store(false, Ordering::SeqCst);
        let _ = self.cmd_tx.send(TrainerCommand::Pause);
        let _ = self.cmd_tx.send(TrainerCommand::Shutdown);
    }

    /// Whether the thread has ended (or was already taken).
    pub fn is_finished(&self) -> bool {
        self.join.as_ref().is_none_or(JoinHandle::is_finished)
    }

    /// Joins the thread when it has ended; a running thread is left to finish on its own.
    pub fn reap(mut self) {
        if let Some(handle) = self.join.take()
            && handle.is_finished()
        {
            // A panic was already reported through `WorkerEvent::Died`.
            let _ = handle.join();
        }
    }
}

/// Where the window close sequence stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClosePhase {
    /// Nothing requested.
    Open,
    /// "Save and Close": waiting for the checkpoint to be written.
    SavingFirst,
    /// The worker is shutting down; the window closes when its thread ends.
    Stopping,
}

/// What a close request leads to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseAction {
    /// Close the window now.
    Hide,
    /// Ask first (training or unsaved weights).
    Prompt,
    /// Stop the worker, then close.
    BeginStop,
    /// A save is in flight; keep the window.
    Wait,
}

/// Decides what a close request does.
pub const fn close_action(
    training: bool,
    dirty: bool,
    worker_finished: bool,
    phase: ClosePhase,
) -> CloseAction {
    match phase {
        // A second request while stopping is the user's "quit now".
        ClosePhase::Stopping => CloseAction::Hide,
        ClosePhase::SavingFirst => CloseAction::Wait,
        ClosePhase::Open => {
            if worker_finished {
                CloseAction::Hide
            } else if training || dirty {
                CloseAction::Prompt
            } else {
                CloseAction::BeginStop
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_idle_clean_window_stops_the_worker_then_closes() {
        assert_eq!(
            close_action(false, false, false, ClosePhase::Open),
            CloseAction::BeginStop
        );
    }

    #[test]
    fn training_or_unsaved_weights_ask_first() {
        assert_eq!(
            close_action(true, false, false, ClosePhase::Open),
            CloseAction::Prompt
        );
        assert_eq!(
            close_action(false, true, false, ClosePhase::Open),
            CloseAction::Prompt
        );
    }

    #[test]
    fn a_dead_worker_closes_at_once() {
        assert_eq!(
            close_action(true, true, true, ClosePhase::Open),
            CloseAction::Hide
        );
    }

    #[test]
    fn a_second_request_while_stopping_quits() {
        assert_eq!(
            close_action(false, false, false, ClosePhase::Stopping),
            CloseAction::Hide
        );
    }

    #[test]
    fn a_save_in_flight_keeps_the_window() {
        assert_eq!(
            close_action(true, true, false, ClosePhase::SavingFirst),
            CloseAction::Wait
        );
    }
}
