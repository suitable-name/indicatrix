//! Dragging in the Render mode: what the picture area shows while the pose or the light
//! moves, and when the trace may start again.
//!
//! A ray-traced epoch cannot follow a finger: every pose is a new scene, and the first pass
//! of a new epoch takes longer than a drag event. So while the camera or the light moves:
//!
//! - the running trace is cancelled once (cheap: flags and an epoch id; the workers are not
//!   joined) and no other epoch starts while a finger is down;
//! - a **camera** move shows the Solid raster at the live pose ([`Picture::Solid`]), which
//!   the raster queue draws as smoothly as in the Solid view;
//! - a **light** move cannot be shown by the Solid raster, so the last traced picture stays;
//! - when the finger is up and the pose has rested for [`SETTLE`], one epoch starts at the
//!   final pose ([`Gate::Go`]);
//! - the Solid raster stays on show until the first traced pass of that epoch has landed
//!   ([`LiveDrag::frame_landed`]), so the two pictures never alternate.
//!
//! Everything here is plain data and arithmetic so that it is tested natively;
//! [`RenderDriver`](super::render_view::RenderDriver) applies it to the trace.

use std::time::Duration;

use indicatrix_solid::preview::CameraPose;

/// How long the pose or the light must rest, with no finger down, before the trace starts.
pub const SETTLE: Duration = Duration::from_millis(120);

/// What the Render mode's picture area shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Picture {
    /// The ray-traced picture (or the status text while there is none).
    #[default]
    Trace,
    /// The Solid raster at the live pose.
    Solid,
}

/// What [`LiveDrag::gate`] decided about starting a trace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Nothing moved since the last start: the normal change check applies.
    Idle,
    /// A finger is down: do not start; the trace stays cancelled.
    Hold,
    /// The finger is up but the pose has not rested yet: ask again after this long.
    Wait(Duration),
    /// Start one epoch now, at the final pose.
    Go,
}

/// The drag state of the Render mode. Kept in
/// [`ViewportState::render_live`](super::ViewportState::render_live).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiveDrag {
    picture: Picture,
    /// The camera or the light moved since the last start: a trace is owed.
    owes: bool,
    /// A move happened that has not been timed yet (the driver stamps it).
    unstamped: bool,
    /// When the last move was seen, since the app started.
    changed_at: Duration,
    /// The running trace was cancelled for this drag.
    stopped: bool,
    /// The epoch the last start began and the pose it began at.
    started: Option<(u64, CameraPose)>,
}

impl LiveDrag {
    /// What the picture area shows.
    #[must_use]
    pub const fn picture(&self) -> Picture {
        self.picture
    }

    /// Whether the Solid raster is on show in place of the traced picture.
    #[must_use]
    pub const fn solid(&self) -> bool {
        matches!(self.picture, Picture::Solid)
    }

    /// Whether a trace is owed because the camera or the light moved.
    #[must_use]
    pub const fn owes_restart(&self) -> bool {
        self.owes
    }

    /// The camera moved: show the Solid raster at the new pose and owe a trace.
    pub const fn camera_moved(&mut self) {
        self.picture = Picture::Solid;
        self.owes = true;
        self.unstamped = true;
    }

    /// The light moved: keep the traced picture and owe a trace.
    pub const fn light_moved(&mut self) {
        self.owes = true;
        self.unstamped = true;
    }

    /// Times a move that has not been timed yet at `now`.
    pub const fn stamp(&mut self, now: Duration) {
        if self.unstamped {
            self.changed_at = now;
            self.unstamped = false;
        }
    }

    /// Whether the running trace was already cancelled for this drag.
    #[must_use]
    pub const fn is_stopped(&self) -> bool {
        self.stopped
    }

    /// Notes that the running trace was cancelled for this drag.
    pub const fn mark_stopped(&mut self) {
        self.stopped = true;
    }

    /// Whether a trace may start at `now`; `touching` says a finger is down.
    #[must_use]
    pub fn gate(&self, now: Duration, touching: bool) -> Gate {
        if !self.owes {
            return Gate::Idle;
        }
        if touching {
            return Gate::Hold;
        }
        (self.changed_at + SETTLE)
            .checked_sub(now)
            .filter(|rest| !rest.is_zero())
            .map_or(Gate::Go, Gate::Wait)
    }

    /// Epoch `epoch` was started at `camera`: nothing is owed any more.
    pub const fn started(&mut self, epoch: u64, camera: CameraPose) {
        self.owes = false;
        self.unstamped = false;
        self.stopped = false;
        self.started = Some((epoch, camera));
    }

    /// The start failed (nothing to render): show the traced side (its status text) and owe
    /// nothing until the next move.
    pub const fn abandoned(&mut self) {
        self.picture = Picture::Trace;
        self.owes = false;
        self.unstamped = false;
        self.stopped = false;
        self.started = None;
    }

    /// A traced frame of `epoch` is on screen while the camera is at `camera`. Returns
    /// whether the picture area switched from the Solid raster to the traced picture.
    ///
    /// Only the first frame of the epoch started at the current pose switches; a frame of an
    /// older epoch, or one that landed after the camera moved again, leaves the Solid raster.
    pub fn frame_landed(&mut self, epoch: u64, camera: CameraPose) -> bool {
        let current = !self.owes && self.started == Some((epoch, camera));
        if current && self.solid() {
            self.picture = Picture::Trace;
            return true;
        }
        false
    }

    /// Forgets everything (the Render mode went off show).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
#[path = "render_drag_tests.rs"]
mod tests;
