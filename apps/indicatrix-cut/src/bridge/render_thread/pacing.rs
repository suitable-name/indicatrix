//! Display pacing constants, shared unchanged by both image engines.
use std::time::Duration;

/// Minimum real-time gap between two display cycles (snapshot + hand-off to
/// [`super::display_thread`]) being SENT while samples are still accumulating. See the loop
/// body's `denoise_due` computation for the exceptions that always send regardless
/// (first frame, `dirty`, a dimension change, denoising disabled, reaching
/// `target_samples`). The À-Trous denoiser dominates every live frame (267ms/1072ms
/// per call at 800x600/1920x1080), so re-running it every ~16ms iteration while still
/// converging capped the viewport at a few FPS for no visible benefit. 120ms is fast
/// enough to feel live while converging and cuts denoiser invocations ~7-8x.
///
/// The actual denoise+tonemap+push work runs on [`super::display_thread`], so the render
/// thread never blocks on a cycle's cost -- there is nothing to scale a duty-cycle gap
/// against, unlike when that cost was paid synchronously on this thread.
pub(super) const DENOISE_MIN_INTERVAL: Duration = Duration::from_millis(120);

/// Timeout for the convergence wait at [`display.busy()`]. À-Trous cycles run in
/// the 100ms-1s range, so 5s is generous for a healthy display thread.
pub(super) const DISPLAY_BUSY_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Target length of one loop iteration while the picture is being shown or the camera is
/// moving (~60 FPS).
///
/// # Pacing rule
///
/// The loop sleeps only the *remainder* of this interval, measured from the start of the
/// iteration, and only when the iteration either handled a moving camera or had a display
/// cycle due (`denoise_due` in the loop body). An iteration that is merely adding samples
/// to a still picture and has nothing to show does not sleep at all: the GPU is fed the
/// next chunk immediately instead of idling through readback, hand-off and a fixed sleep.
/// The idle states keep their own sleeps (converged, waiting on the remote lane,
/// suspended), so the loop never spins when there is nothing to trace.
pub(super) const FRAME_PACE: Duration = Duration::from_millis(16);
