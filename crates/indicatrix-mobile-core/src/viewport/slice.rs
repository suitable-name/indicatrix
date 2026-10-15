//! The Slice tool: a line dragged across the stone becomes a PROVISIONAL facet that the
//! same three handles tweak, then Keep commits it as one `Edit::AddTier` or Discard drops
//! it.
//!
//! The session is `indicatrix_editor::manipulate::ProvisionalSlice` (shared with the
//! desktop and the web app): a clone of the design with the new tier appended, edited in
//! place, never part of the editor session or its undo history. The viewport draws it by
//! planning that clone under the reserved `PROVISIONAL_GENERATION`. While it exists the
//! committed design must not change underneath it; [`ViewportState::check_expiry`]
//! discards it when the design or the selected tier moves on.

use std::sync::Arc;

use indicatrix_cut_core::Edit;
use indicatrix_editor::manipulate::{
    DiscardReason, ProvisionalSlice, SliceLine, SliceSide,
    gesture::tier_label,
    provisional::{CornerPoints, session_outlives},
    text,
};

use super::{Env, Outcome, ViewportState};

/// The hint while the Slice tool waits for a line.
pub const SLICE_MODE_HINT: &str =
    "Slice: drag a line across the stone. The part to the right of your line is cut away.";

/// The rubber band of the line being drawn, in view coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LineDraw {
    /// Where the finger went down.
    pub start: (f32, f32),
    /// Where the finger is now.
    pub end: (f32, f32),
}

/// The Slice tool's state.
#[derive(Debug)]
pub struct SliceTool {
    /// Whether Slice mode is on.
    pub mode: bool,
    /// Whether the new facet is repeated around the whole symmetric set.
    pub symmetric: bool,
    /// The provisional tier, if a line was drawn.
    pub session: Option<ProvisionalSlice>,
    /// The line being drawn.
    pub line: Option<LineDraw>,
    /// The tier selected when the session began; a change of it ends the session.
    pub(crate) selected_at_start: Option<usize>,
    /// The provisional tier's facet ids (outlined green).
    pub(crate) outline: Vec<u32>,
}

impl Default for SliceTool {
    fn default() -> Self {
        Self {
            mode: false,
            symmetric: true,
            session: None,
            line: None,
            selected_at_start: None,
            outline: Vec::new(),
        }
    }
}

impl ViewportState {
    /// Turns Slice mode on or off. Turning it off drops a provisional tier.
    pub fn set_slice_mode(&mut self, on: bool) -> Outcome {
        if self.slice.mode == on {
            return Outcome::default();
        }
        self.slice.mode = on;
        self.slice.line = None;
        self.hint.clear();
        let mut out = Outcome::redraw();
        if !on {
            out.merge(self.slice_discard(DiscardReason::User));
        }
        self.touch();
        out.refresh = true;
        out
    }

    /// The hint for the provisional tier as it stands.
    #[must_use]
    pub fn provisional_hint(&self) -> String {
        let Some(session) = self.slice.session.as_ref() else {
            return String::new();
        };
        let Some(tier) = session.tier() else {
            return String::new();
        };
        let label = tier_label(tier, session.tier_index);
        if session.may_keep() {
            format!(
                "New tier {label}: {} facets at {:.1} deg. Drag A, D or I to adjust it, then tap Keep.",
                session.facet_count(),
                tier.angle_deg.abs()
            )
        } else {
            format!(
                "New tier {label}: {}. Tap Discard to drop it.",
                text::SLICE_NO_DEPTH_HINT
            )
        }
    }

    /// A finger went down in Slice mode: the line starts here.
    pub(super) fn slice_begin(&mut self, x: f32, y: f32) -> Outcome {
        if !self.slice.mode {
            return Outcome::default();
        }
        self.slice.line = Some(LineDraw {
            start: (x, y),
            end: (x, y),
        });
        self.hint.clear();
        Outcome::redraw()
    }

    /// The line's end follows the finger.
    pub(super) fn slice_move(&mut self, x: f32, y: f32) -> Outcome {
        if let Some(line) = self.slice.line.as_mut() {
            line.end = (x, y);
            return Outcome::redraw();
        }
        Outcome::default()
    }

    /// The line was interrupted.
    pub(super) fn slice_cancel(&mut self) -> Outcome {
        if self.slice.line.take().is_some() {
            return Outcome::redraw();
        }
        Outcome::default()
    }

    /// The finger lifted: the line becomes a snapped cutting plane and the provisional
    /// tier appears. A second line replaces the first, against the same committed stone.
    pub(super) fn slice_end(&mut self, x: f32, y: f32, env: &Env<'_>) -> Outcome {
        let Some(drawn) = self.slice.line.take() else {
            return Outcome::default();
        };
        let line = (|| {
            let geometry = self.frame.as_ref()?.geometry.as_ref()?;
            let a = self.pick_point(drawn.start.0, drawn.start.1)?;
            let b = self.pick_point(x, y)?;
            let corners = self.slice.session.as_ref().map_or_else(
                || Arc::clone(&geometry.corner_points),
                |session| Arc::clone(&session.base_corners),
            );
            Some((SliceLine::new(a, b, geometry), corners))
        })();
        let Some((line, corners)) = line else {
            text::SLICE_NEEDS_PICTURE_HINT.clone_into(&mut self.hint);
            return Outcome::redraw();
        };
        self.install(line, SliceSide::Right, corners, env)
    }

    /// Builds the session for `line` cutting away `side` and starts drawing it. A
    /// degenerate line leaves everything as it was and says why.
    fn install(
        &mut self,
        line: SliceLine,
        side: SliceSide,
        corners: CornerPoints,
        env: &Env<'_>,
    ) -> Outcome {
        let built = ProvisionalSlice::build(
            line,
            side,
            corners,
            &env.session.design,
            env.session.current_generation(),
            self.slice.symmetric,
        );
        let Some(session) = built else {
            text::SLICE_TOO_SHORT_HINT.clone_into(&mut self.hint);
            return Outcome::redraw();
        };
        self.slice.session = Some(session);
        self.slice.selected_at_start = *env.selected_tier;
        self.slice.outline.clear();
        self.target = None;
        self.hint.clear();
        self.touch();
        let mut out = Outcome::redraw();
        out.refresh = true;
        out
    }

    /// The same line with the other side cut away.
    pub fn slice_flip(&mut self, env: &mut Env<'_>) -> Outcome {
        if let Some(out) = self.discard_if_expired(env) {
            return out;
        }
        let again = self.slice.session.as_ref().map(|session| {
            (
                session.line,
                session.side.flipped(),
                Arc::clone(&session.base_corners),
            )
        });
        again.map_or_else(Outcome::default, |(line, side, corners)| {
            self.install(line, side, corners, env)
        })
    }

    /// Repeats the new facet around the whole symmetric set, or not.
    pub fn set_slice_symmetric(&mut self, on: bool, env: &mut Env<'_>) -> Outcome {
        self.slice.symmetric = on;
        if let Some(out) = self.discard_if_expired(env) {
            return out;
        }
        let rebuilt = self
            .slice
            .session
            .as_mut()
            .is_some_and(|session| session.rebuild_indices(on));
        if rebuilt {
            self.touch();
        }
        Outcome::redraw()
    }

    /// Commits the provisional tier as ONE `Edit::AddTier`.
    pub fn slice_keep(&mut self, env: &mut Env<'_>) -> Outcome {
        if self.drag.is_some() {
            return Outcome::default();
        }
        if let Some(out) = self.discard_if_expired(env) {
            return out;
        }
        let Some((tier, count, may_keep)) = self.slice.session.as_ref().and_then(|session| {
            Some((
                session.tier()?.clone(),
                session.facet_count(),
                session.may_keep(),
            ))
        }) else {
            return Outcome::default();
        };
        // A freshly sliced tier sits at the tangency mast: its facet has no area, so
        // keeping it would add a tier that cuts nothing.
        if !may_keep {
            text::SLICE_NO_DEPTH_HINT.clone_into(&mut self.hint);
            return Outcome {
                redraw: true,
                toast: Some(text::SLICE_NO_DEPTH_HINT.to_owned()),
                ..Outcome::default()
            };
        }
        let index = env.session.design.tiers.len();
        env.session.history.end_coalesce_run();
        match env.session.apply(Edit::AddTier {
            index,
            tier: tier.clone(),
        }) {
            Ok(_) => {
                self.end_session();
                self.slice.mode = false;
                env.select(Some(index));
                Outcome {
                    redraw: true,
                    edited: true,
                    refresh: true,
                    select: Some(Some(index)),
                    toast: Some(text::slice_kept_toast(
                        &tier_label(&tier, index),
                        count,
                        tier.angle_deg,
                    )),
                    live_edit: false,
                }
            }
            Err(error) => Outcome {
                redraw: true,
                toast: Some(error.to_string()),
                ..Outcome::default()
            },
        }
    }

    /// Drops the provisional tier and everything drawn for it. A no-op without one.
    pub fn slice_discard(&mut self, reason: DiscardReason) -> Outcome {
        let Some(label) = self
            .slice
            .session
            .as_ref()
            .map(|session| session.label().unwrap_or_default())
        else {
            return Outcome::default();
        };
        self.end_session();
        Outcome {
            redraw: true,
            refresh: true,
            toast: Some(reason.toast(&label)),
            ..Outcome::default()
        }
    }

    /// Removes the session and its outline, and any drag on it.
    fn end_session(&mut self) {
        self.slice.session = None;
        self.slice.selected_at_start = None;
        self.slice.outline.clear();
        self.slice.line = None;
        if self.drag.as_ref().is_some_and(|drag| drag.provisional) {
            self.drag = None;
        }
        self.target = None;
        self.hint.clear();
        self.touch();
    }

    /// Why the provisional tier must go now, if it must: the committed design (at
    /// `generation`) or the selected tier moved on since it began.
    fn expiry(&self, generation: u64, selected: Option<usize>) -> Option<DiscardReason> {
        let session = self.slice.session.as_ref()?;
        if !session_outlives(session.base_generation, generation) {
            Some(DiscardReason::DesignChanged)
        } else if selected != self.slice.selected_at_start {
            Some(DiscardReason::SelectionChanged)
        } else {
            None
        }
    }

    /// Discards the provisional tier when the design or the selection moved on since it
    /// began. Call it after any change to either; a no-op without a session.
    pub fn check_expiry(&mut self, generation: u64, selected: Option<usize>) -> Outcome {
        self.expiry(generation, selected)
            .map_or_else(Outcome::default, |reason| self.slice_discard(reason))
    }

    /// [`Self::check_expiry`] for a button press: `Some` when it discarded.
    fn discard_if_expired(&mut self, env: &Env<'_>) -> Option<Outcome> {
        let reason = self.expiry(env.session.current_generation(), *env.selected_tier)?;
        Some(self.slice_discard(reason))
    }

    /// Turns a landed provisional frame's masts into the green outline. Returns whether
    /// the outline changed.
    pub(super) fn slice_sync_outline(&mut self) -> bool {
        let geometry = self
            .frame
            .as_ref()
            .and_then(|frame| frame.geometry.as_ref());
        let Some(session) = self.slice.session.as_mut() else {
            return false;
        };
        let Some((changed, ids)) = session.take_outline_update(geometry) else {
            return false;
        };
        if changed {
            self.slice.outline = ids;
        }
        changed
    }
}
