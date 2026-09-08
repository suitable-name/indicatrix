//! [`HandoffMachine`]: the preview-then-handoff state machine.
//!
//! While the user manipulates the gem's orientation, the viewport renders locally at
//! low spp, exactly as it always has (`render_thread`'s ordinary progressive
//! accumulation). Once the orientation settles (a short debounce with no further
//! movement), the local low-spp preview is discarded -- never summed into anything --
//! and a full-quality render is requested from a configured remote worker instead. If
//! the user resumes dragging before that remote render finishes, it is cancelled and
//! its partial result discarded too, and the viewport falls back to local preview
//! rendering again.
//!
//! This module is deliberately pure: no sockets, no threads, no Slint. Every
//! transition is a plain function of `(current state, event)` -> `(next state,
//! actions)`, which is what makes it exercisable by the unit tests below with no GUI
//! and no worker running. `apps/indicatrix-cut/src/bridge/render_thread.rs` (or a
//! dedicated orchestration thread alongside it) is the one place that actually DRIVES
//! this machine -- translating real camera-drag ticks, a real debounce timer, and real
//! `indicatrix_net::client` events into [`HandoffEvent`]s, and carrying out the
//! [`HandoffAction`]s this returns (discarding a buffer, sending a `RenderRequest`,
//! sending `CANCEL`) against the real accumulator/socket. That wiring is the part that
//! genuinely can't be unit-tested without a live worker; this state machine is the part
//! that can be, and is, tested exhaustively below.
//!
//! # States
//!
//! - [`HandoffState::Idle`]: no drag activity. Whatever is currently displayed is
//!   settled -- either a locally-converging render, or a finished remote result.
//! - [`HandoffState::Previewing`]: the user is actively dragging (orientation-changed
//!   ticks are arriving). The local low-spp preview is what's on screen; that
//!   accumulation itself is `render_thread`'s ordinary `dirty`-triggered behavior, not
//!   something this machine has to separately trigger.
//! - [`HandoffState::Settling`]: the debounce elapsed with no further orientation
//!   change, a remote worker is configured/reachable, and a `RenderRequest` has just
//!   been dispatched -- this state covers the connect/handshake/dispatch window, before
//!   any reply has actually started streaming back.
//! - [`HandoffState::RemoteRendering`]: the worker's reply stream has started (its
//!   `WELCOME`/first `StreamEvent` was observed) and samples are actively
//!   accumulating from the remote side.
//! - [`HandoffState::Cancelled`]: the user resumed dragging while [`Settling`](HandoffState::Settling)
//!   or [`RemoteRendering`](HandoffState::RemoteRendering) was in progress. Transient --
//!   the very next [`HandoffEvent::OrientationChanged`] (which is, in practice, exactly
//!   what's still arriving from the still-ongoing drag) moves straight to
//!   [`Previewing`](HandoffState::Previewing).
//!
//! A completed remote render (`served_by() == Remote`) leaves this machine in `Idle`
//! indefinitely -- there is no timeout or separate "settled" state, because `Idle`
//! already means exactly that: nothing is dragging, and whatever's on screen is settled.
//! The caller is the one that decides how long the CALLER'S OWN `remote_active` flag
//! (outside this module -- see `render_thread::RenderContext::remote_active`'s doc
//! comment) keeps local tracing suspended while that Remote image is displayed;
//! [`HandoffEvent::SceneInvalidated`] is how the caller reports back here, once it does
//! release that flag for a reason other than a fresh drag, so `served_by` stays truthful.
//!
//! # The invariant this machine protects
//!
//! The local preview buffer and the remote accumulator must never share samples --
//! that is the same one-backend-per-image guarantee `indicatrix_net::client::Accumulator`
//! enforces across a single epoch switch, expressed here over TIME instead: a
//! [`HandoffAction::DiscardLocalPreview`] always precedes
//! [`HandoffAction::SendRenderRequestToWorker`] (both fire together, entering
//! [`Settling`](HandoffState::Settling)), and a
//! [`HandoffAction::DiscardRemotePartial`] always accompanies
//! [`HandoffAction::SendCancelToWorker`] (both fire together, entering
//! [`Cancelled`](HandoffState::Cancelled)) -- there is no transition in this machine
//! that lets a caller carry a buffer from one source into the other.

/// See the module doc comment for what each state covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffState {
    Idle,
    Previewing,
    Settling,
    RemoteRendering,
    Cancelled,
}

/// Which backend produced the image currently on screen -- the "indication of which
/// backend served the current image" the Slint panel surfaces. Updated only on a
/// SUCCESSFUL remote completion or a fresh local preview starting; a failed/cancelled
/// remote attempt leaves it unchanged (the graceful fallback: whatever was showing
/// before stays showing, or local rendering resumes and will update it once IT next
/// starts).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageSource {
    Local,
    Remote,
}

/// Inputs to [`HandoffMachine::handle`]. See the module doc comment for which real
/// occurrence each corresponds to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffEvent {
    /// The user moved the camera. Always the trigger that (re)starts local previewing,
    /// and -- if a remote attempt was in flight -- the trigger that interrupts it.
    OrientationChanged,
    /// The settle debounce elapsed with no further `OrientationChanged` since the last
    /// one. `worker_available` is decided by the caller at the moment the timer fires
    /// (a worker is configured AND was reachable last time this session checked) --
    /// this machine has no I/O of its own to determine that itself.
    SettleElapsed { worker_available: bool },
    /// The dispatched `RenderRequest`'s reply stream has started (e.g. the worker's
    /// `WELCOME` and/or its first `StreamEvent` was observed).
    RemoteStreamStarted,
    /// The remote render finished normally (`DONE { cancelled: false }`).
    RemoteDone,
    /// The remote attempt failed for any reason short of the user interrupting it --
    /// a connection/handshake failure, a worker `ERROR`, or a transport error mid-stream.
    RemoteFailed,
    /// The scene changed for a reason that has nothing to do with camera/light
    /// orientation -- material, lighting preset, sample target, resolution, bounce
    /// count, exposure, c-axis override, girdle finish, edge rounding, stone width, an
    /// HDR environment load/clear, a lighting-preset apply, or a different design being
    /// loaded (see `render_thread::mod`'s `resolve_remote_ownership` doc comment for the
    /// exhaustive enumeration) -- observed while a completed remote render was still the
    /// displayed image (`served_by() == Remote`). Never changes `state`: there is
    /// nothing here to discard or cancel -- the render loop already reset its own
    /// accumulation buffer via the very same `ctx.dirty = true` write that produced this
    /// event, entirely independently of this machine. This event exists solely to keep
    /// `served_by` truthful once local tracing has silently taken back over, so the
    /// "served by remote" indicator stops claiming a now-changing image is still
    /// remote-sourced.
    SceneInvalidated,
}

/// Outputs of [`HandoffMachine::handle`] -- what the caller must actually carry out
/// against the real accumulator/socket for this transition to be correct. See the
/// module doc comment's "invariant this machine protects" section for why
/// [`DiscardLocalPreview`](Self::DiscardLocalPreview) and
/// [`SendRenderRequestToWorker`](Self::SendRenderRequestToWorker) always appear
/// together, and likewise for [`SendCancelToWorker`](Self::SendCancelToWorker) /
/// [`DiscardRemotePartial`](Self::DiscardRemotePartial).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HandoffAction {
    /// Discard whatever the local low-spp accumulation buffer currently holds. Never
    /// summed into the remote accumulator that's about to start -- see
    /// `indicatrix_net::client::Accumulator`'s own module docs for the identical rule
    /// applied across a request-id epoch switch instead of across backends.
    DiscardLocalPreview,
    /// Send the (session-resolution, full-quality) `RenderRequest` to the configured
    /// worker.
    SendRenderRequestToWorker,
    /// Send `CANCEL` for the request currently in flight.
    SendCancelToWorker,
    /// Discard whatever the remote accumulator currently holds for the
    /// just-cancelled request -- it must never be shown, and never summed into
    /// whatever comes next (the next local preview, or a future remote attempt's own
    /// fresh accumulator).
    DiscardRemotePartial,
}

/// The preview-then-handoff state machine. See the module doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffMachine {
    state: HandoffState,
    served_by: ImageSource,
}

impl Default for HandoffMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl HandoffMachine {
    /// Starts in [`HandoffState::Idle`], `served_by` [`ImageSource::Local`] (matching
    /// the graceful-fallback default: with nothing configured or attempted yet, the
    /// image on screen is whatever local rendering already produces today).
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: HandoffState::Idle,
            served_by: ImageSource::Local,
        }
    }

    #[must_use]
    pub const fn state(self) -> HandoffState {
        self.state
    }

    #[must_use]
    pub const fn served_by(self) -> ImageSource {
        self.served_by
    }

    /// Advances the machine by one `event`, returning the [`HandoffAction`]s the
    /// caller must carry out for this transition to be correct (possibly empty).
    ///
    /// Total: every `(state, event)` pair produces SOME transition, including ones
    /// that don't correspond to a real occurrence in normal operation (e.g. a stray
    /// `RemoteDone` while [`Idle`](HandoffState::Idle)) -- those are treated as
    /// harmless no-ops (stay in the current state, no actions) rather than panicking,
    /// since a caller driving this from real, possibly-racy async events should never
    /// be able to crash it by delivering one out of the order this module anticipated.
    pub fn handle(&mut self, event: HandoffEvent) -> Vec<HandoffAction> {
        use HandoffAction::{
            DiscardLocalPreview, DiscardRemotePartial, SendCancelToWorker,
            SendRenderRequestToWorker,
        };
        use HandoffEvent::{
            OrientationChanged, RemoteDone, RemoteFailed, RemoteStreamStarted, SettleElapsed,
        };
        use HandoffState::{Cancelled, Idle, Previewing, RemoteRendering, Settling};

        let (next_state, actions) = match (self.state, event) {
            // -- Starting / continuing a drag --------------------------------------
            (Idle | Cancelled | Previewing, OrientationChanged) => (Previewing, vec![]),

            // -- The debounce elapsed -----------------------------------------------
            (
                Previewing,
                SettleElapsed {
                    worker_available: true,
                },
            ) => (
                Settling,
                vec![DiscardLocalPreview, SendRenderRequestToWorker],
            ),
            (
                Previewing,
                SettleElapsed {
                    worker_available: false,
                },
            ) => {
                // Graceful fallback: keep converging locally, exactly as today.
                (Idle, vec![])
            }

            // -- The dispatched request's reply stream begins ------------------------
            (Settling, RemoteStreamStarted) => (RemoteRendering, vec![]),

            // -- Interrupted by the user dragging again ------------------------------
            (Settling | RemoteRendering, OrientationChanged) => {
                (Cancelled, vec![SendCancelToWorker, DiscardRemotePartial])
            }

            // -- A remote attempt concludes -------------------------------------------
            (RemoteRendering, RemoteDone) | (Settling | RemoteRendering, RemoteFailed) => {
                (Idle, vec![])
            }

            // -- Everything else: a stray/out-of-order event, treated as a no-op ------
            (state, _) => (state, vec![]),
        };

        self.state = next_state;
        if matches!(event, HandoffEvent::RemoteDone) && next_state == HandoffState::Idle {
            self.served_by = ImageSource::Remote;
        } else if matches!(event, HandoffEvent::OrientationChanged)
            && next_state == HandoffState::Previewing
        {
            self.served_by = ImageSource::Local;
        } else if matches!(event, HandoffEvent::SceneInvalidated) {
            // Unconditional, not gated on `next_state`/`self.state` -- `SceneInvalidated`
            // always falls into the wildcard match arm above (`(state, _) => (state,
            // vec![])`), so `next_state` here always equals whatever `self.state` already
            // was. The caller (`gui::remote::orchestrator`) only actually sends this event
            // while `served_by() == Remote` (which, per the two branches above, only ever
            // holds while `state() == Idle`), but setting `served_by` to `Local`
            // unconditionally on this event is harmless even outside that window -- it is
            // already `Local` in every other state.
            self.served_by = ImageSource::Local;
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_idle_and_local() {
        let m = HandoffMachine::new();
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(m.served_by(), ImageSource::Local);
    }

    #[test]
    fn dragging_from_idle_enters_previewing_with_no_actions() {
        let mut m = HandoffMachine::new();
        let actions = m.handle(HandoffEvent::OrientationChanged);
        assert_eq!(m.state(), HandoffState::Previewing);
        assert_eq!(actions, Vec::new());
        assert_eq!(m.served_by(), ImageSource::Local);
    }

    #[test]
    fn continuing_to_drag_stays_in_previewing() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        for _ in 0..5 {
            let actions = m.handle(HandoffEvent::OrientationChanged);
            assert_eq!(m.state(), HandoffState::Previewing);
            assert_eq!(actions, Vec::new());
        }
    }

    #[test]
    fn settling_with_no_worker_falls_back_to_idle_with_no_actions() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        let actions = m.handle(HandoffEvent::SettleElapsed {
            worker_available: false,
        });
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(actions, Vec::new());
        // No successful remote completion happened -- still local.
        assert_eq!(m.served_by(), ImageSource::Local);
    }

    #[test]
    fn settling_with_a_worker_discards_the_local_preview_and_dispatches() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        let actions = m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        assert_eq!(m.state(), HandoffState::Settling);
        assert_eq!(
            actions,
            vec![
                HandoffAction::DiscardLocalPreview,
                HandoffAction::SendRenderRequestToWorker,
            ]
        );
    }

    #[test]
    fn the_full_happy_path_ends_remote_and_idle() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        let started_actions = m.handle(HandoffEvent::RemoteStreamStarted);
        assert_eq!(m.state(), HandoffState::RemoteRendering);
        assert_eq!(started_actions, Vec::new());

        let done_actions = m.handle(HandoffEvent::RemoteDone);
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(done_actions, Vec::new());
        assert_eq!(
            m.served_by(),
            ImageSource::Remote,
            "a normally-completed remote render must be reflected in served_by"
        );
    }

    /// The scenario the module doc comment calls out specifically: resuming a drag
    /// while a remote render is in flight must cancel it and discard the partial --
    /// and must never let that partial reach `served_by`/the display.
    #[test]
    fn resuming_the_drag_mid_remote_render_cancels_and_discards() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        m.handle(HandoffEvent::RemoteStreamStarted);
        assert_eq!(m.state(), HandoffState::RemoteRendering);

        let cancel_actions = m.handle(HandoffEvent::OrientationChanged);
        assert_eq!(m.state(), HandoffState::Cancelled);
        assert_eq!(
            cancel_actions,
            vec![
                HandoffAction::SendCancelToWorker,
                HandoffAction::DiscardRemotePartial,
            ]
        );
        // served_by must NOT have flipped to Remote for a cancelled render.
        assert_eq!(m.served_by(), ImageSource::Local);

        // The drag continues (the interrupting event above was itself a drag tick) --
        // the very next OrientationChanged resumes local previewing.
        let resume_actions = m.handle(HandoffEvent::OrientationChanged);
        assert_eq!(m.state(), HandoffState::Previewing);
        assert_eq!(resume_actions, Vec::new());
    }

    /// Same interruption, but during `Settling` -- before the worker's stream even
    /// started. Still must cancel/discard, never let a half-dispatched request's
    /// eventual reply be treated as current.
    #[test]
    fn resuming_the_drag_during_settling_also_cancels_and_discards() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        assert_eq!(m.state(), HandoffState::Settling);

        let actions = m.handle(HandoffEvent::OrientationChanged);
        assert_eq!(m.state(), HandoffState::Cancelled);
        assert_eq!(
            actions,
            vec![
                HandoffAction::SendCancelToWorker,
                HandoffAction::DiscardRemotePartial,
            ]
        );
    }

    #[test]
    fn a_failed_connection_during_settling_falls_back_to_idle() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        let actions = m.handle(HandoffEvent::RemoteFailed);
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(actions, Vec::new());
        assert_eq!(m.served_by(), ImageSource::Local);
    }

    #[test]
    fn a_failure_mid_stream_falls_back_to_idle_and_leaves_served_by_local() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        m.handle(HandoffEvent::RemoteStreamStarted);
        let actions = m.handle(HandoffEvent::RemoteFailed);
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(actions, Vec::new());
        assert_eq!(m.served_by(), ImageSource::Local);
    }

    /// A full cycle can repeat: after one remote render completes, dragging again
    /// starts a brand new preview-then-handoff cycle from scratch.
    #[test]
    fn a_second_drag_after_a_completed_remote_render_starts_a_fresh_cycle() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        m.handle(HandoffEvent::RemoteStreamStarted);
        m.handle(HandoffEvent::RemoteDone);
        assert_eq!(m.served_by(), ImageSource::Remote);

        m.handle(HandoffEvent::OrientationChanged);
        assert_eq!(m.state(), HandoffState::Previewing);
        assert_eq!(
            m.served_by(),
            ImageSource::Local,
            "a fresh drag must flip the displayed-source indicator back to Local \
             immediately, not wait for the next remote completion"
        );
    }

    /// The scenario `HandoffEvent::SceneInvalidated` exists for: a non-drag scene
    /// change (material/lighting/quality/etc.) arrives while a completed remote render
    /// is still the displayed image. `state` must not move (nothing to discard here --
    /// the render loop's own `dirty` write already did that), but `served_by` must
    /// truthfully flip back to `Local`.
    #[test]
    fn scene_invalidated_after_a_completed_remote_render_flips_served_by_to_local() {
        let mut m = HandoffMachine::new();
        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        m.handle(HandoffEvent::RemoteStreamStarted);
        m.handle(HandoffEvent::RemoteDone);
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(m.served_by(), ImageSource::Remote);

        let actions = m.handle(HandoffEvent::SceneInvalidated);
        assert_eq!(
            m.state(),
            HandoffState::Idle,
            "a non-drag scene change must not move the handoff state machine at all"
        );
        assert_eq!(actions, Vec::new());
        assert_eq!(
            m.served_by(),
            ImageSource::Local,
            "served_by must stop claiming Remote once local tracing has taken back over"
        );
    }

    /// `SceneInvalidated` while nothing remote has ever completed (still `Local`) is a
    /// harmless no-op -- it should never, say, move the machine out of `Previewing` or
    /// `RemoteRendering`.
    #[test]
    fn scene_invalidated_elsewhere_is_a_harmless_no_op() {
        let mut m = HandoffMachine::new();
        let actions = m.handle(HandoffEvent::SceneInvalidated);
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(actions, Vec::new());
        assert_eq!(m.served_by(), ImageSource::Local);

        m.handle(HandoffEvent::OrientationChanged);
        m.handle(HandoffEvent::SettleElapsed {
            worker_available: true,
        });
        m.handle(HandoffEvent::RemoteStreamStarted);
        assert_eq!(m.state(), HandoffState::RemoteRendering);
        let actions = m.handle(HandoffEvent::SceneInvalidated);
        assert_eq!(
            m.state(),
            HandoffState::RemoteRendering,
            "SceneInvalidated must not interrupt an in-flight remote render -- only a \
             real OrientationChanged (via SendCancelToWorker) does that"
        );
        assert_eq!(actions, Vec::new());
        assert_eq!(m.served_by(), ImageSource::Local);
    }

    #[test]
    fn stray_events_in_unexpected_states_are_harmless_no_ops() {
        let mut m = HandoffMachine::new();
        // RemoteDone/RemoteFailed/RemoteStreamStarted while Idle: no-op.
        assert_eq!(m.handle(HandoffEvent::RemoteDone), Vec::new());
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(m.handle(HandoffEvent::RemoteFailed), Vec::new());
        assert_eq!(m.state(), HandoffState::Idle);
        assert_eq!(m.handle(HandoffEvent::RemoteStreamStarted), Vec::new());
        assert_eq!(m.state(), HandoffState::Idle);

        // A stray SettleElapsed while Idle: no-op.
        assert_eq!(
            m.handle(HandoffEvent::SettleElapsed {
                worker_available: true
            }),
            Vec::new()
        );
        assert_eq!(m.state(), HandoffState::Idle);
    }

    #[test]
    fn every_state_and_event_combination_terminates_without_panicking() {
        // Exhaustive sweep: no (state, event) pair may panic, regardless of how
        // unrealistic the ordering -- a real caller driving this off async
        // socket/timer events can't be trusted to always deliver a "sensible" order.
        let events = [
            HandoffEvent::OrientationChanged,
            HandoffEvent::SettleElapsed {
                worker_available: true,
            },
            HandoffEvent::SettleElapsed {
                worker_available: false,
            },
            HandoffEvent::RemoteStreamStarted,
            HandoffEvent::RemoteDone,
            HandoffEvent::RemoteFailed,
            HandoffEvent::SceneInvalidated,
        ];

        for &event in &events {
            let mut m = HandoffMachine::new();
            m.handle(event); // from Idle
            m.handle(HandoffEvent::OrientationChanged);
            m.handle(event); // from Previewing
            m.handle(HandoffEvent::SettleElapsed {
                worker_available: true,
            });
            m.handle(event); // from Settling
            m.handle(HandoffEvent::RemoteStreamStarted);
            m.handle(event); // from RemoteRendering
        }
    }
}
