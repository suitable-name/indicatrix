//! Small, pure, unit-tested per-frame helpers [`spawn_render_thread`] calls into: the
//! accumulation-buffer reset ([`update_accumulation_state`]), the
//! suspension/ownership/combining decisions for Local+Remote combined live rendering
//! ([`SuspensionFlags`]/[`remote_suspends_local`]/[`should_combine_remote`]/
//! [`resolve_remote_ownership`]), and the UI hand-off
//! ([`push_frame_to_ui`]). See `mod.rs`'s doc comment for the full design.
//!
//! [`spawn_render_thread`]: super::spawn_render_thread

use super::{display_thread::FrameMetricsSnapshot, redraw_gate::RedrawGate};
use crate::{ActivityModel, settings::model::LiveComputeTarget};
use glam::Vec3;
use slint::{ComponentHandle, Weak};
use std::{cell::RefCell, sync::Arc, time::Duration};

thread_local! {
    /// The `crate::ActivityModel` id for the "full quality trace" activity
    /// currently showing in the status strip, if any. UI-thread-only: accessed
    /// only from inside [`push_frame_to_ui`]'s `upgrade_in_event_loop` closure.
    static TRACE_ACTIVITY_ID: RefCell<Option<i32>> = const { RefCell::new(None) };
}

/// Camera movement and convergence flags for activity tracking: whether the
/// current frame is a camera-orbit redraw (which should not start/finish a trace
/// activity) and whether it has converged (which finishes the activity). A named
/// struct rather than two positional bools prevents silent transposition errors.
#[derive(Debug, Clone, Copy)]
pub(super) struct FrameActivityFlags {
    /// Mirrors `RenderContext::camera_moving` at the moment this frame's cycle was
    /// handed to the display thread -- while true, this frame is a lower-quality
    /// camera-orbit redraw, not a step toward a converged full-quality trace, so
    /// [`push_frame_to_ui`] neither starts nor finishes an activity for it.
    pub(super) camera_moving: bool,
    /// Whether THIS frame is the one where progressive accumulation first reached
    /// `RenderContext::target_samples` (`mod.rs`'s own `converged_now`) -- the
    /// "finished when the frame arrives" moment.
    pub(super) converged: bool,
}

/// Lets [`push_frame_to_ui`] reach `crate::ActivityModel` while staying generic
/// over `T: ComponentHandle`. Avoids naming `crate::MainWindow` directly in this
/// module; Rust infers `T = MainWindow` from the sole call site in
/// `gui::mod::run_gui`.
pub trait TraceActivitySink: ComponentHandle {
    /// Registers an indeterminate, non-cancellable "Path tracing" activity and
    /// returns its id.
    fn start_trace_activity(&self) -> i32;
    /// Finishes the activity `id` names.
    fn finish_trace_activity(&self, id: i32);
}

impl TraceActivitySink for crate::MainWindow {
    fn start_trace_activity(&self) -> i32 {
        self.global::<ActivityModel>().invoke_start_external(
            "trace".into(),
            "Path tracing".into(),
            false,
        )
    }

    fn finish_trace_activity(&self, id: i32) {
        self.global::<ActivityModel>().invoke_finish_external(id);
    }
}

/// One finished display cycle's payload, carried by [`push_frame_to_ui`]'s
/// [`RedrawGate`] from the display thread to the Slint UI-thread closure that shows it
/// (via [`RedrawGate::take`]). Needs an owned payload here, unlike the remote path's
/// `()`, since this path has no already-live state a late-running closure could re-read.
pub(super) struct FramePayload {
    pub(super) image: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    pub(super) scene_generation: u64,
    pub(super) metrics_snapshot: FrameMetricsSnapshot,
    pub(super) activity_flags: FrameActivityFlags,
}

/// Pushes one finished frame's image and gemological metrics to the UI thread's event
/// loop, through `redraw_gate` -- see [`RedrawGate`] for why: `spawn_display_thread`'s
/// loop can finish converting and call this again before Slint runs the PREVIOUS call's
/// closure, so without this gate a burst of fast cycles could queue one closure per
/// cycle. `redraw_gate` collapses any such burst to at most one pending closure, which
/// always shows the LATEST image once it runs.
pub(super) fn push_frame_to_ui<T, F, M>(
    ui_weak: &Weak<T>,
    update_image: &F,
    update_metrics: &M,
    redraw_gate: &Arc<RedrawGate<FramePayload>>,
    payload: FramePayload,
) where
    T: TraceActivitySink + 'static,
    F: Fn(&T, slint::SharedPixelBuffer<slint::Rgba8Pixel>, u64) + Send + 'static + Clone,
    M: Fn(&T, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
{
    if redraw_gate.submit(payload).is_none() {
        // An enqueued closure is still pending -- it will pick up the payload
        // just submitted when it runs, making a second closure redundant.
        return;
    }
    let redraw_gate = Arc::clone(redraw_gate);
    let _ = ui_weak.upgrade_in_event_loop({
        let update_image = update_image.clone();
        let update_metrics = update_metrics.clone();
        move |ui| {
            let Some(FramePayload {
                image,
                scene_generation,
                metrics_snapshot,
                activity_flags,
            }) = redraw_gate.take()
            else {
                // Can't happen in practice -- enqueued only right after a `submit`
                // that returned `Some`. Guarded rather than `unwrap` on principle.
                return;
            };
            update_image(&ui, image, scene_generation);
            let metrics = metrics_snapshot.metrics;
            update_metrics(
                &ui,
                metrics.brilliance_pct,
                metrics.fire_index,
                metrics.scintillation_pct,
                metrics.windowing_pct,
                metrics.extinction_pct,
                metrics_snapshot.graph_brilliance,
                metrics_snapshot.graph_extinction,
                metrics_snapshot.graph_windowing,
                metrics_snapshot.cam_pitch_deg,
            );
            track_trace_activity(&ui, activity_flags);
        }
    });
}

/// The activity start/finish decision itself, split out purely so
/// [`push_frame_to_ui`]'s own closure stays readable. See [`FrameActivityFlags`]'s
/// own doc comment for `camera_moving`/`converged`'s meaning. Starts an
/// indeterminate, non-cancellable "Path tracing" activity on the first non-orbit,
/// non-converged frame after one finishes (or after startup), and finishes it the
/// moment a frame converges -- camera-orbit frames touch neither state, so
/// continuous orbiting never flickers the status strip.
fn track_trace_activity<T: TraceActivitySink>(ui: &T, flags: FrameActivityFlags) {
    if !flags.camera_moving {
        let already_running = TRACE_ACTIVITY_ID.with(|cell| cell.borrow().is_some());
        if !already_running && !flags.converged {
            let id = ui.start_trace_activity();
            TRACE_ACTIVITY_ID.with(|cell| *cell.borrow_mut() = Some(id));
        }
    }
    if flags.converged
        && let Some(id) = TRACE_ACTIVITY_ID.with(|cell| cell.borrow_mut().take())
    {
        ui.finish_trace_activity(id);
    }
}

/// Pushes just this iteration's gemological metrics to the UI thread, with no
/// image. Invisible 3D tabs (e.g., the Edit tab's Solid view) must continue
/// updating HUD and tilt-dialog metrics without paying for denoise/tonemap/
/// framebuffer-copy cycles. No `RedrawGate` here; the ~100ms cadence of the
/// suspended path is already far below burst-coalescing rates.
pub(super) fn push_metrics_to_ui<T, M>(
    ui_weak: &Weak<T>,
    update_metrics: &M,
    metrics_snapshot: FrameMetricsSnapshot,
) where
    T: ComponentHandle + 'static,
    M: Fn(&T, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
{
    let update_metrics = update_metrics.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        let metrics = metrics_snapshot.metrics;
        update_metrics(
            &ui,
            metrics.brilliance_pct,
            metrics.fire_index,
            metrics.scintillation_pct,
            metrics.windowing_pct,
            metrics.extinction_pct,
            metrics_snapshot.graph_brilliance,
            metrics_snapshot.graph_extinction,
            metrics_snapshot.graph_windowing,
            metrics_snapshot.cam_pitch_deg,
        );
    });
}

/// Resets the progressive-accumulation state (buffer, sample count, and the three
/// first-hit guide buffers) whenever the output dimensions change, and separately
/// whenever the frame is marked `dirty` (camera/material/etc. moved). The guide
/// buffers only need resizing (not re-zeroing) on `dirty`: `render_frame_scanlines`
/// unconditionally overwrites every pixel's guide values every call, so there's no
/// stale value to clear.
///
/// This is also what makes local preview-then-settle rendering work for free: its
/// caller feeds `width`/`height` shadowed to a (possibly reduced) EFFECTIVE resolution,
/// so every preview<->full transition is just another dimension-changed reset this
/// function already handles.
/// The render loop's own accumulation state, kept alive across frames (never
/// reallocated except on a reset) so steady-state rendering does no extra per-frame
/// heap allocation. Bundled here to keep [`update_accumulation_state`]'s parameter
/// list under clippy's argument-count lint.
pub(super) struct AccumulationBuffers<'a> {
    pub(super) accum: &'a mut Vec<Vec3>,
    pub(super) first_hit_depth: &'a mut Vec<f32>,
    pub(super) first_hit_normal: &'a mut Vec<Vec3>,
    pub(super) first_hit_facet_id: &'a mut Vec<i32>,
}

pub(super) fn update_accumulation_state(
    width: u32,
    height: u32,
    dirty: bool,
    buffers: &mut AccumulationBuffers<'_>,
    accum_samples: &mut u32,
    last_width: &mut u32,
    last_height: &mut u32,
) {
    // Dimension change or camera movement resets progressive accumulation
    if width != *last_width || height != *last_height {
        let pixel_count = (width * height) as usize;
        *buffers.accum = vec![Vec3::ZERO; pixel_count];
        *buffers.first_hit_depth = vec![1.0e6; pixel_count];
        *buffers.first_hit_normal = vec![Vec3::ZERO; pixel_count];
        *buffers.first_hit_facet_id = vec![-1; pixel_count];
        *accum_samples = 0;
        *last_width = width;
        *last_height = height;
    }

    if dirty {
        buffers.accum.fill(Vec3::ZERO);
        *accum_samples = 0;
    }
}

/// Whether the render loop should skip tracing this iteration: an explicit user pause,
/// the 3D tab not being visible, a remote worker SOLELY owning the displayed image
/// (`remote_suspends` -- see its own doc comment for why this isn't simply
/// `RenderContext::remote_active`), or a running high-resolution export.
///
/// A named struct rather than four positional `bool`s: transposing any two at a call
/// site would compile and silently suspend for the wrong reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "five independent suspension reasons, deliberately named fields instead of positional bools"
)]
pub(super) struct SuspensionFlags {
    pub(super) paused: bool,
    pub(super) tab_visible: bool,
    /// `RenderContext::remote_active`'s resolved value, already narrowed by the caller
    /// to only the case where it should suspend tracing -- see [`remote_suspends_local`].
    /// Named differently from the source field on purpose: this is "does that activity
    /// mean local must stay off", not "is remote active at all".
    pub(super) remote_suspends: bool,
    pub(super) export_active: bool,
    /// `RenderContext::material_unresolved.is_some()`. Unlike other flags, this
    /// is not about ownership; it reflects whether computed results would be valid.
    pub(super) material_unresolved: bool,
}

impl SuspensionFlags {
    pub(super) const fn tracing_suspended(self) -> bool {
        self.paused
            || !self.tab_visible
            || self.remote_suspends
            || self.export_active
            || self.material_unresolved
    }

    /// Whether gemological METRICS evaluation should be skipped, distinct from
    /// full path-tracing suspension. An invisible 3D tab must not freeze HUD
    /// metrics while the cutter edits. Only explicit pause, remote/export handoff
    /// blocks metrics, since those mean "nothing should compute now" versus just
    /// "nothing is on screen".
    pub(super) const fn metrics_suspended(self) -> bool {
        // `material_unresolved` blocks metrics (unlike just an invisible tab)
        // because there is no honest answer to compute when the material is
        // unresolved; all other flags mean "not right now".
        self.paused || self.remote_suspends || self.export_active || self.material_unresolved
    }
}

/// Whether a resolved `remote_active == true` should suspend LOCAL tracing this frame --
/// only ever true for [`LiveComputeTarget::RemoteOnly`], the one mode where local
/// contributing anything would be immediately-discarded work. For
/// [`LiveComputeTarget::Both`], `remote_active` instead gates combining (see
/// [`should_combine_remote`]) and local keeps tracing regardless; for
/// [`LiveComputeTarget::LocalOnly`], `remote_active` never becomes `true` (no remote
/// render is ever dispatched), so this is unreachable in practice but stays total
/// rather than `unreachable!()`. Pure and unit-tested.
#[must_use]
pub(super) const fn remote_suspends_local(
    remote_active: bool,
    live_compute_target: LiveComputeTarget,
) -> bool {
    remote_active && matches!(live_compute_target, LiveComputeTarget::RemoteOnly)
}

/// Whether the render loop should claim its per-frame ranges from `RenderContext::
/// live_epoch` and fold that epoch's remote contribution into the shown image -- true
/// only for [`LiveComputeTarget::Both`] while a resolved `remote_active` says a remote
/// epoch still owns part of the image (this stays `true` well past completion, so local's
/// continued tracing keeps adding to remote's finished contribution). Always `false`
/// for `RemoteOnly`/`LocalOnly` -- `RemoteOnly` never reaches this at all since
/// [`remote_suspends_local`] suspends the whole display step outright in that mode.
/// Pure and unit-tested.
#[must_use]
pub(super) const fn should_combine_remote(
    remote_active: bool,
    live_compute_target: LiveComputeTarget,
) -> bool {
    remote_active && matches!(live_compute_target, LiveComputeTarget::Both)
}

/// The single choke point that releases stale remote-image ownership -- see
/// `RenderContext::remote_active`'s doc comment for why it must stay set all the way
/// through a *completed* remote render, and why it can only be released here, not at
/// each of the ~25 individual `ctx.dirty = true` call sites scattered across `gui::*`.
///
/// `remote_active` can only ever transition `false -> true` one way: the handoff
/// orchestrator's `DiscardLocalPreview` action sets it `true` in the SAME locked
/// mutation as `dirty = true`, entering `Settling`, always from `remote_active ==
/// false`. So the FIRST frame this loop observes `remote_active == true` after a frame
/// where it was `false`, a fresh `dirty` alongside it is that legitimate hand-off
/// starting -- `was_remote_active_last_frame` is `false`, and this returns
/// `remote_active` unchanged (`true`).
///
/// Any OTHER frame where `dirty` is freshly `true` while `remote_active` was ALREADY
/// `true` one frame ago can only mean some other `gui::*` callback touched the scene
/// (this covers a completed remote render too, since `remote_active` is deliberately
/// left `true` after `RemoteUpdate::Done`). That releases ownership (`false`)
/// regardless of which callback caused it -- this function never enumerates call
/// sites, it only looks at `dirty`/`remote_active`, which every site already touches
/// or leaves alone. Pure and unit-tested.
pub(super) const fn resolve_remote_ownership(
    dirty: bool,
    remote_active: bool,
    was_remote_active_last_frame: bool,
) -> bool {
    if dirty && remote_active && was_remote_active_last_frame {
        false
    } else {
        remote_active
    }
}

/// How long the render loop should sleep at the end of an iteration that took `elapsed`,
/// given the target iteration length `frame`.
///
/// `paced` is whether this iteration handled a moving camera or had a display cycle due
/// (see `mod.rs`'s `FRAME_PACE`): only those iterations are held to the frame interval,
/// and then only by the remainder the work itself did not already use. An iteration that
/// has nothing to show, or that already ran past `frame`, does not sleep. Pure and
/// unit-tested.
#[must_use]
pub(super) fn pacing_sleep(paced: bool, elapsed: Duration, frame: Duration) -> Option<Duration> {
    if !paced {
        return None;
    }
    frame.checked_sub(elapsed).filter(|rest| !rest.is_zero())
}

#[cfg(test)]
#[path = "frame_helpers_tests.rs"]
mod tests;
