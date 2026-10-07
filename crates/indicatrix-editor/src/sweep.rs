//! Angle sweeps: one tier's angle over a range, every angle solved and scored.
//!
//! A cutter who wants to know "what does 40.5 degrees do to this pavilion compared
//! with 41.5?" sweeps that tier: the design is cloned once per angle, the tier is set
//! to it (the tiers whose angles follow a relation are re-evaluated on the clone, like
//! every real edit), the clone is solved, checked and scored, and the figures come back
//! as a table. The score is the same cheap one Optimize searches with (table-up
//! brilliance, windowing, extinction, fire and scintillation under the lighting the
//! caller names), plus the yield and the manufacturability count; the tilt-averaged
//! figures (the Tilt Performance averages over four axes and 181 angles, about 1.4 s a
//! row) are opt-in.
//!
//! - [`plan_sweep`] turns a [`SweepRange`] into the angles to try and refuses what
//!   cannot be swept (a driven, flat or vertical tier, a range too flat or too steep,
//!   more than [`MAX_SWEEP_STEPS`] angles). It never solves, so a dialog can call it on
//!   every keystroke.
//! - [`sweep_tier_angle`] runs the plan: pure, cancellable (a [`SweepOutcome`] with the
//!   rows finished so far and `cancelled` set), parallel over the angles (the rows come
//!   back in angle order whatever the number of workers). The current angle is always a
//!   row, so there is a baseline to compare to.
//! - [`sweep_csv`] writes the rows as CSV (angles as magnitudes, like the table);
//!   [`best_flags`], [`chart_path`] and [`nearest_row`] are what a table and a chart need
//!   on top of the rows.
//! - [`apply_sweep_angle`] makes a row's angle the tier's real angle, as one undo step
//!   (the relations follow in the same step).
//!
//! # Magnitudes and signs
//!
//! A person works in magnitudes: "39 to 43 degrees" for a pavilion tier, whose side of
//! the girdle comes from the tier itself. The range is therefore read as magnitudes (a
//! sign typed in front is ignored), the angles run from the flattest to the steepest, and
//! every figure shown to a person is a magnitude. Inside, the engine keeps the stored
//! signed convention: a pavilion row carries its negative angle, because that is what
//! [`apply_sweep_angle`] and the design file take.
//!
//! The work runs on threads natively; on `wasm32-unknown-unknown`, which cannot spawn
//! one, it runs inline in the same order, so the result is identical either way.
//!
//! Tools of concave tiers are not part of the figures, as in Optimize: the stone is
//! scored on its flat facets.

mod chart;
mod csv;
mod form;
mod metric;
#[cfg(test)]
mod tests;

pub use chart::{
    chart_path, chart_x_percent, current_x_percent, hover_text, nearest_row, series_range_text,
};
pub use csv::sweep_csv;
pub use form::{
    DEFAULT_SWEEP_HALF_RANGE_DEG, DEFAULT_SWEEP_STEP_DEG, default_range, estimate_seconds,
    format_angle_input, format_duration_estimate, parse_sweep_range, plan_summary,
};
pub use metric::{BestFlags, SweepMetric, best_flags};

use crate::{
    EditChange, EditorSession,
    metric_deltas::measure_tilt_average,
    retarget::plan::tier_display_name,
    session::SessionEditError,
    solve_policy::{design_to_gpu_planes_from_solved, solve_cancellably},
};
use indicatrix::{
    color::metrics::evaluate_gem_optical_metrics,
    geometry::{
        meet_solver::{SolveError, SolveStrategy},
        stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
    },
    optics::{materials::GemMaterial, raytracer::EnvironmentSource},
};
use indicatrix_cut_core::{
    DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, DesignSolveError, Edit, EditError,
    ManufacturabilityWarning, check_manufacturability, design::snap_noise, volumetric_yield,
};
use std::{
    fmt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// The most angles one sweep takes, the current one included.
pub const MAX_SWEEP_STEPS: usize = 200;

/// The flattest angle (degrees from flat, either side) a sweep takes.
pub const MIN_SWEEP_ANGLE_DEG: f64 = 0.1;

/// The steepest angle (degrees from flat, either side) a sweep takes; a tier steeper
/// than this is a girdle tier and has no angle to sweep.
pub const MAX_SWEEP_ANGLE_DEG: f64 = 89.9;

/// A tier closer to flat than this is a table or a culet.
const FLAT_TIER_DEG: f64 = 1e-9;

/// A grid angle this close to the current angle is the current angle.
const CURRENT_MATCH_DEG: f64 = 1e-4;

/// The camera pitch of the fast score (90 degrees): looking straight down on the table,
/// as Optimize's own fast score does.
const TABLE_UP_PITCH_RAD: f32 = std::f32::consts::FRAC_PI_2;

/// Most worker threads a sweep starts, whatever the machine has.
#[cfg(not(target_arch = "wasm32"))]
const MAX_WORKERS: usize = 16;

/// Why a sweep cannot start.
#[derive(Debug, Clone, PartialEq)]
pub enum SweepError {
    /// The tier is not in the design.
    NoSuchTier {
        /// The index asked for.
        tier: usize,
        /// How many tiers the design has.
        tier_count: usize,
    },
    /// The tier's angle follows a relation, so it is not free.
    Driven {
        /// The tier's name for messages.
        tier: String,
        /// Its relation as a cutter reads it.
        relation: String,
    },
    /// The tier is a table or a culet.
    Flat {
        /// The tier's name for messages.
        tier: String,
    },
    /// The tier is a girdle tier (vertical).
    Vertical {
        /// The tier's name for messages.
        tier: String,
    },
    /// A number field could not be read; the text is the whole message.
    BadNumber(String),
    /// The step is zero, negative or not a number.
    StepNotPositive,
    /// An end of the range is too flat or too steep to sweep.
    OutOfBounds {
        /// The angle, in degrees.
        value: f64,
    },
    /// The range holds more angles than a sweep takes.
    TooManySteps {
        /// How many angles the range holds (the current one included).
        steps: usize,
        /// The most a sweep takes.
        max: usize,
    },
}

impl fmt::Display for SweepError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoSuchTier { .. } => f.write_str("That tier is not in the design."),
            Self::Driven { tier, relation } => write!(
                f,
                "{tier} follows a relation ({tier} = {relation}), so its angle is not free to \
                 sweep. Sweep a tier it reads instead, or remove the relation."
            ),
            Self::Flat { tier } => write!(
                f,
                "{tier} is flat (a table or a culet), so it has no angle to sweep."
            ),
            Self::Vertical { tier } => write!(
                f,
                "{tier} is a girdle tier (vertical), so it has no angle to sweep."
            ),
            Self::BadNumber(message) => f.write_str(message),
            Self::StepNotPositive => f.write_str("The step must be more than 0."),
            Self::OutOfBounds { value } => write!(
                f,
                "An angle of {:.2} degrees is outside what a sweep takes: between \
                 {MIN_SWEEP_ANGLE_DEG} and {MAX_SWEEP_ANGLE_DEG} degrees from flat.",
                value.abs()
            ),
            Self::TooManySteps { steps, max } => write!(
                f,
                "That is {steps} angles, and a sweep takes at most {max}. Use a larger step \
                 or a shorter range."
            ),
        }
    }
}

impl std::error::Error for SweepError {}

/// The range to sweep, in degrees from flat.
///
/// The ends are magnitudes: `39` to `43` for a pavilion tier, whose side of the girdle
/// comes from the tier. A sign typed in front of an end (`-39`) is ignored, so a signed
/// and an unsigned entry give the same range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SweepRange {
    /// One end of the range.
    pub from_deg: f64,
    /// The other end; either may be the larger.
    pub to_deg: f64,
    /// The distance between angles, more than 0.
    pub step_deg: f64,
}

/// The angles a sweep tries, ready to run: see [`plan_sweep`].
#[derive(Debug, Clone, PartialEq)]
pub struct SweepPlan {
    /// The tier swept.
    pub tier: usize,
    /// Its name for messages and files.
    pub tier_name: String,
    /// Its angle now, signed as the tier stores it.
    pub current_deg: f64,
    /// Every angle to try, the current one among them, from the flattest to the steepest
    /// (ascending magnitude). Signed as the tier stores them: a pavilion tier's are
    /// negative, so they run `-39`, `-40`, `-41`.
    pub angles: Vec<f64>,
    /// The position of the current angle in `angles`.
    pub current_row: usize,
}

/// What a sweep is scored under and how it runs.
#[derive(Debug, Clone, Copy)]
pub struct SweepOptions {
    /// Also average the tilt performance over all four axes and 181 angles (about
    /// 1.4 s of work per row).
    pub tilt_average: bool,
    /// How many worker threads to use; `0` and `1` run inline. A `wasm32` build
    /// always runs inline.
    pub workers: usize,
}

impl Default for SweepOptions {
    fn default() -> Self {
        Self {
            tilt_average: false,
            workers: default_worker_count(),
        }
    }
}

/// The stone's material and the light it is scored under.
#[derive(Clone, Copy)]
pub struct SweepScene<'a> {
    /// The material of the stone.
    pub material: &'a GemMaterial,
    /// The lighting of the score.
    pub environment: EnvironmentSource<'a>,
}

/// The tilt performance averaged over the four axes and the 181 angles.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TiltAverages {
    /// Mean brilliance, percent.
    pub brilliance_pct: f32,
    /// Mean windowing, percent.
    pub windowing_pct: f32,
    /// Mean extinction, percent.
    pub extinction_pct: f32,
}

/// The figures of one angle that solved and closed.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowMetrics {
    /// Table-up brilliance, percent (higher is better).
    pub brilliance_pct: f32,
    /// Table-up windowing, percent (lower is better).
    pub windowing_pct: f32,
    /// Table-up extinction, percent (lower is better).
    pub extinction_pct: f32,
    /// The fire index (higher is more fire).
    pub fire_index: f32,
    /// Scintillation, percent.
    pub scintillation_pct: f32,
    /// The finished stone's volume over the rough's, percent; `None` when the rough
    /// has no volume.
    pub yield_pct: Option<f64>,
    /// How many facets vanish or come out very small at this angle.
    pub warning_count: usize,
    /// Whether the stone has a girdle band at this angle.
    pub has_girdle: bool,
    /// The tilt averages, when the sweep asked for them.
    pub tilt: Option<TiltAverages>,
}

/// One angle of a sweep.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepRow {
    /// The angle tried, in degrees, signed as the tier stores it (a pavilion tier's is
    /// negative). Everything shown to a person uses its magnitude.
    pub angle_deg: f64,
    /// Whether this is the design's own angle.
    pub is_current: bool,
    /// The figures; `None` when the angle does not give a stone (see `notes`).
    pub metrics: Option<RowMetrics>,
    /// Plain-English remarks: why the row is invalid, or what to watch at this angle.
    pub notes: Vec<String>,
}

impl SweepRow {
    /// Whether the angle gives a stone that was scored.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.metrics.is_some()
    }

    /// The notes as one line.
    #[must_use]
    pub fn notes_text(&self) -> String {
        self.notes.join("; ")
    }
}

/// The rows of a sweep, finished or stopped.
#[derive(Debug, Clone, PartialEq)]
pub struct SweepOutcome {
    /// The tier swept.
    pub tier: usize,
    /// Its name.
    pub tier_name: String,
    /// Its angle when the sweep started.
    pub current_deg: f64,
    /// How many angles the plan held.
    pub requested: usize,
    /// Whether the sweep was stopped before every angle was done.
    pub cancelled: bool,
    /// Whether the rows carry tilt averages.
    pub tilt_average: bool,
    /// The finished rows, in the plan's order: flattest angle first.
    pub rows: Vec<SweepRow>,
}

impl SweepOutcome {
    /// The row of the design's own angle, when it finished.
    #[must_use]
    pub fn current_row(&self) -> Option<&SweepRow> {
        self.rows.iter().find(|row| row.is_current)
    }

    /// How many finished rows are valid.
    #[must_use]
    pub fn valid_count(&self) -> usize {
        self.rows.iter().filter(|row| row.is_valid()).count()
    }
}

/// How many worker threads a sweep uses unless told otherwise: one fewer than the
/// machine has (the window keeps one), at least 1 and at most 16. Always 1 on `wasm32`.
#[cfg(not(target_arch = "wasm32"))]
#[must_use]
pub fn default_worker_count() -> usize {
    std::thread::available_parallelism()
        .map_or(1, |cores| cores.get().saturating_sub(1).max(1))
        .min(MAX_WORKERS)
}

/// How many worker threads a sweep uses unless told otherwise: `wasm32` cannot spawn
/// one.
#[cfg(target_arch = "wasm32")]
#[must_use]
pub const fn default_worker_count() -> usize {
    1
}

/// Why the tier at `tier` cannot be swept whatever the range: it is not in the design, its
/// angle follows a relation, or it is flat (a table or a culet) or vertical (a girdle).
fn tier_refusal(design: &Design, tier: usize) -> Option<SweepError> {
    let Some(swept) = design.tiers.get(tier) else {
        return Some(SweepError::NoSuchTier {
            tier,
            tier_count: design.tiers.len(),
        });
    };
    let name = tier_display_name(design, tier);
    if let Some(relation) = design.relation_text(tier) {
        return Some(SweepError::Driven {
            tier: name,
            relation,
        });
    }
    let magnitude = swept.angle_deg.abs();
    if magnitude < FLAT_TIER_DEG {
        return Some(SweepError::Flat { tier: name });
    }
    if magnitude > MAX_SWEEP_ANGLE_DEG {
        return Some(SweepError::Vertical { tier: name });
    }
    None
}

/// The tiers of `design` a sweep can take (positions, ascending).
///
/// These are the ones with a free angle on the crown or the pavilion. A table, a culet, a
/// girdle tier and a tier whose angle follows a relation are left out.
#[must_use]
pub fn sweepable_tiers(design: &Design) -> Vec<usize> {
    (0..design.tiers.len())
        .filter(|&tier| tier_refusal(design, tier).is_none())
        .collect()
}

/// Works out the angles of a sweep of `tier` over `range`, and refuses what cannot be
/// swept. Never solves.
///
/// The range is read as magnitudes (a sign on an end is ignored): the angles are
/// `min + k * step` degrees from flat for every `k` that stays within the range, flattest
/// first, each cleaned of float noise (`41.1`, not `41.099999999999994`), and each given
/// the tier's own side of the girdle (a pavilion tier's angles come out negative, as the
/// design stores them). The tier's current angle is added when the grid misses it, so the
/// table always has the design as it is to compare to; a grid angle within 0.0001 degrees
/// of it is the current row.
///
/// # Errors
///
/// [`SweepError::NoSuchTier`], [`SweepError::Driven`] (the tier follows a relation),
/// [`SweepError::Flat`], [`SweepError::Vertical`], [`SweepError::StepNotPositive`],
/// [`SweepError::OutOfBounds`] for an end of the range, and [`SweepError::TooManySteps`]
/// for more than [`MAX_SWEEP_STEPS`] angles.
pub fn plan_sweep(
    design: &Design,
    tier: usize,
    range: SweepRange,
) -> Result<SweepPlan, SweepError> {
    if let Some(refusal) = tier_refusal(design, tier) {
        return Err(refusal);
    }
    let Some(swept) = design.tiers.get(tier) else {
        return Err(SweepError::NoSuchTier {
            tier,
            tier_count: design.tiers.len(),
        });
    };
    let name = tier_display_name(design, tier);
    let current_deg = swept.angle_deg;
    if !(range.step_deg.is_finite() && range.step_deg > 0.0) {
        return Err(SweepError::StepNotPositive);
    }
    for end in [range.from_deg, range.to_deg] {
        if !end.is_finite() || !(MIN_SWEEP_ANGLE_DEG..=MAX_SWEEP_ANGLE_DEG).contains(&end.abs()) {
            return Err(SweepError::OutOfBounds { value: end });
        }
    }
    // The grid is worked out in magnitudes; the tier's side comes back at the end.
    let side = if current_deg.is_sign_negative() {
        -1.0
    } else {
        1.0
    };
    let current_magnitude = current_deg.abs();
    let low = range.from_deg.abs().min(range.to_deg.abs());
    let high = range.from_deg.abs().max(range.to_deg.abs());
    // The count is worked out in floating point first: a tiny step over a wide range
    // must be refused before anything is allocated.
    let grid_count = ((high - low) / range.step_deg + 1e-9).floor() + 1.0;
    if grid_count >= (MAX_SWEEP_STEPS + 1) as f64 {
        return Err(SweepError::TooManySteps {
            steps: grid_count as usize,
            max: MAX_SWEEP_STEPS,
        });
    }
    let mut magnitudes: Vec<f64> = (0..grid_count as usize)
        .map(|k| snap_noise((k as f64).mul_add(range.step_deg, low)))
        .collect();
    let current_row = if let Some(found) = magnitudes
        .iter()
        .position(|magnitude| (magnitude - current_magnitude).abs() < CURRENT_MATCH_DEG)
    {
        magnitudes[found] = current_magnitude;
        found
    } else {
        let at = magnitudes.partition_point(|magnitude| *magnitude < current_magnitude);
        magnitudes.insert(at, current_magnitude);
        at
    };
    let mut angles: Vec<f64> = magnitudes
        .into_iter()
        .map(|magnitude| side * magnitude)
        .collect();
    // The current row is exactly the angle the tier has, whatever float noise the
    // magnitude carried.
    angles[current_row] = current_deg;
    if angles.len() > MAX_SWEEP_STEPS {
        return Err(SweepError::TooManySteps {
            steps: angles.len(),
            max: MAX_SWEEP_STEPS,
        });
    }
    Ok(SweepPlan {
        tier,
        tier_name: name,
        current_deg,
        angles,
        current_row,
    })
}

/// What the workers share while a sweep runs.
struct Shared<'a> {
    design: &'a Design,
    plan: &'a SweepPlan,
    scene: &'a SweepScene<'a>,
    options: &'a SweepOptions,
    /// The rough's volume, for the yield.
    preform_volume: Option<f64>,
    cancel: &'a AtomicBool,
    progress: &'a (dyn Fn(usize, usize) + Sync),
    /// The next angle (index into `plan.angles`) to claim.
    next: AtomicUsize,
    /// How many rows are finished.
    done: AtomicUsize,
}

/// What one angle came to.
enum Step {
    Row(SweepRow),
    Cancelled,
}

/// Sweeps the tier of `plan`: every angle of the plan is solved and scored on its own
/// clone of `design`.
///
/// The rows are in the plan's angle order (flattest first) whatever `options.workers` is,
/// and a row is a pure function of its angle, so the result does not depend on the
/// number of workers. `progress(done, total)` is called after each finished row, from
/// whichever worker finished it. Setting `cancel` stops the sweep: the rows finished
/// so far come back with [`SweepOutcome::cancelled`] set, and a solve or tilt sweep in
/// flight is abandoned at its next check.
#[must_use]
pub fn sweep_tier_angle(
    design: &Design,
    plan: &SweepPlan,
    scene: &SweepScene<'_>,
    options: &SweepOptions,
    cancel: &AtomicBool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> SweepOutcome {
    let shared = Shared {
        design,
        plan,
        scene,
        options,
        preform_volume: measure_solid(&design.preform.planes()).map(|solid| solid.volume),
        cancel,
        progress,
        next: AtomicUsize::new(0),
        done: AtomicUsize::new(0),
    };
    let workers = options.workers.clamp(1, plan.angles.len().max(1));
    let mut found = run_workers(&shared, workers);
    found.sort_by_key(|(index, _)| *index);
    let mut rows: Vec<SweepRow> = found.into_iter().map(|(_, row)| row).collect();
    note_lost_girdles(&mut rows);
    SweepOutcome {
        tier: plan.tier,
        tier_name: plan.tier_name.clone(),
        current_deg: plan.current_deg,
        requested: plan.angles.len(),
        cancelled: rows.len() < plan.angles.len(),
        tilt_average: options.tilt_average,
        rows,
    }
}

/// Runs the claim loop on `workers` scoped threads and gathers what they finished.
#[cfg(not(target_arch = "wasm32"))]
fn run_workers(shared: &Shared<'_>, workers: usize) -> Vec<(usize, SweepRow)> {
    if workers <= 1 {
        return claim_rows(shared);
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            handles.push(scope.spawn(|| claim_rows(shared)));
        }
        let mut found = Vec::new();
        for handle in handles {
            found.extend(handle.join().expect("a sweep worker must not panic"));
        }
        found
    })
}

/// The `wasm32` arm of [`run_workers`]: no thread to spawn, the same loop inline.
#[cfg(target_arch = "wasm32")]
fn run_workers(shared: &Shared<'_>, _workers: usize) -> Vec<(usize, SweepRow)> {
    claim_rows(shared)
}

/// One worker: claims the next angle until none is left or the sweep is cancelled.
fn claim_rows(shared: &Shared<'_>) -> Vec<(usize, SweepRow)> {
    let total = shared.plan.angles.len();
    let mut finished = Vec::new();
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let index = shared.next.fetch_add(1, Ordering::Relaxed);
        let Some(&angle_deg) = shared.plan.angles.get(index) else {
            break;
        };
        let is_current = index == shared.plan.current_row;
        match evaluate_step(shared, angle_deg, is_current) {
            Step::Row(row) => finished.push((index, row)),
            Step::Cancelled => break,
        }
        let done = shared.done.fetch_add(1, Ordering::Relaxed) + 1;
        (shared.progress)(done, total);
    }
    finished
}

/// Solves and scores one angle on a clone of the design.
fn evaluate_step(shared: &Shared<'_>, angle_deg: f64, is_current: bool) -> Step {
    let mut row = SweepRow {
        angle_deg,
        is_current,
        metrics: None,
        notes: Vec::new(),
    };
    let mut scratch = shared.design.clone();
    if let Err(message) = set_swept_angle(&mut scratch, shared.plan.tier, angle_deg) {
        row.notes.push(message);
        return Step::Row(row);
    }
    let solved = match solve_cancellably(&scratch, shared.cancel) {
        Ok(solved) => solved,
        Err(DesignSolveError::Solve(SolveError::Cancelled)) => return Step::Cancelled,
        Err(error) => {
            row.notes
                .push(format!("The design does not solve: {error}"));
            return Step::Row(row);
        }
    };
    let failed = solved
        .iter()
        .filter(|tier| matches!(tier.strategy, SolveStrategy::Failed))
        .count();
    if failed > 0 {
        row.notes.push(format!(
            "The solver could not place {failed} tier{}.",
            if failed == 1 { "" } else { "s" }
        ));
        return Step::Row(row);
    }
    let planes = scratch.planes_from_solved(&solved);
    if !matches!(build_solid_mesh(&planes), SolidStatus::Closed(_)) {
        row.notes
            .push("The facets do not close into a stone at this angle.".to_owned());
        return Step::Row(row);
    }
    let Some(solid) = measure_solid(&planes) else {
        row.notes
            .push("The stone cannot be measured at this angle.".to_owned());
        return Step::Row(row);
    };
    let warning_count = count_warnings(
        &check_manufacturability(&scratch, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2),
        &mut row.notes,
    );
    let gpu_planes = design_to_gpu_planes_from_solved(&scratch, &solved);
    let fast = evaluate_gem_optical_metrics(
        &gpu_planes,
        shared.scene.material,
        0.0,
        TABLE_UP_PITCH_RAD,
        shared.scene.environment,
    );
    let tilt = if shared.options.tilt_average {
        // The same measurement the compare window takes, stopped by the sweep's cancel flag.
        let Some(averages) = measure_tilt_average(
            &gpu_planes,
            shared.scene.material,
            shared.scene.environment,
            &mut |_| !shared.cancel.load(Ordering::Relaxed),
        ) else {
            return Step::Cancelled;
        };
        Some(averages)
    } else {
        None
    };
    row.metrics = Some(RowMetrics {
        brilliance_pct: fast.brilliance_pct,
        windowing_pct: fast.windowing_pct,
        extinction_pct: fast.extinction_pct,
        fire_index: fast.fire_index,
        scintillation_pct: fast.scintillation_pct,
        yield_pct: shared
            .preform_volume
            .and_then(|preform| volumetric_yield(solid.volume, preform))
            .map(|fraction| fraction * 100.0),
        warning_count,
        has_girdle: solid
            .girdle_thickness
            .is_some_and(|thickness| thickness > 0.0),
        tilt,
    });
    Step::Row(row)
}

/// Sets the swept tier's angle on the clone and lets every tier whose angle follows a
/// relation follow, as a real edit does. The error is the relation's plain-English
/// message (a result outside 0 to 90 degrees, a loop).
fn set_swept_angle(scratch: &mut Design, tier: usize, angle_deg: f64) -> Result<(), String> {
    if let Some(swept) = scratch.tiers.get_mut(tier) {
        swept.angle_deg = angle_deg;
    }
    let updates = scratch
        .evaluate_relations()
        .map_err(|error| error.to_string())?;
    for (position, deg) in updates {
        if let Some(driven) = scratch.tiers.get_mut(position) {
            driven.angle_deg = deg;
        }
    }
    Ok(())
}

/// Counts the vanishing and undersized facets of a row and says so in its notes.
fn count_warnings(warnings: &[ManufacturabilityWarning], notes: &mut Vec<String>) -> usize {
    let vanishing = warnings
        .iter()
        .filter(|warning| matches!(warning, ManufacturabilityWarning::VanishingFacet { .. }))
        .count();
    let undersized = warnings
        .iter()
        .filter(|warning| matches!(warning, ManufacturabilityWarning::UndersizedFacet { .. }))
        .count();
    if vanishing > 0 {
        notes.push(format!(
            "{vanishing} facet{} vanish{}.",
            if vanishing == 1 { "" } else { "s" },
            if vanishing == 1 { "es" } else { "" }
        ));
    }
    if undersized > 0 {
        notes.push(format!(
            "{undersized} facet{} very small.",
            if undersized == 1 { " is" } else { "s are" }
        ));
    }
    vanishing + undersized
}

/// Says "the girdle band is gone" on every valid row that lost the band the current
/// row has.
fn note_lost_girdles(rows: &mut [SweepRow]) {
    let had_girdle = rows
        .iter()
        .find(|row| row.is_current)
        .and_then(|row| row.metrics)
        .is_some_and(|metrics| metrics.has_girdle);
    if !had_girdle {
        return;
    }
    for row in rows.iter_mut().filter(|row| !row.is_current) {
        if row.metrics.is_some_and(|metrics| !metrics.has_girdle) {
            row.notes
                .push("The girdle band is gone at this angle.".to_owned());
        }
    }
}

/// Makes `new_deg` the real angle of `tier`, as one undo step: the tiers whose angles
/// follow a relation to it follow in the same step. `Ok(None)` when the tier already
/// has the angle.
///
/// # Errors
///
/// [`SessionEditError::Edit`] for a tier the design does not have;
/// [`SessionEditError::Driven`] if the tier has come to follow a relation since the
/// sweep ran; [`SessionEditError::Relation`] when a relation cannot be satisfied at
/// the angle. The design is untouched on `Err`.
pub fn apply_sweep_angle(
    session: &mut EditorSession,
    tier: usize,
    new_deg: f64,
) -> Result<Option<EditChange>, SessionEditError> {
    let Some(old_deg) = session.design.tiers.get(tier).map(|t| t.angle_deg) else {
        return Err(SessionEditError::Edit(EditError {
            index: tier,
            tier_count: session.design.tiers.len(),
        }));
    };
    if (old_deg - new_deg).abs() < 1e-9 {
        return Ok(None);
    }
    session
        .try_apply(Edit::RetargetAngles {
            changes: vec![(tier, old_deg, new_deg)],
        })
        .map(Some)
}
