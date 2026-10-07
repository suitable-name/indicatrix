//! The sweep's worker thread: starting it, and looking at it from the UI thread.
//!
//! The sweep itself ([`indicatrix_editor::sweep::sweep_tier_angle`]) runs on one thread of
//! its own, which fans out over scoped worker threads. The UI thread never waits for it:
//! a timer polls the [`RunHandle`] every so often for the progress and, at the end, the
//! outcome. (Polling rather than `upgrade_in_event_loop`, because the callbacks that use
//! the outcome hold `Rc`s, which cannot be sent to the worker.)

use crate::gui::editor::material_lookup::{EditorMaterialLookup, resolved_gem_material};
use indicatrix::optics::{LightingPreset, materials::GemMaterial};
use indicatrix_cut_core::{
    Design, MaterialSelection,
    optimize::{CANONICAL_LIGHT_PITCH, CANONICAL_LIGHT_YAW},
};
use indicatrix_editor::sweep::{
    SweepOptions, SweepOutcome, SweepPlan, SweepScene, sweep_tier_angle,
};
use std::{
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread::{self, JoinHandle},
};

/// Everything a sweep reads, owned so it can move to the worker thread.
pub(super) struct Job {
    /// The design as it is now; every angle is solved on a clone of it.
    pub design: Design,
    pub plan: SweepPlan,
    /// The design's material (with its refractive index filled in when it names none).
    pub material: MaterialSelection,
    /// The custom catalogue materials the selection may name.
    pub custom_materials: Vec<GemMaterial>,
    /// The lighting the figures are scored under (the viewport's choice).
    pub lighting: LightingPreset,
    pub options: SweepOptions,
}

/// What a look at a running sweep finds.
#[derive(Debug)]
pub(super) enum Poll {
    /// Still going: how many angles are done.
    Running { done: usize },
    /// Over, finished or stopped.
    Finished(SweepOutcome),
    /// The worker thread ended without an outcome (it panicked).
    Died,
}

/// A running sweep, seen from the UI thread. Dropping it stops the sweep.
pub(super) struct RunHandle {
    cancel: Arc<AtomicBool>,
    done: Arc<AtomicUsize>,
    total: usize,
    result: Arc<Mutex<Option<SweepOutcome>>>,
    worker: Option<JoinHandle<()>>,
}

impl RunHandle {
    /// How many angles the sweep tries.
    pub(super) const fn total(&self) -> usize {
        self.total
    }

    /// Asks the sweep to stop; the rows finished so far still come back from [`Self::poll`].
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Looks at the sweep: still running, over, or dead.
    pub(super) fn poll(&mut self) -> Poll {
        // Whether the thread has ended is read before the outcome: the thread stores the
        // outcome before it ends, so an ended thread with no outcome really died.
        let ended = self.worker.as_ref().is_some_and(JoinHandle::is_finished);
        let outcome = self
            .result
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        match (outcome, ended) {
            (Some(outcome), _) => {
                self.reap();
                Poll::Finished(outcome)
            }
            (None, true) => {
                self.reap();
                Poll::Died
            }
            (None, false) => Poll::Running {
                done: self.done.load(Ordering::Relaxed),
            },
        }
    }

    /// Joins the ended worker thread.
    fn reap(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for RunHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// Starts `job` on a worker thread.
///
/// # Errors
///
/// A sentence for the dialog when the operating system refuses the thread.
pub(super) fn start(job: Job) -> Result<RunHandle, String> {
    let cancel = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicUsize::new(0));
    let result = Arc::new(Mutex::new(None));
    let total = job.plan.angles.len();
    let worker = {
        let cancel = Arc::clone(&cancel);
        let done = Arc::clone(&done);
        let result = Arc::clone(&result);
        thread::Builder::new()
            .name("angle-sweep".to_owned())
            .spawn(move || {
                let lookup = EditorMaterialLookup::new(&job.custom_materials);
                let material = resolved_gem_material(&job.material, &lookup);
                let scene = SweepScene {
                    material: &material,
                    environment: job.lighting.studio(
                        1.0,
                        CANONICAL_LIGHT_YAW,
                        CANONICAL_LIGHT_PITCH,
                    ),
                };
                let progress = |finished: usize, _total: usize| {
                    done.store(finished, Ordering::Relaxed);
                };
                let outcome = sweep_tier_angle(
                    &job.design,
                    &job.plan,
                    &scene,
                    &job.options,
                    &cancel,
                    &progress,
                );
                *result.lock().unwrap_or_else(PoisonError::into_inner) = Some(outcome);
            })
            .map_err(|error| format!("The sweep could not start: {error}."))?
    };
    Ok(RunHandle {
        cancel,
        done,
        total,
        result,
        worker: Some(worker),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use indicatrix_editor::sweep::{SweepRange, plan_sweep};
    use std::time::{Duration, Instant};

    fn job(workers: usize) -> Job {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.ensure_tier_ids();
        let plan = plan_sweep(
            &design,
            5,
            SweepRange {
                from_deg: -42.0,
                to_deg: -40.0,
                step_deg: 1.0,
            },
        )
        .expect("a pavilion sweep");
        Job {
            design,
            plan,
            material: MaterialSelection::none(),
            custom_materials: Vec::new(),
            lighting: LightingPreset::default(),
            options: SweepOptions {
                tilt_average: false,
                workers,
            },
        }
    }

    fn wait_for(handle: &mut RunHandle) -> Poll {
        let give_up = Instant::now() + Duration::from_secs(120);
        loop {
            match handle.poll() {
                Poll::Running { .. } if Instant::now() < give_up => {
                    thread::sleep(Duration::from_millis(20));
                }
                other => return other,
            }
        }
    }

    #[test]
    fn a_started_sweep_is_polled_to_its_outcome() {
        let mut handle = start(job(2)).expect("the thread starts");
        assert_eq!(handle.total(), 3);
        let Poll::Finished(outcome) = wait_for(&mut handle) else {
            panic!("the sweep should finish");
        };
        assert!(!outcome.cancelled);
        assert_eq!(outcome.rows.len(), 3);
        assert_eq!(outcome.tier, 5);
        assert_eq!(outcome.rows.iter().filter(|row| row.is_current).count(), 1);
    }

    #[test]
    fn a_cancelled_sweep_still_hands_back_what_it_finished() {
        let mut handle = start(job(1)).expect("the thread starts");
        handle.cancel();
        let Poll::Finished(outcome) = wait_for(&mut handle) else {
            panic!("a cancelled sweep still ends with an outcome");
        };
        assert!(outcome.rows.len() <= 3);
        assert_eq!(outcome.cancelled, outcome.rows.len() < 3);
    }
}
