//! Which lanes a viewer request gets, decided from
//! the request and a snapshot of the registry before anything is streamed.

use super::{Coordinator, RateBook};
use crate::coordinator::{LaneNeed, WorkerInfo};
use indicatrix_dispatch::PoolConfig;
use indicatrix_net::messages::{Backend, ErrorMsg, RequestIntent, TransferMode, error_codes};
use std::sync::atomic::{AtomicBool, Ordering};

/// Which joined workers a job takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerPick {
    /// None (the own lane alone).
    None,
    /// Every idle eligible worker (`Batch`).
    All,
    /// At most this many idle eligible workers, fastest first -- the pinned worker
    /// ([`InteractivePin`]) first when it is one of them (`Interactive` only).
    Fastest(u32),
}

/// A coordinator job's lanes and scheduling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JobPlan {
    /// Whether the own lane takes part.
    pub own: bool,
    /// Which joined workers take part.
    pub workers: WorkerPick,
    /// Whether the job waits its turn in the viewer's FIFO (one active job per viewer).
    pub fifo: bool,
    /// Chunk sizing and failure handling.
    pub pool: PoolConfig,
}

/// How a request is served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    /// By the own lane alone, streamed directly exactly like a plain worker (no chunking,
    /// no pool) -- `Interactive` requests by default, and any request no joined
    /// worker could take.
    Direct,
    /// A lane pool over joined workers (and the own lane, if any).
    Job(JobPlan),
}

/// What kind of request is being planned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ask {
    /// The request's intent (a `FinalImageRequest` counts as `Batch`).
    pub intent: RequestIntent,
    /// The request's transfer mode (`FinalOnly` for a `FinalImageRequest`).
    pub transfer_mode: TransferMode,
    /// `width * height`.
    pub pixels: u32,
    /// The scene is lit by an HDR map: only joined workers advertising `hdr`
    /// take part.
    pub hdr: bool,
}

impl Ask {
    /// What a joined worker must accept to take part.
    #[must_use]
    pub const fn need(self) -> LaneNeed {
        LaneNeed {
            pixels: self.pixels,
            hdr: self.hdr,
        }
    }

    /// Throughput work: EXPORT-sized chunks and the viewer's FIFO. `Batch` with
    /// `FinalOnly` (exports, tilt-video frames, `FinalImageRequest`); anything progressive
    /// or interactive gets INTERACTIVE-sized chunks so progress arrives often.
    const fn is_throughput(self) -> bool {
        matches!(self.intent, RequestIntent::Batch)
            && matches!(self.transfer_mode, TransferMode::FinalOnly)
    }
}

/// Plans `ask` on `coordinator` (see [`Route`]).
///
/// - `Batch`: the own lane (if any) plus every idle worker whose `max_pixels` accepts the
///   image (and, for an HDR scene, that advertises `hdr`).
/// - `Interactive`: the own lane plus at most `--interactive-workers` workers (default
///   0: the own lane alone); with no own lane and 0 workers configured, the single
///   fastest worker, so a render-less coordinator still serves the live view.
/// - A request only the own lane would serve goes [`Route::Direct`].
///
/// # Errors
///
/// The refusal to send when no lane can take the request: `NO_RENDER_CAPACITY` with
/// neither an own lane nor joined workers, `UNSUPPORTED_REQUEST` (a capability limit the
/// viewer falls back from) when the image is larger than every joined worker accepts, or
/// the scene is HDR-lit and no joined worker renders HDR.
pub fn plan(coordinator: &Coordinator, ask: Ask) -> Result<Route, ErrorMsg> {
    let own = coordinator.own.is_some();
    let workers = coordinator
        .registry
        .as_ref()
        .map(|r| r.workers())
        .unwrap_or_default();
    let need = ask.need();
    let eligible = workers.iter().filter(|(w, _)| need.accepts(w)).count();
    if !own && eligible == 0 {
        return Err(refusal(&workers, need));
    }
    let wanted = if ask.intent == RequestIntent::Interactive {
        match coordinator.interactive_workers {
            0 if own => WorkerPick::None,
            0 => WorkerPick::Fastest(1),
            n => WorkerPick::Fastest(n),
        }
    } else {
        WorkerPick::All
    };
    let workers = if eligible == 0 {
        WorkerPick::None
    } else {
        wanted
    };
    if own && workers == WorkerPick::None {
        return Ok(Route::Direct);
    }
    let config = coordinator.job_config();
    Ok(Route::Job(JobPlan {
        own,
        workers,
        fifo: ask.is_throughput(),
        pool: if ask.is_throughput() {
            config.batch
        } else {
            config.interactive
        },
    }))
}

/// The refusal for a request no lane can take.
fn refusal(workers: &[(WorkerInfo, bool)], need: LaneNeed) -> ErrorMsg {
    let pixels = need.pixels;
    if need.hdr
        && workers
            .iter()
            .any(|(w, _)| LaneNeed::pixels(pixels).accepts(w))
    {
        return ErrorMsg {
            code: error_codes::UNSUPPORTED_REQUEST,
            request_id: None,
            message: "the scene is lit by an HDR map, but no joined worker renders HDR environments \
                      (their asset caches are disabled) and this coordinator has no own render lane \
                      (--render)"
                .to_string(),
        };
    }
    if workers.is_empty() {
        return ErrorMsg {
            code: error_codes::NO_RENDER_CAPACITY,
            request_id: None,
            message:
                "this coordinator has no render lane -- no joined workers and no --render; its \
                      WELCOME advertised no render capability"
                    .to_string(),
        };
    }
    let largest = workers
        .iter()
        .map(|(w, _)| w.capability.max_pixels)
        .max()
        .unwrap_or(0);
    ErrorMsg {
        code: error_codes::UNSUPPORTED_REQUEST,
        request_id: None,
        message: format!(
            "the image has {pixels} px, more than any joined worker accepts (largest max_pixels \
             {largest}) and this coordinator has no own render lane (--render)"
        ),
    }
}

/// Orders `candidates` for an `Interactive` pick: workers never measured on this
/// viewer connection first (GPU before CPU, more threads first -- each gets one chance
/// to be measured), then measured ones by rate, fastest first. `pixels` is the current
/// request's image size, used to read each worker's [`RateBook`] entry back at a
/// comparable resolution (see [`RateBook`]'s doc comment).
pub fn rank_fastest(candidates: &mut [WorkerInfo], rates: &RateBook, pixels: u32) {
    candidates.sort_by(|a, b| {
        let key = |w: &WorkerInfo| {
            let measured = rates.worker(w, pixels);
            let guess = match w.capability.backend {
                Backend::Gpu { .. } => f64::from(u32::MAX),
                Backend::Cpu { threads } => f64::from(threads),
                Backend::Coordinator { threads, gpus, .. } => {
                    f64::from(gpus).mul_add(f64::from(u32::MAX), f64::from(threads))
                }
            };
            (measured.is_some(), measured.unwrap_or(guess))
        };
        let (a_measured, a_rate) = key(a);
        let (b_measured, b_rate) = key(b);
        a_measured
            .cmp(&b_measured)
            .then_with(|| b_rate.total_cmp(&a_rate))
            .then_with(|| a.worker_id.cmp(&b.worker_id))
    });
}

/// `serve --pin-interactive-worker <label>` (advanced): the joined worker whose
/// certificate label (`WorkerInfo::label`) an `Interactive` request that takes workers
/// uses first.
///
/// Applies only where [`plan`] picks [`WorkerPick::Fastest`] -- a coordinator without
/// `--render`, or `--interactive-workers N > 0`. When no idle eligible connection of the
/// pinned worker exists, the pick falls back to the fastest-by-rate order
/// ([`rank_fastest`]) and says so in the log -- once per change, not per request, since a
/// live view sends several requests a second.
#[derive(Debug)]
pub struct InteractivePin {
    label: String,
    /// The last pick fell back (the pinned worker was not available).
    falling_back: AtomicBool,
}

impl InteractivePin {
    /// Pins the worker whose certificate label is `label`.
    #[must_use]
    pub const fn new(label: String) -> Self {
        Self {
            label,
            falling_back: AtomicBool::new(false),
        }
    }

    /// The pinned label.
    #[must_use]
    pub fn label(&self) -> &str {
        &self.label
    }

    /// Moves the best-ranked candidate carrying the pinned label to the front of
    /// `ranked` (already in [`rank_fastest`] order), keeping everyone else's order.
    /// Returns whether it was there; logs when that changes.
    pub fn apply(&self, ranked: &mut [WorkerInfo]) -> bool {
        let found = ranked
            .iter()
            .position(|w| w.label.as_deref() == Some(self.label.as_str()));
        let Some(index) = found else {
            if !self.falling_back.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    "coordinator: pinned interactive worker {:?} is not connected, idle and able to \
                     take the image; live-view requests fall back to the fastest idle worker(s)",
                    self.label
                );
            }
            return false;
        };
        ranked[..=index].rotate_right(1);
        if self.falling_back.swap(false, Ordering::Relaxed) {
            tracing::info!(
                "coordinator: pinned interactive worker {:?} is available again; live-view \
                 requests use it first",
                self.label
            );
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{super::producer::checkout_fastest, *};
    use crate::coordinator::{LivenessConfig, Registry, WorkerHandle};
    use indicatrix_net::messages::{PayloadEncoding, RenderCapability};
    use std::{
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
    };

    fn info(worker_id: u32, backend: Backend, label: Option<&str>) -> WorkerInfo {
        WorkerInfo {
            worker_id,
            capability: RenderCapability {
                backend,
                max_pixels: 1_000,
                min_cadence_ms: 100,
                hdr: false,
            },
            peer: None,
            label: label.map(str::to_string),
            payload_encoding: PayloadEncoding::Raw,
        }
    }

    /// A registry of three idle fake workers: a GPU box (ranked fastest), and two CPU
    /// connections -- `slow` (2 threads) and `mid` (8 threads). The far ends are
    /// returned to keep the connections open.
    fn registry() -> (Arc<Registry>, Vec<TcpStream>) {
        let registry = Registry::new(LivenessConfig::default());
        let mut far_ends = Vec::new();
        let gpu = Backend::Gpu {
            adapter: "test".to_string(),
        };
        for (backend, label) in [
            (gpu, "gpu-box"),
            (Backend::Cpu { threads: 2 }, "slow"),
            (Backend::Cpu { threads: 8 }, "mid"),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let near = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
            far_ends.push(listener.accept().unwrap().0);
            let worker_id = registry.allocate_id();
            registry.insert(info(worker_id, backend, Some(label)), Box::new(near), None);
        }
        (registry, far_ends)
    }

    fn labels(handles: &[WorkerHandle]) -> Vec<&str> {
        handles
            .iter()
            .map(|h| h.info().label.as_deref().unwrap_or("-"))
            .collect()
    }

    /// The pinned worker is checked out first even though the rate order ranks it last;
    /// the remaining picks keep the fastest-first order.
    #[test]
    fn an_available_pinned_worker_is_used_first() {
        let (registry, _far) = registry();
        let rates = Mutex::new(RateBook::default());
        let pin = InteractivePin::new("slow".to_string());
        let one = checkout_fastest(&registry, &rates, LaneNeed::pixels(100), 1, Some(&pin));
        assert_eq!(labels(&one), ["slow"]);
        drop(one);
        let two = checkout_fastest(&registry, &rates, LaneNeed::pixels(100), 2, Some(&pin));
        assert_eq!(labels(&two), ["slow", "gpu-box"]);
        assert!(!pin.falling_back.load(Ordering::Relaxed));
    }

    /// A pinned worker that is absent, or busy, falls back to the plain fastest-first
    /// pick (and flags the fallback for the log).
    #[test]
    fn an_unavailable_pinned_worker_falls_back_to_the_fastest() {
        let (registry, _far) = registry();
        let rates = Mutex::new(RateBook::default());
        let absent = InteractivePin::new("not-joined".to_string());
        let picked = checkout_fastest(&registry, &rates, LaneNeed::pixels(100), 1, Some(&absent));
        assert_eq!(labels(&picked), ["gpu-box"]);
        assert!(absent.falling_back.load(Ordering::Relaxed));

        let busy = InteractivePin::new("mid".to_string());
        let held = checkout_fastest(&registry, &rates, LaneNeed::pixels(100), 1, Some(&busy));
        assert_eq!(labels(&held), ["mid"]);
        let next = checkout_fastest(&registry, &rates, LaneNeed::pixels(100), 1, Some(&busy));
        assert_eq!(
            labels(&next),
            ["slow"],
            "gpu-box and mid are both checked out"
        );
        assert!(busy.falling_back.load(Ordering::Relaxed));
    }

    /// `apply` keeps the relative order of everyone but the pinned worker.
    #[test]
    fn apply_moves_only_the_pinned_worker() {
        let mut ranked = vec![
            info(1, Backend::Cpu { threads: 8 }, Some("a")),
            info(2, Backend::Cpu { threads: 4 }, None),
            info(3, Backend::Cpu { threads: 2 }, Some("c")),
        ];
        assert!(InteractivePin::new("c".to_string()).apply(&mut ranked));
        let ids: Vec<u32> = ranked.iter().map(|w| w.worker_id).collect();
        assert_eq!(ids, [3, 1, 2]);
        assert!(!InteractivePin::new("zzz".to_string()).apply(&mut ranked));
        let ids: Vec<u32> = ranked.iter().map(|w| w.worker_id).collect();
        assert_eq!(ids, [3, 1, 2]);
    }
}
