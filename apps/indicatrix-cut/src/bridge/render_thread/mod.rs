//! The live-viewport render thread: `RenderContext` (the shared, live-mutated render
//! configuration the GUI writes into), the progressive-accumulation loop that reads a
//! per-frame snapshot from it, and everything that loop dispatches to -- the CPU
//! scanline tracer, the GPU backend wrapper, and the gemological metrics cache.
//!
//! Split into submodules: [`context`] (the `RenderContext`/`FrameInputs` state, plus
//! material resolution), [`metrics`] (gemological-metrics cache), [`scanline`] (CPU
//! scanline tracer), [`gpu_backend`] (GPU backend wrapper and frame dispatch),
//! [`denoise`] (denoise + tone-map, pure functions), [`display_thread`] (the dedicated
//! thread that calls them, off the trace loop -- see its own doc comment for the
//! pipelining design), [`frame_helpers`] (small, pure, unit-tested per-frame decisions
//! `spawn_render_thread`'s loop calls into), and [`redraw_gate`] (coalesces a burst of
//! redraw-worthy events into at most one pending Slint UI-thread closure; also used by
//! `gui::remote::orchestrator`). This file keeps only the thread loop itself
//! (`spawn_render_thread`), deliberately not split further -- see its own
//! `#[expect(clippy::too_many_lines)]`.
//!
//! # Local+Remote combined live rendering
//!
//! Once the camera settles, `LiveComputeTarget::Both` (the default whenever a worker is
//! configured) lets this thread's local tracing keep running alongside a dispatched
//! remote render, both contributing to the same displayed image, rather than local
//! being suspended for the image's whole lifetime the way `LiveComputeTarget::RemoteOnly`
//! still works.
//!
//! **The two engines never write one buffer.** `accum_buffer` stays a plain `Vec<Vec3>`
//! owned outright by this thread, touched under no lock. `RenderContext::remote_accumulator`
//! is the same `Accumulator` the remote render's socket thread sums `FRAME` deltas
//! into; this thread only reads its `buffer()`/`samples_done()`, and only at the
//! display cadence ([`DENOISE_MIN_INTERVAL`], not per traced sample): a short lock, a
//! full-buffer read, an elementwise add into a scratch buffer for that cycle alone.
//! Neither engine's buffer is ever discarded or overwritten by the other.
//!
//! **Disjoint sample ranges.** A live remote render is dispatched as one request
//! covering exactly `[0, remote_render_samples)`, fixed at dispatch time.
//! `RenderContext::remote_reserved_samples` records that reserved size (`0` when not
//! combining), and every frame this loop shifts its own absolute sample index (the
//! jitter/RNG seed) past it, so local's indices always start where remote's range ends.
//!
//! **The old suspend-while-remote mechanism.** `remote_active`/`resolve_remote_ownership`
//! keep an unchanged pure contract, but what a resolved `remote_active == true` does now
//! depends on `live_compute_target`: for `RemoteOnly` it still suspends tracing outright
//! (local racing back in over a finished remote image is still a live risk there); for
//! `Both` that's structurally impossible (local only ever adds into a display-time
//! sum), so `remote_active` instead gates [`should_combine_remote`].
//! `resolve_remote_ownership`'s release condition is the one mechanism behind both
//! effects.

mod context;
mod denoise;
mod display_thread;
mod frame_helpers;
mod gpu_backend;
mod local_preview;
mod metrics;
mod redraw_gate;
mod scanline;

pub use context::{
    MaterialOverrides, MaterialSources, PlanesOwner, RenderContext, apply_material_overrides,
    load_env_map, resolve_material, resolve_material_with_override,
};
pub use denoise::{
    DenoiseScratch, FirstHitSnapshot, denoise_and_tonemap_frame, tonemap_running_average,
};
pub use metrics::hash_planes;
pub use redraw_gate::RedrawGate;

use crate::bridge::frame_cache::{girdle_finish::GirdleFinishCache, stone_width::StoneWidthCache};
use context::{FrameInputs, resolve_material_and_quality, snapshot_frame_inputs};
use display_thread::{FrameMetricsSnapshot, spawn_display_thread};
use glam::Vec3;
use gpu_backend::{
    BackendFrame, FrameOutputs, HybridPacing, ViewportGpu, accumulate_frame_samples,
};
use indicatrix::optics::{
    materials::GemMaterial,
    raytracer::{Camera, EnvironmentSource, FacetFinish},
};
use metrics::{MetricsCache, compute_or_reuse_metrics};
use slint::Weak;
use std::{
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

/// Minimum real-time gap between two display cycles (snapshot + hand-off to
/// [`display_thread`]) being SENT while samples are still accumulating. See the loop
/// body's `denoise_due` computation for the exceptions that always send regardless
/// (first frame, `dirty`, a dimension change, denoising disabled, reaching
/// `target_samples`). The À-Trous denoiser dominates every live frame (267ms/1072ms
/// per call at 800x600/1920x1080), so re-running it every ~16ms iteration while still
/// converging capped the viewport at a few FPS for no visible benefit. 120ms is fast
/// enough to feel live while converging and cuts denoiser invocations ~7-8x.
///
/// The actual denoise+tonemap+push work runs on [`display_thread`], so the render
/// thread never blocks on a cycle's cost -- there is nothing to scale a duty-cycle gap
/// against, unlike when that cost was paid synchronously on this thread.
const DENOISE_MIN_INTERVAL: Duration = Duration::from_millis(120);

/// Timeout for the convergence wait at [`display.busy()`]. À-Trous cycles run in
/// the 100ms-1s range, so 5s is generous for a healthy display thread.
const DISPLAY_BUSY_WAIT_TIMEOUT: Duration = Duration::from_secs(5);

use frame_helpers::{
    AccumulationBuffers, FrameActivityFlags, SuspensionFlags, TraceActivitySink,
    combined_sample_offset, push_metrics_to_ui, remote_suspends_local, resolve_remote_ownership,
    should_combine_remote, update_accumulation_state,
};

#[expect(
    clippy::too_many_lines,
    reason = "the render worker thread's full loop (resize/dirty handling, sampling, \
              progressive accumulation, UI callbacks); splitting it apart risks changing \
              GUI render-thread behaviour verifiable only by launching the app"
)]
pub fn spawn_render_thread<T, F, M, S>(
    ui_weak: Weak<T>,
    ctx: Arc<Mutex<RenderContext>>,
    update_image: F,
    update_metrics: M,
    update_gpu_status: S,
) where
    // `TraceActivitySink` lets this function stay generic over `T`.
    T: TraceActivitySink + 'static,
    F: Fn(&T, slint::SharedPixelBuffer<slint::Rgba8Pixel>) + Send + 'static + Clone,
    M: Fn(&T, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
    // Callback to push GPU status messages to the UI. `slint::SharedString` must
    // outlive the `upgrade_in_event_loop` closure.
    S: Fn(&T, slint::SharedString) + Send + 'static + Clone,
{
    thread::spawn(move || {
        let mut last_width = 0;
        let mut last_height = 0;

        let materials = GemMaterial::all_materials();
        let mut accum_buffer: Vec<Vec3> = Vec::new();
        // First-hit guide buffers alongside the accumulation buffer -- see
        // `render_frame_scanlines` for how they're populated. Kept alive across frames
        // so steady-state rendering does no extra per-frame heap allocation.
        let mut first_hit_depth: Vec<f32> = Vec::new();
        let mut first_hit_normal: Vec<Vec3> = Vec::new();
        let mut first_hit_facet_id: Vec<i32> = Vec::new();
        // Scratch buffer for folding a remote accumulator's current total into a
        // display cycle's input (see `should_combine_remote`'s call site below).
        // Reused across cycles like the buffers above; stays empty when nothing combines.
        let mut combined_scratch: Vec<Vec3> = Vec::new();
        // The combined remote sample count last sent to `display`, used for the
        // "local has converged" check in the loop.
        let mut last_displayed_remote_samples: u32 = 0;
        let mut accum_samples: u32 = 0;
        // Cadence gate for handing a display cycle to `display` -- see
        // `DENOISE_MIN_INTERVAL`. `None` forces an immediate send (first frame, or any
        // `dirty`/dimension-change reset).
        let mut last_denoise_at: Option<Instant> = None;
        let mut metrics_cache: Option<MetricsCache> = None;
        // Acquired once, off the frame loop: adapter acquisition and shader compilation
        // are worth doing exactly once. Declines on a machine with no usable GPU,
        // which is not an error -- see `ViewportGpu`.
        let mut gpu_backend = ViewportGpu::acquire();
        // Last GPU status message pushed to UI, cached to avoid redundant writes.
        let mut last_gpu_status: Option<String> = None;
        let mut hybrid_pacing = HybridPacing::new();
        // Recomputed only when the active design's geometry actually changes.
        let mut girdle_cache = GirdleFinishCache::new();
        let mut stone_width_cache = StoneWidthCache::new();
        // Denoise+tonemap+push runs on a dedicated thread. This loop keeps copies
        // of `ui_weak`/`update_metrics` for the metrics-only push on the suspended
        // path, which bypasses `display` so invisible tabs don't pay for
        // denoise+tonemap cycles.
        let ui_weak_metrics_only = ui_weak.clone();
        let update_metrics_metrics_only = update_metrics.clone();
        let display = spawn_display_thread(ui_weak, update_image, update_metrics);
        // Single choke point for releasing stale remote-image ownership -- see
        // `resolve_remote_ownership`. `false` matches `RenderContext::remote_active`'s
        // `Default`, so the first iteration never misfires as a release.
        let mut prev_remote_active = false;

        loop {
            let FrameInputs {
                width,
                height,
                yaw,
                pitch,
                distance,
                light_yaw,
                light_pitch,
                material_name,
                material_override,
                material_unresolved,
                lighting_preset,
                backdrop,
                target_samples,
                max_bounces,
                exposure,
                inclusion_sigma_s,
                c_axis_override,
                girdle_frosted,
                edge_rounding_radius,
                stone_width_mm,
                active_planes,
                custom_materials,
                running,
                dirty,
                paused,
                tab_visible,
                denoise_enabled,
                remote_active: remote_active_snapshot,
                remote_accumulator,
                remote_reserved_samples,
                export_active,
                live_compute_target,
                local_compute_target,
                local_preview_scale,
                camera_moving,
                env_map,
            } = snapshot_frame_inputs(&ctx);

            if !running {
                break;
            }

            // Release stale remote-image ownership the instant a fresh `dirty` arrives
            // for a reason other than the handoff's own start -- see
            // `resolve_remote_ownership`. Shadows `remote_active` with the resolved
            // value for the rest of this frame so a release also resumes tracing THIS
            // iteration, not after a 100ms suspended sleep.
            let remote_active =
                resolve_remote_ownership(dirty, remote_active_snapshot, prev_remote_active);
            if remote_active != remote_active_snapshot {
                // Write back so other readers (`orchestrator::poll_tick`'s served_by
                // reconciliation) observe the release promptly.
                ctx.lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .remote_active = false;
            }
            prev_remote_active = remote_active;

            // Read the remote accumulator's current sample count once per iteration (a
            // cheap lock + `u32` read, never the full buffer -- that happens at the
            // display cadence below). Used to decide whether this iteration can skip
            // tracing (combined total may already reach `target_samples`) and as part
            // of the combined image's true sample count.
            let remote_samples_done_now: u32 =
                if should_combine_remote(remote_active, live_compute_target) {
                    remote_accumulator.as_ref().map_or(0, |acc| {
                        acc.lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .samples_done()
                    })
                } else {
                    0
                };

            if width == 0 || height == 0 {
                thread::sleep(std::time::Duration::from_millis(16));
                continue;
            }

            // Local preview-then-settle rendering: shadows `width`/`height` with the
            // (possibly reduced) dimensions to trace THIS frame; `ctx.width`/`ctx.height`
            // (the CONFIGURED resolution) are never mutated by this. `&& !remote_active`
            // is belt-and-suspenders -- see `RenderContext::camera_moving`'s doc comment.
            let (width, height) = local_preview::effective_dimensions(
                width,
                height,
                local_preview_scale,
                camera_moving && !remote_active,
            );

            // Captured before `update_accumulation_state` overwrites `last_width`/
            // `last_height` -- invalidates the denoise cadence cache immediately on any
            // reset rather than leaving a stale frame on screen for up to
            // `DENOISE_MIN_INTERVAL` longer. `dimensions_changed` is the narrower of
            // the two: `update_accumulation_state` only reallocates the first-hit guide
            // buffers on a dimension change (see its own doc comment), never on a plain
            // `dirty`, which only clears `accum`.
            let dimensions_changed = width != last_width || height != last_height;
            let accumulation_reset = dirty || dimensions_changed;

            update_accumulation_state(
                width,
                height,
                dirty,
                &mut AccumulationBuffers {
                    accum: &mut accum_buffer,
                    first_hit_depth: &mut first_hit_depth,
                    first_hit_normal: &mut first_hit_normal,
                    first_hit_facet_id: &mut first_hit_facet_id,
                },
                &mut accum_samples,
                &mut last_width,
                &mut last_height,
            );
            if dimensions_changed {
                // The guide buffers were just reallocated to fresh,
                // unrelated-to-any-key contents -- the GPU's cached `applied_guide_key`
                // must not be trusted to still describe what they hold.
                gpu_backend.invalidate_guide_cache();
            }

            if accumulation_reset {
                last_denoise_at = None;
                // Invalidates any display cycle already in flight/queued from before
                // this reset -- see `DisplayHandle::bump_generation`.
                display.bump_generation();
            }

            // Suspended by an explicit user pause, an invisible 3D tab, a remote worker
            // owning the displayed image (`remote_active`), or a running high-res export
            // (`export_active`). Skips raytracing entirely -- the accumulation buffer
            // stays in sync with `dirty`, so resuming continues converging. The sleep is
            // long enough to cost no CPU but short enough (~100ms) to feel responsive.
            let suspension = SuspensionFlags {
                paused,
                tab_visible,
                remote_suspends: remote_suspends_local(remote_active, live_compute_target),
                export_active,
                material_unresolved: material_unresolved.is_some(),
            };
            if suspension.tracing_suspended() {
                // An invisible tab alone (the Edit tab's default
                // Solid view mode) must not also freeze the gemological HUD/tilt-dialog
                // metrics while the cutter keeps editing -- only a hard suspend does
                // (see `SuspensionFlags::metrics_suspended`). `compute_or_reuse_metrics`
                // is a cache hit whenever planes/material/pose haven't moved since the
                // last frame that computed them, so this costs nothing extra in the
                // common case. Pushed directly via `push_metrics_to_ui`, bypassing
                // `display` entirely, so nothing pays for a denoise+tonemap cycle for an
                // image nobody can see.
                if !suspension.metrics_suspended() {
                    let (current_mat, _spp) = resolve_material_and_quality(
                        &MaterialSources {
                            materials: &materials,
                            custom_materials: &custom_materials,
                            material_override: material_override.as_ref(),
                            material_name: &material_name,
                        },
                        target_samples,
                        &MaterialOverrides {
                            inclusion_sigma_s,
                            c_axis_override,
                            edge_rounding_radius,
                            stone_width_mm,
                        },
                        &active_planes,
                        &mut stone_width_cache,
                    );
                    let (metrics, graph_brilliance, graph_extinction, graph_windowing) =
                        compute_or_reuse_metrics(
                            &mut metrics_cache,
                            &active_planes,
                            &current_mat,
                            yaw,
                            pitch,
                            light_yaw,
                            light_pitch,
                        );
                    push_metrics_to_ui(
                        &ui_weak_metrics_only,
                        &update_metrics_metrics_only,
                        FrameMetricsSnapshot {
                            metrics,
                            graph_brilliance,
                            graph_extinction,
                            graph_windowing,
                            cam_pitch_deg: pitch.to_degrees(),
                        },
                    );
                }
                thread::sleep(std::time::Duration::from_millis(100));
                continue;
            }

            let (current_mat, spp) = resolve_material_and_quality(
                &MaterialSources {
                    materials: &materials,
                    custom_materials: &custom_materials,
                    material_override: material_override.as_ref(),
                    material_name: &material_name,
                },
                target_samples,
                &MaterialOverrides {
                    inclusion_sigma_s,
                    c_axis_override,
                    edge_rounding_radius,
                    stone_width_mm,
                },
                &active_planes,
                &mut stone_width_cache,
            );

            // Enough samples accumulated in still mode: sleep to conserve power.
            // Combined against remote's contribution too (`0` whenever not combining) --
            // once local plus remote reach the target, further local tracing is wasted.
            //
            // In `LiveComputeTarget::Both`, a converged local half must not
            // stall the display forever at whatever combined count it last actually
            // sent. `remote_samples_done_now` rising past `last_displayed_remote_samples`
            // also catches `RemoteUpdate::Done`, with no separate check needed: `Done`
            // never clears `remote_active` (`resolve_remote_ownership`'s doc comment),
            // so `should_combine_remote` keeps reading true, and the accumulator's own
            // FINAL `samples_done()` is itself a rise past whatever was last shown. So
            // whenever remote has moved on, fall through to send ONE more display cycle
            // below with no new local tracing, instead of sleeping on a stale image;
            // sleep only when local is converged AND remote has nothing new to show.
            let remote_advanced = should_combine_remote(remote_active, live_compute_target)
                && remote_samples_done_now > last_displayed_remote_samples;
            let local_converged = accum_samples + remote_samples_done_now >= target_samples;
            if local_converged && !dirty && !remote_advanced {
                thread::sleep(std::time::Duration::from_millis(60));
                continue;
            }
            // Nothing left for LOCAL tracing to usefully add -- this iteration only
            // exists to refresh the display with remote's latest contribution.
            let skip_local_tracing = local_converged && !dirty;

            if !skip_local_tracing {
                accum_samples += spp;
            }

            // Optical metrics: analytical raytracing from the camera PoV accounting for
            // light direction. Expensive (single-threaded); result depends only on
            // (active_planes, current_mat, yaw, pitch, light_yaw, light_pitch), which
            // don't change between accumulation samples, so recompute only when they do.
            let (metrics, graph_brilliance, graph_extinction, graph_windowing) =
                compute_or_reuse_metrics(
                    &mut metrics_cache,
                    &active_planes,
                    &current_mat,
                    yaw,
                    pitch,
                    light_yaw,
                    light_pitch,
                );
            let cam_pitch_deg = pitch.to_degrees();

            let camera = Camera::new(yaw, pitch, distance, 42.0);

            let current_sample_count = accum_samples;

            // A loaded HDR panorama replaces the analytic studio rig as this frame's
            // environment source. The GPU megakernel has its own `env_mode` for
            // `HdrMap` and renders it directly; `accumulate_frame_samples` still falls
            // through to the CPU path (`render_frame_scanlines`) on a decline, but that
            // is the same generic per-frame fallback every other scene gets (no
            // adapter, device lost, `gpu` feature off), not an HDR-specific one -- see
            // `docs/history/indicatrix-cut.md` for the CPU-only HDR path.
            let environment = env_map.as_deref().map_or_else(
                || {
                    lighting_preset
                        .studio(exposure, light_yaw, light_pitch)
                        .with_backdrop(backdrop.level())
                },
                EnvironmentSource::HdrMap,
            );

            // Frosted girdle: `&[]` at the off position means all-polished.
            let facet_finishes: &[FacetFinish] = if girdle_frosted {
                girdle_cache.ensure(&active_planes)
            } else {
                &[]
            };

            // Skipped when this iteration exists only to refresh the
            // display with remote's latest contribution -- see `skip_local_tracing`'s
            // own comment above. `accum_samples` was left un-incremented for exactly
            // this case, so `accum_buffer`'s sample count stays accurate.
            if !skip_local_tracing {
                accumulate_frame_samples(
                    &mut gpu_backend,
                    &BackendFrame {
                        width,
                        height,
                        yaw,
                        pitch,
                        distance,
                        camera: &camera,
                        planes: &active_planes,
                        facet_finishes,
                        material: &current_mat,
                        max_bounces,
                        environment,
                        spp,
                        // Shifted past whatever `remote_reserved_samples` reserves (`0`
                        // whenever not combining) -- the disjointness guarantee: local's
                        // sample index (the jitter/RNG seed) never falls inside remote's
                        // assigned range. See this module's doc comment.
                        sample_offset: combined_sample_offset(
                            remote_reserved_samples,
                            current_sample_count - spp,
                        ),
                    },
                    &mut FrameOutputs {
                        accum: &mut accum_buffer,
                        depth: &mut first_hit_depth,
                        normal: &mut first_hit_normal,
                        facet_id: &mut first_hit_facet_id,
                    },
                    &mut hybrid_pacing,
                    local_compute_target,
                );
            }

            // Push `ViewportGpu`'s current self-healing
            // status to the UI whenever it actually changed -- see
            // `ViewportGpu::status_message`'s own doc comment for when this is
            // `Some` (transiently re-acquiring, or permanently on CPU fallback) vs
            // `None` (healthy, or nothing traced this iteration to change it).
            // Compared as `&str` so an unchanged `Some("...")` costs one comparison,
            // not a clone, on every iteration.
            if gpu_backend.status_message() != last_gpu_status.as_deref() {
                last_gpu_status = gpu_backend.status_message().map(str::to_string);
                let text: slint::SharedString = last_gpu_status.as_deref().unwrap_or("").into();
                let update_gpu_status = update_gpu_status.clone();
                let _ = ui_weak_metrics_only.upgrade_in_event_loop(move |ui| {
                    update_gpu_status(&ui, text);
                });
            }

            // Denoise on the READBACK path only -- `accum_buffer` stays the raw running
            // sum (never overwritten), so the progressive estimator stays unbiased;
            // only the displayed tone-mapped image is filtered.
            //
            // `denoise_enabled` gates whether that filtering happens at all -- off is a
            // straight tone-map of the raw average, cheap enough to run every frame.
            //
            // While accumulating, the display cycle is rate-limited to at most once per
            // `DENOISE_MIN_INTERVAL`: tracing is unaffected, but the UI keeps showing
            // the last pushed frame in between (nothing resent, to avoid flickering
            // between filtered and unfiltered). `converged_now` (first iteration
            // reaching `target_samples`) and `accumulation_reset` always attempt a
            // cycle regardless of the gap, since each is the last send before settling
            // or the first after invalidation.
            let converged_now = accum_samples + remote_samples_done_now >= target_samples;
            let denoise_due = !denoise_enabled
                || accumulation_reset
                || converged_now
                || last_denoise_at.is_none_or(|t| t.elapsed() >= DENOISE_MIN_INTERVAL);

            if denoise_due {
                // The render thread never blocks on a display cycle's cost -- it hands
                // the frame off to `display` and keeps tracing. At most one cycle may
                // be in flight, so a cycle is skipped (not queued) while the previous
                // one runs; `denoise_due` retries next iteration. Exception:
                // `converged_now` must always display exactly once, so that send waits
                // out any in-flight cycle instead of skipping.
                let can_send = if converged_now {
                    // Bounded, not an unconditional spin -- a display
                    // thread that has stopped finishing cycles (e.g. it died mid-panic)
                    // must not hang this loop forever; give up on THIS cycle instead.
                    let wait_start = Instant::now();
                    let mut timed_out = false;
                    while display.busy() {
                        if wait_start.elapsed() >= DISPLAY_BUSY_WAIT_TIMEOUT {
                            timed_out = true;
                            break;
                        }
                        thread::sleep(display_thread::CONVERGENCE_WAIT_POLL);
                    }
                    if timed_out {
                        tracing::warn!(
                            timeout_secs = DISPLAY_BUSY_WAIT_TIMEOUT.as_secs(),
                            "display thread still busy after the convergence wait \
                             timeout; skipping this display cycle instead of blocking \
                             the render thread forever"
                        );
                    }
                    !timed_out
                } else {
                    !display.busy()
                };

                if can_send {
                    // Fold the remote accumulator's current running total into a
                    // scratch buffer for THIS display cycle only -- never merged into
                    // `accum_buffer` itself. This read-lock-and-add happens at the
                    // display cadence, not the hot per-sample trace path. Falls through
                    // to `&accum_buffer` with no extra copy when not combining.
                    let (display_accum, display_sample_count): (&[Vec3], u32) =
                        if should_combine_remote(remote_active, live_compute_target)
                            && let Some(remote_acc) = &remote_accumulator
                        {
                            let acc = remote_acc
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner);
                            let remote_done = acc.samples_done();
                            combined_scratch.clear();
                            combined_scratch.extend(
                                accum_buffer.iter().zip(acc.buffer()).map(|(a, b)| *a + *b),
                            );
                            drop(acc);
                            // Records what THIS cycle actually displayed,
                            // so the "local converged" check above knows remote has
                            // nothing new the next time it reads a bigger count.
                            last_displayed_remote_samples = remote_done;
                            (&combined_scratch, current_sample_count + remote_done)
                        } else {
                            (&accum_buffer, current_sample_count)
                        };

                    let mut work = display.reclaim();
                    work.fill(
                        display.current_generation(),
                        denoise_enabled,
                        FirstHitSnapshot {
                            width,
                            height,
                            current_sample_count: display_sample_count,
                            accum_buffer: display_accum,
                            first_hit_depth: &first_hit_depth,
                            first_hit_normal: &first_hit_normal,
                            first_hit_facet_id: &first_hit_facet_id,
                        },
                        FrameMetricsSnapshot {
                            metrics,
                            graph_brilliance,
                            graph_extinction,
                            graph_windowing,
                            cam_pitch_deg,
                        },
                        // `push_frame_to_ui`'s own activity start/finish reads
                        // these straight off this cycle's already-computed
                        // `camera_moving`/`converged_now` -- see
                        // `frame_helpers::FrameActivityFlags`'s own doc comment.
                        FrameActivityFlags {
                            camera_moving,
                            converged: converged_now,
                        },
                    );
                    display.send(work);
                    last_denoise_at = Some(Instant::now());
                }
            }

            // Target smooth interactive framerate ~30-60 FPS
            thread::sleep(std::time::Duration::from_millis(16));
        }
    });
}
