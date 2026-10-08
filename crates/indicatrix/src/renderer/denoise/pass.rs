//! The per-pixel À-Trous kernel and the pass scheduler behind `AtrousDenoiser`.
//!
//! # Scheduling
//!
//! Every pass reads the previous pass's whole image, so passes are separated by a full
//! barrier, but all passes of one denoise share a single `std::thread::scope`. The worker
//! threads are spawned once and sleep on a [`PhaseGate`] between passes instead of being
//! spawned and joined once per pass, and the calling thread works too. A pass is cut into
//! contiguous row bands ([`BANDS_PER_THREAD`] per thread) that threads claim one at a
//! time, so a stone over a cheap backdrop still balances across cores.
//!
//! [`atrous_pixel`] is a pure function of the previous pass's image and the guide
//! buffers, so which thread filters which band is only a scheduling decision: the result
//! is bit-identical for any thread count.
//!
//! # Shared images
//!
//! Safe Rust cannot give every thread `&[Vec3]` of a buffer in one pass and hand out
//! disjoint `&mut` bands of the same buffer in the next, so [`SharedColors`] keeps each
//! channel as the bit pattern of an `f32` in a relaxed `AtomicU32`. The gate's mutex
//! orders the phases (a band's writes happen-before the claim that reads them), so
//! relaxed access is enough and compiles to plain loads and stores.

use super::{GBuffers, SIGMA_EPS};
use glam::Vec3;
use std::{
    any::Any,
    ops::Range,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    sync::{
        Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
    thread,
};

/// 1D B3-spline kernel taps for offsets `-2, -1, 0, 1, 2`. The 2D 5x5 kernel weight at
/// `(dx, dy)` is the separable product `B3[dx+2] * B3[dy+2]`.
const B3_SPLINE: [f32; 5] = [1.0 / 16.0, 1.0 / 4.0, 3.0 / 8.0, 1.0 / 4.0, 1.0 / 16.0];

/// How many row bands each thread's share of a pass is cut into. More bands than threads
/// lets a thread that drew cheap rows (backdrop) claim more of the expensive ones.
const BANDS_PER_THREAD: usize = 4;

#[inline]
const fn sanitize(v: f32) -> f32 {
    if v.is_finite() { v } else { 0.0 }
}

#[inline]
const fn sanitize_vec3(v: Vec3) -> Vec3 {
    Vec3::new(sanitize(v.x), sanitize(v.y), sanitize(v.z))
}

/// Hard Kronecker-delta facet weight: `1.0` on a match, `0.0` otherwise. Negative
/// values (background/miss) only match an equal negative value.
#[inline]
const fn facet_weight(a: i32, b: i32) -> f32 {
    if a == b { 1.0 } else { 0.0 }
}

/// Normalises `n` once (defensively -- see `GBuffers::normal`) into a unit vector, or
/// returns [`Vec3::ZERO`] when there is no usable normal information (zero/near-zero
/// length or non-finite components, after [`sanitize_vec3`]).
///
/// `Vec3::ZERO` doubles as that sentinel rather than needing a separate validity
/// buffer: a true unit vector can never have exactly zero magnitude. Used to
/// pre-normalise `GBuffers::normal` once per call instead of on every one of the up
/// to 250 (25 taps x `num_passes`) per-pixel evaluations.
#[inline]
pub(super) fn unit_normal_or_zero(n: Vec3) -> Vec3 {
    let n = sanitize_vec3(n);
    let len = n.length();
    if len <= SIGMA_EPS {
        Vec3::ZERO
    } else {
        n / len
    }
}

/// `base.powf(power)` for the clamped-nonnegative cosine `base` used by
/// [`normal_weight_unit`], computed by exact repeated squaring when `power` is a
/// nonnegative power-of-two integer fitting a `u32` (in particular the
/// `AtrousParams::default` value `64.0`: 6 squarings, never entering `f32::powf`'s
/// transcendental path), falling back to [`f32::powf`] otherwise.
///
/// Not required to be bit-identical to `powf`, only close: this feeds a *display*
/// filter where such differences are far below anything visible.
#[inline]
pub(super) fn cos_pow(base: f32, power: f32) -> f32 {
    if power >= 0.0 && power.fract() == 0.0 && power <= u32::MAX as f32 {
        let int_power = power as u32;
        if int_power.is_power_of_two() {
            let squarings = int_power.trailing_zeros();
            let mut v = base;
            for _ in 0..squarings {
                v *= v;
            }
            return v;
        }
    }
    base.powf(power)
}

/// Same edge-stopping term as the module docs describe for "Normal", but taking
/// pre-normalised unit vectors (or the [`unit_normal_or_zero`] zero-sentinel) instead of
/// raw normals -- see `AtrousDenoiser::denoise_into_with_threads`'s precompute step.
#[inline]
fn normal_weight_unit(a_n: Vec3, b_n: Vec3, power: f32) -> f32 {
    if a_n == Vec3::ZERO || b_n == Vec3::ZERO {
        // No usable normal information on one side: treat as neutral rather than
        // rejecting, matching the pre-precompute behaviour this replaced.
        return 1.0;
    }
    let cos_theta = a_n.dot(b_n).clamp(-1.0, 1.0).max(0.0);
    cos_pow(cos_theta, power.max(0.0))
}

/// Same edge-stopping term as the module docs describe for "Depth", but taking a
/// precomputed `-1 / sigma_depth` instead of `sigma_depth` itself (`sigma_depth` is
/// constant across every tap of every pass within a call). An exact tie skips the `exp`:
/// `exp(-0.0)` is exactly `1.0`, so the result is unchanged.
#[inline]
fn depth_weight(a: f32, b: f32, neg_inv_sigma_depth: f32) -> f32 {
    let gap = (sanitize(a) - sanitize(b)).abs();
    if gap == 0.0 {
        1.0
    } else {
        (gap * neg_inv_sigma_depth).exp()
    }
}

/// Same edge-stopping term as the module docs describe for "color", but taking a
/// precomputed `-1 / (2 * sigma_color_effective^2)` instead of `sigma_color_effective`
/// itself. Both colors must be finite (the caller skips non-finite texels). An exact
/// tie skips the `exp` for the same reason as in [`depth_weight`].
#[inline]
fn color_weight(a: Vec3, b: Vec3, neg_inv_two_sigma_sq: f32) -> f32 {
    let dist_sq = (a - b).length_squared();
    if dist_sq == 0.0 {
        1.0
    } else {
        (dist_sq * neg_inv_two_sigma_sq).exp()
    }
}

/// Per-call constants and scratch buffers threaded through every tap of every pass,
/// computed once rather than re-derived on each of the up to 250 (25 taps x 8 passes)
/// evaluations a single pixel can see.
pub(super) struct TapConstants<'a> {
    /// `GBuffers::normal`, pre-normalised per pixel via [`unit_normal_or_zero`].
    pub(super) normal_n: &'a [Vec3],
    /// `-1 / max(sigma_depth, SIGMA_EPS)`, folding [`depth_weight`]'s division into a
    /// multiply.
    pub(super) neg_inv_sigma_depth: f32,
    /// `-1 / (2 * max(sigma_color_effective^2, SIGMA_EPS))`, folding [`color_weight`]'s
    /// division into a multiply.
    pub(super) neg_inv_two_sigma_color_sq: f32,
    /// `AtrousParams::normal_power`, unmodified (not a reciprocal -- [`cos_pow`] uses
    /// it directly).
    pub(super) normal_power: f32,
}

/// An image that every thread can read while each thread writes its own bands, see the
/// module docs. Indexed like the `Vec3` slices it mirrors (`y * width + x`).
#[derive(Default)]
pub(super) struct SharedColors {
    texels: Vec<[AtomicU32; 3]>,
}

impl SharedColors {
    /// An empty image; [`Self::resize`] sizes it.
    pub(super) const fn new() -> Self {
        Self { texels: Vec::new() }
    }

    /// Resizes to `len` texels, discarding the contents when the length changes.
    pub(super) fn resize(&mut self, len: usize) {
        if self.texels.len() != len {
            self.texels.clear();
            self.texels.resize_with(len, Default::default);
        }
    }

    /// Reads texel `idx`.
    #[inline]
    fn load(&self, idx: usize) -> Vec3 {
        let [x, y, z] = &self.texels[idx];
        Vec3::new(
            f32::from_bits(x.load(Ordering::Relaxed)),
            f32::from_bits(y.load(Ordering::Relaxed)),
            f32::from_bits(z.load(Ordering::Relaxed)),
        )
    }

    /// Writes texel `idx`.
    #[inline]
    fn store(&self, idx: usize, value: Vec3) {
        let [x, y, z] = &self.texels[idx];
        x.store(value.x.to_bits(), Ordering::Relaxed);
        y.store(value.y.to_bits(), Ordering::Relaxed);
        z.store(value.z.to_bits(), Ordering::Relaxed);
    }

    /// Overwrites the leading texels with `source`.
    pub(super) fn fill_from(&self, source: &[Vec3]) {
        for (idx, &value) in source.iter().enumerate().take(self.texels.len()) {
            self.store(idx, value);
        }
    }

    /// Copies the leading texels into `dest`.
    pub(super) fn copy_to(&self, dest: &mut [Vec3]) {
        let shared = self.texels.len();
        for (idx, out) in dest.iter_mut().enumerate().take(shared) {
            *out = self.load(idx);
        }
    }
}

/// The centre pixel's own guide values, read once per output pixel.
struct Centre {
    facet: i32,
    /// `GBuffers::path_sig` at the centre pixel; `0` when the buffer is absent.
    sig: u32,
    depth: f32,
    normal: Vec3,
    color: Vec3,
}

/// `base + offset` when that lands inside `0..limit`.
#[inline]
fn shifted(base: usize, offset: i64, limit: usize) -> Option<usize> {
    usize::try_from(base as i64 + offset)
        .ok()
        .filter(|&pos| pos < limit)
}

/// The neighbour at `idx` as `(texel, weight)`, or `None` when it must not contribute:
/// a different facet, a different interior path signature (when `GBuffers::path_sig` is
/// present; a pure early return, no weight is touched), a non-finite texel (it would contaminate every tap that shares
/// it), or a normal weight of exactly zero (the product would be zero anyway, so the
/// two `exp` calls are skipped). The factor order is fixed: it is the filter's
/// bit-exact definition.
#[inline]
fn tap_contribution(
    centre: &Centre,
    g: &GBuffers<'_>,
    tap: &TapConstants<'_>,
    src: &SharedColors,
    idx: usize,
    kernel_w: f32,
) -> Option<(Vec3, f32)> {
    let wf = facet_weight(centre.facet, g.facet_id[idx]);
    if wf == 0.0 {
        // Hard rejection: skip the (cheap) remaining term evaluation too.
        return None;
    }
    if let Some(sig) = g.path_sig
        && sig[idx] != centre.sig
    {
        // Same facet, different interior path: a different reflection region.
        return None;
    }
    let texel = src.load(idx);
    if !texel.is_finite() {
        return None;
    }
    let wn = normal_weight_unit(centre.normal, tap.normal_n[idx], tap.normal_power);
    if wn == 0.0 {
        return None;
    }
    let wd = depth_weight(centre.depth, g.depth[idx], tap.neg_inv_sigma_depth);
    let wc = color_weight(centre.color, texel, tap.neg_inv_two_sigma_color_sq);
    Some((texel, kernel_w * wf * wn * wd * wc))
}

/// Computes the filtered color for a single output pixel at `(x, y)`. Pure function of
/// `src` and the guide buffers `g`/`tap` -- no accumulation across pixels, no ordering
/// dependency on any other pixel's result. This is what makes the band-parallel
/// schedule bit-identical to the single-threaded form.
///
/// A non-finite centre texel is treated as black, and a pixel with a negative facet id
/// (background) is copied through unfiltered.
#[inline]
fn atrous_pixel(
    src: &SharedColors,
    g: &GBuffers<'_>,
    x: usize,
    y: usize,
    stride: i64,
    tap: &TapConstants<'_>,
) -> Vec3 {
    let center_idx = y * g.width + x;
    let raw = src.load(center_idx);
    let color = if raw.is_finite() { raw } else { Vec3::ZERO };
    let facet = g.facet_id[center_idx];
    if facet < 0 {
        return color;
    }
    let centre = Centre {
        facet,
        sig: g.path_sig.map_or(0, |sig| sig[center_idx]),
        depth: g.depth[center_idx],
        normal: tap.normal_n[center_idx],
        color,
    };

    let mut sum = Vec3::ZERO;
    let mut weight_sum = 0.0f32;
    for (ky, &hy) in B3_SPLINE.iter().enumerate() {
        let Some(sy) = shifted(y, (ky as i64 - 2) * stride, g.height) else {
            continue;
        };
        for (kx, &hx) in B3_SPLINE.iter().enumerate() {
            let Some(sx) = shifted(x, (kx as i64 - 2) * stride, g.width) else {
                continue;
            };
            let idx = sy * g.width + sx;
            let Some((texel, w)) = tap_contribution(&centre, g, tap, src, idx, hy * hx) else {
                continue;
            };
            sum += texel * w;
            weight_sum += w;
        }
    }

    if weight_sum > SIGMA_EPS {
        sum / weight_sum
    } else {
        color
    }
}

/// Filters the rows `rows` of one pass at the given dilation `stride`, reading `src` and
/// writing the same rows of `dst`.
fn filter_rows(
    src: &SharedColors,
    dst: &SharedColors,
    g: &GBuffers<'_>,
    stride: i64,
    tap: &TapConstants<'_>,
    rows: Range<usize>,
) {
    for y in rows {
        for x in 0..g.width {
            dst.store(y * g.width + x, atrous_pixel(src, g, x, y, stride, tap));
        }
    }
}

/// What a caught panic carries.
type PanicPayload = Box<dyn Any + Send>;

/// The mutable state behind [`PhaseGate`].
struct GateState {
    /// Number of phases begun so far; the running phase is `phase - 1`.
    phase: usize,
    /// Bands per phase.
    bands: usize,
    /// Bands of the running phase already claimed. Starts at `bands` so nothing is
    /// claimable before the first [`PhaseGate::begin_phase`].
    claimed: usize,
    /// Bands of the running phase already finished.
    finished: usize,
    /// Set once no further phase will begin; sleeping workers exit.
    shutdown: bool,
    /// The first panic a band raised.
    failure: Option<PanicPayload>,
}

/// Hands the bands of each phase to whichever thread asks, and lets the owner wait for a
/// phase to drain before it opens the next. Workers sleep between phases.
struct PhaseGate {
    state: Mutex<GateState>,
    changed: Condvar,
}

impl PhaseGate {
    const fn new(bands: usize) -> Self {
        Self {
            state: Mutex::new(GateState {
                phase: 0,
                bands,
                claimed: bands,
                finished: bands,
                shutdown: false,
                failure: None,
            }),
            changed: Condvar::new(),
        }
    }

    /// Locks the state. A poisoned lock is still usable: the state is only ever mutated
    /// by short, panic-free critical sections.
    fn lock(&self) -> MutexGuard<'_, GateState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens the next phase: every band becomes claimable and sleeping workers wake.
    fn begin_phase(&self) {
        {
            let mut state = self.lock();
            state.phase += 1;
            state.claimed = 0;
            state.finished = 0;
        }
        self.changed.notify_all();
    }

    /// Claims one band of the running phase as `(pass, band)`, or `None` when all are
    /// taken. The pass index is read under the same lock as the claim, so a claim can
    /// never pair a band with a stale pass.
    fn claim(&self) -> Option<(usize, usize)> {
        let mut state = self.lock();
        if state.claimed >= state.bands {
            return None;
        }
        let band = state.claimed;
        state.claimed += 1;
        Some((state.phase.saturating_sub(1), band))
    }

    /// Records a finished band (with the panic it raised, if any) and wakes the owner
    /// when it was the last one.
    fn finish(&self, failure: Option<PanicPayload>) {
        let all_done = {
            let mut state = self.lock();
            state.finished += 1;
            if state.failure.is_none() {
                state.failure = failure;
            }
            state.finished == state.bands
        };
        if all_done {
            self.changed.notify_all();
        }
    }

    /// Claims and runs bands until none are left. A panicking band is recorded, never
    /// propagated, so the phase always drains and nobody waits forever.
    fn serve(&self, run_band: &(dyn Fn(usize, usize) + Sync)) {
        while let Some((pass, band)) = self.claim() {
            let outcome = catch_unwind(AssertUnwindSafe(|| run_band(pass, band)));
            self.finish(outcome.err());
        }
    }

    /// Blocks until every band of the running phase has finished.
    fn wait_done(&self) {
        drop(
            self.changed
                .wait_while(self.lock(), |state| state.finished < state.bands)
                .unwrap_or_else(PoisonError::into_inner),
        );
    }

    /// A worker thread's whole life: sleep until a phase begins, serve it, repeat, until
    /// [`Self::shutdown`].
    fn work(&self, run_band: &(dyn Fn(usize, usize) + Sync)) {
        let mut served = 0;
        loop {
            let (shutdown, phase) = {
                let state = self
                    .changed
                    .wait_while(self.lock(), |state| {
                        !state.shutdown && state.phase == served
                    })
                    .unwrap_or_else(PoisonError::into_inner);
                (state.shutdown, state.phase)
            };
            if shutdown {
                return;
            }
            served = phase;
            self.serve(run_band);
        }
    }

    /// Tells every worker to exit once it is idle.
    fn shutdown(&self) {
        self.lock().shutdown = true;
        self.changed.notify_all();
    }

    /// Takes the first panic a band raised, if any.
    fn take_failure(&self) -> Option<PanicPayload> {
        self.lock().failure.take()
    }
}

/// Shuts the gate down on any exit from the scope body, including an unwind, so workers
/// never outlive it asleep.
struct ShutdownOnDrop<'a>(&'a PhaseGate);

impl Drop for ShutdownOnDrop<'_> {
    fn drop(&mut self) {
        self.0.shutdown();
    }
}

/// Runs `passes` phases of `bands` bands each, separated by full barriers, on the
/// calling thread plus up to `workers` helper threads inside one `thread::scope`.
/// `run_band(pass, band)` is called exactly once per pair.
///
/// A helper that cannot be spawned only costs parallelism: the calling thread claims
/// whatever nobody else does. A panic inside `run_band` is re-raised on the calling
/// thread after the scope has ended.
fn run_scoped(
    run_band: &(dyn Fn(usize, usize) + Sync),
    bands: usize,
    workers: usize,
    passes: usize,
) {
    let gate = PhaseGate::new(bands);
    thread::scope(|scope| {
        let _shutdown = ShutdownOnDrop(&gate);
        for _ in 0..workers {
            if thread::Builder::new()
                .spawn_scoped(scope, || gate.work(run_band))
                .is_err()
            {
                break;
            }
        }
        for _ in 0..passes {
            gate.begin_phase();
            gate.serve(run_band);
            gate.wait_done();
        }
    });
    if let Some(payload) = gate.take_failure() {
        resume_unwind(payload);
    }
}

/// Runs `num_passes` À-Trous passes over `buffers`, pass `n` reading `buffers[n % 2]`
/// and writing the other, with `buffers[0]` holding the noisy input.
///
/// `num_threads <= 1` (or an image too short to split) runs on the calling thread with
/// no scope at all. Returns the index of the buffer holding the final image.
pub(super) fn run_passes(
    buffers: &[SharedColors; 2],
    g: &GBuffers<'_>,
    tap: &TapConstants<'_>,
    num_passes: u32,
    num_threads: usize,
) -> usize {
    let height = g.height;
    if g.width == 0 || height == 0 {
        return 0;
    }
    let band_target = if num_threads <= 1 {
        1
    } else {
        num_threads.saturating_mul(BANDS_PER_THREAD).min(height)
    };
    let rows_per_band = height.div_ceil(band_target);
    let bands = height.div_ceil(rows_per_band);
    let passes = num_passes as usize;

    let run_band = |pass: usize, band: usize| {
        let first = band * rows_per_band;
        filter_rows(
            &buffers[pass & 1],
            &buffers[(pass & 1) ^ 1],
            g,
            1_i64 << pass,
            tap,
            first..(first + rows_per_band).min(height),
        );
    };

    if bands == 1 {
        for pass in 0..passes {
            run_band(pass, 0);
        }
    } else {
        run_scoped(&run_band, bands, num_threads.min(bands) - 1, passes);
    }
    passes & 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;

    /// Every band of every pass runs exactly once, and a pass never starts before the
    /// previous one has fully finished.
    #[test]
    fn every_band_runs_once_per_pass_and_passes_do_not_overlap() {
        const BANDS: usize = 13;
        const PASSES: usize = 5;
        let done: Vec<AtomicUsize> = (0..PASSES).map(|_| AtomicUsize::new(0)).collect();
        let run = |pass: usize, _band: usize| {
            if pass > 0 {
                assert_eq!(
                    done[pass - 1].load(Ordering::SeqCst),
                    BANDS,
                    "pass {pass} started before the previous pass finished"
                );
            }
            done[pass].fetch_add(1, Ordering::SeqCst);
        };
        run_scoped(&run, BANDS, 3, PASSES);
        for (pass, count) in done.iter().enumerate() {
            assert_eq!(count.load(Ordering::SeqCst), BANDS, "pass {pass}");
        }
    }

    /// A band that panics unwinds the caller instead of leaving the workers and the
    /// caller waiting on each other forever.
    #[test]
    fn a_panicking_band_unwinds_the_caller_instead_of_hanging() {
        let run = |_pass: usize, band: usize| {
            assert_ne!(band, 2, "deliberate test panic");
        };
        let outcome = catch_unwind(AssertUnwindSafe(|| run_scoped(&run, 6, 3, 2)));
        assert!(outcome.is_err());
    }

    /// With no helper threads the calling thread alone drains every phase.
    #[test]
    fn the_caller_alone_drains_every_phase() {
        let ran = AtomicUsize::new(0);
        let run = |_pass: usize, _band: usize| {
            ran.fetch_add(1, Ordering::SeqCst);
        };
        run_scoped(&run, 7, 0, 4);
        assert_eq!(ran.load(Ordering::SeqCst), 7 * 4);
    }
}
