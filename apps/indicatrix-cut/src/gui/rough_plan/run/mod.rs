//! Worker thread orchestration and execution for rough planning.
//!
//! A run resolves the candidate designs, measures what the catalogue has no extents or
//! hull for, plans on scoped threads ([`drive`]) and hands the finished layouts to
//! [`show_layouts`], which loads the previews and metrics and pushes the result rows.

mod drive;
#[cfg(test)]
mod drive_tests;
mod hulls;
mod results;
mod scan;
mod stages;
mod start;
mod state;
#[cfg(test)]
mod tests;
mod tracker;

use super::{
    format::{group_thousands, load_titles, to_i32},
    host::{Host, on_host},
    inputs::{FilterSnapshot, sorted_unique},
    saved::{convert::CandidateSource, dto::DesignShape, format::design_shape},
};
use crate::{
    RoughPlanModel, RoughPlannerWindow,
    gui::{batch::batch_queue::local_lane_count, tutorial_events::raise},
    plan_limit::{MAX_LIMIT_SECS, nothing_found_message, stop_note},
};
use indicatrix_cut_core::rough_plan::{
    CandidateDesign, PlanInput, PlanSettings, RoughLayout, RoughModel,
};
use indicatrix_editor::guide::viewing_events::ROUGH_PLAN_FINISHED;
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        solid_extents::{SolidExtentsSource, StoredSolidExtents},
        solid_hull::SolidHull,
    },
};
use slint::{ComponentHandle, Weak};
use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
use tracing::warn;

use self::{
    drive::drive,
    hulls::prepare_design_hulls,
    tracker::{Progress, Tracker},
};
pub(super) use self::{
    hulls::{rotate_into_caliper_frame, to_caliper_frame},
    results::show_layouts,
    start::{guard_unsaved_results, start_plan},
    state::{DesignStatus, ResultsSource, RunState, ShownLayouts},
};

/// The smallest gap between two progress pushes to the UI thread, unless forced.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(50);

/// The stage line while a cancel request is being honoured.
const CANCELLING_STAGE: &str = "Cancelling...";

/// Stack size of the planner's worker threads. Generous, since the core's layout
/// trees are built on them.
const WORKER_STACK_BYTES: usize = 8 * 1024 * 1024;

/// A progress sink every worker thread shares by reference: it throttles the pushes to
/// the UI thread and carries the cancel flag.
///
/// The `Weak<RoughPlannerWindow>` sits behind a `Mutex` so the sink is `Sync` and the
/// scoped threads of [`scan`] and [`stages`] can all report through the one instance.
pub struct Reporter {
    ui_weak: Mutex<Weak<RoughPlannerWindow>>,
    last_push: Mutex<Option<Instant>>,
    cancel: Arc<AtomicBool>,
}

impl Reporter {
    /// A sink pushing to `ui_weak`'s window and stopping when `cancel` is set.
    pub const fn new(ui_weak: Weak<RoughPlannerWindow>, cancel: Arc<AtomicBool>) -> Self {
        Self {
            ui_weak: Mutex::new(ui_weak),
            last_push: Mutex::new(None),
            cancel,
        }
    }

    /// Whether the user asked to cancel.
    pub fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// Stops the run from inside: every lane that polls [`Reporter::cancelled`] ends. Used
    /// when one lane panicked and the plan is lost anyway.
    pub fn abort(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The stage line to show for `stage`: once the user asked to cancel, "Cancelling..."
    /// stays on screen whatever the running stage reports.
    fn shown_stage<'a>(&self, stage: &'a str) -> &'a str {
        if self.cancelled() {
            CANCELLING_STAGE
        } else {
            stage
        }
    }

    /// Pushes a stage line and a progress fraction (`0..=1`) to the window. Unless
    /// `force` is set, a push within [`PROGRESS_INTERVAL`] of the previous one is
    /// dropped. After a cancel request the stage line reads "Cancelling...".
    pub fn report(&self, stage: &str, fraction: f32, force: bool) {
        {
            let mut last = self
                .last_push
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            if !force && last.is_some_and(|at| now.duration_since(at) < PROGRESS_INTERVAL) {
                return;
            }
            *last = Some(now);
        }
        let stage = self.shown_stage(stage).to_string();
        let fraction = fraction.clamp(0.0, 1.0);
        let weak = self.ui_weak.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let model = ui.global::<RoughPlanModel>();
            model.set_stage(stage.into());
            model.set_progress_fraction(fraction);
        });
    }
}

/// Where a run gets its candidate design ids. Resolved on the worker thread when the run
/// starts, so no library query ever runs on the UI thread and the ids are always the
/// library's current ones.
pub enum IdSource {
    /// The library filter, read off the window on the UI thread and queried here.
    Filter(Box<FilterSnapshot>),
    /// The whole library.
    Library,
}

/// Shown when the source has designs but every one of them is excluded from the planner.
pub const ALL_EXCLUDED_MESSAGE: &str = "Every candidate design is excluded from planning. \
     Restore one under Candidate designs, or widen the filter.";

/// The ids a run plans, and how many of the source's designs were left out
/// because they are excluded from the planner.
#[derive(Debug, PartialEq, Eq)]
pub struct Candidates {
    /// The plannable ids, sorted ascending and unique.
    pub ids: Vec<i64>,
    /// How many ids of the source were excluded.
    pub excluded: usize,
}

/// `ids` without the members of `excluded`, and how many were removed. The order of `ids`
/// is kept.
fn without_excluded(ids: Vec<i64>, excluded: &BTreeSet<i64>) -> (Vec<i64>, usize) {
    let before = ids.len();
    let kept: Vec<i64> = ids
        .into_iter()
        .filter(|id| !excluded.contains(id))
        .collect();
    let removed = before - kept.len();
    (kept, removed)
}

/// The message for a source that left no design to plan: the two plain ones when nothing
/// was excluded, [`ALL_EXCLUDED_MESSAGE`] when the exclusions emptied it.
const fn no_candidates_message(filtered: bool, excluded: usize) -> &'static str {
    if excluded > 0 {
        ALL_EXCLUDED_MESSAGE
    } else if filtered {
        "No designs match the current filters."
    } else {
        "The library has no designs yet."
    }
}

impl IdSource {
    /// The candidates of the source: its sorted ids without the designs excluded from the
    /// planner, and how many were left out. `Err` is the message for the window's
    /// `error_text`.
    pub fn resolve(&self, db: &Mutex<Database>) -> Result<Candidates, String> {
        let (ids, filtered) = match self {
            Self::Filter(snapshot) => (
                snapshot
                    .query(db)
                    .map_err(|e| format!("Could not resolve the filtered set: {e}"))?,
                true,
            ),
            Self::Library => (
                db.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .all_entry_ids()
                    .map(sorted_unique)
                    .map_err(|e| format!("Could not list the library: {e}"))?,
                false,
            ),
        };
        let excluded_ids = db
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .planner_excluded_ids()
            .map_err(|e| format!("Could not read the designs excluded from planning: {e}"))?;
        let (ids, excluded) = without_excluded(ids, &excluded_ids);
        if ids.is_empty() {
            Err(no_candidates_message(filtered, excluded).to_string())
        } else {
            Ok(Candidates { ids, excluded })
        }
    }
}

/// One run's inputs: a snapshot, so editing the model while it runs changes nothing.
pub struct PlanJob {
    /// Where the candidate design ids come from.
    pub ids: IdSource,
    /// The modelled rough.
    pub model: RoughModel,
    /// Stone-count limit, losses, minimum width and specific gravity.
    pub settings: PlanSettings,
    /// The material's name (for saving and labels).
    pub material_name: String,
    /// The weighed carat of the rough, if one was entered.
    pub weighed_ct: Option<f64>,
    /// Whether the candidates were the library filter's designs or the whole library.
    pub candidate_source: CandidateSource,
    /// The scan plan time limit in seconds, `0` for none. Only a mesh rough obeys it
    /// (see [`crate::plan_limit`]).
    pub time_limit_secs: u32,
}

/// A finished run.
pub struct Finished {
    /// The layouts and everything they were planned from.
    pub shown: ShownLayouts,
    /// The summary line, including the time the planning took.
    pub summary: String,
    /// Designs that were measured but cannot be planned with (angle-table geometry, or
    /// geometry that relies on its preform). Designs that could not be loaded or
    /// measured at all are named in `summary` instead.
    pub skipped: usize,
}

/// How a run ended.
pub enum PlanOutcome {
    /// The plan finished; the layouts and the summary are ready to show.
    Done(Box<Finished>),
    /// The user cancelled.
    Cancelled,
    /// The plan could not run; the message is for the window's `error_text`.
    Failed(String),
}

/// The plannable designs: rows measured from a design file whose figures are all finite
/// and positive, in `entry_id` order.
fn candidates_from(stored: &BTreeMap<i64, StoredSolidExtents>) -> Vec<CandidateDesign> {
    stored
        .iter()
        .filter_map(|(&entry_id, row)| {
            if row.source != SolidExtentsSource::DesignFile {
                return None;
            }
            let extents = row.extents?;
            // Checked on the stored figures: `f64::min` and `max` treat a NaN operand as
            // absent, so a NaN caliper would otherwise come out as a finite pair.
            let usable = [
                extents.width_caliper,
                extents.length_caliper,
                extents.height,
                extents.volume,
            ]
            .iter()
            .all(|figure| figure.is_finite() && *figure > 0.0);
            usable.then(|| CandidateDesign {
                entry_id,
                width: extents.width_caliper.min(extents.length_caliper),
                length: extents.width_caliper.max(extents.length_caliper),
                height: extents.height,
                volume: extents.volume,
            })
        })
        .collect()
}

/// The summary suffix naming designs left out because they are excluded from the planner
/// ("; 3 designs excluded from planning"), empty when there are none.
fn excluded_note(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "; 1 design excluded from planning".to_string(),
        n => format!("; {} designs excluded from planning", group_thousands(n)),
    }
}

/// The summary suffix naming designs that could not be loaded or measured
/// ("; 3 designs could not be measured"), empty when there are none.
fn unmeasured_note(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "; 1 design could not be measured".to_string(),
        n => format!("; {} designs could not be measured", group_thousands(n)),
    }
}

/// The summary suffix naming designs skipped because their concave tiers (curved-tool
/// cuts) could not be resolved ("; 2 designs skipped: concave tiers could not be
/// resolved"), empty when there are none. They are never planned as flat stones: that
/// would overstate their yield and carat. Nothing is saved for them, so the next run
/// tries again.
fn concave_note(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "; 1 design skipped: its concave tiers could not be resolved".to_string(),
        n => format!(
            "; {} designs skipped: their concave tiers could not be resolved",
            group_thousands(n)
        ),
    }
}

/// The summary suffix for measurements the cache could not keep ("; cache could not be
/// saved (2 designs)"), empty when every measurement was saved. Those designs were still
/// planned with, from memory; the next run measures them again.
fn cache_note(save_failures: usize) -> String {
    match save_failures {
        0 => String::new(),
        1 => "; cache could not be saved (1 design)".to_string(),
        n => format!(
            "; cache could not be saved ({} designs)",
            group_thousands(n)
        ),
    }
}

/// The summary suffix for designs that were planned as boxes only because they have no
/// usable outline ("; 4 designs without an outline were only planned as boxes"), empty
/// when every design was fitted by its outline.
fn outline_note(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "; 1 design without an outline was only planned as a box".to_string(),
        n => format!(
            "; {} designs without an outline were only planned as boxes",
            group_thousands(n)
        ),
    }
}

/// Checks that the model is a rough that exists, so an invalid one says why instead of
/// planning nothing.
///
/// # Errors
///
/// Returns the message for the window's `error_text`.
fn validate_model(model: &RoughModel) -> Result<(), String> {
    model.measure().map(|_| ()).map_err(|e| e.to_string())
}

/// The summary line of a run.
fn summary_text(layouts: usize, designs: usize, elapsed: Duration, note: &str) -> String {
    if layouts == 0 {
        return format!(
            "No layout fits: the rough is too small for the minimum stone width, or no \
             measured design is small enough{note}."
        );
    }
    format!(
        "{layouts} layout{} from {} designs -- {:.1} s{note}",
        if layouts == 1 { "" } else { "s" },
        group_thousands(designs),
        elapsed.as_secs_f64()
    )
}

/// [`summary_text`] for a run with the settings' minimum stone count: with a floor above 1,
/// the stone range ("2-5 stones") joins the summary, and an empty answer says that no layout
/// reaches the floor.
fn floor_summary_text(
    settings: &PlanSettings,
    layouts: usize,
    designs: usize,
    elapsed: Duration,
    note: &str,
) -> String {
    let (min, max) = (settings.min_count_usize(), settings.count_usize());
    if min <= 1 {
        return summary_text(layouts, designs, elapsed, note);
    }
    if layouts == 0 {
        return format!(
            "No layout with at least {min} stones fits this rough: lower Min stones, or allow \
             smaller stones (a smaller minimum width) or more designs{note}."
        );
    }
    summary_text(
        layouts,
        designs,
        elapsed,
        &format!(", {min}-{max} stones{note}"),
    )
}

/// Why gathering the designs stopped early.
enum GatherStop {
    Cancelled,
    Failed(String),
}

/// The cached extents and hulls of the candidate designs.
struct Gathered {
    stored: BTreeMap<i64, StoredSolidExtents>,
    hulls: BTreeMap<i64, SolidHull>,
    /// Whether a scan ran (it takes a share of the progress bar).
    scanned: bool,
    /// Measurements that were planned with but could not be saved to the cache.
    save_failures: usize,
    /// Designs skipped because their concave tiers could not be resolved.
    concave_unresolved: usize,
}

/// Loads the cached extents and hulls of `ids` and measures whatever has none. What the
/// scan measured but could not save is laid over the cache read back, so a failed cache
/// write costs the next run a measurement, not this run a design.
fn gather(db: &Mutex<Database>, reporter: &Reporter, ids: &[i64]) -> Result<Gathered, GatherStop> {
    let (mut stored, mut hulls) =
        scan::load_extents_and_hulls(db, ids).map_err(GatherStop::Failed)?;
    let (missing, outlines_only) = scan::ids_needing_scan(ids, &stored, &hulls);
    let outcome = scan::scan_missing(db, reporter, &missing, outlines_only);
    if outcome.cancelled {
        return Err(GatherStop::Cancelled);
    }
    if !missing.is_empty() {
        let (fresh_extents, fresh_hulls) =
            scan::load_extents_and_hulls(db, &missing).map_err(GatherStop::Failed)?;
        stored.extend(fresh_extents);
        hulls.extend(fresh_hulls);
    }
    scan::merge_measured(&mut stored, &mut hulls, outcome.measured);
    Ok(Gathered {
        stored,
        hulls,
        scanned: !missing.is_empty(),
        save_failures: outcome.save_failures,
        concave_unresolved: outcome.concave_unresolved,
    })
}

/// Runs a whole plan. Panics anywhere in it (including the scoped threads) reach the
/// caller, which catches them.
pub fn run_plan(db: &Mutex<Database>, job: &PlanJob, reporter: &Reporter) -> PlanOutcome {
    let started = Instant::now();
    if let Err(message) = validate_model(&job.model) {
        return PlanOutcome::Failed(message);
    }
    let Candidates { ids, excluded } = match job.ids.resolve(db) {
        Ok(candidates) => candidates,
        Err(message) => return PlanOutcome::Failed(message),
    };
    let gathered = match gather(db, reporter, &ids) {
        Ok(gathered) => gathered,
        Err(GatherStop::Cancelled) => return PlanOutcome::Cancelled,
        Err(GatherStop::Failed(message)) => return PlanOutcome::Failed(message),
    };
    let designs = candidates_from(&gathered.stored);
    let prepared = prepare_design_hulls(&gathered.stored, &gathered.hulls);
    if prepared.stale > 0 || prepared.skipped > 0 || prepared.duplicates > 0 {
        warn!(
            "Rough planner: {} cached outlines no longer match their extents and {} are \
             unusable (those designs are planned as boxes only); {} repeat an earlier \
             design's outline exactly (fitted once)",
            prepared.stale, prepared.skipped, prepared.duplicates
        );
    }
    // Rows that exist but are not plannable (angle table, preform-bounded) are
    // "skipped"; ids with no row at all could not be loaded or measured (they are not
    // saved, so the next run retries them) and are named in the summary instead.
    let skipped = gathered.stored.len().saturating_sub(designs.len());
    // A design whose exact twin is fitted is covered; every other plannable design that
    // has no outline in the fit is planned as a box only.
    let boxes_only = designs
        .len()
        .saturating_sub(prepared.hulls.len() + prepared.duplicates);
    // The concave-unresolved designs have no row either, but get their own note.
    let unmeasured = ids
        .len()
        .saturating_sub(gathered.stored.len())
        .saturating_sub(gathered.concave_unresolved);
    let note = format!(
        "{}{}{}{}{}",
        excluded_note(excluded),
        unmeasured_note(unmeasured),
        concave_note(gathered.concave_unresolved),
        cache_note(gathered.save_failures),
        outline_note(boxes_only)
    );
    if designs.is_empty() {
        let summary = format!(
            "None of the {} selected designs has a usable design file{note}.",
            group_thousands(ids.len())
        );
        return finished(
            job,
            Vec::new(),
            BTreeMap::new(),
            BTreeMap::new(),
            summary,
            skipped,
        );
    }

    let input = PlanInput {
        model: &job.model,
        settings: &job.settings,
        designs: &designs,
        hulls: &prepared.hulls,
    };
    let base = if gathered.scanned {
        scan::SCAN_BAND
    } else {
        0.0
    };
    let tracker = Tracker::new(reporter, base, input.path()).with_limit(job.time_limit_secs);
    let driven = drive(&input, local_lane_count(), &tracker);
    let limit_secs = u64::from(job.time_limit_secs.min(MAX_LIMIT_SECS));
    // The user's cancel wins over the limit (`time_stopped` is false after a cancel).
    let stopped = tracker.time_stopped();
    let layouts = match driven {
        Some(layouts) => layouts,
        None if stopped => Vec::new(),
        None => return PlanOutcome::Cancelled,
    };
    if stopped && layouts.is_empty() {
        return PlanOutcome::Failed(nothing_found_message(limit_secs));
    }
    let titles = load_titles(db, &layouts);
    let shapes = planned_shapes(&gathered.stored, &layouts);
    let mut summary = floor_summary_text(
        &job.settings,
        layouts.len(),
        designs.len(),
        started.elapsed(),
        &note,
    );
    if stopped {
        summary = format!("{summary}. {}", stop_note(limit_secs));
    }
    finished(job, layouts, titles, shapes, summary, skipped)
}

/// The shape of every design the layouts use, as this run measured it (designs whose
/// planes did not close have no shape).
fn planned_shapes(
    stored: &BTreeMap<i64, StoredSolidExtents>,
    layouts: &[RoughLayout],
) -> BTreeMap<i64, DesignShape> {
    layouts
        .iter()
        .flat_map(|layout| layout.stones.iter().map(|stone| stone.entry_id))
        .filter_map(|id| {
            let extents = stored.get(&id)?.extents.as_ref()?;
            Some((id, design_shape(extents)))
        })
        .collect()
}

/// The finished run of `job` with `layouts`.
fn finished(
    job: &PlanJob,
    layouts: Vec<RoughLayout>,
    titles: BTreeMap<i64, String>,
    shapes: BTreeMap<i64, DesignShape>,
    summary: String,
    skipped: usize,
) -> PlanOutcome {
    PlanOutcome::Done(Box::new(Finished {
        shown: ShownLayouts {
            layouts,
            plan_model: Some(job.model.clone()),
            plan_settings: Some(job.settings),
            material_name: job.material_name.clone(),
            weighed_ct: job.weighed_ct,
            candidate_source: job.candidate_source,
            titles,
            statuses: BTreeMap::new(),
            shapes,
            source: ResultsSource::Planned,
        },
        summary,
        skipped,
    }))
}

/// A readable message for a `catch_unwind` payload.
fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// The run is over: the window stops showing progress. A tutorial step may wait for the
/// finished plan, and the result cards are on screen by now.
fn end_run(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.set_running(false);
    model.set_stage("".into());
    model.set_progress_fraction(1.0);
    if let Some(main) = host.main.upgrade() {
        raise(&main, ROUGH_PLAN_FINISHED);
    }
}

/// Shows a finished run on the UI thread. The window keeps its progress display until
/// the result rows are pushed, so it never flashes an empty result list.
fn apply_outcome(host: &Rc<Host>, outcome: PlanOutcome) {
    let model = host.window.global::<RoughPlanModel>();
    match outcome {
        PlanOutcome::Done(finished) => {
            let Finished {
                shown,
                summary,
                skipped,
            } = *finished;
            model.set_skipped_count(to_i32(skipped));
            model.set_stage("Preparing the results...".into());
            host.session.borrow_mut().run.summary = Some(summary);
            results::show_layouts(host, shown, Some(end_run));
        }
        PlanOutcome::Cancelled => {
            model.set_running(false);
            model.set_stage("".into());
            model.set_progress_fraction(0.0);
            // The results the run set aside come back, so a cancel never leaves an empty
            // list behind.
            start::restore_previous(host);
            model.set_summary("Planning cancelled.".into());
        }
        PlanOutcome::Failed(message) => {
            model.set_running(false);
            model.set_stage("".into());
            model.set_progress_fraction(0.0);
            start::restore_previous(host);
            model.set_error_text(message.into());
        }
    }
}

/// Runs [`run_plan`] on a named worker thread. Whatever happens there, the one
/// completion push runs afterwards (a panic is caught and reported as an error).
pub fn spawn_worker(
    ui_weak: Weak<RoughPlannerWindow>,
    db: Arc<Mutex<Database>>,
    job: PlanJob,
    cancel: Arc<AtomicBool>,
) -> std::io::Result<()> {
    std::thread::Builder::new()
        .name("rough-plan".to_string())
        .stack_size(WORKER_STACK_BYTES)
        .spawn(move || {
            let run_flag = Arc::clone(&cancel);
            let reporter = Reporter::new(ui_weak.clone(), cancel);
            let outcome = catch_unwind(AssertUnwindSafe(|| run_plan(&db, &job, &reporter)))
                .unwrap_or_else(|payload| {
                    let message = panic_message(&*payload);
                    warn!("Rough planner panicked: {message}");
                    PlanOutcome::Failed(format!("Internal error: {message}"))
                });
            let _ = ui_weak.upgrade_in_event_loop(move |_ui| {
                on_host(|host| {
                    // A window closed and opened again meanwhile is a new host: the old
                    // run's outcome is not its own.
                    let is_this_run = host
                        .session
                        .borrow()
                        .cancel
                        .as_ref()
                        .is_some_and(|flag| Arc::ptr_eq(flag, &run_flag));
                    if is_this_run {
                        apply_outcome(host, outcome);
                    }
                });
            });
        })
        .map(|_handle| ())
}

/// Registers the Plan, Cancel, keep-toggle and re-plan callbacks on the planner window.
pub(super) fn setup_run_callbacks(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_plan(|| on_host(start_plan));
    model.on_replan_loaded(|| on_host(start_plan));
    model.on_cancel(|| on_host(start::cancel_plan));
    model.on_toggle_keep(|index| on_host(|host| start::toggle_keep(host, index)));
    model.on_confirm_replace(|replace| on_host(|host| start::answer_replace(host, replace)));
}
