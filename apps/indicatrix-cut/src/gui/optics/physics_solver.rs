//! The physics-color solver worker: one background thread, latest request wins.
//!
//! The UI thread never solves. [`SolverWorker::submit`] hands a [`SolveJob`] over a channel and
//! returns at once; the worker coalesces whatever queued up behind the job it is about to start
//! (only the newest survives), runs it, and reports the result through the `on_done` callback
//! unless a newer submission cancelled it meanwhile. A superseded result is *dropped*, never
//! reported -- and the receiving side checks the job's generation against the state's latest as
//! a second guard (`PhysicsState::finish_solve`).

use std::{
    sync::{
        atomic::Ordering,
        mpsc::{self, Receiver, Sender},
    },
    thread,
};

use indicatrix::optics::chromophore::{ChromophoreCatalogue, SolveResult};
use indicatrix_cut_core::material::color::solve_cancellable;

use super::physics_state::SolveJob;

/// A finished solve, with the generation of the job that produced it.
#[derive(Debug, Clone)]
pub struct SolveOutcome {
    /// The job's generation stamp.
    pub generation: u64,
    /// The solver's answer.
    pub result: SolveResult,
}

/// Handle to the solver thread. Dropping it ends the thread once its queue is drained.
pub struct SolverWorker {
    tx: Sender<SolveJob>,
}

/// The real solve: the cancellable `solve_physics_with` through the cut-core wrapper. `None`
/// when the job's cancel flag was set mid-solve (the solver returns nothing partial).
fn solve_real(job: &SolveJob) -> Option<SolveResult> {
    solve_cancellable(
        ChromophoreCatalogue::global(),
        &job.host,
        job.target_lab,
        job.reference_path_mm,
        &job.locked,
        &job.cancel,
    )
}

impl SolverWorker {
    /// Starts the worker; `on_done` is called on the worker thread with each result that was
    /// not superseded.
    #[must_use]
    pub fn spawn(on_done: impl Fn(SolveOutcome) + Send + 'static) -> Self {
        Self::spawn_with(solve_real, on_done)
    }

    /// [`Self::spawn`] with an injectable solve function (tests).
    #[must_use]
    pub fn spawn_with(
        solve: impl Fn(&SolveJob) -> Option<SolveResult> + Send + 'static,
        on_done: impl Fn(SolveOutcome) + Send + 'static,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<SolveJob>();
        let _ = thread::Builder::new()
            .name("physics-color-solver".to_string())
            .spawn(move || run(&rx, &solve, &on_done));
        Self { tx }
    }

    /// Queues `job`. Never blocks. Cancelling older jobs is the caller's `cancel` token
    /// (`PhysicsState` flips the previous job's token when it issues a new one).
    pub fn submit(&self, job: SolveJob) {
        let _ = self.tx.send(job);
    }
}

fn run(
    rx: &Receiver<SolveJob>,
    solve: &impl Fn(&SolveJob) -> Option<SolveResult>,
    on_done: &impl Fn(SolveOutcome),
) {
    while let Ok(mut job) = rx.recv() {
        // Coalesce: only the newest queued job is worth running.
        while let Ok(newer) = rx.try_recv() {
            job = newer;
        }
        if job.cancel.load(Ordering::SeqCst) {
            continue;
        }
        // `None`: the solver itself noticed the cancel flag and stopped.
        let Some(result) = solve(&job) else {
            continue;
        };
        // A newer submission arrived while solving: this result is stale -- discard it.
        if job.cancel.load(Ordering::SeqCst) {
            continue;
        }
        on_done(SolveOutcome {
            generation: job.generation,
            result,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{color::body_color::Bodycolor, optics::chromophore::colorRecipe};
    use std::{
        sync::{Arc, Mutex, atomic::AtomicBool},
        time::Duration,
    };

    fn job(generation: u64) -> SolveJob {
        SolveJob {
            generation,
            host: "h".to_string(),
            target_lab: [50.0, 0.0, 0.0],
            reference_path_mm: 5.0,
            locked: Vec::new(),
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    fn fake_result(tag: &str) -> SolveResult {
        SolveResult {
            recipe: colorRecipe::new(tag, 1),
            achieved: Bodycolor {
                xyz: [0.0; 3],
                lab: [0.0; 3],
                srgb: [0; 3],
                out_of_gamut: false,
            },
            delta_e: 0.0,
            delta_e_a: None,
            reachable: true,
            capped: false,
            evals: 0,
        }
    }

    /// Two quick picks: the first is cancelled mid-solve and dropped, the latest wins, and the
    /// submitting side never blocks while the solver is busy.
    #[test]
    fn the_latest_request_wins_and_stale_results_are_dropped() {
        let (gate_tx, gate_rx) = mpsc::channel::<()>();
        let gate_rx = Mutex::new(gate_rx);
        let (done_tx, done_rx) = mpsc::channel::<u64>();
        let done_tx = Mutex::new(done_tx);
        let worker = SolverWorker::spawn_with(
            move |j| {
                // Job 1 blocks until released, simulating a slow solve.
                if j.generation == 1 {
                    let _ = gate_rx.lock().unwrap().recv_timeout(Duration::from_secs(5));
                }
                Some(fake_result(&j.host))
            },
            move |outcome| {
                let _ = done_tx.lock().unwrap().send(outcome.generation);
            },
        );
        let first = job(1);
        let first_cancel = Arc::clone(&first.cancel);
        let started = std::time::Instant::now();
        worker.submit(first);
        std::thread::sleep(Duration::from_millis(50));
        // A newer job supersedes the running one (what `PhysicsState` does).
        first_cancel.store(true, Ordering::SeqCst);
        worker.submit(job(2));
        worker.submit(job(3));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "submit must not block"
        );
        gate_tx.send(()).unwrap();
        let mut got = Vec::new();
        while let Ok(g) = done_rx.recv_timeout(Duration::from_millis(500)) {
            got.push(g);
        }
        assert_eq!(got.last(), Some(&3), "the latest request wins: {got:?}");
        assert!(!got.contains(&1), "the cancelled solve is dropped: {got:?}");
        assert!(
            !got.contains(&2),
            "the coalesced request never runs: {got:?}"
        );
    }
}
