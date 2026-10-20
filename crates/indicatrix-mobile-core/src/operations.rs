//! Identity and cooperative cancellation for immutable editor-operation inputs.
//!
//! The caller fingerprints every captured input, including settings and targets,
//! and re-computes that fingerprint before applying a result. Generation alone is
//! deliberately insufficient. These tokens are process-local, never recovery IDs.

use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

use sha2::{Digest, Sha256};

static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);

/// Hash of length-framed immutable input components. Never hash display rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputFingerprint(pub [u8; 32]);

impl InputFingerprint {
    /// Components must use a canonical, lossless encoding and a stable order.
    #[must_use]
    pub fn from_parts<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Self {
        let mut hash = Sha256::new();
        hash.update(b"indicatrix-mobile-operation-v1");
        for part in parts {
            hash.update((part.len() as u64).to_le_bytes());
            hash.update(part);
        }
        Self(hash.finalize().into())
    }
}

/// All four fields must match before progress, completion or Apply is accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationIdentity {
    /// Unique to this installed document, even when generations happen to match.
    pub document: u64,
    /// Shared editor generation at capture.
    pub generation: u64,
    /// Monotonic within this controller.
    pub run: u64,
    /// Complete captured inputs, including non-authoring operation controls.
    pub inputs: InputFingerprint,
}

/// Honest engine lifecycle; cancellation is not completion or resumable pause.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationPhase {
    Preparing,
    Running,
    Cancelling,
    /// Only for engines with an actual continuation mechanism.
    Paused,
    Completed,
    Failed(String),
    Stale,
}

/// A worker checks this at bounded engine checkpoints, including before expensive setup.
#[derive(Debug, Clone)]
pub struct OperationToken {
    pub identity: OperationIdentity,
    cancelled: Arc<AtomicBool>,
}

impl OperationToken {
    /// The engine observes this exact flag at its own cooperative checkpoints.
    #[must_use]
    pub fn cancel_flag(&self) -> &AtomicBool {
        &self.cancelled
    }

    pub fn request_cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// UI-thread owner of the currently selected operation. Workers own tokens only.
pub struct OperationState {
    document: u64,
    next_run: u64,
    token: Option<OperationToken>,
    phase: Option<OperationPhase>,
}

impl Default for OperationState {
    fn default() -> Self {
        Self {
            document: NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed),
            next_run: 0,
            token: None,
            phase: None,
        }
    }
}

impl OperationState {
    /// Process-local installed-document identity for editor drafts and async loads.
    #[must_use]
    pub const fn document_id(&self) -> u64 {
        self.document
    }
    /// Supersedes earlier work. The returned token belongs to immutable worker inputs.
    ///
    /// # Panics
    ///
    /// When the 64-bit run counter is exhausted, which no session reaches.
    pub fn begin(&mut self, generation: u64, inputs: InputFingerprint) -> OperationToken {
        self.invalidate();
        self.next_run = self
            .next_run
            .checked_add(1)
            .expect("operation run ID exhausted");
        let token = OperationToken {
            identity: OperationIdentity {
                document: self.document,
                generation,
                run: self.next_run,
                inputs,
            },
            cancelled: Arc::new(AtomicBool::new(false)),
        };
        self.token = Some(token.clone());
        self.phase = Some(OperationPhase::Preparing);
        token
    }

    #[must_use]
    pub const fn phase(&self) -> Option<&OperationPhase> {
        self.phase.as_ref()
    }

    /// Checks live document, run, generation, complete inputs and cancellation.
    #[must_use]
    pub fn accepts(
        &self,
        identity: OperationIdentity,
        generation: u64,
        inputs: InputFingerprint,
    ) -> bool {
        self.token.as_ref().is_some_and(|token| {
            token.identity == identity
                && identity.document == self.document
                && identity.generation == generation
                && identity.inputs == inputs
                && !token.is_cancelled()
        }) && !matches!(
            self.phase,
            Some(OperationPhase::Stale | OperationPhase::Cancelling)
        )
    }

    /// Progress/completion publication must pass through this on the UI thread.
    pub fn publish(
        &mut self,
        identity: OperationIdentity,
        generation: u64,
        inputs: InputFingerprint,
        phase: OperationPhase,
    ) -> bool {
        if !self.accepts(identity, generation, inputs) {
            return false;
        }
        let legal = matches!(
            (&self.phase, &phase),
            (
                Some(OperationPhase::Preparing),
                OperationPhase::Running | OperationPhase::Failed(_)
            ) | (
                Some(OperationPhase::Running),
                OperationPhase::Running | OperationPhase::Completed | OperationPhase::Failed(_)
            )
        );
        if legal {
            self.phase = Some(phase);
        }
        legal
    }

    /// Only completed, current results may be applied. Partial candidates need their
    /// own explicit validated result contract; cancellation never grants Apply here.
    #[must_use]
    pub fn can_apply(
        &self,
        identity: OperationIdentity,
        generation: u64,
        inputs: InputFingerprint,
    ) -> bool {
        self.phase == Some(OperationPhase::Completed) && self.accepts(identity, generation, inputs)
    }

    pub fn cancel(&mut self) {
        if let Some(token) = &self.token {
            token.cancelled.store(true, Ordering::Release);
            if matches!(
                self.phase,
                Some(OperationPhase::Preparing | OperationPhase::Running | OperationPhase::Paused)
            ) {
                self.phase = Some(OperationPhase::Cancelling);
            }
        }
    }

    /// An engine has STOPPED and returned genuine validated partial candidates.
    /// This explicit terminal handoff grants result use without calling a cancelled
    /// search "resumed" or "complete" in its presentation. Lifecycle/edit invalidation
    /// stays stale and cannot be revived by this path.
    pub fn finish_partial(
        &mut self,
        identity: OperationIdentity,
        generation: u64,
        inputs: InputFingerprint,
    ) -> bool {
        let same_source = self
            .token
            .as_ref()
            .is_some_and(|token| token.identity == identity)
            && identity.document == self.document
            && identity.generation == generation
            && identity.inputs == inputs;
        if !same_source
            || !matches!(
                self.phase,
                Some(
                    OperationPhase::Preparing
                        | OperationPhase::Running
                        | OperationPhase::Cancelling
                )
            )
        {
            return false;
        }
        self.token = Some(OperationToken {
            identity,
            cancelled: Arc::new(AtomicBool::new(false)),
        });
        self.phase = Some(OperationPhase::Completed);
        true
    }

    /// Called for edits, Undo/Redo, input changes and lifecycle interruption.
    pub fn invalidate(&mut self) {
        self.cancel();
        if self.token.is_some() {
            self.phase = Some(OperationPhase::Stale);
        }
    }

    /// The app left the foreground (a file picker, the share sheet, another window).
    ///
    /// Work in flight is stopped and stays stale, as after [`Self::invalidate`]. A result
    /// that is already complete is kept: nothing about the design changed, so the user
    /// who saved a file or shared a picture can come back and still apply it.
    pub fn interrupt(&mut self) {
        if self.phase != Some(OperationPhase::Completed) {
            self.invalidate();
        }
    }

    /// A new document cannot inherit any earlier document's result, even at the same generation.
    pub fn replace_document(&mut self) {
        self.invalidate();
        self.document = NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed);
    }
}

impl Drop for OperationState {
    fn drop(&mut self) {
        self.cancel();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs() -> InputFingerprint {
        InputFingerprint::from_parts([b"authoring".as_slice(), b"settings".as_slice()])
    }

    #[test]
    fn replaced_document_rejects_completion_at_same_generation() {
        let mut state = OperationState::default();
        let token = state.begin(4, inputs());
        state.replace_document();
        assert!(token.is_cancelled());
        assert!(!state.publish(token.identity, 4, inputs(), OperationPhase::Completed));
        let new = state.begin(4, inputs());
        assert_ne!(new.identity.document, token.identity.document);
    }

    #[test]
    fn cancellation_and_replacement_cannot_publish_or_apply() {
        let mut state = OperationState::default();
        let old = state.begin(1, inputs());
        let current = state.begin(1, inputs());
        assert!(old.is_cancelled());
        assert!(!state.publish(old.identity, 1, inputs(), OperationPhase::Running));
        assert!(state.publish(current.identity, 1, inputs(), OperationPhase::Running));
        assert!(state.publish(current.identity, 1, inputs(), OperationPhase::Completed));
        assert!(state.can_apply(current.identity, 1, inputs()));
        assert!(!state.can_apply(current.identity, 2, inputs()));
        state.cancel();
        assert!(!state.can_apply(current.identity, 1, inputs()));
    }

    #[test]
    fn leaving_the_foreground_stops_work_but_keeps_a_finished_result() {
        let mut state = OperationState::default();
        let running = state.begin(1, inputs());
        assert!(state.publish(running.identity, 1, inputs(), OperationPhase::Running));
        state.interrupt();
        assert!(running.is_cancelled());
        assert!(!state.accepts(running.identity, 1, inputs()));
        assert_eq!(state.phase(), Some(&OperationPhase::Stale));

        let done = state.begin(1, inputs());
        assert!(state.publish(done.identity, 1, inputs(), OperationPhase::Running));
        assert!(state.publish(done.identity, 1, inputs(), OperationPhase::Completed));
        state.interrupt();
        assert!(!done.is_cancelled());
        assert!(state.can_apply(done.identity, 1, inputs()));
        // An edit still ends it.
        state.invalidate();
        assert!(!state.can_apply(done.identity, 1, inputs()));
    }

    #[test]
    fn component_boundaries_and_changed_controls_matter() {
        let first = InputFingerprint::from_parts([b"ab".as_slice(), b"c".as_slice()]);
        let second = InputFingerprint::from_parts([b"a".as_slice(), b"bc".as_slice()]);
        assert_ne!(first, second);
        let mut state = OperationState::default();
        let token = state.begin(1, first);
        assert!(!state.accepts(token.identity, 1, second));
    }
}
