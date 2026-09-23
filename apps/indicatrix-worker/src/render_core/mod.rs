//! The one function both subcommands use to actually run the tracer: [`trace_samples`].
//!
//! It traces an explicit `[first_sample, first_sample + samples)` range across a whole
//! frame and returns the summed (never averaged) per-pixel XYZ radiance -- exactly what
//! a caller accumulating contributions from multiple workers needs to add into its own
//! running total. See `indicatrix_net`'s crate docs for why sample-index partitioning
//! (not screen-space tiling) is what makes that additive.
//!
//! # Keeping the seed formula in sync with the viewer
//!
//! The per-sample RNG seed and pixel-jitter derivation below is copied verbatim from
//! `apps/indicatrix-cut/src/bridge/render_thread.rs`'s `render_frame_scanlines`. It is
//! not re-derived here: the formula is a function of `(global_pixel_idx, sample_num)`
//! alone, `sample_num` being the absolute sample index, never a batch-relative offset --
//! exactly what lets tracing a sample range here, remotely, produce numbers identical to
//! the viewer tracing that range itself. If this formula and the viewer's ever drift
//! apart, a remote worker's contribution silently composes into a wrong but
//! plausible-looking image (the wire-protocol analogue of the build-hash check that
//! catches drift on the physics side).
//!
//! # A process-wide permit pool bounds total CPU-tracer threads across connections
//!
//! This worker runs one thread per connection and one tracer thread per request; each
//! tracer's [`trace_into`] then spawns `effective_thread_count(threads)` scoped threads
//! of its own for one sub-batch. Each of those numbers is reasonable in isolation, but
//! nothing before this coordinated them across CONCURRENT requests: two clients
//! streaming at once each ask for (typically) every available core, oversubscribing the
//! CPU 2x (and N clients, Nx) -- every thread fighting the others for the same cores,
//! worse throughput for everyone rather than either request simply queueing its share.
//!
//! [`THREAD_PERMITS`] fixes this: a process-wide pool sized to
//! `effective_thread_count(0)` (this machine's `available_parallelism`, the same number
//! a solo request would ask for) that every [`trace_into`] call acquires `num_threads`
//! permits from before its `thread::scope` and releases right after. A request wanting
//! more threads than the pool's total capacity is clamped to that capacity (see
//! [`ThreadPermits::acquire`]) rather than blocking forever on permits that could never
//! all be free simultaneously. Permits are held only for the duration of one `trace_into`
//! call (one sub-batch, not a whole request), so a large request doesn't starve a second
//! client indefinitely -- it interleaves at sub-batch granularity instead.
//!
//! The GPU submitter thread (`hybrid::dispatch_concurrently`'s scoped thread that submits
//! and polls a GPU dispatch) takes no permits at all: it isn't a CPU tracer thread, and
//! the "one core reserved for the GPU submitter" accounting in
//! [`hybrid::cpu_threads_beside_gpu`] already shrinks the CPU side's OWN permit request
//! by one to leave that thread room -- the two mechanisms compose without double-counting
//! or deadlocking each other, since neither ever waits on the other's permits.

use glam::Vec3;
use indicatrix::optics::raytracer::{
    Camera, EnvironmentSource, FacetFinish, build_plane_soa, pixel_rotations, sample_draws,
    trace_spectral_ray_with_finish_soa,
};
use indicatrix_net::SceneState;
use std::{
    sync::{
        Condvar, LazyLock, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
};

use indicatrix::renderer::gpu_backend::{GpuAccumulate, GpuBackend, GpuSceneRef};

pub(crate) mod hybrid;

/// Resolves the per-facet finish `scene.girdle_frosted` implies.
///
/// `indicatrix_net::SceneState` carries no `Vec<FacetFinish>` of its own -- the
/// wire-format encoding is the single `girdle_frosted` bool, and the worker re-derives
/// the same `Vec<FacetFinish>` the viewer would via `indicatrix::geometry::girdle_facet_finishes`,
/// a pure, deterministic function of `scene.planes` alone. `false` returns an empty
/// slice, equivalent to every facet using `FacetFinish::default() == Polished`.
#[must_use]
fn resolve_facet_finishes(scene: &SceneState) -> Vec<FacetFinish> {
    if scene.girdle_frosted {
        indicatrix::geometry::girdle_facet_finishes(&scene.planes)
    } else {
        Vec::new()
    }
}

/// Fixed FOV matching the viewer's own hard-coded value. `SceneState` carries no FOV
/// field of its own (see that struct's doc comment on what it deliberately does and
/// doesn't carry), so this must match `apps/indicatrix-cut/src/bridge/render_thread.rs`
/// and `export_thread.rs`'s `42.0` exactly for a remote worker's camera rays to line up
/// with the viewer's.
const VIEWER_FOV_DEG: f32 = 42.0;

/// Resolves a `--threads`-style argument (`0` meaning "let the OS decide") to an actual
/// thread count.
///
/// Shared by [`trace_samples`]'s own chunking and by `serve`'s `Welcome` message (which
/// reports the thread count it actually renders with, not the literal `0` sentinel).
#[must_use]
pub fn effective_thread_count(threads: usize) -> usize {
    if threads == 0 {
        thread::available_parallelism().map_or(8, std::num::NonZero::get)
    } else {
        threads
    }
}

/// A process-wide counting semaphore over CPU tracer threads -- see this module's own
/// doc comment ("A process-wide permit pool...") for why this exists.
struct ThreadPermits {
    /// Total capacity, fixed at construction (`effective_thread_count(0)`) -- never
    /// mutated afterward; only [`Self::available`] moves.
    capacity: usize,
    /// How many permits are currently unclaimed. Guarded by the same `Mutex` the
    /// [`Condvar`] waits against, per the standard condvar pattern.
    available: Mutex<usize>,
    /// Notified every time [`Self::release`] returns permits, so a blocked
    /// [`Self::acquire`] wakes up to re-check rather than polling.
    freed: Condvar,
}

impl ThreadPermits {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            available: Mutex::new(capacity.max(1)),
            freed: Condvar::new(),
        }
    }

    /// Blocks until `wanted` permits are free, then claims them, returning how many
    /// were actually acquired (== `wanted.clamp(1, self.capacity)`).
    ///
    /// `wanted` is clamped to `self.capacity` first: a single request asking for more
    /// threads than exist on this machine (or than this pool was ever sized for) must
    /// still be satisfiable -- waiting for MORE permits than the pool's total capacity
    /// would otherwise block forever, since that many can never be simultaneously free.
    /// Clamped to at least 1 too, so a caller can't accidentally acquire zero permits
    /// and skip the accounting entirely.
    fn acquire(&self, wanted: usize) -> usize {
        let wanted = wanted.clamp(1, self.capacity);
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *available < wanted {
            available = self
                .freed
                .wait(available)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        *available -= wanted;
        wanted
    }

    /// Returns `held` permits (as returned by a matching [`Self::acquire`] call) to the
    /// pool and wakes every waiter to re-check -- `notify_all`, not `notify_one`: more
    /// than one blocked request can each now have enough newly-freed permits to
    /// proceed, and `notify_one` could wake the wrong one first and starve the others
    /// for another full cycle.
    fn release(&self, held: usize) {
        let mut available = self
            .available
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *available += held;
        drop(available);
        self.freed.notify_all();
    }
}

/// The process-wide pool itself, sized once at first use to `effective_thread_count(0)`
/// -- this machine's `available_parallelism`, matching what a single solo request would
/// ask for. Every [`trace_into`] call acquires from and releases back to this same
/// instance regardless of which connection or request it's tracing for.
static THREAD_PERMITS: LazyLock<ThreadPermits> =
    LazyLock::new(|| ThreadPermits::new(effective_thread_count(0)));

/// RAII guard releasing `n` permits back to [`THREAD_PERMITS`] on drop -- including on
/// an unwind, so a tracer thread panicking mid-`thread::scope` (already re-raised by
/// `hybrid::dispatch_concurrently`'s own `resume_unwind`, or a panic surfacing through
/// `run_tracer`'s `catch_unwind`) never leaks permits the pool would otherwise never see
/// again, slowly starving every future request on this worker.
struct PermitGuard(usize);

impl Drop for PermitGuard {
    fn drop(&mut self) {
        THREAD_PERMITS.release(self.0);
    }
}

/// Traces samples `[first_sample, first_sample + samples)` across the whole frame.
///
/// Parallel across `threads` CPU threads (`0` for "all available cores" -- see
/// [`effective_thread_count`]), over `scene.width x scene.height`.
///
/// Returns the summed (not averaged) per-pixel XYZ radiance, one [`Vec3`] per pixel in
/// row-major order. Callers that want a displayable image divide by their own total
/// sample count when tone-mapping; callers accumulating contributions from multiple
/// workers add these sums directly into their own running total -- this function itself
/// never divides.
///
/// Returns an all-[`Vec3::ZERO`] buffer (still correctly sized) if `samples`, `width`,
/// or `height` is zero -- callers are expected to have rejected those via
/// [`crate::validate`] if invalid for their use case; this function has no opinion.
#[must_use]
pub fn trace_samples(
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
) -> Vec<Vec3> {
    let width = scene.width;
    let height = scene.height;
    let mut buffer = vec![Vec3::ZERO; width as usize * height as usize];
    if samples == 0 || width == 0 || height == 0 {
        return buffer;
    }

    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, VIEWER_FOV_DEG);
    let environment = scene
        .lighting_preset
        .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
        .with_backdrop(scene.backdrop);

    trace_into(
        scene,
        first_sample,
        samples,
        threads,
        &camera,
        environment,
        &mut buffer,
    );
    buffer
}

/// Traces samples `[first_sample, first_sample + samples)` across the whole frame,
/// preferring `gpu`.
///
/// Falls back to the CPU tracer (via [`trace_into`], the same code [`trace_samples`]
/// runs) whenever `gpu` declines -- a decline is normal, not an error.
///
/// A drop-in replacement for [`trace_samples`] at every call site: same return contract
/// (summed, never averaged), same disjoint-and-additive sample-range property both
/// `render_cmd` and `serve` depend on -- both backends add into a
/// `Vec3::ZERO`-initialized buffer with identical semantics, keeping a GPU worker's
/// sample ranges mergeable with a CPU viewer's.
///
/// A thin wrapper over [`trace_samples_with_gpu_cancellable`] with a cancellation flag
/// that is never set -- kept for `render_cmd` (a one-shot CLI render with no
/// cancellation concept) and tests that predate cooperative cancellation.
///
/// # Panics
///
/// Never panics in practice: the `.expect` below can only fire if
/// [`trace_samples_with_gpu_cancellable`] returned `None`, which means it observed
/// `cancel` set -- impossible here, since `never_cancel` is never set.
#[must_use]
pub fn trace_samples_with_gpu(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
) -> Vec<Vec3> {
    let never_cancel = AtomicBool::new(false);
    trace_samples_with_gpu_cancellable(gpu, scene, first_sample, samples, threads, &never_cancel)
        .expect("cancel is never set, so this dispatch can never observe GpuAccumulate::Cancelled")
}

/// Cancellable counterpart of [`trace_samples_with_gpu`], used by
/// `stream_emit::tracer::run_tracer`'s single-engine sub-batch dispatch.
///
/// Used whenever a job hasn't calibrated a hybrid split, so a GPU sub-batch (which can
/// run up to `MAX_SUBBATCH` samples, far more than `TARGET_SUBBATCH`'s wall-clock target
/// on a fast adapter) can be interrupted mid-dispatch rather than only between whole
/// sub-batches, tightening worst-case cancellation latency.
///
/// Returns `None` if `cancel` was observed set before the GPU dispatch finished --
/// `buffer` is guaranteed untouched in that case, so the caller has nothing to fold in.
/// A `Declined` GPU still falls back to the CPU tracer for the full `samples` exactly as
/// [`trace_samples_with_gpu`] does; `cancel` plays no further role once declined.
#[must_use]
pub fn trace_samples_with_gpu_cancellable(
    gpu: &GpuBackend,
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    cancel: &AtomicBool,
) -> Option<Vec<Vec3>> {
    let width = scene.width;
    let height = scene.height;
    let mut buffer = vec![Vec3::ZERO; width as usize * height as usize];
    if samples == 0 || width == 0 || height == 0 {
        return Some(buffer);
    }

    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, VIEWER_FOV_DEG);
    let environment = scene
        .lighting_preset
        .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
        .with_backdrop(scene.backdrop);

    // scene.girdle_frosted re-expanded into Vec<FacetFinish> (see
    // resolve_facet_finishes). The CPU fallback below resolves identical finishes from
    // the same field, so a GPU decline mid-trace can't silently switch polish state.
    let facet_finishes = resolve_facet_finishes(scene);
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width,
        height,
        planes: &scene.planes,
        facet_finishes: &facet_finishes,
        material: &scene.material,
        max_bounces: scene.max_bounces,
        environment,
    };
    match gpu.try_accumulate_cancellable(&gpu_scene, first_sample, samples, &mut buffer, cancel) {
        GpuAccumulate::Done => return Some(buffer),
        GpuAccumulate::Cancelled => return None,
        GpuAccumulate::Declined => {}
    }

    // The GPU declined (no adapter, ComputeMode::OnlyCpu, or an unsupported material).
    // `buffer` is guaranteed still all-zero -- a decline never partially writes -- so
    // falling back is exactly trace_samples' own zero-initialized start.
    trace_into(
        scene,
        first_sample,
        samples,
        threads,
        &camera,
        environment,
        &mut buffer,
    );
    Some(buffer)
}

/// The CPU tracing loop itself, adding into an already-sized `buffer` (`width x height`
/// entries, row-major). Shared by [`trace_samples`] and [`trace_samples_with_gpu`]'s CPU
/// fallback so both go through the identical seed formula and thread-chunking.
///
/// # Work distribution: a shared atomic row counter, not contiguous row bands
///
/// Rows through the stone cost far more than background rows, so this claims rows
/// dynamically through a shared `AtomicUsize` counter (`fetch_add` per row) rather than
/// splitting the frame into `num_threads` contiguous bands, the same load-balancing
/// approach `apps/indicatrix-cut`'s scanline/batch renderers use. `buffer` is pre-split
/// into per-row slices behind a `Mutex<Vec<Option<&mut [Vec3]>>>`; a thread claims row
/// `y`, locks just long enough to take that row's slice, then traces it without the
/// lock held. Every row is claimed by exactly one thread, so per-pixel sums stay
/// bit-identical regardless of how rows are distributed across threads.
fn trace_into(
    scene: &SceneState,
    first_sample: u32,
    samples: u32,
    threads: usize,
    camera: &Camera,
    environment: EnvironmentSource<'_>,
    buffer: &mut [Vec3],
) {
    let width = scene.width;
    let height = scene.height;
    let width_usize = width as usize;

    let requested_threads = effective_thread_count(threads).max(1);
    // Acquire the process-wide CPU-tracer permits before spawning anything -- see this
    // module's own doc comment ("A process-wide permit pool...") and `THREAD_PERMITS`.
    // `acquire` blocks until enough are free (clamping to the pool's total capacity if
    // `requested_threads` exceeds it) and returns exactly how many were granted; that's
    // the number of scoped threads actually spawned below, so held permits and live
    // threads always agree. `_permits` releases them (even on panic) once this
    // `trace_into` call's `thread::scope` returns.
    let num_threads = THREAD_PERMITS.acquire(requested_threads);
    let _permits = PermitGuard(num_threads);

    let planes = &scene.planes;
    let material = &scene.material;
    let max_bounces = scene.max_bounces;
    // Frosted girdle -- see `resolve_facet_finishes`'s doc comment.
    let facet_finishes = resolve_facet_finishes(scene);
    let facet_finishes = facet_finishes.as_slice();
    // Built once for this whole `trace_into` call (one sub-batch, potentially many
    // samples x many rows) rather than once per traced sample -- see
    // `trace_spectral_ray_with_finish_soa`'s doc comment: rebuilding this arena per
    // sample was a heap allocation on every single traced sample, measured at ~10-20% of
    // per-sample cost on a 57-plane cut. `planes` is the exact slice this is built from,
    // as that function requires; results stay bit-identical to the rebuild-every-call
    // `trace_spectral_ray_with_finish` this replaces.
    let plane_soa = build_plane_soa(planes);

    let rows: Vec<Option<&mut [Vec3]>> = buffer.chunks_mut(width_usize).map(Some).collect();
    let rows = Mutex::new(rows);
    let next_row = AtomicUsize::new(0);

    thread::scope(|s| {
        for _ in 0..num_threads {
            let rows = &rows;
            let next_row = &next_row;
            let plane_soa = &plane_soa;

            s.spawn(move || {
                loop {
                    let y = next_row.fetch_add(1, Ordering::Relaxed);
                    if y >= height as usize {
                        break;
                    }

                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("each row index is claimed by exactly one thread via fetch_add");

                    for (x, pixel) in row.iter_mut().enumerate() {
                        let global_pixel_idx = (y * width_usize + x) as u32;
                        let mut sample_sum = Vec3::ZERO;

                        // Per-pixel Cranley-Patterson rotations for the stratified
                        // pixel-jitter/hero-wavelength draws below -- pure functions of
                        // `global_pixel_idx`, hoisted out of the sample loop. jx/jy/
                        // hero_rand each use a different prime base (2, 3, 5) rather
                        // than one base rotated three ways -- measurably better
                        // variance for the highest-variance pixels this targets.
                        let rot = pixel_rotations(global_pixel_idx);

                        for s_idx in 0..samples {
                            let sample_num = first_sample.wrapping_add(s_idx);
                            // Stratified, a pure function of the absolute sample index
                            // alone (never batch-relative) -- what lets disjoint worker
                            // sample ranges compose correctly.
                            let draws = sample_draws(global_pixel_idx, sample_num, &rot);

                            let ray = camera.generate_ray(
                                x as f32,
                                y as f32,
                                width as f32,
                                height as f32,
                                draws.jitter_x,
                                draws.jitter_y,
                            );

                            let sample = trace_spectral_ray_with_finish_soa(
                                ray,
                                planes,
                                plane_soa,
                                facet_finishes,
                                material,
                                max_bounces,
                                environment,
                                draws.seed,
                                draws.hero_rand,
                                None,
                            );
                            // Defensive: no reachable NaN/Inf producer is known in this
                            // CPU path, but a single non-finite sample
                            // summed in would poison this pixel's accumulator
                            // permanently (NaN propagates through every future `+=`,
                            // including across later chunks/requests that reuse this
                            // buffer) -- cheap enough to guard unconditionally rather
                            // than trust that no future change ever introduces one.
                            if sample.is_finite() {
                                sample_sum += sample;
                            }
                        }

                        *pixel += sample_sum;
                    }
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };

    fn tiny_scene() -> SceneState {
        SceneState {
            width: 8,
            height: 8,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
        }
    }

    #[test]
    #[ignore = "manual probe: reports whether this machine has a usable wgpu adapter"]
    fn probe_gpu_adapter() {
        let backend = GpuBackend::acquire();
        match backend.adapter_label() {
            Some(label) => eprintln!("GPU ADAPTER FOUND: {label}"),
            None => eprintln!("NO GPU ADAPTER (declined)"),
        }
    }

    #[test]
    fn returns_a_buffer_sized_to_the_scene() {
        let scene = tiny_scene();
        let buf = trace_samples(&scene, 0, 4, 1);
        assert_eq!(buf.len(), 64);
    }

    #[test]
    fn zero_samples_returns_an_all_zero_buffer_of_the_right_size() {
        let scene = tiny_scene();
        let buf = trace_samples(&scene, 0, 0, 1);
        assert_eq!(buf.len(), 64);
        assert!(buf.iter().all(|v| *v == Vec3::ZERO));
    }

    #[test]
    fn single_and_multi_threaded_traces_agree() {
        let scene = tiny_scene();
        let single = trace_samples(&scene, 0, 4, 1);
        let multi = trace_samples(&scene, 0, 4, 4);
        for (a, b) in single.iter().zip(multi.iter()) {
            // Thread count only changes chunking, never which (pixel, sample) pairs
            // are traced -- must be bit-exact.
            assert_eq!(a, b);
        }
    }

    #[test]
    fn splitting_a_sample_range_across_two_calls_sums_to_the_same_result_as_one_call() {
        // The additivity property this crate exists to preserve. Relative tolerance,
        // not bit-exact: float addition isn't associative, so a different grouping can
        // differ in the last bit or two of an f32 even when correct.
        let scene = tiny_scene();
        let whole = trace_samples(&scene, 0, 8, 2);
        let first_half = trace_samples(&scene, 0, 4, 2);
        let second_half = trace_samples(&scene, 4, 4, 2);

        for i in 0..whole.len() {
            let split_sum = first_half[i] + second_half[i];
            let diff = (whole[i] - split_sum).abs();
            let scale = whole[i].abs().max(split_sum.abs()).max(Vec3::splat(1e-6));
            let rel = diff / scale;
            assert!(
                rel.max_element() < 1e-3,
                "pixel {i}: whole={:?} split_sum={:?} rel={:?}",
                whole[i],
                split_sum,
                rel
            );
        }
    }
}
