//! Dragging the three handles of the selected tier: angle (A), depth (D) and index (I).
//!
//! The math (projection, hit radius, snapping, the value a pointer position stands for,
//! the wording) lives in `indicatrix_editor::manipulate`, shared with the desktop and the
//! web app. This module is the touch wiring around it, a port of the web app's
//! `views/manip`:
//!
//! - a press within the grab radius of a handle tip starts a drag
//!   ([`ViewportState::handle_begin`]); the gesture is its own undo step because the
//!   history's coalescing run is ended before and after it and every edit is stamped
//!   with the clock of the press;
//! - pointer moves become a [`DragValue`] that is applied at most once per
//!   [`DRAG_INTERVAL`] through `EditorSession::{set_tier_angle, pin_tier_mast,
//!   rotate_tier_indices}`; the live raster follows from the new generation, and the
//!   tiers whose solved mast moved because of the drag are outlined;
//! - the release flushes the newest value and asks for the after-edit path; a cancel
//!   undoes the gesture's one step.
//!
//! On a provisional Slice tier the same drag edits the Slice tool's design clone, with
//! no history and no after-edit path.

use std::{collections::BTreeSet, time::Duration};

use indicatrix_editor::manipulate::{
    ActiveDrag, DragValue, GestureInputs, HandleKind, SnapMode, drag_value,
    gesture::{apply_step, drain_value},
    target::masts_aligned,
    text,
};
use indicatrix_solid::preview::view::facets_of_tiers;

use super::{Env, HANDLE_GRAB_RADIUS_DP, Outcome, ViewportState};

/// The shortest time between two applications of a drag value (one per frame).
pub const DRAG_INTERVAL: Duration = Duration::from_millis(16);

impl ViewportState {
    /// The handle under the view position `(x, y)`, if handles are on show.
    #[must_use]
    pub fn hit_handle(&self, x: f32, y: f32, selected_tier: Option<usize>) -> Option<HandleKind> {
        let target = self.active_target(selected_tier)?;
        let fit = self.image_fit()?;
        let point = self.pick_point(x, y)?;
        target.hit(
            point,
            fit.length_to_image(HANDLE_GRAB_RADIUS_DP * self.options.get().handle_scale),
        )
    }

    /// Whether a handle drag is in progress.
    #[must_use]
    pub const fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// A press on a handle: captures what the drag edits and starts it.
    pub(super) fn handle_begin(&mut self, x: f32, y: f32, env: &mut Env<'_>) -> Outcome {
        let selected = *env.selected_tier;
        let (Some(kind), Some(target), Some(pointer)) = (
            self.hit_handle(x, y, selected),
            self.active_target(selected).cloned(),
            self.pick_point(x, y),
        ) else {
            return Outcome::default();
        };
        // A drag whose release was lost ends here, before the new one starts.
        if self.drag.is_some() {
            let _ended = self.handle_end(env);
        }
        if kind == HandleKind::Index && target.frame.is_indexless() {
            return Outcome::default();
        }
        let inputs = if target.provisional {
            self.slice
                .session
                .as_ref()
                .and_then(indicatrix_editor::manipulate::ProvisionalSlice::gesture_inputs)
        } else {
            let tier_count = env.session.design.tiers.len();
            let Some(start_angle_deg) = env
                .session
                .design
                .tiers
                .get(target.tier)
                .map(|tier| tier.angle_deg)
            else {
                return Outcome::default();
            };
            // The gesture must not be absorbed by (or leak into) a neighbouring edit.
            env.session.history.end_coalesce_run();
            let masts = self
                .committed_masts
                .clone()
                .filter(|masts| masts_aligned(tier_count, masts.len()));
            Some(GestureInputs {
                start_angle_deg,
                masts,
                restore: None,
            })
        };
        let Some(inputs) = inputs else {
            return Outcome::default();
        };
        let Some(drag) = ActiveDrag::begin(kind, &target, inputs, pointer, env.now) else {
            text::NEEDS_SOLVE_HINT.clone_into(&mut self.hint);
            return Outcome::redraw();
        };
        let provisional_hint = drag.provisional.then(|| self.provisional_hint());
        self.hint = drag.opening_hint(provisional_hint);
        self.drag = Some(drag);
        self.drag_applied_at = env.now;
        self.touch();
        Outcome::redraw()
    }

    /// The pointer moved while a handle is held.
    pub(super) fn handle_move(&mut self, x: f32, y: f32, env: &mut Env<'_>) -> Outcome {
        let Some(point) = self.pick_point(x, y) else {
            return Outcome::default();
        };
        let snap = if self.options.get().snap {
            SnapMode::Coarse
        } else {
            SnapMode::Off
        };
        let Some(drag) = self.drag.as_mut() else {
            return Outcome::default();
        };
        let value = drag_value(&drag.start, point, snap);
        drag.requested = Some(value);
        if env.now.saturating_sub(self.drag_applied_at) < DRAG_INTERVAL {
            return Outcome::default();
        }
        self.drag_applied_at = env.now;
        self.apply_drag(env)
    }

    /// Applies the newest requested value to the design (or the provisional tier) when
    /// it differs from the last one, then refreshes the live feedback.
    pub fn apply_drag(&mut self, env: &mut Env<'_>) -> Outcome {
        let Some(mut drag) = self.drag.take() else {
            return Outcome::default();
        };
        let mut out = Outcome::redraw();
        if let Some(value) = drag.requested {
            if drag.provisional {
                let applied = self.slice.session.as_mut().is_some_and(|session| {
                    drain_value(&mut drag.progress, value, |step| session.apply_step(step))
                });
                if applied {
                    self.touch();
                }
            } else {
                let (tier, now) = (drag.tier, drag.gesture_now);
                let mut failure: Option<String> = None;
                let applied = drain_value(&mut drag.progress, value, |step| {
                    match apply_step(env.session, tier, step, now) {
                        Ok(edit) => edit,
                        Err(error) => {
                            failure = Some(error.to_string());
                            None
                        }
                    }
                });
                out.live_edit = applied;
                out.toast = failure;
            }
        }
        self.drag_feedback(&mut drag);
        self.drag = Some(drag);
        out
    }

    /// The hint line and the outline of the tiers that follow the drag.
    fn drag_feedback(&mut self, drag: &mut ActiveDrag) {
        if drag.provisional {
            self.hint = self.provisional_hint();
            return;
        }
        let now = self
            .committed_masts
            .as_deref()
            .filter(|masts| masts.len() == drag.start_masts.len());
        let (hint, moved) = drag.live_feedback(now);
        self.hint = hint;
        if moved != drag.outlined {
            let tiers: BTreeSet<usize> = moved.iter().copied().collect();
            self.moved_facets = self
                .frame
                .as_ref()
                .map_or_else(Vec::new, |frame| facets_of_tiers(&frame.facet_tier, &tiers));
            drag.outlined = moved;
            self.touch();
        }
    }

    /// A frame landed: the masts it brought show which tiers follow the drag now.
    pub(super) fn refresh_drag_feedback(&mut self) {
        if let Some(mut drag) = self.drag.take() {
            self.drag_feedback(&mut drag);
            self.drag = Some(drag);
        }
    }

    /// The release: flushes the newest value, keeps the gesture as one undo step and
    /// says what it did.
    pub(super) fn handle_end(&mut self, env: &mut Env<'_>) -> Outcome {
        if self.drag.is_none() {
            return Outcome::default();
        }
        let mut out = self.apply_drag(env);
        let Some(drag) = self.drag.take() else {
            return out;
        };
        if !drag.provisional {
            env.session.history.end_coalesce_run();
        }
        if !drag.outlined.is_empty() {
            self.moved_facets.clear();
        }
        self.hint.clear();
        self.touch();
        out.toast = out.toast.or_else(|| drag.done_toast());
        out.edited = drag.progress.applied_any && !drag.provisional;
        out.refresh = out.edited;
        out.live_edit = false;
        out.redraw = true;
        out
    }

    /// The drag was interrupted: undoes the gesture's one step, if it applied any.
    pub(super) fn handle_cancel(&mut self, env: &mut Env<'_>) -> Outcome {
        let Some(drag) = self.drag.take() else {
            return Outcome::default();
        };
        let mut out = Outcome::redraw();
        if drag.provisional {
            let restored = match (drag.restore.clone(), self.slice.session.as_mut()) {
                (Some(snapshot), Some(session)) => session.restore(snapshot),
                _ => false,
            };
            if restored {
                self.touch();
            }
        } else {
            env.session.history.end_coalesce_run();
            if drag.progress.applied_any {
                if Some(env.session.current_generation()) == drag.progress.applied_generation {
                    match env.session.undo() {
                        Ok(Some(_)) => {
                            out.edited = true;
                            out.refresh = true;
                            out.toast = Some(text::DRAG_CANCELLED_TOAST.to_owned());
                        }
                        Ok(None) => {}
                        Err(error) => {
                            out.toast = Some(text::drag_cancel_failed_toast(&error.to_string()));
                        }
                    }
                } else {
                    out.toast = Some(text::DRAG_CANCEL_SKIPPED_TOAST.to_owned());
                }
            }
        }
        if !drag.outlined.is_empty() {
            self.moved_facets.clear();
        }
        self.hint.clear();
        self.touch();
        out
    }

    /// The value the held handle currently asks for, for tests and readouts.
    #[must_use]
    pub fn drag_request(&self) -> Option<DragValue> {
        self.drag.as_ref().and_then(|drag| drag.requested)
    }
}
