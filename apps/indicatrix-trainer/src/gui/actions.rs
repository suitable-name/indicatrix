//! What the buttons do: run control, model and data-set choices, the hyperparameter form and the
//! console.

use super::{
    Gui,
    apply::clear_display,
    hyperparams::{HyperInput, format_learning_rate, parse_hyperparams},
    logs::copy_to_clipboard,
    models::{is_known_tier, step_design},
    worker_handle::{ClosePhase, WorkerHandle},
};
use crate::trainer_worker::TrainerCommand;

impl Gui {
    /// A command that needs a loaded data set; says why when there is none.
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the method path does not satisfy the higher-ranked closure bound of with_c (TrainingController carries a lifetime); the closure does"
    )]
    fn send_with_data(&self, command: TrainerCommand) {
        match self.with_c(|c| c.get_db_state()).unwrap_or(0) {
            1 => {
                self.send(command);
            }
            0 => self.notify("The data set is still loading.", false),
            _ => self.notify(
                "No data set is loaded. Choose a library database first.",
                true,
            ),
        }
    }

    pub(super) fn start(&self) {
        self.send_with_data(TrainerCommand::Start);
    }

    pub(super) fn pause(&self) {
        self.send(TrainerCommand::Pause);
    }

    pub(super) fn step(&self) {
        self.send_with_data(TrainerCommand::StepOnce);
    }

    pub(super) fn reset(&self) {
        self.send(TrainerCommand::Reset);
    }

    /// Replaces an ended worker by a fresh one with the remembered configuration.
    pub(super) fn restart_worker(&self) {
        if !self.worker.borrow().is_finished() {
            self.notify("The trainer is still running.", false);
            return;
        }
        let old = std::mem::replace(&mut *self.worker.borrow_mut(), WorkerHandle::spawn());
        old.reap();
        self.dead.set(false);
        self.phase.set(ClosePhase::Open);
        *self.live.borrow_mut() = super::Live::default();
        self.charts.borrow_mut().clear();
        self.charts_dirty.set(true);
        self.with_c(|c| {
            c.set_worker_alive(true);
            c.set_status("Idle".into());
            c.set_is_training(false);
            c.set_busy_text("".into());
            c.set_notice("".into());
            c.set_db_state(0);
            c.set_device_known(false);
            c.set_device_switching(false);
            c.set_device_name("Detecting…".into());
            clear_display(c);
        });
        self.log("INIT", "Trainer restarted");
        self.send_startup_commands();
    }

    pub(super) fn toggle_device(&self) {
        let ready = self
            .with_c(|c| c.get_device_known() && !c.get_device_switching())
            .unwrap_or(false);
        if ready && self.send(TrainerCommand::ToggleDevice) {
            self.with_c(|c| {
                c.set_device_switching(true);
                c.set_notice("".into());
            });
        }
    }

    pub(super) fn select_mode(&self, mode: i32) {
        let Ok(index) = usize::try_from(mode) else {
            return;
        };
        if index > 1 {
            return;
        }
        self.update_settings(|s| s.mode = Some(mode));
        self.send(TrainerCommand::SwitchMode(index));
    }

    pub(super) fn select_tier(&self, code: &str) {
        if !is_known_tier(code) {
            return;
        }
        self.update_settings(|s| s.tier = Some(code.to_owned()));
        self.send(TrainerCommand::SwitchTier(code.to_owned()));
    }

    pub(super) fn select_material(&self, name: &str) {
        self.update_settings(|s| s.target_material = Some(name.to_owned()));
        self.send(TrainerCommand::SetTargetMaterial(name.to_owned()));
    }

    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the method path does not satisfy the higher-ranked closure bound of with_c (TrainingController carries a lifetime); the closure does"
    )]
    pub(super) fn toggle_spectrum(&self) {
        let now = self
            .with_c(|c| c.get_is_universal_spectrum())
            .unwrap_or(true);
        self.update_settings(|s| s.universal_spectrum = Some(!now));
        self.send(TrainerCommand::ToggleSpectrumMode);
    }

    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the method path does not satisfy the higher-ranked closure bound of with_c (TrainingController carries a lifetime); the closure does"
    )]
    pub(super) fn toggle_corpus(&self) {
        let now = self.with_c(|c| c.get_is_corpus_mode()).unwrap_or(true);
        self.update_settings(|s| s.corpus_mode = Some(!now));
        self.send(TrainerCommand::ToggleCorpusMode);
    }

    pub(super) fn filter_designs(&self, text: &str) {
        self.with_c(|c| c.set_design_filter(text.into()));
        self.refresh_designs();
    }

    pub(super) fn select_design(&self, index: i32) {
        if let Ok(index) = usize::try_from(index) {
            self.send(TrainerCommand::SelectDbDesign(index));
        }
    }

    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the method path does not satisfy the higher-ranked closure bound of with_c (TrainingController carries a lifetime); the closure does"
    )]
    pub(super) fn step_design(&self, forward: bool) {
        let current = self.with_c(|c| c.get_current_db_index()).unwrap_or(-1);
        let next = step_design(&self.designs.borrow(), current, forward);
        if let Some(index) = next {
            self.send(TrainerCommand::SelectDbDesign(index));
        }
    }

    /// Opens the hyperparameter form seeded with the applied values.
    pub(super) fn begin_edit_hyperparams(&self) {
        self.with_c(|c| {
            c.set_edit_workers(c.get_worker_count().to_string().into());
            c.set_edit_rollout(c.get_rollout_size().to_string().into());
            c.set_edit_batch(c.get_batch_size().to_string().into());
            c.set_edit_epochs(c.get_epochs().to_string().into());
            c.set_edit_lr(format_learning_rate(c.get_learning_rate()).into());
            c.set_hyper_error("".into());
            c.set_hyper_editing(true);
        });
    }

    /// Validates the typed text and hands the worker one atomic command. A refusal by the worker
    /// comes back as an event that reopens the form with the reason.
    pub(super) fn apply_hyperparams(&self) {
        let input = self.with_c(|c| HyperInput {
            workers: c.get_edit_workers().to_string(),
            rollout_size: c.get_edit_rollout().to_string(),
            mini_batch: c.get_edit_batch().to_string(),
            epochs: c.get_edit_epochs().to_string(),
            learning_rate: c.get_edit_lr().to_string(),
        });
        let Some(input) = input else {
            return;
        };
        match parse_hyperparams(&input, self.retained()) {
            Err(message) => {
                self.with_c(|c| c.set_hyper_error(message.into()));
            }
            Ok(command) => {
                if self.send(command) {
                    self.with_c(|c| {
                        c.set_hyper_error("".into());
                        c.set_hyper_editing(false);
                    });
                }
            }
        }
    }

    pub(super) fn cancel_edit_hyperparams(&self) {
        self.with_c(|c| {
            c.set_hyper_error("".into());
            c.set_hyper_editing(false);
        });
    }

    pub(super) fn dismiss_notice(&self) {
        self.with_c(|c| c.set_notice("".into()));
    }

    pub(super) fn set_log_filter(&self, filter: i32) {
        self.logs.borrow_mut().set_filter(filter);
        let applied = self.logs.borrow().filter();
        self.with_c(|c| c.set_log_filter(applied));
        self.update_settings(|s| s.log_filter = applied);
    }

    pub(super) fn copy_logs(&self) {
        let (text, lines) = {
            let logs = self.logs.borrow();
            (logs.visible_text(), logs.visible().count())
        };
        match copy_to_clipboard(&text) {
            Ok(()) => self.notify(&format!("Copied {lines} console lines."), false),
            Err(reason) => self.notify(&format!("Could not copy: {reason}"), true),
        }
    }

    pub(super) fn clear_logs(&self) {
        self.logs.borrow_mut().clear();
        self.logs_dirty.set(true);
    }
}
