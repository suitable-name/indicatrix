//! [`LocalComputeTarget`]: which engine(s) the live viewport's local (non-remote)
//! render loop uses, on a build compiled with the `gpu` feature.
//!
//! Separate from `worker.rs`'s [`super::worker::LiveComputeTarget`] (the
//! Local/Remote/Local+Remote choice): this answers a different question -- which
//! engine(s) trace local's own share of the work -- unrelated to remote workers.

use serde::{Deserialize, Serialize};

/// Which engine(s) the local (non-remote) live render loop uses -- exposed in
/// `settings_dialog.slint`'s "Local Compute" pill row, shown only on a build compiled
/// with the `gpu` feature; a CPU-only build has no GPU path to choose between, so the
/// control stays hidden rather than offered and ignored.
///
/// See `gpu_backend::accumulate_frame_samples` for how each variant changes per-frame
/// dispatch, and `GpuBackend` for the decline-and-fall-back policy [`Gpu`](Self::Gpu)
/// relies on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum LocalComputeTarget {
    /// Never dispatches to the GPU, even with a usable adapter -- every frame traces
    /// on the CPU scanline tracer. For A/B comparison against the GPU path, or a
    /// machine whose adapter misbehaves -- the live-viewport analogue of
    /// `indicatrix-worker`'s `--only-cpu`.
    Cpu,
    /// Today's hybrid behaviour: once both engines' throughputs are known, each
    /// frame's samples split between CPU and GPU and trace CONCURRENTLY over disjoint
    /// sub-ranges (see `HybridPacing`), falling back to a single engine while pacing
    /// is still measuring. The default, so an existing settings file and a fresh
    /// install both get unchanged viewport behaviour.
    #[default]
    CpuGpu,
    /// GPU carries the WHOLE frame -- no hybrid split -- but still falls back to the
    /// CPU tracer for any individual frame the GPU declines (no adapter, or an
    /// unsupported material). Without that per-frame fallback, a declined scene would
    /// render nothing at all rather than just running slower than `CpuGpu`.
    Gpu,
}
