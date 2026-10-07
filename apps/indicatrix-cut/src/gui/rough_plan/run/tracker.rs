//! Maps the core's progress events onto the planner window's stage line and progress bar.
//!
//! The parallel stages report from several threads at once, so the tracker counts events
//! instead of trusting the `done` figure of any single one, and the bar never moves back.

use super::Reporter;
use crate::{
    gui::rough_plan::format::group_thousands,
    plan_limit::{PlanDeadline, remaining_text},
};
use indicatrix_cut_core::rough_plan::{CutOrder, FitStage, PlanPath, PlanProgress, REFINE_TOP};
use std::{
    sync::{
        OnceLock,
        atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering},
    },
    time::Instant,
};

/// How many cut orders the plan runs.
pub(super) const ORDER_COUNT: usize = CutOrder::ALL.len();

/// What the driver tells the progress sink about the work ahead.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Note {
    /// The piece-table stages will report this many `Grid` events in all.
    Grid(usize),
    /// The cut-order DPs will report this many `Dp` ticks in all.
    Dp(usize),
    /// A fit stage will report this many designs in all (over every lane).
    Fit(FitStage, usize),
    /// One cut order's DP has returned.
    OrderFinished,
}

/// Where a plan's progress goes. Shared by reference across the worker's lanes.
pub(super) trait Progress: Sync {
    /// Records a core event. Returns `false` when the plan should stop.
    fn event(&self, event: PlanProgress) -> bool;

    /// Records an expectation or a completed unit of parallel work.
    fn note(&self, note: Note);

    /// Makes every later [`Progress::event`] return `false`, so sibling lanes stop once
    /// one lane has panicked and the plan is lost.
    fn abort(&self);

    /// Starts the clock of the scan plan time limit. The driver calls it once, for a mesh
    /// rough only, so a convex or hull rough never has a deadline. A sink without a limit
    /// ignores it.
    fn arm_deadline(&self) {}

    /// Whether the time limit (and not the user) stopped the plan: some [`Progress::event`]
    /// returned `false` because the deadline had passed.
    fn time_stopped(&self) -> bool {
        false
    }
}

/// A stretch of the progress bar: `start + len * t` for `t` in `0..=1`.
#[derive(Debug, Clone, Copy)]
struct Span {
    start: f32,
    len: f32,
}

impl Span {
    const fn new(start: f32, len: f32) -> Self {
        Self { start, len }
    }

    const fn at(self, t: f32) -> f32 {
        self.len.mul_add(t, self.start)
    }
}

/// Where each planning stage sits on the bar (after the scan's share).
#[derive(Debug, Clone, Copy)]
pub(super) struct Bands {
    grid: Span,
    dp: Span,
    alternatives: Span,
    uniform: Span,
    refine: Span,
    /// Screening, exact search, polish.
    fit: [Span; 3],
}

/// The plain block runs its legacy stages first and the single-stone fit last.
const PLAIN_BANDS: Bands = Bands {
    grid: Span::new(0.02, 0.10),
    dp: Span::new(0.12, 0.38),
    alternatives: Span::new(0.50, 0.10),
    uniform: Span::new(0.60, 0.04),
    refine: Span::new(0.64, 0.08),
    fit: [
        Span::new(0.72, 0.08),
        Span::new(0.80, 0.10),
        Span::new(0.90, 0.10),
    ],
};

/// A shaped rough fits the single stones between the alternatives and the uniform pass.
const SHAPED_BANDS: Bands = Bands {
    grid: Span::new(0.02, 0.23),
    dp: Span::new(0.25, 0.30),
    alternatives: Span::new(0.55, 0.10),
    fit: [
        Span::new(0.65, 0.05),
        Span::new(0.70, 0.07),
        Span::new(0.77, 0.05),
    ],
    uniform: Span::new(0.82, 0.06),
    refine: Span::new(0.88, 0.12),
};

/// The bar layout of `path`.
pub(super) const fn bands_for(path: PlanPath) -> &'static Bands {
    match path {
        PlanPath::PlainBlock => &PLAIN_BANDS,
        PlanPath::Shaped => &SHAPED_BANDS,
    }
}

/// `done / total` in `0..=1` (`0` for an empty total).
fn ratio(done: usize, total: usize) -> f32 {
    (done as f32 / total.max(1) as f32).clamp(0.0, 1.0)
}

const fn stage_index(stage: FitStage) -> usize {
    match stage {
        FitStage::Screen => 0,
        FitStage::Exact => 1,
        FitStage::Polish => 2,
    }
}

/// Adds one to `counter` and returns the new count.
fn bump(counter: &AtomicUsize) -> usize {
    counter.fetch_add(1, Ordering::Relaxed) + 1
}

/// The real sink: throttled pushes to the window through the [`Reporter`].
pub(super) struct Tracker<'a> {
    reporter: &'a Reporter,
    /// Where the planning stages start on the bar (after the scan's share).
    base: f32,
    bands: &'static Bands,
    grid_total: AtomicUsize,
    grid_steps: AtomicUsize,
    dp_total: AtomicUsize,
    dp_steps: AtomicUsize,
    /// Set by the first `Alternatives` event. From then on the `Grid` and `Dp` events belong
    /// to a leave-one-out round (each builds its own tables and runs its own DP), not to the
    /// first piece-table and cut-order stages.
    alternatives_seen: AtomicBool,
    /// `done` of the last `Alternatives` event.
    alternatives_done: AtomicUsize,
    /// `total` of the last `Alternatives` event.
    alternatives_total: AtomicUsize,
    orders_done: AtomicUsize,
    refines: AtomicUsize,
    fit_total: [AtomicUsize; 3],
    fit_done: [AtomicUsize; 3],
    /// The highest fraction pushed so far, as `f32` bits (non-negative floats order like
    /// their bits), so parallel lanes can never make the bar step back.
    high: AtomicU32,
    /// The scan plan time limit in seconds, `0` for none. It only runs once armed.
    limit_secs: u32,
    /// The deadline, set by [`Progress::arm_deadline`] (a mesh rough only).
    deadline: OnceLock<PlanDeadline>,
    /// Set by the first event that found the deadline passed; every later event is refused.
    stopped: AtomicBool,
}

impl<'a> Tracker<'a> {
    /// A tracker pushing through `reporter`; planning starts at `base` on the bar.
    pub(super) fn new(reporter: &'a Reporter, base: f32, path: PlanPath) -> Self {
        Self {
            reporter,
            base,
            bands: bands_for(path),
            grid_total: AtomicUsize::new(1),
            grid_steps: AtomicUsize::new(0),
            dp_total: AtomicUsize::new(1),
            dp_steps: AtomicUsize::new(0),
            alternatives_seen: AtomicBool::new(false),
            alternatives_done: AtomicUsize::new(0),
            alternatives_total: AtomicUsize::new(1),
            orders_done: AtomicUsize::new(0),
            refines: AtomicUsize::new(0),
            fit_total: [1, 1, 1].map(AtomicUsize::new),
            fit_done: [0, 0, 0].map(AtomicUsize::new),
            high: AtomicU32::new(0),
            limit_secs: 0,
            deadline: OnceLock::new(),
            stopped: AtomicBool::new(false),
        }
    }

    /// The same tracker with the scan plan time limit `secs` (`0` for none), which starts
    /// to run when the driver arms it.
    pub(super) const fn with_limit(mut self, secs: u32) -> Self {
        self.limit_secs = secs;
        self
    }

    /// The stage line and the position inside the planning share (`0..=1`) of `event`.
    fn describe(&self, event: PlanProgress) -> (String, f32) {
        let bands = self.bands;
        match event {
            PlanProgress::Pareto => ("Planning: pruning designs".to_string(), 0.0),
            PlanProgress::Grid { .. } | PlanProgress::Dp { .. }
                if self.alternatives_seen.load(Ordering::Acquire) =>
            {
                self.alternatives_line()
            }
            PlanProgress::Grid { .. } => {
                let steps = bump(&self.grid_steps);
                let total = self.grid_total.load(Ordering::Relaxed);
                (
                    "Planning: sizing the pieces".to_string(),
                    bands.grid.at(ratio(steps, total)),
                )
            }
            PlanProgress::Dp { .. } => {
                let steps = bump(&self.dp_steps);
                let total = self.dp_total.load(Ordering::Relaxed);
                let finished = self.orders_done.load(Ordering::Relaxed).min(ORDER_COUNT);
                (
                    format!("Planning: cut orders ({finished} of {ORDER_COUNT} finished)"),
                    bands.dp.at(ratio(steps, total)),
                )
            }
            PlanProgress::Alternatives { done, total } => {
                self.alternatives_done.store(done, Ordering::Relaxed);
                self.alternatives_total.store(total, Ordering::Relaxed);
                self.alternatives_seen.store(true, Ordering::Release);
                self.alternatives_line()
            }
            PlanProgress::Uniform { done, total } => (
                "Planning: single-design layouts".to_string(),
                bands.uniform.at(ratio(done, total)),
            ),
            PlanProgress::Refine => {
                let started = bump(&self.refines);
                (
                    "Refining".to_string(),
                    bands.refine.at(ratio(started, REFINE_TOP)),
                )
            }
            PlanProgress::Fit { stage, done, .. } => self.describe_fit(stage, done),
        }
    }

    /// The alternatives stage at the last `Alternatives` event: the round's own `Grid` and
    /// `Dp` events show the same line and the same bar position, never an earlier stage.
    fn alternatives_line(&self) -> (String, f32) {
        let done = self.alternatives_done.load(Ordering::Relaxed);
        let total = self.alternatives_total.load(Ordering::Relaxed);
        (
            format!("Planning: alternatives {done} / {total}"),
            self.bands.alternatives.at(ratio(done, total)),
        )
    }

    /// A fit event: `done > 0` means one more design finished the stage in some lane.
    fn describe_fit(&self, stage: FitStage, done: usize) -> (String, f32) {
        let index = stage_index(stage);
        let finished = if done > 0 {
            bump(&self.fit_done[index])
        } else {
            self.fit_done[index].load(Ordering::Relaxed)
        };
        let total = self.fit_total[index].load(Ordering::Relaxed).max(1);
        let finished = finished.min(total);
        let verb = ["screening", "exact search", "polishing"][index];
        (
            format!(
                "Fitting single stones: {verb} {} / {}",
                group_thousands(finished),
                group_thousands(total)
            ),
            self.bands.fit[index].at(ratio(finished, total)),
        )
    }

    /// The bar position for `within` (`0..=1` of the planning share), never below an
    /// earlier push.
    fn monotone_fraction(&self, within: f32) -> f32 {
        let fraction = (1.0 - self.base).mul_add(within, self.base);
        let bits = fraction.to_bits();
        let previous = self.high.fetch_max(bits, Ordering::Relaxed);
        f32::from_bits(previous.max(bits))
    }
}

impl Progress for Tracker<'_> {
    fn event(&self, event: PlanProgress) -> bool {
        let (stage, within) = self.describe(event);
        let fraction = self.monotone_fraction(within);
        let stage = match self.deadline.get() {
            Some(deadline) => {
                let now = Instant::now();
                if deadline.expired(now) {
                    self.stopped.store(true, Ordering::Relaxed);
                    "Stopping at the time limit...".to_string()
                } else {
                    format!("{stage} - {} left", remaining_text(deadline.remaining(now)))
                }
            }
            None => stage,
        };
        self.reporter.report(&stage, fraction, false);
        !self.reporter.cancelled() && !self.stopped.load(Ordering::Relaxed)
    }

    fn note(&self, note: Note) {
        match note {
            Note::Grid(total) => self.grid_total.store(total.max(1), Ordering::Relaxed),
            Note::Dp(total) => self.dp_total.store(total.max(1), Ordering::Relaxed),
            Note::Fit(stage, total) => {
                self.fit_total[stage_index(stage)].store(total.max(1), Ordering::Relaxed);
            }
            Note::OrderFinished => {
                self.orders_done.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    fn abort(&self) {
        self.reporter.abort();
    }

    fn arm_deadline(&self) {
        if let Some(deadline) = PlanDeadline::new(Instant::now(), self.limit_secs) {
            let _ = self.deadline.set(deadline);
        }
    }

    fn time_stopped(&self) -> bool {
        self.stopped.load(Ordering::Relaxed) && !self.reporter.cancelled()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Weak;
    use std::sync::{Arc, atomic::AtomicBool};

    fn tracker(path: PlanPath) -> Tracker<'static> {
        let reporter = Box::leak(Box::new(Reporter::new(
            Weak::default(),
            Arc::new(AtomicBool::new(false)),
        )));
        Tracker::new(reporter, 0.0, path)
    }

    /// The bands of `bands` in the order the path runs them.
    fn in_running_order(bands: &Bands, plain: bool) -> Vec<Span> {
        let mut spans = vec![bands.grid, bands.dp, bands.alternatives];
        if plain {
            spans.extend([bands.uniform, bands.refine]);
            spans.extend(bands.fit);
        } else {
            spans.extend(bands.fit);
            spans.extend([bands.uniform, bands.refine]);
        }
        spans
    }

    #[test]
    fn the_bands_follow_the_running_order_and_stay_inside_the_bar() {
        for (path, plain) in [(PlanPath::PlainBlock, true), (PlanPath::Shaped, false)] {
            let spans = in_running_order(bands_for(path), plain);
            let mut cursor = 0.0_f32;
            for span in &spans {
                assert!(
                    span.start >= cursor - 1e-6,
                    "{span:?} starts before {cursor}"
                );
                assert!(span.len > 0.0);
                cursor = span.start + span.len;
            }
            assert!(cursor <= 1.0 + 1e-6, "the last band ends at {cursor}");
        }
    }

    #[test]
    fn fit_events_from_several_lanes_add_up_to_one_progress_figure() {
        let tracker = tracker(PlanPath::Shaped);
        tracker.note(Note::Fit(FitStage::Screen, 4));
        let fit = |done| PlanProgress::Fit {
            stage: FitStage::Screen,
            done,
            total: 2,
        };
        // Two lanes with two designs each: the local `done` restarts per lane.
        let starts = [fit(0), fit(0)];
        for event in starts {
            let (text, _) = tracker.describe(event);
            assert!(text.ends_with("screening 0 / 4"), "{text}");
        }
        let mut last = 0.0;
        for event in [fit(1), fit(1), fit(2), fit(2)] {
            let (_, within) = tracker.describe(event);
            assert!(within >= last);
            last = within;
        }
        let (text, within) = tracker.describe(fit(0));
        assert!(text.ends_with("screening 4 / 4"), "{text}");
        let screen = bands_for(PlanPath::Shaped).fit[0];
        assert!((within - (screen.start + screen.len)).abs() < 1e-6);
    }

    #[test]
    fn the_bar_never_steps_back_and_stops_at_the_planning_share() {
        let tracker = tracker(PlanPath::PlainBlock);
        let late = tracker.monotone_fraction(0.9);
        let early = tracker.monotone_fraction(0.2);
        assert!((late - 0.9).abs() < 1e-6);
        assert!((early - late).abs() < 1e-9, "a late lane's report wins");

        let scanned = Tracker {
            base: 0.35,
            ..self::tracker(PlanPath::PlainBlock)
        };
        assert!((scanned.monotone_fraction(0.0) - 0.35).abs() < 1e-6);
        assert!((scanned.monotone_fraction(1.0) - 1.0).abs() < 1e-6);
    }

    /// Before any `Alternatives` event a `Grid` event is the first sizing stage and counts
    /// against the announced total; after one, `Grid` and `Dp` events belong to the round
    /// and show the alternatives line at the band position of the last `Alternatives`
    /// event (`1 / 3` into the band, by the band's own definition), touching neither
    /// first-stage counter.
    #[test]
    fn the_grid_and_dp_events_of_an_alternative_round_stay_on_the_alternatives_stage() {
        let tracker = tracker(PlanPath::Shaped);
        tracker.note(Note::Grid(4));
        let grid = PlanProgress::Grid { done: 1, total: 4 };
        let dp = PlanProgress::Dp { order: 0, of: 6 };
        let (first, _) = tracker.describe(grid);
        assert_eq!(first, "Planning: sizing the pieces");

        let band = bands_for(PlanPath::Shaped).alternatives;
        let (line, at_label) = tracker.describe(PlanProgress::Alternatives { done: 1, total: 3 });
        assert_eq!(line, "Planning: alternatives 1 / 3");
        assert!((at_label - band.at(1.0 / 3.0)).abs() < 1e-6);

        for event in [grid, dp, grid] {
            let (text, within) = tracker.describe(event);
            assert_eq!(text, "Planning: alternatives 1 / 3");
            assert!(within >= at_label, "{within} < {at_label}");
        }
        assert_eq!(tracker.grid_steps.load(Ordering::Relaxed), 1);
        assert_eq!(tracker.dp_steps.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn dp_ticks_are_counted_against_the_announced_total() {
        let tracker = tracker(PlanPath::PlainBlock);
        tracker.note(Note::Dp(4));
        tracker.note(Note::OrderFinished);
        let dp = PlanProgress::Dp { order: 0, of: 6 };
        let (first, at_first) = tracker.describe(dp);
        assert_eq!(first, "Planning: cut orders (1 of 6 finished)");
        let mut last = at_first;
        for _ in 0..8 {
            last = tracker.describe(dp).1;
        }
        let band = bands_for(PlanPath::PlainBlock).dp;
        assert!(
            (last - (band.start + band.len)).abs() < 1e-6,
            "clamped at the end"
        );
    }
}
