//! The connection caps: [`ConnectionLimiter`] (`--max-connections`, one instance per
//! listener role -- viewers and joined workers are counted separately) and its RAII
//! [`ConnectionSlot`]. See `crate::serve`'s "Robustness" section.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

/// Caps how many connections this worker handles at once (`--max-connections`, default 64).
///
/// One instance per listener ROLE, owned by [`super::run`]: the viewer listener's is
/// shared by every accepted viewer connection regardless of transport (TLS or
/// `--insecure-no-tls`) or build mode (library-only or `worker`); the coordinator's
/// worker port gets a second, separate instance (separate pools for viewers and
/// workers), held for as long as a joined worker stays registered.
///
/// `Clone` (cheap: `active` is already an `Arc`) so [`super::run`] can also hand a clone to the
/// token-based enrollment listener (`crate::enroll`) -- a genuinely separate
/// listener/concern (bootstrapping trust, not serving render/library requests), but one
/// whose TLS accept requires no client certificate at all (see that module's own doc
/// comment), so unauthenticated connections there need the same bound.
#[derive(Debug, Clone)]
pub struct ConnectionLimiter {
    active: Arc<AtomicUsize>,
    max: usize,
}

impl ConnectionLimiter {
    /// `pub(crate)` (not just used by [`super::run`]) so other listeners sharing this cap --
    /// `crate::enroll`'s own tests build a throwaway one the same way `run` does --
    /// can construct one without going through a full `serve` startup.
    #[must_use]
    pub(crate) fn new(max: usize) -> Self {
        Self {
            active: Arc::new(AtomicUsize::new(0)),
            max,
        }
    }

    /// Attempts to reserve one connection slot for a just-accepted socket.
    ///
    /// `Ok` hands back a [`ConnectionSlot`] that releases the slot when dropped --
    /// including when the connection thread's `catch_unwind`-wrapped handler panics:
    /// `Drop::drop` still runs while a panic unwinds (just not through an `abort`), and
    /// nothing in the connection-handling path aborts. `Err` carries the active count
    /// observed at the moment of refusal (for [`super::connection::refuse_for_capacity`]'s log
    /// line); the slot this call provisionally reserved to make that observation is
    /// released again immediately, so a refused connection never itself counts against
    /// the cap.
    ///
    /// # Errors
    ///
    /// `Err(active)` when this cap is already reached, carrying the active count
    /// observed at the moment of refusal -- not a "hard" error, just this call's own
    /// verdict for the caller to act on (typically [`super::connection::refuse_for_capacity`]).
    pub fn try_acquire(&self) -> Result<ConnectionSlot, usize> {
        let active = self.active.fetch_add(1, Ordering::SeqCst) + 1;
        if active > self.max {
            self.active.fetch_sub(1, Ordering::SeqCst);
            Err(active - 1)
        } else {
            Ok(ConnectionSlot {
                active: Arc::clone(&self.active),
            })
        }
    }
}

/// How many bare, not-yet-authenticated connections [`super::run`]'s accept loop allows in
/// flight at once, as a multiple of `--max-connections`: `handshake_limiter` in [`super::run`]
/// is a second, separate [`ConnectionLimiter`] with this larger cap.
///
/// The real slot in the REAL `--max-connections` limiter is deliberately reserved only
/// AFTER `accept_tls` succeeds (see `super::accept::spawn_connection_handler`'s doc comment), not in
/// the accept loop before `accept_tls` ever runs: reserving it earlier would let 64 bare
/// TCP connects that never send a `ClientHello` hold every slot for up to
/// [`super::HANDSHAKE_TIMEOUT`] (20s), locking out every certificate-holding viewer with
/// `CONNECTION_LIMIT_REACHED_CODE`, while the accept loop keeps spawning threads over the
/// cap regardless (bounding neither threads nor authenticated work). Even with the real
/// slot's acquisition deferred to AFTER `accept_tls` succeeds, an unauthenticated peer
/// can still pin a thread for up to `HANDSHAKE_TIMEOUT` just by connecting, so a second,
/// wider cap bounds THAT too: 4x headroom is enough that a real burst of
/// `--max-connections` legitimate viewers reconnecting at once is never refused by this
/// counter, while still bounding a flood of bare connects to a multiple of the real cap
/// rather than the entire OS thread budget.
pub const PRE_AUTH_HANDSHAKE_MULTIPLIER: usize = 4;

/// RAII handle for one reserved [`ConnectionLimiter`] slot.
///
/// Held by a connection thread for the lifetime of
/// the connection handler (moved
/// into the `thread::spawn` closure alongside everything else that connection needs);
/// [`Drop::drop`] releases the slot regardless of how that call returns -- normally, via
/// `?`, or via a caught panic.
#[derive(Debug)]
pub struct ConnectionSlot {
    active: Arc<AtomicUsize>,
}

impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::SeqCst);
    }
}

/// Unit tests for [`ConnectionLimiter`]/[`ConnectionSlot`] and
/// [`super::connection::refuse_for_capacity`] -- deliberately not gated on `feature = "worker"`
/// (unlike `mod tests` above/`serve::tests`), since the connection cap applies to a
/// library-only build exactly as much as a `worker` one (see this module's own doc
/// comment). No real sockets or threads: [`ConnectionLimiter`] is tested purely as
/// counter logic, and `refuse_for_capacity` against an in-memory buffer -- see
/// `serve::tests`/`serve::tests::mtls` (both `worker`-only) for the real-socket,
/// real-TLS end of the test pyramid this complements.
#[cfg(test)]
mod limiter_tests {
    use super::ConnectionLimiter;
    use crate::serve::connection;
    use std::sync::atomic::Ordering;

    #[test]
    fn acquires_up_to_max_and_refuses_the_next() {
        let limiter = ConnectionLimiter::new(2);
        let a = limiter
            .try_acquire()
            .expect("1st connection is under the cap");
        let b = limiter
            .try_acquire()
            .expect("2nd connection is exactly at the cap");
        let refused = limiter
            .try_acquire()
            .expect_err("3rd connection is over the cap");
        assert_eq!(
            refused, 2,
            "the refusal should report the active count that caused it"
        );

        drop(a);
        let c = limiter
            .try_acquire()
            .expect("a slot freed by drop can be reacquired");
        drop(b);
        drop(c);
        assert_eq!(limiter.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn a_refused_attempt_never_permanently_eats_into_the_cap() {
        let limiter = ConnectionLimiter::new(1);
        let _held = limiter.try_acquire().unwrap();
        for _ in 0..5 {
            limiter.try_acquire().unwrap_err();
        }
        // Each refusal above released the slot it provisionally reserved to observe the
        // count -- the active count must still read exactly 1 (`_held`), not 6.
        assert_eq!(limiter.active.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn the_slot_releases_its_count_even_when_the_holder_panics() {
        let limiter = ConnectionLimiter::new(1);
        let slot = limiter.try_acquire().unwrap();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _slot = slot; // moved in; dropped while unwinding out of this closure
            panic!("simulated connection-thread panic while holding a ConnectionSlot");
        }));
        assert!(result.is_err());
        // `Drop::drop` runs while a panic unwinds (this is what `serve::run`'s real
        // connection threads rely on -- see `ConnectionLimiter::try_acquire`'s doc
        // comment) -- the slot must be released exactly as if the closure had returned
        // normally instead of panicking.
        assert_eq!(limiter.active.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn refuse_for_capacity_writes_a_decodable_error_naming_the_capacity_code() {
        let mut buf: Vec<u8> = Vec::new();
        connection::refuse_for_capacity(&mut buf, None, 64, 64);

        let mut cursor = std::io::Cursor::new(buf);
        let err: indicatrix_net::messages::ErrorMsg =
            indicatrix_net::messages::read_message(&mut cursor).unwrap();
        assert_eq!(err.code, connection::CONNECTION_LIMIT_REACHED_CODE);
        assert!(err.message.contains("64"), "{}", err.message);

        // The same shape `indicatrix_net::client::handshake` already knows how to fall
        // back to for a BUILD_MISMATCH_CODE refusal (see `refuse_for_capacity`'s doc
        // comment) -- confirmed here by decoding as a bare `ErrorMsg` with no `Welcome`
        // ever written first.
    }
}
