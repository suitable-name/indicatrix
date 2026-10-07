//! The multi-start driver: several descents from different starting points of the
//! search box, run side by side, merged into one result. See the parent module's
//! "Several starts" section for the strategy; this file is the generic machinery.
//!
//! The driver knows nothing about [`crate::design::Design`]: it works on free-angle
//! vectors (in the run's free-tier order) and asks a [`StartEngine`] to score a point,
//! to descend from one and to polish one. The real engine lives in
//! `search::multi`; the tests use a synthetic two-basin function.
//!
//! Everything the result depends on is decided before a wave begins (per-start
//! budgets, seeds, starting points) or at a wave barrier from per-start evaluation
//! counts, and results are merged in start-index order, so the outcome is the same for
//! any number of lanes.

use super::{
    pool::CandidatePool,
    search::{SearchStage, StartProgress, splitmix64_next},
};
use std::{
    cell::Cell,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// The number of primes tabulated for the Halton bases; more free tiers than this fall
/// back to plain splitmix64 uniforms (such designs never multistart, see the cost rule).
const MAX_HALTON_DIMS: usize = 100;

/// Every [`LOCAL_PERIOD`]-th screening draw is a local one (a box shrunk around the
/// incumbent); the rest are global.
const LOCAL_PERIOD: usize = 3;

/// How much of the box a local draw spans, as a fraction of each side's reach from the
/// incumbent.
const LOCAL_FRACTION: f64 = 0.3;

/// Screening draws per extra start.
const DRAWS_PER_START: usize = 4;

/// The most screening draws a run makes.
const MAX_DRAWS: usize = 64;

/// Sweeps' worth of evaluations (per free tier) every start must be able to afford.
const MIN_SWEEPS_PER_START: usize = 8;

/// Tolerance so a gap of exactly the separation counts as distinct.
const SEPARATION_TOLERANCE_DEG: f64 = 1e-9;

/// How many starts a run really uses: `starts` capped so every start gets at least
/// `8 * free_tier_count` evaluations of `budget`, and never below one (`0` and `1` both
/// mean the single descent).
pub(super) fn effective_starts(starts: usize, budget: usize, free_tier_count: usize) -> usize {
    if starts <= 1 || free_tier_count == 0 {
        return 1;
    }
    starts
        .min(budget / (MIN_SWEEPS_PER_START * free_tier_count))
        .max(1)
}

/// The screening draws of a run with `starts` effective starts: four per extra start,
/// at most 64, none for a single start.
pub(super) fn screening_draws(starts: usize) -> usize {
    if starts <= 1 {
        0
    } else {
        (DRAWS_PER_START * (starts - 1)).min(MAX_DRAWS)
    }
}

/// How many starts get a polish stage: the best `max(1, keep_candidates)`, never more
/// than there are starts.
pub(super) fn polished_starts(starts: usize, keep_candidates: usize) -> usize {
    starts.min(keep_candidates.max(1))
}

/// `index` read in `base` and mirrored about the radix point: the van der Corput term.
fn radical_inverse(mut index: u64, base: u64) -> f64 {
    let mut fraction = 1.0_f64;
    let mut value = 0.0_f64;
    let base_f = base as f64;
    while index > 0 {
        fraction /= base_f;
        value = fraction.mul_add((index % base) as f64, value);
        index /= base;
    }
    value
}

/// The first `count` primes.
fn first_primes(count: usize) -> Vec<u64> {
    let mut primes: Vec<u64> = Vec::with_capacity(count);
    let mut candidate = 2_u64;
    while primes.len() < count {
        if primes.iter().all(|&p| !candidate.is_multiple_of(p)) {
            primes.push(candidate);
        }
        candidate += 1;
    }
    primes
}

/// A uniform `[0, 1)` value from 64 random bits.
fn unit_from_bits(bits: u64) -> f64 {
    (bits >> 11) as f64 / (1_u64 << 53) as f64
}

/// `count` points of a scrambled Halton sequence in `dims` dimensions, every
/// coordinate in `[0, 1)`: the first `dims` primes as bases, the points indexed from one,
/// each dimension shifted modulo one by a constant drawn from `seed` (a Cranley-Patterson
/// rotation). Deterministic for `(seed, dims, count)`; the first `n` points do not
/// depend on `count`. More than 100 dimensions fall back to splitmix64 uniforms.
pub(super) fn halton_points(seed: u64, dims: usize, count: usize) -> Vec<Vec<f64>> {
    let mut state = seed ^ 0xA5A5_A5A5_A5A5_A5A5;
    if dims > MAX_HALTON_DIMS {
        return (0..count)
            .map(|_| {
                (0..dims)
                    .map(|_| unit_from_bits(splitmix64_next(&mut state)))
                    .collect()
            })
            .collect();
    }
    let shifts: Vec<f64> = (0..dims)
        .map(|_| unit_from_bits(splitmix64_next(&mut state)))
        .collect();
    let bases = first_primes(dims);
    (0..count)
        .map(|point| {
            bases
                .iter()
                .zip(&shifts)
                .map(|(&base, &shift)| {
                    let value = radical_inverse(point as u64 + 1, base) + shift;
                    value - value.floor()
                })
                .collect()
        })
        .collect()
}

/// Maps a unit-cube `point` into the per-tier `boxes`. A `local` draw uses a box shrunk
/// to 30 % of each side's reach from the incumbent's angle (still inside the original
/// box); a global draw uses the whole box.
pub(super) fn map_point(
    point: &[f64],
    incumbent: &[f64],
    boxes: &[(f64, f64)],
    local: bool,
) -> Vec<f64> {
    point
        .iter()
        .zip(incumbent)
        .zip(boxes)
        .map(|((&unit, &current), &(low, high))| {
            let (lo, hi) = if local {
                (
                    LOCAL_FRACTION
                        .mul_add(low - current, current)
                        .clamp(low, high),
                    LOCAL_FRACTION
                        .mul_add(high - current, current)
                        .clamp(low, high),
                )
            } else {
                (low, high)
            };
            unit.mul_add(hi - lo, lo).clamp(low, high)
        })
        .collect()
}

/// Whether some angle of `a` and `b` differs by at least `separation_deg`.
fn distinct(separation_deg: f64, a: &[f64], b: &[f64]) -> bool {
    a.iter()
        .zip(b)
        .any(|(x, y)| (x - y).abs() >= separation_deg - SEPARATION_TOLERANCE_DEG)
}

/// A free-angle vector with its `Fast` score.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StartPoint {
    pub(super) angles: Vec<f64>,
    pub(super) score: f32,
}

/// One start's state: where its descent (and polish) ended.
#[derive(Debug, Clone)]
pub(super) struct StartState {
    /// The free angles the start ended on.
    pub(super) angles: Vec<f64>,
    /// Their `Fast` score (infinite for a start that could not be built).
    pub(super) score: f32,
    /// Evaluations the coordinate stage spent.
    pub(super) evaluations: usize,
    /// Evaluations the polish stage spent (`0` before the polish stage).
    pub(super) polish_evaluations: usize,
    /// The score improvement the polish stage adopted.
    pub(super) polish_improvement: f32,
    /// Whether the cancel flag cut the start short.
    pub(super) cancelled: bool,
    /// The distinct good states the start saw.
    pub(super) pool: CandidatePool,
}

/// What a [`StartEngine::descend`] call is asked to do.
pub(super) struct DescentRun<'a> {
    /// The free angles to start from.
    pub(super) angles: &'a [f64],
    /// Their `Fast` score.
    pub(super) score: f32,
    /// The evaluation budget of this descent.
    pub(super) max_evaluations: usize,
    /// The seed of this descent's sweep orders.
    pub(super) seed: u64,
}

/// What the driver needs from the problem it searches.
///
/// `Sync` because the lanes of a wave share one engine; everything an implementation
/// does must be a pure function of its arguments (no timing, no shared mutation).
pub(super) trait StartEngine: Sync {
    /// The `Fast` score of the free-angle vector `angles`, `None` when the point is
    /// rejected (unsafe angle, out of bounds, does not solve, closes badly, ...).
    fn score(&self, angles: &[f64]) -> Option<f32>;
    /// Coordinate descent from `run.angles`; `progress` receives this descent's own
    /// running evaluation count and stage.
    fn descend(&self, run: &DescentRun<'_>, progress: &dyn Fn(usize, SearchStage)) -> StartState;
    /// The polish stage on a finished descent. `progress` receives the running count
    /// STARTING from `state.evaluations` (the same convention as the single search).
    fn polish(&self, state: &StartState, progress: &dyn Fn(usize, SearchStage)) -> StartState;
    /// A fresh, empty candidate pool configured like the run's.
    fn new_pool(&self) -> CandidatePool;
}

/// The knobs of one multi-start run.
pub(super) struct MultiSpec<'a> {
    /// Effective number of starts (at least two for a multi-start run).
    pub(super) starts: usize,
    /// Lane override; `0` picks half the available parallelism.
    pub(super) max_lanes: usize,
    /// The coordinate-stage evaluation budget to split between the starts.
    pub(super) budget: usize,
    /// How many of the best starts get a polish stage.
    pub(super) polish_keep: usize,
    /// The run's seed.
    pub(super) seed: u64,
    /// Two states count as different starts only when some angle differs by this much.
    pub(super) separation_deg: f64,
    /// The cancel flag, polled between draws and by every descent.
    pub(super) cancel: Option<&'a AtomicBool>,
}

/// The caller's callbacks, invoked on the calling thread only.
pub(super) struct DriverHooks<'a> {
    pub(super) report: Option<&'a dyn Fn(usize, SearchStage)>,
    pub(super) on_start: Option<&'a dyn Fn(StartProgress)>,
}

/// What a multi-start run found.
pub(super) struct MultiOutcome {
    /// The best state's free angles.
    pub(super) best_angles: Vec<f64>,
    /// Every start's pool, merged in start order.
    pub(super) pool: CandidatePool,
    /// Every evaluation spent: screening draws, descents and polishes.
    pub(super) evaluations: usize,
    /// How many of them were polish evaluations.
    pub(super) polish_evaluations: usize,
    /// The polish improvement of the best start.
    pub(super) polish_improvement: f32,
    /// Whether the cancel flag ended the run early.
    pub(super) cancelled: bool,
    /// How many starts ran their descent.
    pub(super) starts_run: usize,
    /// The start whose end point is the result.
    pub(super) best_start: usize,
}

/// The number of lanes a wave of `tasks` tasks uses: `max_lanes`, or half the available
/// parallelism, between one and `tasks`. Always one on `wasm32`.
#[cfg(not(target_arch = "wasm32"))]
fn lane_count(max_lanes: usize, tasks: usize) -> usize {
    let wanted = if max_lanes > 0 {
        max_lanes
    } else {
        std::thread::available_parallelism().map_or(1, |n| n.get() / 2)
    };
    wanted.clamp(1, tasks.max(1))
}

/// The `wasm32` arm of [`lane_count`]: no threads, one lane.
#[cfg(target_arch = "wasm32")]
const fn lane_count(_max_lanes: usize, _tasks: usize) -> usize {
    1
}

/// What every lane of one wave shares: how the tasks are dealt out, the per-lane
/// evaluation counters and the count already spent before the wave.
struct LaneSet<'a> {
    lanes: usize,
    count: usize,
    live: &'a [AtomicUsize],
    base: usize,
}

/// The loop one lane runs: tasks `lane`, `lane + lanes`, ... in order. `set.live[lane]`
/// holds the evaluations the lane has spent so far; the lane the caller's thread runs
/// (`report` given) forwards `set.base +` the sum of all lanes to the caller after every
/// progress call.
fn lane_loop<T, F>(
    set: &LaneSet<'_>,
    lane: usize,
    task: &F,
    report: Option<&dyn Fn(usize, SearchStage)>,
    on_task_start: Option<&dyn Fn(usize)>,
) -> Vec<(usize, T)>
where
    F: Fn(usize, &dyn Fn(usize, SearchStage)) -> T,
{
    let mut done = 0usize;
    let mut results = Vec::new();
    for index in (lane..set.count).step_by(set.lanes) {
        if let Some(start) = on_task_start {
            start(index);
        }
        let last = Cell::new(0usize);
        let progress = |evaluations: usize, stage: SearchStage| {
            last.set(evaluations);
            set.live[lane].store(done + evaluations, Ordering::Relaxed);
            if let Some(report) = report {
                let total: usize = set.live.iter().map(|c| c.load(Ordering::Relaxed)).sum();
                report(set.base + total, stage);
            }
        };
        results.push((index, task(index, &progress)));
        done += last.get();
        set.live[lane].store(done, Ordering::Relaxed);
    }
    results
}

/// Runs `count` independent tasks over `lanes` lanes and returns their results in task
/// order. The calling thread runs lane 0 itself (so `hooks.report` and `on_task_start`,
/// which are not `Sync`, only ever run there).
#[cfg(not(target_arch = "wasm32"))]
fn run_tasks<T, F>(
    count: usize,
    lanes: usize,
    base: usize,
    hooks: &DriverHooks<'_>,
    on_task_start: &dyn Fn(usize),
    task: F,
) -> Vec<T>
where
    T: Send,
    F: Fn(usize, &dyn Fn(usize, SearchStage)) -> T + Sync,
{
    let live: Vec<AtomicUsize> = (0..lanes).map(|_| AtomicUsize::new(0)).collect();
    let set = LaneSet {
        lanes,
        count,
        live: &live,
        base,
    };
    let mut results: Vec<(usize, T)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (1..lanes)
            .map(|lane| {
                let set = &set;
                let task = &task;
                scope.spawn(move || lane_loop(set, lane, task, None, None))
            })
            .collect();
        let mut all = lane_loop(&set, 0, &task, hooks.report, Some(on_task_start));
        for handle in handles {
            all.extend(handle.join().expect("start lane must not panic"));
        }
        all
    });
    results.sort_by_key(|&(index, _)| index);
    results.into_iter().map(|(_, value)| value).collect()
}

/// The `wasm32` arm of [`run_tasks`]: the tasks run one after the other in task order,
/// so the progress hook keeps its per-tier-decision cadence.
#[cfg(target_arch = "wasm32")]
fn run_tasks<T, F>(
    count: usize,
    _lanes: usize,
    base: usize,
    hooks: &DriverHooks<'_>,
    on_task_start: &dyn Fn(usize),
    task: F,
) -> Vec<T>
where
    T: Send,
    F: Fn(usize, &dyn Fn(usize, SearchStage)) -> T + Sync,
{
    let live = [AtomicUsize::new(0)];
    let set = LaneSet {
        lanes: 1,
        count,
        live: &live,
        base,
    };
    lane_loop(&set, 0, &task, hooks.report, Some(on_task_start))
        .into_iter()
        .map(|(_, value)| value)
        .collect()
}

/// A screened sample: its angles, `Fast` score and draw number.
struct Screened {
    angles: Vec<f64>,
    score: f32,
    draw: usize,
}

/// The seed of start `index`'s sweeps: the run's own for the incumbent (so start 0
/// sweeps exactly like the single-start search), a splitmix64 mix for the others.
const fn start_seed(seed: u64, index: usize) -> u64 {
    if index == 0 {
        return seed;
    }
    let mut state = index as u64;
    seed ^ splitmix64_next(&mut state)
}

/// The descents' outcome: every start's state in start order, the best `Fast` score any
/// reached and whether the cancel flag cut the waves short.
struct Waves {
    states: Vec<StartState>,
    best_score: f32,
    cancelled: bool,
}

/// One multi-start run: the engine, the knobs and callbacks, and the search box.
struct Driver<'a, E: StartEngine> {
    engine: &'a E,
    spec: &'a MultiSpec<'a>,
    hooks: &'a DriverHooks<'a>,
    boxes: &'a [(f64, f64)],
    incumbent: &'a StartPoint,
}

impl<E: StartEngine> Driver<'_, E> {
    fn is_cancelled(&self) -> bool {
        self.spec
            .cancel
            .is_some_and(|flag| flag.load(Ordering::Relaxed))
    }

    /// Phase S. Draws and screens the extra starting points: `screening_draws` draws
    /// from the box, each scored at `Fast` (one evaluation each, rejected ones too),
    /// ranked best first with ties by draw number. Also reports whether the cancel flag
    /// stopped the screening.
    fn screen(&self, evaluations: &mut usize) -> (Vec<Screened>, bool) {
        let draws = screening_draws(self.spec.starts);
        let unit = halton_points(self.spec.seed, self.boxes.len(), draws);
        let mut screened: Vec<Screened> = Vec::new();
        let mut cancelled = false;
        for (draw, point) in unit.iter().enumerate() {
            if self.is_cancelled() {
                cancelled = true;
                break;
            }
            let local = draw % LOCAL_PERIOD == 0;
            let angles = map_point(point, &self.incumbent.angles, self.boxes, local);
            if let Some(score) = self.engine.score(&angles) {
                screened.push(Screened {
                    angles,
                    score,
                    draw,
                });
            }
            *evaluations += 1;
            if let Some(report) = self.hooks.report {
                report(*evaluations, SearchStage::Screening);
            }
        }
        screened.sort_by(|a, b| a.score.total_cmp(&b.score).then(a.draw.cmp(&b.draw)));
        (screened, cancelled)
    }

    /// Splits the ranked samples into the starts (the incumbent first, then the best
    /// samples that are pairwise distinct, up to the effective start count) and the
    /// ranked reserve of the rest.
    fn pick_starts(&self, screened: Vec<Screened>) -> (Vec<StartPoint>, Vec<StartPoint>) {
        let mut starts = vec![self.incumbent.clone()];
        let mut reserve = Vec::new();
        for sample in screened {
            let point = StartPoint {
                angles: sample.angles,
                score: sample.score,
            };
            let apart = starts
                .iter()
                .all(|s| distinct(self.spec.separation_deg, &s.angles, &point.angles));
            if starts.len() < self.spec.starts && apart {
                starts.push(point);
            } else {
                reserve.push(point);
            }
        }
        (starts, reserve)
    }

    /// Phases C and X. Runs the descents in waves: the planned starts first, each with
    /// an equal share of the budget, then, while at least `8 * free` evaluations are
    /// left over (descents stop early on plateaus), the next best reserve samples,
    /// until the budget is spent, the reserve is empty or a whole extra wave improved
    /// nothing. Every decision is taken at a wave barrier from per-start counts.
    fn descend_waves(
        &self,
        starts: Vec<StartPoint>,
        mut reserve: Vec<StartPoint>,
        evaluations: &mut usize,
    ) -> Waves {
        let planned = starts.len();
        let per_start = (self.spec.budget / planned).max(1);
        let wave_floor = MIN_SWEEPS_PER_START * self.boxes.len();
        let mut run_points: Vec<Vec<f64>> = starts.iter().map(|s| s.angles.clone()).collect();
        let mut waves = Waves {
            states: Vec::new(),
            best_score: self.incumbent.score,
            cancelled: false,
        };
        let mut wave_tasks = starts;
        let mut wave_budget = per_start;
        let mut wave_number = 0usize;
        while !wave_tasks.is_empty() {
            let first = waves.states.len();
            let count_now = planned.max(first + wave_tasks.len());
            let best_now = waves.best_score;
            let announce = |task: usize| {
                if let Some(on_start) = self.hooks.on_start {
                    on_start(StartProgress {
                        index: first + task,
                        count: count_now,
                        best_fast_score: best_now,
                    });
                }
            };
            let engine = self.engine;
            let seed = self.spec.seed;
            let wave: Vec<StartState> = run_tasks(
                wave_tasks.len(),
                lane_count(self.spec.max_lanes, wave_tasks.len()),
                *evaluations,
                self.hooks,
                &announce,
                |task, progress| {
                    let point = &wave_tasks[task];
                    engine.descend(
                        &DescentRun {
                            angles: &point.angles,
                            score: point.score,
                            max_evaluations: wave_budget,
                            seed: start_seed(seed, first + task),
                        },
                        progress,
                    )
                },
            );
            let wave_best = wave
                .iter()
                .map(|state| state.score)
                .min_by(f32::total_cmp)
                .unwrap_or(f32::INFINITY);
            *evaluations += wave.iter().map(|state| state.evaluations).sum::<usize>();
            waves.cancelled |= wave.iter().any(|state| state.cancelled) || self.is_cancelled();
            let improved = wave_best < waves.best_score;
            waves.best_score = waves.best_score.min(wave_best);
            waves.states.extend(wave);
            wave_number += 1;

            let spent: usize = waves.states.iter().map(|state| state.evaluations).sum();
            let leftover = self.spec.budget.saturating_sub(spent);
            if waves.cancelled || (wave_number > 1 && !improved) || leftover < wave_floor {
                break;
            }
            let wanted = (leftover / per_start).max(1);
            let mut next: Vec<StartPoint> = Vec::new();
            reserve.retain(|candidate| {
                let take = next.len() < wanted
                    && run_points
                        .iter()
                        .all(|p| distinct(self.spec.separation_deg, p, &candidate.angles));
                if take {
                    run_points.push(candidate.angles.clone());
                    next.push(candidate.clone());
                }
                !take
            });
            if next.is_empty() {
                break;
            }
            wave_budget = (leftover / next.len()).clamp(1, per_start);
            wave_tasks = next;
        }
        waves
    }

    /// Phase P. Polishes the best `max(1, keep_candidates)` starts by their
    /// coordinate-stage score (ties by start index) in place. Returns the polish
    /// evaluations spent and whether the cancel flag cut a polish short.
    fn polish_best(
        &self,
        states: &mut [StartState],
        evaluations: usize,
        best_score: f32,
    ) -> (usize, bool) {
        let mut ranked: Vec<usize> = (0..states.len()).collect();
        ranked.sort_by(|&a, &b| states[a].score.total_cmp(&states[b].score).then(a.cmp(&b)));
        ranked.truncate(polished_starts(states.len(), self.spec.polish_keep));
        ranked.sort_unstable();
        let count_now = states.len();
        let announce = |task: usize| {
            if let Some(on_start) = self.hooks.on_start {
                on_start(StartProgress {
                    index: ranked[task],
                    count: count_now,
                    best_fast_score: best_score,
                });
            }
        };
        let engine = self.engine;
        let polished: Vec<StartState> = run_tasks(
            ranked.len(),
            lane_count(self.spec.max_lanes, ranked.len()),
            evaluations,
            self.hooks,
            &announce,
            |task, progress| {
                let state = &states[ranked[task]];
                let offset = state.evaluations;
                engine.polish(state, &|count, stage| {
                    progress(count.saturating_sub(offset), stage);
                })
            },
        );
        let mut polish_evaluations = 0usize;
        let mut cancelled = false;
        for (&slot, state) in ranked.iter().zip(polished) {
            polish_evaluations += state.polish_evaluations;
            cancelled |= state.cancelled;
            states[slot] = state;
        }
        (polish_evaluations, cancelled)
    }

    /// Phase F. Picks the best state (lowest score, ties by start index; the incumbent
    /// when no start ran) and merges every start's pool in start order.
    fn finish(
        &self,
        states: Vec<StartState>,
        evaluations: usize,
        polish_evaluations: usize,
        cancelled: bool,
    ) -> MultiOutcome {
        let mut best_start = 0usize;
        for (index, state) in states.iter().enumerate() {
            if state.score.total_cmp(&states[best_start].score).is_lt() {
                best_start = index;
            }
        }
        let starts_run = states.len().max(1);
        let (best_angles, polish_improvement) = states.get(best_start).map_or_else(
            || (self.incumbent.angles.clone(), 0.0),
            |state| (state.angles.clone(), state.polish_improvement),
        );
        let mut pool = self.engine.new_pool();
        for state in states {
            for entry in state.pool.into_entries() {
                pool.offer(entry.angles, entry.score);
            }
        }
        MultiOutcome {
            best_angles,
            pool,
            evaluations,
            polish_evaluations,
            polish_improvement,
            cancelled,
            starts_run,
            best_start,
        }
    }
}

/// Runs the whole multi-start search; see the parent module's "Several starts" section.
///
/// `incumbent` is the design's own free angles and `Fast` score (start 0), `boxes` the
/// per-free-tier box the extra starts are drawn from.
pub(super) fn run_multistart<E: StartEngine>(
    engine: &E,
    spec: &MultiSpec<'_>,
    incumbent: &StartPoint,
    boxes: &[(f64, f64)],
    hooks: &DriverHooks<'_>,
) -> MultiOutcome {
    let driver = Driver {
        engine,
        spec,
        hooks,
        boxes,
        incumbent,
    };
    let mut evaluations = 0usize;
    let (screened, screening_cancelled) = driver.screen(&mut evaluations);
    if screening_cancelled {
        return driver.finish(Vec::new(), evaluations, 0, true);
    }
    let (starts, reserve) = driver.pick_starts(screened);
    let mut waves = driver.descend_waves(starts, reserve, &mut evaluations);
    let mut polish_evaluations = 0usize;
    if !waves.cancelled {
        let (spent, cancelled) =
            driver.polish_best(&mut waves.states, evaluations, waves.best_score);
        polish_evaluations = spent;
        evaluations += spent;
        waves.cancelled |= cancelled;
    }
    driver.finish(
        waves.states,
        evaluations,
        polish_evaluations,
        waves.cancelled,
    )
}
