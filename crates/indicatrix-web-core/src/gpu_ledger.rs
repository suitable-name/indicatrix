//! The book the render pool keeps of the passes it has posted to the GPU Worker.
//!
//! The GPU Worker answers every `TraceFullPass` it is sent, one at a time and in order. The
//! pool therefore counts the passes still owed an answer ([`GpuLedger::is_busy`]) and
//! remembers which one it still wants ([`GpuLedger::live`]). A pass the pool no longer wants
//! (its scene was replaced, or the self-check scene ran in between) stays counted until the
//! Worker answers it, so the GPU slot is never handed a second pass while the Worker is
//! still tracing the first, and a stale answer can never clear the supervision of a newer
//! pass.

/// The pass the pool is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LivePass {
    /// The scene it traces.
    pub scene_id: u64,
    /// The planner's pass id.
    pub pass_id: u64,
    /// Its first sample index (the only id a dropped-chunk answer carries).
    pub sample_offset: u32,
}

/// What an answer from the Worker was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    /// The pass the pool was waiting for.
    Live(LivePass),
    /// An answer to a pass nobody wants any more (or to the self-check).
    Stale,
}

/// The GPU slot's pass bookkeeping.
#[derive(Debug, Default)]
pub struct GpuLedger {
    outstanding: u32,
    live: Option<LivePass>,
}

impl GpuLedger {
    /// Records a pass posted to the Worker that the pool wants the result of.
    pub const fn begin(&mut self, pass: LivePass) {
        self.outstanding += 1;
        self.live = Some(pass);
    }

    /// Records a pass posted to the Worker whose result the pool does not merge (the
    /// self-check).
    pub const fn begin_foreign(&mut self) {
        self.outstanding += 1;
    }

    /// The pool no longer wants the live pass (its scene was replaced); the Worker still
    /// owes the answer. Returns it.
    pub const fn abandon_live(&mut self) -> Option<LivePass> {
        self.live.take()
    }

    /// Matches one answer of the Worker to the live pass. `pass_id` is `None` for a
    /// dropped-chunk answer, which carries only the scene and the sample offset.
    pub fn answered(&mut self, scene_id: u64, pass_id: Option<u64>, sample_offset: u32) -> Answer {
        self.outstanding = self.outstanding.saturating_sub(1);
        match self.live {
            Some(live)
                if live.scene_id == scene_id
                    && live.sample_offset == sample_offset
                    && pass_id.is_none_or(|id| id == live.pass_id) =>
            {
                self.live = None;
                Answer::Live(live)
            }
            _ => Answer::Stale,
        }
    }

    /// The Worker is gone (lost, crashed, timed out, terminated): nothing is owed any more.
    /// Returns the live pass, which the caller gives back to the planner.
    pub const fn clear(&mut self) -> Option<LivePass> {
        self.outstanding = 0;
        self.live.take()
    }

    /// Whether the Worker still owes an answer.
    #[must_use]
    pub const fn is_busy(&self) -> bool {
        self.outstanding > 0
    }

    /// The pass the pool is waiting for, if any.
    #[must_use]
    pub const fn live(&self) -> Option<LivePass> {
        self.live
    }

    /// Passes the Worker still owes an answer for.
    #[must_use]
    pub const fn outstanding(&self) -> u32 {
        self.outstanding
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass(scene_id: u64, pass_id: u64) -> LivePass {
        LivePass {
            scene_id,
            pass_id,
            sample_offset: u32::try_from(pass_id).expect("test pass ids are small") * 4,
        }
    }

    #[test]
    fn a_fresh_ledger_is_idle() {
        let ledger = GpuLedger::default();
        assert!(!ledger.is_busy());
        assert_eq!(ledger.live(), None);
    }

    #[test]
    fn the_live_answer_frees_the_slot() {
        let mut ledger = GpuLedger::default();
        ledger.begin(pass(5, 2));
        assert!(ledger.is_busy());
        assert_eq!(ledger.live(), Some(pass(5, 2)));
        assert_eq!(ledger.answered(5, Some(2), 8), Answer::Live(pass(5, 2)));
        assert!(!ledger.is_busy());
        assert_eq!(ledger.live(), None);
    }

    #[test]
    fn a_dropped_chunk_answer_matches_by_scene_and_offset() {
        let mut ledger = GpuLedger::default();
        ledger.begin(pass(5, 2));
        assert_eq!(ledger.answered(5, None, 8), Answer::Live(pass(5, 2)));
        assert!(!ledger.is_busy());
    }

    #[test]
    fn a_stale_answer_keeps_the_new_pass_supervised() {
        let mut ledger = GpuLedger::default();
        // Scene 5 pass 2 is in flight, then the scene is replaced.
        ledger.begin(pass(5, 2));
        assert_eq!(ledger.abandon_live(), Some(pass(5, 2)));
        assert!(ledger.is_busy(), "the Worker still owes the answer");
        // The new scene's pass is queued behind it.
        ledger.begin(pass(6, 0));
        assert_eq!(ledger.outstanding(), 2);
        // The stale answer arrives first and must not clear the new pass.
        assert_eq!(ledger.answered(5, Some(2), 8), Answer::Stale);
        assert!(ledger.is_busy());
        assert_eq!(ledger.live(), Some(pass(6, 0)));
        assert_eq!(ledger.answered(6, Some(0), 0), Answer::Live(pass(6, 0)));
        assert!(!ledger.is_busy());
    }

    #[test]
    fn the_self_check_answer_is_stale_and_counted() {
        let mut ledger = GpuLedger::default();
        ledger.begin_foreign();
        assert!(ledger.is_busy());
        assert_eq!(ledger.answered(u64::MAX, Some(0), 0), Answer::Stale);
        assert!(!ledger.is_busy());
    }

    #[test]
    fn an_answer_with_nothing_owed_does_not_underflow() {
        let mut ledger = GpuLedger::default();
        assert_eq!(ledger.answered(1, Some(0), 0), Answer::Stale);
        assert!(!ledger.is_busy());
    }

    #[test]
    fn clear_returns_the_live_pass_and_forgets_everything() {
        let mut ledger = GpuLedger::default();
        ledger.begin_foreign();
        ledger.begin(pass(3, 1));
        assert_eq!(ledger.clear(), Some(pass(3, 1)));
        assert!(!ledger.is_busy());
        assert_eq!(ledger.live(), None);
        assert_eq!(ledger.clear(), None);
    }
}
