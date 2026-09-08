//! Small, pure, unit-tested per-frame helpers [`spawn_render_thread`] calls into: the
//! accumulation-buffer reset ([`update_accumulation_state`]), the
//! suspension/ownership/combining decisions for Local+Remote combined live rendering
//! ([`SuspensionFlags`]/[`remote_suspends_local`]/[`should_combine_remote`]/
//! [`resolve_remote_ownership`]/[`combined_sample_offset`]), and the UI hand-off
//! ([`push_frame_to_ui`]). See `mod.rs`'s doc comment for the full design.
//!
//! [`spawn_render_thread`]: super::spawn_render_thread

use super::{display_thread::FrameMetricsSnapshot, redraw_gate::RedrawGate};
use crate::settings::model::LiveComputeTarget;
use glam::Vec3;
use slint::{ComponentHandle, Weak};
use std::sync::Arc;

/// One finished display cycle's payload, carried by [`push_frame_to_ui`]'s
/// [`RedrawGate`] from the display thread to the Slint UI-thread closure that shows it
/// (via [`RedrawGate::take`]). Needs an owned payload here, unlike the remote path's
/// `()`, since this path has no already-live state a late-running closure could re-read.
pub(super) struct FramePayload {
    image: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    metrics_snapshot: FrameMetricsSnapshot,
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
    image: slint::SharedPixelBuffer<slint::Rgba8Pixel>,
    metrics_snapshot: FrameMetricsSnapshot,
) where
    T: ComponentHandle + 'static,
    F: Fn(&T, slint::SharedPixelBuffer<slint::Rgba8Pixel>) + Send + 'static + Clone,
    M: Fn(&T, f32, f32, f32, f32, f32, [f32; 19], [f32; 19], [f32; 19], f32)
        + Send
        + 'static
        + Clone,
{
    if redraw_gate
        .submit(FramePayload {
            image,
            metrics_snapshot,
        })
        .is_none()
    {
        // A previously enqueued closure is still pending -- it will pick up the
        // payload just submitted once it runs, so a second closure would be redundant.
        return;
    }
    let redraw_gate = Arc::clone(redraw_gate);
    let _ = ui_weak.upgrade_in_event_loop({
        let update_image = update_image.clone();
        let update_metrics = update_metrics.clone();
        move |ui| {
            let Some(FramePayload {
                image,
                metrics_snapshot,
            }) = redraw_gate.take()
            else {
                // Can't happen in practice -- enqueued only right after a `submit`
                // that returned `Some`. Guarded rather than `unwrap` on principle.
                return;
            };
            update_image(&ui, image);
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
        }
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
pub(super) struct SuspensionFlags {
    pub(super) paused: bool,
    pub(super) tab_visible: bool,
    /// `RenderContext::remote_active`'s resolved value, already narrowed by the caller
    /// to only the case where it should suspend tracing -- see [`remote_suspends_local`].
    /// Named differently from the source field on purpose: this is "does that activity
    /// mean local must stay off", not "is remote active at all".
    pub(super) remote_suspends: bool,
    pub(super) export_active: bool,
}

impl SuspensionFlags {
    pub(super) const fn tracing_suspended(self) -> bool {
        self.paused || !self.tab_visible || self.remote_suspends || self.export_active
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

/// Whether the render loop's display cycle should fold `RenderContext::
/// remote_accumulator`'s running total into the shown image -- true only for
/// [`LiveComputeTarget::Both`] while a resolved `remote_active` says a remote render
/// still owns part of the image (this stays `true` well past completion, so local's
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

/// The absolute sample index local tracing's `sample_offset` should use for a frame
/// that has already traced `local_pre_frame_count` samples this epoch -- see the
/// module doc comment's disjointness argument. `remote_reserved_samples` is `0`
/// whenever nothing is reserved, making this an identity on `local_pre_frame_count`,
/// same as before combining existed. Pure and unit-tested.
#[must_use]
pub(super) const fn combined_sample_offset(
    remote_reserved_samples: u32,
    local_pre_frame_count: u32,
) -> u32 {
    remote_reserved_samples + local_pre_frame_count
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

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tracing_suspended: the four independent suspend flags ----------------------

    #[test]
    fn tracing_runs_only_when_all_four_flags_allow_it() {
        assert!(
            !SuspensionFlags {
                paused: false,
                tab_visible: true,
                remote_suspends: false,
                export_active: false
            }
            .tracing_suspended()
        );
    }

    #[test]
    fn each_flag_alone_suspends_tracing() {
        assert!(
            SuspensionFlags {
                paused: true,
                tab_visible: true,
                remote_suspends: false,
                export_active: false
            }
            .tracing_suspended(),
            "paused"
        );
        assert!(
            SuspensionFlags {
                paused: false,
                tab_visible: false,
                remote_suspends: false,
                export_active: false
            }
            .tracing_suspended(),
            "!tab_visible"
        );
        assert!(
            SuspensionFlags {
                paused: false,
                tab_visible: true,
                remote_suspends: true,
                export_active: false
            }
            .tracing_suspended(),
            "remote_suspends"
        );
        assert!(
            SuspensionFlags {
                paused: false,
                tab_visible: true,
                remote_suspends: false,
                export_active: true
            }
            .tracing_suspended(),
            "export_active"
        );
    }

    #[test]
    fn every_flag_combination_still_suspends_if_any_one_is_set() {
        for paused in [false, true] {
            for tab_visible in [false, true] {
                for remote_suspends in [false, true] {
                    for export_active in [false, true] {
                        let expected = paused || !tab_visible || remote_suspends || export_active;
                        assert_eq!(
                            SuspensionFlags {
                                paused,
                                tab_visible,
                                remote_suspends,
                                export_active,
                            }
                            .tracing_suspended(),
                            expected,
                            "paused={paused} tab_visible={tab_visible} \
                             remote_suspends={remote_suspends} export_active={export_active}"
                        );
                    }
                }
            }
        }
    }

    // ---- remote_suspends_local / should_combine_remote: the mode-dependent split ----

    #[test]
    fn remote_only_is_the_sole_mode_where_remote_activity_suspends_local_tracing() {
        for mode in [
            LiveComputeTarget::LocalOnly,
            LiveComputeTarget::RemoteOnly,
            LiveComputeTarget::Both,
        ] {
            assert!(
                !remote_suspends_local(false, mode),
                "remote_active == false must never suspend, regardless of mode"
            );
        }
        assert!(
            !remote_suspends_local(true, LiveComputeTarget::LocalOnly),
            "LocalOnly never dispatches remote, but even if remote_active were \
             somehow true, LocalOnly must not suspend local tracing"
        );
        assert!(
            remote_suspends_local(true, LiveComputeTarget::RemoteOnly),
            "RemoteOnly must reproduce the pre-existing suspend-while-remote-active \
             behaviour exactly"
        );
        assert!(
            !remote_suspends_local(true, LiveComputeTarget::Both),
            "Both must never suspend local tracing -- it keeps contributing, it never \
             yields the buffer to remote"
        );
    }

    #[test]
    fn only_both_mode_combines_and_only_while_remote_is_still_active() {
        for mode in [
            LiveComputeTarget::LocalOnly,
            LiveComputeTarget::RemoteOnly,
            LiveComputeTarget::Both,
        ] {
            assert!(
                !should_combine_remote(false, mode),
                "remote_active == false must never combine, regardless of mode"
            );
        }
        assert!(!should_combine_remote(true, LiveComputeTarget::LocalOnly));
        assert!(
            !should_combine_remote(true, LiveComputeTarget::RemoteOnly),
            "RemoteOnly's local tracing is suspended whenever this could matter -- it \
             must never itself decide to combine"
        );
        assert!(should_combine_remote(true, LiveComputeTarget::Both));
    }

    #[test]
    fn combining_persists_past_a_successful_remote_completion() {
        // `remote_active` deliberately stays `true` past `RemoteUpdate::Done` -- for
        // `Both`, this lets local's continued tracing keep ADDING to remote's finished
        // contribution.
        assert!(should_combine_remote(true, LiveComputeTarget::Both));
    }

    /// Verify requirement: "a settled combined render is not overwritten by either
    /// engine" -- local's growing sample count only ever ADDS to what
    /// `remote_accumulator` already holds, so the combined image can only converge
    /// further, never regress to a from-scratch restart.
    #[test]
    fn a_settled_combined_render_keeps_combining_rather_than_being_overwritten() {
        for _quiet_frame in 0..5 {
            let resolved = resolve_remote_ownership(false, true, true);
            assert!(resolved);
            assert!(should_combine_remote(resolved, LiveComputeTarget::Both));
        }
    }

    /// Verify requirement: "a scene change during a combined render restarts
    /// accumulation correctly" -- must stop folding a now-stale-scene remote
    /// contribution into the display, the same release `update_accumulation_state`'s
    /// `dirty` handling already resets `accum_buffer` for.
    #[test]
    fn a_scene_change_during_a_combined_render_stops_combining_a_now_stale_contribution() {
        let resolved = resolve_remote_ownership(true, true, true);
        assert!(
            !resolved,
            "a genuine scene invalidation must release ownership even mid-combine"
        );
        assert!(
            !should_combine_remote(resolved, LiveComputeTarget::Both),
            "combining must stop the instant ownership is released, so local's freshly \
             reset accum_buffer (see update_accumulation_state's own dirty handling) is \
             never summed with a now-stale-scene remote contribution"
        );
    }

    // ---- Disjoint sample ranges for the combined live path ---------------------------

    #[test]
    fn local_sample_offset_never_falls_inside_remotes_reserved_range() {
        // The same disjoint-sample-range property `export_thread::sample_cursor::
        // SampleCursor` proves for the export path's dynamically-claimed ranges,
        // specialised here to the live path's fixed reservation.
        for remote_reserved_samples in [0u32, 128, 512, 4096] {
            for local_pre_frame_count in [0u32, 1, 64, 10_000] {
                let sample_offset =
                    combined_sample_offset(remote_reserved_samples, local_pre_frame_count);
                assert!(
                    sample_offset >= remote_reserved_samples,
                    "local's sample_offset must never fall inside remote's reserved \
                     range [0, {remote_reserved_samples})"
                );
            }
        }
    }

    #[test]
    fn a_reservation_of_zero_reproduces_pre_combining_behaviour_exactly() {
        // `remote_reserved_samples == 0` is what every non-combining frame feeds this
        // -- must be a true no-op, bit-identical to before combining existed.
        for local_pre_frame_count in [0u32, 1, 64, 10_000] {
            assert_eq!(
                combined_sample_offset(0, local_pre_frame_count),
                local_pre_frame_count
            );
        }
    }

    // ---- resolve_remote_ownership: every dirty/remote_active transition it must
    // resolve correctly (hand-off start, ownership release, steady states) -- see its
    // own doc comment for the full reasoning these tests hold it to. ----------------

    #[test]
    fn nothing_active_and_no_dirty_stays_inactive() {
        assert!(!resolve_remote_ownership(false, false, false));
    }

    #[test]
    fn the_handoffs_own_settling_entry_does_not_release_ownership() {
        // DiscardLocalPreview sets `dirty` and `remote_active` together in one locked
        // mutation, starting from `remote_active == false` -- the frame that first
        // observes it has `dirty` and `remote_active` freshly true, `false` one frame ago.
        assert!(
            resolve_remote_ownership(true, true, false),
            "the hand-off starting must not immediately cancel itself"
        );
    }

    #[test]
    fn a_completed_remote_render_is_not_overwritten_by_local_tracing() {
        // Steady state after `RemoteUpdate::Done`: `remote_active` stays `true`, and
        // every quiet frame (`dirty == false`) must leave it alone -- the regression
        // test for local tracing restarting and overwriting a finished remote image.
        assert!(resolve_remote_ownership(false, true, true));
    }

    #[test]
    fn a_scene_change_after_a_completed_remote_render_resumes_local_tracing() {
        // A `gui::*` callback sets `ctx.dirty = true` for a reason unrelated to the
        // handoff machine; ownership must be released so local tracing resumes.
        assert!(!resolve_remote_ownership(true, true, true));
    }

    #[test]
    fn a_dirty_frame_while_already_inactive_stays_inactive() {
        assert!(!resolve_remote_ownership(true, false, false));
        assert!(!resolve_remote_ownership(true, false, true));
    }

    #[test]
    fn remote_rendering_survives_quiet_frames_with_no_dirty() {
        assert!(resolve_remote_ownership(false, true, true));
        assert!(resolve_remote_ownership(false, true, false));
    }

    // ---- update_accumulation_state: suspension must never discard samples -----------

    #[test]
    fn a_quiet_frame_with_unchanged_dimensions_preserves_accumulated_samples() {
        // Models a suspended frame (paused/tab-hidden/remote_active/export_active):
        // `dirty == false`, same width/height. Neither branch may fire.
        let mut accum = vec![Vec3::new(1.0, 2.0, 3.0); 4];
        let mut first_hit_depth = vec![0.5; 4];
        let mut first_hit_normal = vec![Vec3::X; 4];
        let mut first_hit_facet_id = vec![7; 4];
        let mut accum_samples = 42;
        let mut last_width = 2;
        let mut last_height = 2;

        update_accumulation_state(
            2,
            2,
            false,
            &mut AccumulationBuffers {
                accum: &mut accum,
                first_hit_depth: &mut first_hit_depth,
                first_hit_normal: &mut first_hit_normal,
                first_hit_facet_id: &mut first_hit_facet_id,
            },
            &mut accum_samples,
            &mut last_width,
            &mut last_height,
        );

        assert_eq!(
            accum_samples, 42,
            "a suspended frame must not reset progress"
        );
        assert_eq!(
            accum,
            vec![Vec3::new(1.0, 2.0, 3.0); 4],
            "the running sum must survive"
        );
    }

    #[test]
    fn a_dirty_frame_still_resets_accumulation_regardless_of_remote_ownership() {
        // `update_accumulation_state` only sees `dirty`/`width`/`height`, not
        // `remote_active`, by design: the reset and ownership-release decisions are
        // independent, both computed from the same `dirty` flag.
        let mut accum = vec![Vec3::new(1.0, 2.0, 3.0); 4];
        let mut first_hit_depth = vec![0.5; 4];
        let mut first_hit_normal = vec![Vec3::X; 4];
        let mut first_hit_facet_id = vec![7; 4];
        let mut accum_samples = 42;
        let mut last_width = 2;
        let mut last_height = 2;

        update_accumulation_state(
            2,
            2,
            true,
            &mut AccumulationBuffers {
                accum: &mut accum,
                first_hit_depth: &mut first_hit_depth,
                first_hit_normal: &mut first_hit_normal,
                first_hit_facet_id: &mut first_hit_facet_id,
            },
            &mut accum_samples,
            &mut last_width,
            &mut last_height,
        );

        assert_eq!(accum_samples, 0);
        assert_eq!(accum, vec![Vec3::ZERO; 4]);
    }
}
