//! The page side of the Worker pool (wasm32 only): what the UI calls to render and to
//! solve.
//!
//! # Use
//!
//! ```ignore
//! let pool = WorkerPool::new()?;                  // N render Workers + 1 solve Worker
//! pool.render().on_progress(|acc| { /* tonemap acc.sum() / acc.sample_count() */ });
//! pool.render().on_error(|message| { /* status strip */ });
//! pool.render().set_scene(spec);                  // new scene id, accumulation restarts
//! pool.render().start(DEFAULT_LIVE_SPP);          // trace until 256 spp
//! pool.render().cancel();                         // stop scheduling (results in flight still merge)
//! pool.render().on_picture(|result| { /* denoised RGBA or export PNG bytes */ });
//! pool.render().request_picture(PictureKind::DenoisedLive)?; // made in the picture Worker
//! pool.render().restart_workers()?;               // after a Worker gave up restarting
//! pool.set_hdr(id, bytes)?;                       // an HDR map on the render Workers and the analysis Worker
//! pool.clear_hdr()?;                              // ...and drop it again
//!
//! let response = pool.solve().solve(toml, SolveRequest::Solve).await; // in spawn_local
//! // An Optimize / Retarget search streams its progress while it runs:
//! let response = pool.solve().solve_with_progress(toml, request, |p| { /* p.message */ }).await;
//! // Current-view metrics and the tilt sweep have their own Worker; they need no design:
//! let response = pool.analysis()?.solve(String::new(), SolveRequest::Metrics { params }).await;
//! ```
//!
//! # Rules the callers can rely on
//!
//! - Everything is single-threaded (the page's main thread); the types are `!Send` and
//!   cheap to clone (they are `Rc` handles).
//! - Callbacks are never run while the pool is borrowed, so a callback may call any pool
//!   method. They run from a Worker's `onmessage` (or the silence watchdog's timer) --
//!   and, for the failures a call itself finds, at the end of that call: a
//!   [`RenderPool`] method that posts to its Workers (`set_scene`, `start`,
//!   `restart_workers`, and [`WorkerPool::set_hdr`] / [`WorkerPool::clear_hdr`] through
//!   it) reports a post that failed through the error callback before it returns. A
//!   progress callback may call [`RenderPool::set_scene`] (the accumulator it is reading
//!   is replaced, not mutated).
//! - An HDR map is held by every render Worker and by the analysis Worker (the metrics
//!   HUD is scored under it); the copies together stay within the 1.5 GiB memory budget
//!   (`crate::hdr::admit_hdr`). The analysis client remembers the map, so an analysis
//!   Worker that is created after it was loaded, or replaced later, is given it too.
//! - Futures from [`SolveClient::solve`] resolve exactly once: `Ok` with the Worker's
//!   answer, or a [`crate::solve_error::SolveError`] when superseded by a newer request,
//!   cancelled, timed out (the Worker is then terminated and respawned), or the Worker
//!   crashed.
//! - A render Worker that crashes, or goes quiet for 30 s (longer for a chunk big enough
//!   to need it) while holding a chunk, is terminated and replaced, and the scene
//!   restarts; after three such replacements without a delivered chunk it stays failed
//!   for [`RenderPool::restart_workers`].
//! - Workers are loaded from [`crate::WORKER_LOADER_URL`].

mod cancel_url;
mod handle;
mod picture_worker;
mod render_callbacks;
mod render_events;
mod render_pool;
mod solve_client;
mod watchdog;

pub use render_callbacks::PictureResult;
pub use render_pool::RenderPool;
pub use solve_client::{
    ANALYSIS_INACTIVITY_TIMEOUT_MS, DEFAULT_SOLVE_TIMEOUT_MS, SUPERSEDE_RESPAWN_AFTER_MS,
    SolveClient, SolveProgress,
};

use std::{cell::RefCell, rc::Rc};

use crate::hdr::{HdrAdmission, render_worker_count};

/// The HDR map the analysis Worker holds: its id and the file's bytes.
type HeldHdr = (u64, Rc<Vec<u8>>);

/// The whole pool: render Workers, the solve Worker, and (created on first use) the
/// analysis Worker.
///
/// # Why a second solve-role Worker
///
/// A [`SolveClient`] runs one job at a time and a new call SUPERSEDES the running one,
/// terminating the Worker if it has run for 500 ms or more. The current-view metrics are
/// requested on every settled camera or light change and the tilt sweep runs for seconds,
/// so on the design solve's client they would kill a long solve (and each other). The
/// analysis client is an independent Worker for exactly those two: a metrics job never
/// waits behind or preempts a design solve or a search, and only supersedes another
/// metrics job. Sharing the analysis client between the two kinds is the caller's rule:
/// it does not submit metrics while a tilt sweep runs (`apps/indicatrix-web`'s
/// `metrics::hud`), since a metrics job would supersede the sweep.
#[derive(Clone)]
pub struct WorkerPool {
    render: RenderPool,
    solve: SolveClient,
    analysis: Rc<RefCell<Option<SolveClient>>>,
    /// The map the analysis Worker is to hold (given to it when it is created, if it is
    /// not yet).
    analysis_hdr: Rc<RefCell<Option<HeldHdr>>>,
}

impl WorkerPool {
    /// Spawns `clamp(navigator.hardwareConcurrency - 1, 1, 8)` render Workers and one
    /// solve Worker. The Workers load in the background; calls made before they are
    /// ready are queued (a scene is sent to each render Worker as it becomes ready, a
    /// solve waits for the solve Worker).
    ///
    /// # Errors
    ///
    /// When there is no `window` or a Worker cannot be constructed (the script URL is
    /// missing, or Workers are blocked).
    pub fn new() -> Result<Self, String> {
        let window = web_sys::window().ok_or("no window: the pool must run on the page")?;
        let cores = window.navigator().hardware_concurrency();
        Self::with_render_workers(render_worker_count(cores))
    }

    /// Like [`Self::new`] with an explicit render-Worker count (at least 1).
    ///
    /// # Errors
    ///
    /// See [`Self::new`].
    pub fn with_render_workers(render_workers: u32) -> Result<Self, String> {
        Ok(Self {
            render: RenderPool::new(render_workers.max(1))?,
            solve: SolveClient::new()?,
            analysis: Rc::new(RefCell::new(None)),
            analysis_hdr: Rc::new(RefCell::new(None)),
        })
    }

    /// The render side.
    #[must_use]
    pub const fn render(&self) -> &RenderPool {
        &self.render
    }

    /// The solve side.
    #[must_use]
    pub const fn solve(&self) -> &SolveClient {
        &self.solve
    }

    /// The analysis Worker's client (current-view metrics, the tilt sweep), spawning the
    /// Worker on the first call -- see the type's doc comment. A Worker created while an
    /// HDR map is loaded is given the map before its first job.
    ///
    /// # Errors
    ///
    /// When the Worker cannot be constructed.
    pub fn analysis(&self) -> Result<SolveClient, String> {
        if let Some(client) = self.analysis.borrow().as_ref() {
            return Ok(client.clone());
        }
        let client = SolveClient::new()?;
        if let Some((id, bytes)) = self.analysis_hdr.borrow().as_ref() {
            client.set_hdr(*id, Rc::clone(bytes));
        }
        *self.analysis.borrow_mut() = Some(client.clone());
        Ok(client)
    }

    /// Loads an HDR map under `id` on the render Workers and the analysis Worker (name it
    /// in `SceneSpec::hdr_id` and `MetricsParams::hdr_id`, and call
    /// [`RenderPool::set_scene`] afterwards).
    ///
    /// The map is never downsampled: the render Workers and the analysis Worker each hold
    /// a decoded copy, and when those would pass the 1.5 GiB budget render Workers are
    /// stopped until they fit. When the budget holds a single copy it is the render
    /// Worker's, and the analysis Worker is given none (the metrics then stay under the
    /// lighting preset: the admission's `analysis_copy` is `false`). The admission's
    /// `workers` is the new render-Worker count.
    ///
    /// # Errors
    ///
    /// The refusal text (file too large, not a Radiance file, more texels than the
    /// browser limit, or too large for even one render Worker). Nothing changes then.
    pub fn set_hdr(&self, id: u64, bytes: Vec<u8>) -> Result<HdrAdmission, String> {
        let bytes = Rc::new(bytes);
        let admission = self.render.set_hdr(id, &bytes, true)?;
        let held = admission.analysis_copy.then_some((id, bytes));
        self.hold_in_analysis(held);
        Ok(admission)
    }

    /// Drops the HDR map from every Worker and restores the full render-Worker count.
    ///
    /// # Errors
    ///
    /// When a replacement render Worker cannot be started.
    pub fn clear_hdr(&self) -> Result<(), String> {
        let cleared = self.render.clear_hdr();
        self.hold_in_analysis(None);
        cleared
    }

    /// Makes `held` the map the analysis Worker holds: given to the Worker now if there is
    /// one, and to one created later.
    fn hold_in_analysis(&self, held: Option<HeldHdr>) {
        if let Some(client) = self.analysis.borrow().as_ref() {
            match &held {
                Some((id, bytes)) => client.set_hdr(*id, Rc::clone(bytes)),
                None => client.clear_hdr(),
            }
        }
        *self.analysis_hdr.borrow_mut() = held;
    }
}

/// `performance.now()` on the page, in milliseconds (0 when unavailable).
fn now_ms() -> f64 {
    web_sys::window()
        .and_then(|w| w.performance())
        .map_or(0.0, |p| p.now())
}
