//! The GUI controller: everything between the Slint window and the training worker.
//!
//! The UI thread owns all Slint state and the [`Gui`] object. The worker runs on its own thread
//! and talks through channels; a 33 ms timer drains its events (see `events.rs`). The status
//! pill, the buttons and the setup controls are driven by the worker's snapshots, never set
//! optimistically, so a worker that pauses itself after an error is shown as paused.
//!
//! Closing never blocks: the window stays up in the "Stopping" state while the worker finishes
//! its update (`close.rs`). File dialogs are `rfd` async dialogs awaited on the Slint event loop
//! (`dialogs.rs`), one at a time.

mod actions;
mod apply;
mod callbacks;
mod charts;
mod close;
mod dialogs;
mod events;
mod hyperparams;
mod import;
mod logs;
mod models;
mod settings;
mod update;
mod worker_handle;

use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
    rc::Rc,
    sync::mpsc::{Receiver, Sender, channel},
};

use slint::{ComponentHandle, ModelRc, VecModel, Weak};

use self::{
    charts::ChartStore,
    logs::{ERROR, LogStore, utc_time_of_day},
    settings::{MIN_WINDOW, Settings},
    worker_handle::{ClosePhase, WorkerHandle},
};
use crate::{
    AppWindow, TrainingController,
    stats::{IterationMetricPoint, TrainingRunStats},
    trainer_worker::{TrainerCommand, runner::MAX_WORKERS},
    ui_state::{DesignInfo, ProgressSnapshot, spectrum_text, split_text},
};

/// Results of background work, delivered to the UI thread through the timer.
enum UiMessage {
    /// A run file was read (or failed to).
    ImportLoaded {
        path: PathBuf,
        result: Result<Box<TrainingRunStats>, String>,
    },
    /// A new-version check finished (`manual`: the user pressed Check Now).
    UpdateChecked {
        manual: bool,
        result: Result<Option<indicatrix_update::Release>, indicatrix_update::CheckError>,
    },
}

/// The latest live data, kept so that leaving an import view can restore it.
#[derive(Default)]
struct Live {
    snapshot: Option<ProgressSnapshot>,
    point: Option<IterationMetricPoint>,
}

/// The controller. Cheap to share: every callback holds an `Rc<Gui>`.
pub struct Gui {
    window: Weak<AppWindow>,
    worker: RefCell<WorkerHandle>,
    logs: RefCell<LogStore>,
    charts: RefCell<ChartStore>,
    settings: RefCell<Settings>,
    designs: RefCell<Vec<DesignInfo>>,
    live: RefCell<Live>,
    phase: Cell<ClosePhase>,
    dead: Cell<bool>,
    dialog_busy: Cell<bool>,
    charts_dirty: Cell<bool>,
    logs_dirty: Cell<bool>,
    /// The newer release the notice announces.
    update_release: RefCell<Option<indicatrix_update::Release>>,
    /// A new-version check is running on its background thread.
    update_in_flight: Cell<bool>,
    /// "Check Now" was pressed while an automatic check ran: that one reports as manual.
    update_manual_wanted: Cell<bool>,
    msg_tx: Sender<UiMessage>,
    msg_rx: Receiver<UiMessage>,
}

impl Gui {
    /// Builds the controller, seeds the window, starts the worker and wires every callback.
    pub fn install(window: &AppWindow) -> Rc<Self> {
        let settings = Settings::load();
        let (msg_tx, msg_rx) = channel();
        let gui = Rc::new(Self {
            window: window.as_weak(),
            worker: RefCell::new(WorkerHandle::spawn()),
            logs: RefCell::new(LogStore::new(settings.log_filter)),
            charts: RefCell::new(ChartStore::default()),
            settings: RefCell::new(settings),
            designs: RefCell::new(Vec::new()),
            live: RefCell::new(Live::default()),
            phase: Cell::new(ClosePhase::Open),
            dead: Cell::new(false),
            dialog_busy: Cell::new(false),
            charts_dirty: Cell::new(true),
            logs_dirty: Cell::new(true),
            update_release: RefCell::new(None),
            update_in_flight: Cell::new(false),
            update_manual_wanted: Cell::new(false),
            msg_tx,
            msg_rx,
        });
        gui.seed_window(window);
        gui.log("INIT", "Indicatrix Trainer started");
        gui.send_startup_commands();
        gui.wire(window);
        gui.install_close_handler(window);
        gui.init_update_check();
        gui
    }

    /// Runs `f` with the controller global; `None` when the window is gone.
    fn with_c<R>(&self, f: impl FnOnce(&TrainingController<'_>) -> R) -> Option<R> {
        let window = self.window.upgrade()?;
        let c = window.global::<TrainingController>();
        Some(f(&c))
    }

    /// Fills the static models and the remembered state before the first frame.
    fn seed_window(&self, window: &AppWindow) {
        let settings = self.settings.borrow().clone();
        if let Some([width, height]) = settings.window_size {
            window.window().set_size(slint::LogicalSize::new(
                width.max(MIN_WINDOW.0),
                height.max(MIN_WINDOW.1),
            ));
        }
        let c = window.global::<TrainingController>();
        apply::apply_constants(&c);
        c.set_tiers(ModelRc::new(VecModel::from(models::tiers())));
        c.set_materials(ModelRc::new(VecModel::from(models::materials())));
        c.set_spectrum_text(spectrum_text().into());
        c.set_split_text(split_text().into());
        c.set_max_workers(i32::try_from(MAX_WORKERS).unwrap_or(i32::MAX));
        let cores = std::thread::available_parallelism().map_or(1, usize::from);
        c.set_core_count(apply::clamp_i32(cores));
        c.set_logs(self.logs.borrow().model());
        c.set_log_filter(self.logs.borrow().filter());
        c.set_designs(ModelRc::new(VecModel::from(Vec::new())));
        if let Some(mode) = settings.mode {
            c.set_model_mode(mode);
        }
        if let Some(tier) = &settings.tier {
            c.set_model_tier_name(tier.as_str().into());
        }
        if let Some(name) = &settings.target_material {
            c.set_target_material(name.as_str().into());
        }
        if let Some(universal) = settings.universal_spectrum {
            c.set_is_universal_spectrum(universal);
        }
        if let Some(corpus) = settings.corpus_mode {
            c.set_is_corpus_mode(corpus);
        }
        if let Some(path) = &settings.db_path {
            c.set_db_path(path.as_str().into());
        }
    }

    /// Brings a fresh worker to the remembered configuration.
    fn send_startup_commands(&self) {
        let commands = self
            .settings
            .borrow()
            .startup_commands(std::path::Path::exists);
        let worker = self.worker.borrow();
        for command in commands {
            let _ = worker.send(command);
        }
    }

    /// Sends a command; reports through the banner when the worker has ended.
    fn send(&self, command: TrainerCommand) -> bool {
        let sent = self.worker.borrow().send(command);
        if !sent {
            self.notify("The trainer is not running. Use Restart Worker.", true);
        }
        sent
    }

    /// Shows the banner above the panels.
    fn notify(&self, text: &str, is_error: bool) {
        self.with_c(|c| {
            c.set_notice(text.into());
            c.set_notice_is_error(is_error);
        });
    }

    /// Appends a console line; an error line also raises the error banner.
    fn log(&self, level: &str, message: impl Into<String>) {
        let message = message.into();
        let severity = self
            .logs
            .borrow_mut()
            .push(&utc_time_of_day(), level, message.clone());
        self.logs_dirty.set(true);
        if severity == ERROR {
            self.notify(&message, true);
        }
    }

    /// Changes the remembered settings and saves them when something changed.
    fn update_settings(&self, change: impl FnOnce(&mut Settings)) {
        let changed = {
            let mut settings = self.settings.borrow_mut();
            let before = settings.clone();
            change(&mut settings);
            (*settings != before).then(|| settings.clone())
        };
        if let Some(settings) = changed
            && let Err(error) = settings.save()
        {
            self.log("WARN", format!("Could not save the settings: {error}"));
        }
    }

    /// Writes the current window size into the settings.
    fn remember_window_size(&self) {
        let size = self.window.upgrade().map(|w| {
            let window = w.window();
            let logical = window.size().to_logical(window.scale_factor());
            [logical.width, logical.height]
        });
        if let Some(size) = size {
            self.update_settings(|s| s.window_size = Some(size));
        }
    }

    /// Rebuilds the design picker for the current filter.
    fn refresh_designs(&self) {
        let filter = self
            .with_c(|c| c.get_design_filter().to_string())
            .unwrap_or_default();
        let items = models::filter_designs(&self.designs.borrow(), &filter);
        self.with_c(|c| c.set_designs(ModelRc::new(VecModel::from(items))));
    }

    /// Redraws the charts for what is on screen.
    fn refresh_charts(&self) {
        let (reward, grid) = self.charts.borrow().render();
        self.with_c(|c| {
            c.set_reward_chart(reward);
            c.set_charts(ModelRc::new(VecModel::from(grid)));
        });
        self.charts_dirty.set(false);
    }

    /// Pushes the warning and error counts of the console.
    fn sync_log_counts(&self) {
        let (warnings, errors) = self.logs.borrow().counts();
        self.with_c(|c| {
            c.set_warning_count(apply::clamp_i32(warnings));
            c.set_error_count(apply::clamp_i32(errors));
        });
        self.logs_dirty.set(false);
    }

    /// Whether an imported run is on screen.
    #[expect(
        clippy::redundant_closure_for_method_calls,
        reason = "the method path does not satisfy the higher-ranked closure bound of with_c (TrainingController carries a lifetime); the closure does"
    )]
    fn viewing_import(&self) -> bool {
        self.with_c(|c| c.get_viewing_import()).unwrap_or(false)
    }
}
