//! The finished-accumulation/outcome/calibration-carry types [`super::core::render_accumulation`]
//! and its callers share.

use crate::bridge::export_thread::remote::RemoteCapability;
use glam::Vec3;

/// One frame/still image's finished linear accumulation buffer, before tone-mapping --
/// [`super::core::render_accumulation`]'s successful output. `accum` holds exactly
/// `width * height` summed XYZ radiance samples; `samples_per_pixel` always equals the
/// [`super::super::params::ExportParams::samples_per_pixel`] that call was rendered
/// with.
pub struct Accumulation {
    /// Summed XYZ radiance, one entry per pixel, `width * height` long.
    pub accum: Vec<Vec3>,
    /// How many samples deep each pixel in [`Self::accum`] is.
    pub samples_per_pixel: u32,
}

/// [`super::core::render_accumulation`]'s result -- mirrors
/// [`super::super::ExportOutcome`] but stops one step earlier, before
/// tone-mapping/writing to disk, which only [`super::export::run_export`] (the
/// still-image export) and the tilt video's own per-frame PNG write need to do.
pub enum AccumulationOutcome {
    /// The render finished; `run_export`/the tilt video tone-map and save this.
    Completed(Accumulation),
    /// `cancel` was observed before this render finished.
    Cancelled,
    /// The render failed outright (only reachable via `ComputeTarget::RemoteOnly` with
    /// no local fallback to pick up a lost remote chunk).
    Failed(String),
}

/// Calibration/probing state one [`super::core::render_accumulation`] call can carry
/// INTO the next call for the SAME render session, so a multi-frame caller (the tilt
/// performance video) pays remote probing and local hybrid CPU/GPU split calibration
/// once for the whole sweep rather than once per frame -- adapter acquisition and a
/// network handshake are both far too slow to repeat every frame.
/// [`super::export::run_export`]'s own single-still-image call always starts from
/// [`AccumulationCarry::default`], which reproduces exactly the fresh-probe/
/// fresh-calibrate behaviour this type replaced (see each field's own doc comment).
#[derive(Default)]
pub struct AccumulationCarry {
    /// Whether remote availability has already been probed for this carry -- once
    /// `true`, [`super::core::render_accumulation`] reuses [`Self::remote_capability`]
    /// instead of dispatching another `probe_remote` call. A fresh
    /// [`AccumulationCarry`] starts `false`, so the still-image export (which always
    /// uses a fresh one) still probes exactly once, every time, like before this type
    /// existed.
    pub(in crate::bridge::export_thread::worker) remote_probed: bool,
    /// The probed remote worker, if any. `None` covers both "not probed yet" (while
    /// [`Self::remote_probed`] is `false`) and "probed and unavailable" (once it's
    /// `true`).
    pub(in crate::bridge::export_thread::worker) remote_capability: Option<RemoteCapability>,
    /// The local hybrid CPU/GPU split's current best estimate -- seeded from a prior
    /// frame's calibration/adaptation instead of re-measuring one from scratch. Reset
    /// to whatever the most recent frame ended on (including `None` if the GPU
    /// declined partway), so a persistently failing GPU is retried, not permanently
    /// written off, across frames.
    pub(in crate::bridge::export_thread::worker) hybrid_frac: Option<f64>,
    /// The remote lane's current best throughput estimate, samples/sec -- seeded from
    /// a prior frame's calibration/adaptation instead of re-measuring one from
    /// scratch.
    pub(in crate::bridge::export_thread::worker) remote_rate: Option<f64>,
    /// Final-picture-only transfer: the remote answered a `FinalImageRequest` with
    /// `UNSUPPORTED_REQUEST` (or failed one) during this carry's sweep, so every later
    /// frame goes straight to full data instead of asking again.
    pub(in crate::bridge::export_thread::worker) final_picture_declined: bool,
    /// Whether the "this remote does not do final pictures" note was already shown
    /// for this carry -- a video shows it once, not once per frame.
    pub(in crate::bridge::export_thread::worker) transfer_noted: bool,
}
