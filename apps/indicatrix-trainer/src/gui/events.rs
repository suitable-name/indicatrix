//! The 33 ms timer: drains the worker's events and the background messages on the UI thread.
//!
//! Every log line, point and outcome event is handled in order; of the snapshots only the newest
//! is applied (the run generation of each is noted on arrival so a reset clears the charts
//! before the points of the new run arrive).

use std::{rc::Rc, sync::mpsc::TryRecvError, time::Duration};

use slint::ComponentHandle;

use super::{
    Gui, UiMessage,
    apply::{apply_display, apply_point, apply_run_state, apply_setup, clamp_i32},
    hyperparams::Retained,
    worker_handle::ClosePhase,
};
use crate::{
    stats::IterationMetricPoint,
    trainer_worker::runner::{CUDA_LABEL, GPU_LABEL},
    ui_state::{DatasetInfo, ProgressSnapshot, WorkerEvent},
};

/// How often the worker's events are drained.
const TICK: Duration = Duration::from_millis(33);

impl Gui {
    /// Starts the drain timer; the caller keeps the returned timer alive.
    pub fn start_timer(self: &Rc<Self>) -> slint::Timer {
        let timer = slint::Timer::default();
        let gui = self.clone();
        timer.start(slint::TimerMode::Repeated, TICK, move || gui.tick());
        timer
    }

    fn tick(&self) {
        let mut newest: Option<Box<ProgressSnapshot>> = None;
        let mut disconnected = false;
        loop {
            let next = self.worker.borrow().try_event();
            match next {
                Ok(WorkerEvent::Metrics(snapshot)) => {
                    self.note_generation(snapshot.run_generation);
                    newest = Some(snapshot);
                }
                Ok(event) => self.handle_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    disconnected = true;
                    break;
                }
            }
        }
        while let Ok(message) = self.msg_rx.try_recv() {
            self.handle_message(message);
        }
        if let Some(snapshot) = newest {
            self.apply_snapshot(*snapshot);
        }
        if disconnected && !self.dead.get() && self.phase.get() != ClosePhase::Stopping {
            self.worker_ended("the worker exited unexpectedly");
        }
        if self.charts_dirty.get() {
            self.refresh_charts();
        }
        if self.logs_dirty.get() {
            self.sync_log_counts();
        }
        self.poll_stop();
    }

    /// A new run generation clears the live points and the figures derived from them.
    fn note_generation(&self, generation: u64) {
        if self.charts.borrow_mut().note_generation(generation) {
            self.live.borrow_mut().point = None;
            if !self.viewing_import() {
                self.with_c(|c| apply_point(c, None));
            }
            self.charts_dirty.set(true);
        }
    }

    fn apply_snapshot(&self, snapshot: ProgressSnapshot) {
        let alive = !self.dead.get();
        let stopping = self.phase.get() == ClosePhase::Stopping;
        let importing = self.viewing_import();
        self.with_c(|c| {
            apply_run_state(c, &snapshot, alive, stopping);
            apply_setup(c, &snapshot);
            if !importing {
                apply_display(c, &snapshot);
            }
        });
        self.live.borrow_mut().snapshot = Some(snapshot);
    }

    /// The PPO-only values the hyperparameter command carries unchanged.
    pub(super) fn retained(&self) -> Retained {
        self.live
            .borrow()
            .snapshot
            .as_ref()
            .map_or_else(Retained::default, |s| {
                Retained::from_worker(s.ppo_clip, s.gae_lambda)
            })
    }

    fn handle_event(&self, event: WorkerEvent) {
        match event {
            // Snapshots are handled in `tick`.
            WorkerEvent::Metrics(_) => {}
            WorkerEvent::Log { level, message } => self.log(&level, message),
            WorkerEvent::Point(point) => self.handle_point(*point),
            WorkerEvent::Busy(text) => {
                self.with_c(|c| c.set_busy_text(text.unwrap_or_default().into()));
            }
            WorkerEvent::DeviceChanged(result) => self.handle_device(result),
            WorkerEvent::HyperparamsApplied => {
                self.with_c(|c| c.set_hyper_error("".into()));
            }
            WorkerEvent::HyperparamsRefused(reason) => {
                self.with_c(|c| {
                    c.set_hyper_error(reason.into());
                    c.set_hyper_editing(true);
                });
            }
            WorkerEvent::WeightsLoaded(result) => match result {
                Ok(path) => self.notify(&format!("Loaded weights from {path}."), false),
                Err(reason) => {
                    self.notify(&format!("Failed to load weights: {reason}"), true);
                }
            },
            WorkerEvent::WeightsSaved(result) => self.handle_weights_saved(result),
            WorkerEvent::StatsExported(result) => match result {
                Ok(path) => self.notify(&format!("Run exported to {path}."), false),
                Err(reason) => self.notify(&format!("Export failed: {reason}"), true),
            },
            WorkerEvent::Dataset(result) => self.handle_dataset(result),
            WorkerEvent::DesignSelected {
                index,
                title,
                tier_count,
                split,
            } => {
                self.with_c(|c| {
                    c.set_current_db_index(clamp_i32(index));
                    c.set_current_design_title(title.into());
                    c.set_current_design_split(split.label().into());
                    c.set_tier_count(clamp_i32(tier_count));
                });
            }
            WorkerEvent::Died(reason) => self.worker_ended(&reason),
        }
    }

    fn handle_point(&self, point: IterationMetricPoint) {
        self.live.borrow_mut().point = Some(point.clone());
        if !self.viewing_import() {
            self.with_c(|c| apply_point(c, Some(&point)));
        }
        self.charts.borrow_mut().push(point);
        self.charts_dirty.set(true);
    }

    fn handle_device(&self, result: Result<String, String>) {
        self.with_c(|c| c.set_device_switching(false));
        match result {
            Ok(label) => {
                self.with_c(|c| {
                    c.set_use_gpu(label == GPU_LABEL || label == CUDA_LABEL);
                    c.set_device_name(label.into());
                    c.set_device_known(true);
                });
            }
            Err(reason) => {
                let name = self
                    .with_c(|c| c.get_device_name().to_string())
                    .unwrap_or_default();
                self.notify(
                    &format!("Backend switch failed, staying on {name}: {reason}"),
                    true,
                );
            }
        }
    }

    fn handle_weights_saved(&self, result: Result<String, String>) {
        let closing = self.phase.get() == ClosePhase::SavingFirst;
        match result {
            Ok(path) => {
                self.notify(&format!("Weights saved to {path}."), false);
                if closing {
                    self.begin_stop();
                }
            }
            Err(reason) => {
                self.phase.set(ClosePhase::Open);
                self.notify(&format!("Saving the weights failed: {reason}"), true);
            }
        }
    }

    fn handle_dataset(&self, result: Result<DatasetInfo, String>) {
        match result {
            Ok(info) => {
                *self.designs.borrow_mut() = info.designs;
                let count = self.designs.borrow().len();
                self.with_c(|c| {
                    c.set_db_state(1);
                    c.set_db_error("".into());
                    c.set_db_path(info.db_path.into());
                    c.set_db_designs_count(clamp_i32(count));
                    c.set_train_designs_count(clamp_i32(info.train));
                    c.set_val_designs_count(clamp_i32(info.val));
                    c.set_test_designs_count(clamp_i32(info.test));
                });
            }
            Err(reason) => {
                self.designs.borrow_mut().clear();
                self.with_c(|c| {
                    c.set_db_state(2);
                    c.set_db_error(reason.into());
                    c.set_db_designs_count(0);
                });
            }
        }
        self.refresh_designs();
    }

    /// The worker ended (error, panic or an unexpected exit): the window stays, the buttons stop.
    fn worker_ended(&self, reason: &str) {
        self.dead.set(true);
        self.log("ERROR", format!("The trainer stopped: {reason}"));
        self.with_c(|c| {
            c.set_worker_alive(false);
            c.set_is_training(false);
            c.set_device_switching(false);
            c.set_busy_text("".into());
            c.set_status("Stopped".into());
        });
    }

    /// Closes the window once a requested shutdown has finished.
    fn poll_stop(&self) {
        if self.phase.get() == ClosePhase::Stopping
            && self.worker.borrow().is_finished()
            && let Some(window) = self.window.upgrade()
        {
            let _ = window.hide();
        }
    }

    fn handle_message(&self, message: UiMessage) {
        match message {
            UiMessage::ImportLoaded { path, result } => self.finish_import(&path, result),
            UiMessage::UpdateChecked { manual, result } => {
                self.finish_update_check(manual, result);
            }
        }
    }
}
