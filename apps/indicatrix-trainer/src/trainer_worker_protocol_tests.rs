//! End-to-end test of the worker protocol: the real worker loop on the built-in fixture data set
//! (one CPU environment, a tiny rollout), driven through the command channel and observed
//! through the event channel.

use std::{
    path::Path,
    sync::mpsc::{Sender, channel},
};

use super::{commands::Loaded, runner::DeviceChoice, *};
use crate::{
    dataset::{DatasetSplit, DbDesignRecord},
    ui_state::{DatasetInfo, DesignSplit, design_infos},
};

/// How long a single event may take; generous, an iteration is well under a second.
const WAIT: Duration = Duration::from_secs(300);

#[expect(
    clippy::unnecessary_wraps,
    reason = "must match the DatasetLoader fn type, whose loaders can fail"
)]
fn fixture_loader(_: Option<PathBuf>) -> Result<Loaded, String> {
    let split = DatasetSplit::fixture();
    let record = DbDesignRecord {
        id: -1,
        title: "Concave fixture (smoke test)".to_owned(),
        filename: "concave_fixture".to_owned(),
        asc_bytes: Vec::new(),
    };
    let info = DatasetInfo {
        db_path: "fixture".to_owned(),
        designs: design_infos(&[record], &split),
        train: split.train.len(),
        val: split.val.len(),
        test: split.test.len(),
    };
    Ok(Loaded { info, split })
}

fn missing_loader(_: Option<PathBuf>) -> Result<Loaded, String> {
    Err("no database here".to_owned())
}

/// One CPU environment, eight steps per update, no validation pass.
fn tiny_config() -> RunConfig {
    let mut config = RunConfig::new(TrainingMode::Retarget, ModelTier::Small);
    config.device = DeviceChoice::Cpu;
    config.workers = 1;
    config.rollout_size = 8;
    config.label_every = 1;
    config.imitation.mini_batch_size = 8;
    config.validation_every = 100_000;
    config.universal_spectrum = false;
    config.fixed_target = "Sapphire".to_owned();
    config
}

struct Harness {
    commands: Sender<TrainerCommand>,
    events: Receiver<WorkerEvent>,
    active: Arc<AtomicBool>,
    thread: thread::JoinHandle<Result<(), String>>,
}

impl Harness {
    fn start(loader: DatasetLoader) -> Self {
        let (commands, cmd_rx) = channel();
        let (event_tx, events) = channel();
        let active = Arc::new(AtomicBool::new(false));
        let flag = active.clone();
        let thread =
            thread::spawn(move || run_worker(&cmd_rx, &event_tx, &flag, loader, &tiny_config()));
        Self {
            commands,
            events,
            active,
            thread,
        }
    }

    fn send(&self, command: TrainerCommand) {
        self.commands.send(command).expect("the worker is alive");
    }

    /// The first following event `pick` accepts; earlier ones are skipped.
    fn wait_for<T>(&self, mut pick: impl FnMut(WorkerEvent) -> Option<T>) -> T {
        loop {
            match self.events.recv_timeout(WAIT) {
                Ok(WorkerEvent::Died(reason)) => panic!("the worker died: {reason}"),
                Ok(event) => {
                    if let Some(found) = pick(event) {
                        return found;
                    }
                }
                Err(e) => panic!("no matching event arrived: {e:?}"),
            }
        }
    }

    /// The first following snapshot that satisfies `accept`.
    fn snapshot(&self, mut accept: impl FnMut(&ProgressSnapshot) -> bool) -> ProgressSnapshot {
        self.wait_for(|event| match event {
            WorkerEvent::Metrics(s) if accept(&s) => Some(*s),
            _ => None,
        })
    }

    /// Waits for the log line with this message.
    fn log_line(&self, text: &str) {
        self.wait_for(|event| match event {
            WorkerEvent::Log { message, .. } if message.contains(text) => Some(()),
            _ => None,
        });
    }

    /// Pauses and returns the snapshot that answers the pause.
    fn pause(&self) -> ProgressSnapshot {
        self.send(TrainerCommand::Pause);
        self.log_line("Training paused");
        self.snapshot(|_| true)
    }

    fn stop(self) {
        self.active.store(false, Ordering::SeqCst);
        self.send(TrainerCommand::Shutdown);
        self.thread
            .join()
            .expect("the worker thread does not panic")
            .expect("the worker ends cleanly");
    }
}

#[test]
fn the_worker_loop_speaks_the_protocol() {
    let worker = Harness::start(fixture_loader);

    // Start-up: the data set, the first design, the first snapshot, the backend.
    let info = worker.wait_for(|e| match e {
        WorkerEvent::Dataset(result) => Some(result.expect("the fixture loads")),
        _ => None,
    });
    assert_eq!((info.train, info.val, info.test), (1, 1, 1));
    assert_eq!(info.designs.len(), 1);
    worker.wait_for(|e| match e {
        WorkerEvent::DesignSelected {
            index: 0,
            tier_count,
            split: DesignSplit::Train,
            ..
        } if tier_count > 0 => Some(()),
        _ => None,
    });
    let first = worker.snapshot(|_| true);
    assert_eq!(first.iteration, 0);
    assert!(!first.running && !first.weights_dirty);
    assert_eq!(first.run_generation, 0);

    // Start: metrics and a point arrive, the weights are dirty.
    worker.send(TrainerCommand::Start);
    let trained = worker.snapshot(|s| s.running && s.iteration >= 1);
    assert!(trained.weights_dirty);
    let point = worker.wait_for(|e| match e {
        WorkerEvent::Point(point) => Some(point),
        _ => None,
    });
    assert!(point.iteration >= 1);

    // Pause stops it; a single step runs exactly one iteration.
    let paused = worker.pause();
    assert!(!paused.running);
    worker.send(TrainerCommand::StepOnce);
    let stepped = worker.snapshot(|s| s.iteration > paused.iteration);
    assert_eq!(stepped.iteration, paused.iteration + 1);
    assert!(!stepped.running);
    assert_eq!(worker.pause().iteration, stepped.iteration);

    // Reset starts the statistics over: the generation moves, the weights stay dirty.
    worker.send(TrainerCommand::Reset);
    let reset = worker.snapshot(|s| s.run_generation > stepped.run_generation);
    assert_eq!(reset.iteration, 0);
    assert!(reset.weights_dirty);

    // Saving answers with the absolute path and clears the dirty flag.
    let dir =
        std::env::temp_dir().join(format!("indicatrix-trainer-worker-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let target = dir.join("weights.json");
    worker.send(TrainerCommand::SaveWeights(target.clone()));
    worker.wait_for(|e| match e {
        WorkerEvent::Busy(Some(text)) if text.contains("Saving") => Some(()),
        _ => None,
    });
    let saved = worker.wait_for(|e| match e {
        WorkerEvent::WeightsSaved(result) => Some(result.expect("the checkpoint is written")),
        _ => None,
    });
    assert!(Path::new(&saved).is_absolute(), "{saved}");
    assert!(saved.ends_with("weights.json"), "{saved}");
    assert!(target.is_file());
    assert!(!worker.snapshot(|_| true).weights_dirty);
    worker.wait_for(|e| matches!(e, WorkerEvent::Busy(None)).then_some(()));

    // A path that cannot be written is reported, not hidden.
    worker.send(TrainerCommand::SaveWeights(
        dir.join("missing").join("w.json"),
    ));
    let refused = worker.wait_for(|e| match e {
        WorkerEvent::WeightsSaved(result) => Some(result),
        _ => None,
    });
    assert!(refused.is_err());

    // The telemetry export answers the same way.
    let export = dir.join("run.json");
    worker.send(TrainerCommand::ExportStats(export.clone()));
    let exported = worker.wait_for(|e| match e {
        WorkerEvent::StatsExported(result) => Some(result.expect("the stats are written")),
        _ => None,
    });
    assert!(Path::new(&exported).is_absolute() && export.is_file());

    // A new data set replaces the run: new generation, clean weights.
    worker.send(TrainerCommand::LoadDatabase(None));
    worker.wait_for(|e| match e {
        WorkerEvent::Dataset(result) => Some(result.expect("the fixture loads again")),
        _ => None,
    });
    let reloaded = worker.snapshot(|s| s.run_generation > reset.run_generation);
    assert!(!reloaded.weights_dirty);
    assert_eq!(reloaded.iteration, 0);

    std::fs::remove_dir_all(&dir).ok();
    worker.stop();
}

#[test]
fn a_worker_without_a_data_set_waits_and_refuses_politely() {
    let worker = Harness::start(missing_loader);
    let reason = worker.wait_for(|e| match e {
        WorkerEvent::Dataset(result) => Some(result.expect_err("the load fails")),
        _ => None,
    });
    assert!(reason.contains("no database here"), "{reason}");

    // A start does nothing but log one error.
    worker.send(TrainerCommand::Start);
    worker.wait_for(|e| match e {
        WorkerEvent::Log { level, message } if level == "ERROR" && message.contains("data set") => {
            Some(())
        }
        _ => None,
    });
    assert!(!worker.active.load(Ordering::SeqCst));

    // File commands are answered with a failure instead of hanging the UI.
    worker.send(TrainerCommand::SaveWeights(PathBuf::from("w.json")));
    worker.wait_for(|e| match e {
        WorkerEvent::WeightsSaved(Err(_)) => Some(()),
        _ => None,
    });

    // Another failing load is reported again and the worker stays alive.
    worker.send(TrainerCommand::LoadDatabase(Some(PathBuf::from(
        "other.sqlite",
    ))));
    worker.wait_for(|e| match e {
        WorkerEvent::Dataset(Err(_)) => Some(()),
        _ => None,
    });
    worker.stop();
}
