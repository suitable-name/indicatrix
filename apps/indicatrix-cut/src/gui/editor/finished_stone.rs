//! The finished stone, for everything that must not follow the Cut slider.
//!
//! The Cut slider draws the open design cut back to some step: the Solid view, the Live
//! Render picture and the live remote dispatch all follow it, on purpose, so a cutter can
//! see the stone after every tier. A file that leaves the program is a different matter: the
//! high-resolution export, the tilt video (and the tilt curves printed on it) and a remote
//! final render must show the FINISHED gem whatever the slider says, or a half-cut stone
//! would be delivered by accident.
//!
//! Those paths capture their scene from the render context, which holds whatever the
//! viewport shows. They ask for the finished stone first ([`finished_stone_then`], or
//! [`finished_stone_job`] from a worker thread): it is `Some` only when the editor owns the
//! viewport and the slider has the design cut back, and then holds the whole design's planes
//! and concave tools, to be swapped into the capture (`SceneSnapshot::capture_finished`).
//! With the slider at Finished, or with a Library row on the viewport, it is `None` and the
//! capture is the render context as it stands.
//!
//! # Never solves on the UI thread
//!
//! The whole stone needs the design's masts, and `Design::solve` takes seconds for a large
//! design. The UI thread therefore only ever reads them from the editor's solve cache, and
//! only when the cache was stored for exactly the design's current generation (the tier
//! count alone is not enough: an angle nudge keeps it). When it has nothing current:
//!
//! - [`finished_stone_then`] hands the solve to the editor's solve worker and runs its
//!   continuation when the result lands;
//! - [`FinishedStoneJob::resolve`] solves on the calling thread, for a caller that is
//!   already a worker (the tilt hover preview).
//!
//! Either way the masts are stored back in the cache under the generation they describe, so
//! a pointer sweeping the tilt curve costs at most one solve per edit, not one per event:
//! [`FinishedStoneJob::resolve`] takes a process-wide gate before it solves, so concurrent
//! callers wait for the one solve that is running and then read its masts from the cache.
//!
//! # Never delivers a wrong stone
//!
//! A stone is delivered only for the design it was built from. The result of
//! [`finished_stone_then`] is a [`Finished`]: when the design does not solve, the editor
//! moved on while the solve ran, or the solve was displaced by a newer request, it is the `Err`
//! side, a [`Withheld`], and the caller must not export (the exports refuse with a sentence,
//! each worded for its own reason; the tilt curves and the hover thumbnail simply draw
//! nothing). It is never the render context's half-cut stone in disguise.
//!
//! The decisions are pure functions; only the two entry points touch the window, the render
//! context and the editor state.

use super::{
    auto_solve::{editor_state, solid_last_solved},
    native_io::{SolveFailure, resolve_solve_at},
};
use crate::{
    MainWindow, ViewportModel,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::solid_preview::{cut_slider, preview_state::SolidLastSolved},
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::Design;
use indicatrix_solid::preview::StoneGeometryBuf;
use slint::ComponentHandle;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicU64, Ordering},
};

/// Why the finished stone is not delivered. See the module comment ("Never delivers a wrong
/// stone").
#[derive(Debug, Clone, PartialEq, Eq)]
pub(in crate::gui) enum Withheld {
    /// The design does not solve (or its stone has no facets). Carries the solver's sentence.
    Unsolvable(String),
    /// The editor's design changed while the stone was being prepared: the stone would
    /// describe a design the cutter no longer has.
    Stale,
    /// The solve was displaced by a newer request (the solve worker takes one at a time) and
    /// its allowance of retries ran out. The design did not change and its geometry is
    /// unknown, so this is not [`Self::Stale`]: it is only a request that never finished.
    Displaced,
}

impl Withheld {
    /// The sentence an export shows when it is refused for this reason.
    #[must_use]
    pub(in crate::gui) fn export_message(&self) -> String {
        match self {
            Self::Unsolvable(reason) => {
                format!("Nothing was exported: the design does not solve ({reason}).")
            }
            Self::Stale => "Nothing was exported: the design changed while the finished stone \
                            was being prepared. Start the export again."
                .to_string(),
            Self::Displaced => "Nothing was exported: preparing the finished stone was \
                                interrupted by a newer request. Start the export again."
                .to_string(),
        }
    }
}

/// What [`finished_stone_then`] delivers: `Ok(None)` when the render context already holds
/// the finished stone (the slider is at Finished, or a Library row is on the viewport),
/// `Ok(Some(stone))` when the stone is to be swapped in, `Err` when no honest stone can be
/// delivered ([`Withheld`]).
pub(in crate::gui) type Finished = Result<Option<StoneGeometryBuf>, Withheld>;

/// The refusal for a stone with no facets: a mast list that does not fit the design draws
/// nothing, which an export must not mistake for "nothing to do".
fn no_facets() -> Withheld {
    Withheld::Unsolvable("the finished stone has no facets".to_string())
}

/// Whether the editor's design (`live`, its current generation, when known) has moved past
/// `job_generation`. An unknown live generation is never stale: the caller has nothing to
/// compare against, and the design the button was pressed for is a fair answer.
#[must_use]
fn is_stale(job_generation: u64, live: Option<u64>) -> bool {
    live.is_some_and(|live| live != job_generation)
}

/// Whether the editor's live viewport (Live Render) shows the editor's own design when `owner`
/// holds the render context's plane slot -- the only case in which the Cut slider's cut is what
/// the picture shows. A Library row on the viewport, or the built-in placeholder cut, is
/// drawn whole whatever the slider says.
#[must_use]
pub(in crate::gui) const fn shows_editor_design(owner: PlanesOwner) -> bool {
    matches!(owner, PlanesOwner::Editor { .. })
}

/// Tells Live Render who owns the viewport now (`ViewportModel.shows_editor_design`), so its
/// "Cut: ..." pill appears only while the picture really is the cut design. Called wherever
/// the plane slot is claimed from the UI thread.
pub(in crate::gui) fn push_viewport_owner(ui: &MainWindow, owner: PlanesOwner) {
    ui.global::<ViewportModel>()
        .set_shows_editor_design(shows_editor_design(owner));
}

/// The whole `design` as a stone, from its mast list `solved`: no solve runs here.
///
/// `None` when the stone is empty (a mast list that does not fit the design): an empty
/// stone would export nothing at all.
#[must_use]
fn whole_stone(design: &Design, solved: &[SolvedTier]) -> Option<StoneGeometryBuf> {
    let stone = cut_slider::cut_geometry(design, Some(solved), None);
    (!stone.planes.is_empty()).then_some(stone)
}

/// The masts in `cached` when they were stored for exactly `generation` and still fit
/// `design`'s tiers.
///
/// An exact generation, unlike `native_io`'s tier-count test: a stone that leaves the
/// program must not be built from the masts of the design one nudge ago.
#[must_use]
fn masts_for_generation(
    cached: Option<&(u64, Vec<SolvedTier>)>,
    generation: u64,
    design: &Design,
) -> Option<Vec<SolvedTier>> {
    let (stored_for, masts) = cached?;
    (*stored_for == generation && masts.len() == design.tiers.len()).then(|| masts.clone())
}

/// [`masts_for_generation`] over the editor's shared solve cache.
fn read_cached_masts(
    cache: &SolidLastSolved,
    generation: u64,
    design: &Design,
) -> Option<Vec<SolvedTier>> {
    let guard = cache.lock().unwrap_or_else(PoisonError::into_inner);
    masts_for_generation(guard.as_ref(), generation, design)
}

/// Stores `solved` in the shared solve cache as the masts of `generation`, unless the cache
/// already holds a solve for that generation or a newer one (a slow solve must not
/// overwrite what a later edit has stored).
fn remember_masts(cache: &SolidLastSolved, generation: u64, solved: Vec<SolvedTier>) {
    let mut guard = cache.lock().unwrap_or_else(PoisonError::into_inner);
    if guard
        .as_ref()
        .is_none_or(|(stored_for, _)| *stored_for < generation)
    {
        *guard = Some((generation, solved));
    }
}

/// The one solve [`FinishedStoneJob::resolve`] lets run at a time. A hover preview fires a
/// job per pointer event and each one finds the cache cold until the first solve lands: without
/// this gate every job that survives the debounce would run its own `Design::solve` in
/// parallel, saturating the cores for as long as the solve takes. The ones that wait find the
/// cache warm when the gate opens.
static SOLVE_GATE: Mutex<()> = Mutex::new(());

/// `Design::solve` with its error as the sentence the cutter reads.
fn solve_design(design: &Design) -> Result<Vec<SolvedTier>, String> {
    design.solve().map_err(|error| error.to_string())
}

/// The finished stone of a design the Cut slider has cut back, to be built once the
/// design's masts are known.
///
/// Holds its own copy of the design, so a worker can finish the job without the editor
/// state (which belongs to the UI thread).
pub(in crate::gui) struct FinishedStoneJob {
    design: Design,
    /// The editor's generation `design` was read at.
    generation: u64,
    /// The solve cache's masts, when they were current for `generation` as the job was made.
    masts: Option<Vec<SolvedTier>>,
    /// The editor's shared solve cache, to read again at the last moment and to store a
    /// fresh solve in.
    cache: Option<SolidLastSolved>,
    /// The editor's live generation counter (the one `EditorState` keeps bumping, shared
    /// with every background job), to tell whether the design moved on while the job waited.
    /// `None` for a job that never checks.
    live_generation: Option<Arc<AtomicU64>>,
}

impl FinishedStoneJob {
    /// The job for `design` as of `generation`, or `None` when `cut_steps` says nothing is
    /// cut (the value `cut_slider::current_steps` reads: `None` is the finished design).
    #[must_use]
    fn when_cut(
        design: &Design,
        generation: u64,
        cut_steps: Option<usize>,
        cache: Option<SolidLastSolved>,
    ) -> Option<Self> {
        cut_steps?;
        let masts = cache
            .as_ref()
            .and_then(|cache| read_cached_masts(cache, generation, design));
        Some(Self {
            design: design.clone(),
            generation,
            masts,
            cache,
            live_generation: None,
        })
    }

    /// This job, watching `live` (the editor's generation counter) so it can tell when the
    /// design has moved on.
    #[must_use]
    fn following(mut self, live: Arc<AtomicU64>) -> Self {
        self.live_generation = Some(live);
        self
    }

    /// Whether the editor's design has changed since the job was made.
    fn is_stale(&self) -> bool {
        is_stale(
            self.generation,
            self.live_generation
                .as_ref()
                .map(|live| live.load(Ordering::Relaxed)),
        )
    }

    /// Whether the job holds masts that were current when it was made, so its stone needs no
    /// solve.
    const fn holds_masts(&self) -> bool {
        self.masts.is_some()
    }

    /// The stone from the masts the job holds, with no solve: `None` when it holds none, or
    /// when they draw nothing (see [`FinishedStoneJob::holds_masts`]).
    fn stone_from_held_masts(&self) -> Option<StoneGeometryBuf> {
        whole_stone(&self.design, self.masts.as_deref()?)
    }

    /// Solves the design with `solve` and stores the masts in the shared cache for the next
    /// job.
    fn solve_and_remember(
        &self,
        solve: &impl Fn(&Design) -> Result<Vec<SolvedTier>, String>,
    ) -> Result<Vec<SolvedTier>, Withheld> {
        let solved = solve(&self.design).map_err(Withheld::Unsolvable)?;
        if let Some(cache) = &self.cache {
            remember_masts(cache, self.generation, solved.clone());
        }
        Ok(solved)
    }

    /// Finishes the job on the calling thread: from the solve cache when it holds the
    /// design's current masts, otherwise by solving the design, one solve at a time
    /// ([`SOLVE_GATE`]).
    ///
    /// Fails with [`Withheld::Unsolvable`] when the design does not solve and with
    /// [`Withheld::Stale`] when the editor's design changed while the job waited: the caller
    /// draws nothing rather than a stone for another design. Call it from a worker thread
    /// only; the UI thread uses [`finished_stone_then`].
    pub(in crate::gui) fn resolve(self) -> Result<StoneGeometryBuf, Withheld> {
        self.resolve_with(solve_design)
    }

    /// [`Self::resolve`] with the solver injected, so a test can count the solves.
    fn resolve_with(
        mut self,
        solve: impl Fn(&Design) -> Result<Vec<SolvedTier>, String>,
    ) -> Result<StoneGeometryBuf, Withheld> {
        // The cache may have been filled since the job was made (a background solve
        // landing while a hover preview waited out its debounce).
        let held = self.masts.take().or_else(|| self.cached_masts());
        let masts = if let Some(masts) = held {
            masts
        } else {
            self.solve_behind_gate(&solve)?
        };
        if self.is_stale() {
            return Err(Withheld::Stale);
        }
        whole_stone(&self.design, &masts).ok_or_else(no_facets)
    }

    /// The masts of a design the cache did not hold: solved here, one solve at a time
    /// ([`SOLVE_GATE`]), unless the design is no longer the editor's.
    fn solve_behind_gate(
        &self,
        solve: &impl Fn(&Design) -> Result<Vec<SolvedTier>, String>,
    ) -> Result<Vec<SolvedTier>, Withheld> {
        // A job for a design the editor has already left would only warm the cache with
        // masts nobody asks for.
        if self.is_stale() {
            return Err(Withheld::Stale);
        }
        let _single_flight = SOLVE_GATE.lock().unwrap_or_else(PoisonError::into_inner);
        // Whoever held the gate may have solved exactly this design.
        if let Some(masts) = self.cached_masts() {
            return Ok(masts);
        }
        if self.is_stale() {
            return Err(Withheld::Stale);
        }
        self.solve_and_remember(solve)
    }

    /// The cache's masts for this job's design, if it holds current ones.
    fn cached_masts(&self) -> Option<Vec<SolvedTier>> {
        self.cache
            .as_ref()
            .and_then(|cache| read_cached_masts(cache, self.generation, &self.design))
    }
}

/// The job that builds the finished stone an export must use in place of what the render
/// context holds, or `None` when the context already holds it (see the module comment).
///
/// Cheap: it clones the design and reads the solve cache, and never solves. Reads the editor
/// state with `try_borrow`: a caller on the UI thread can find it held by a callback that
/// is still running, and then the export keeps the context's planes (the slider's own cut)
/// rather than panicking.
#[must_use]
pub(in crate::gui) fn finished_stone_job(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) -> Option<FinishedStoneJob> {
    let owner = RenderContext::lock(render_ctx).planes_owner;
    if !shows_editor_design(owner) {
        return None;
    }
    let state = editor_state()?;
    let state = state.try_borrow().ok()?;
    let cut_steps = cut_slider::current_steps(ui, &state.design);
    FinishedStoneJob::when_cut(
        &state.design,
        state.generation.load(Ordering::Relaxed),
        cut_steps,
        solid_last_solved(),
    )
    .map(|job| job.following(Arc::clone(&state.generation)))
}

/// What [`finished_stone_then`]'s continuation gets when the solve worker answers: the stone
/// of `design` as of `generation`, unless the design does not solve or the editor has moved
/// past `generation` (`live`, its generation now, when it could be read).
///
/// The masts of a good solve are filed in the cache (`remember`) whether or not the editor
/// moved on: they are right for the generation they describe. A solve that failed for a design
/// the editor has since left says nothing about the design it has now, so it is `Stale`, not
/// `Unsolvable`. A solve displaced by a newer request for the design the editor still has is
/// `Displaced`: neither a verdict on the design nor a change to it.
fn landed(
    design: &Design,
    generation: u64,
    live: Option<u64>,
    result: Result<Vec<SolvedTier>, SolveFailure>,
    remember: impl FnOnce(Vec<SolvedTier>),
) -> Finished {
    let stale = is_stale(generation, live);
    match result {
        Ok(solved) => {
            let stone = whole_stone(design, &solved);
            remember(solved);
            if stale {
                Err(Withheld::Stale)
            } else {
                stone.map(Some).ok_or_else(no_facets)
            }
        }
        Err(SolveFailure::Failed(reason)) if !stale => Err(Withheld::Unsolvable(reason)),
        // A displaced solve says nothing about the geometry at all, and the design did not
        // change: it is no verdict on the design, and not "the design changed" either.
        Err(SolveFailure::Superseded) if !stale => Err(Withheld::Displaced),
        // A failure (or a displacement) of a design the editor has since left says nothing
        // about the design it has now.
        Err(_) => Err(Withheld::Stale),
    }
}

/// Calls `then` with the finished stone an export must use in place of what the render
/// context holds, without ever solving on the calling UI thread. See [`Finished`] for what
/// it gets.
///
/// `then` runs at once when the stone needs no solve: slider at Finished, a Library row on
/// the viewport, or masts in the cache for the design's current generation. Otherwise the
/// solve goes to the editor's solve worker and `then` runs on the UI thread when it lands;
/// `while_solving` runs first, so the caller can say that it is waiting. A stone is only
/// delivered for the design as it still is when the solve lands: if the editor has moved on
/// meanwhile, `then` gets [`Withheld::Stale`] (an export is refused with a sentence; the tilt
/// curves leave it to the newer edit's own request). A solve the worker displaced for a newer
/// request is [`Withheld::Displaced`], worded as the interruption it is.
pub(in crate::gui) fn finished_stone_then(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    while_solving: impl FnOnce(&MainWindow),
    then: impl FnOnce(&MainWindow, Finished) + 'static,
) {
    let Some(job) = finished_stone_job(ui, render_ctx) else {
        then(ui, Ok(None));
        return;
    };
    if job.holds_masts() {
        then(
            ui,
            job.stone_from_held_masts().map(Some).ok_or_else(no_facets),
        );
        return;
    }
    while_solving(ui);
    let FinishedStoneJob {
        design,
        generation,
        cache,
        live_generation,
        ..
    } = job;
    resolve_solve_at(
        ui,
        Arc::new(design),
        generation,
        move |ui, design, result| {
            let live = live_generation
                .as_ref()
                .map(|live| live.load(Ordering::Relaxed));
            then(
                ui,
                landed(&design, generation, live, result, |solved| {
                    if let Some(cache) = &cache {
                        remember_masts(cache, generation, solved);
                    }
                }),
            );
        },
    );
}

#[cfg(test)]
mod tests;
