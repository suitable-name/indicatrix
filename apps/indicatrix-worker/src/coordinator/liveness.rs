//! The registry's liveness thread: `PING` idle joined workers and drop the silent ones
//! (see `super::registry`'s module doc comment for the rules).

use super::registry::{Registry, WorkerConn};
use indicatrix_net::messages::{ClientMessage, StreamEvent};
use std::{
    sync::{Arc, Weak},
    thread,
    time::{Duration, Instant},
};

/// Starts the liveness loop: every `tick`, take the due idle connections and ping each
/// one on its own short-lived thread (one slow or dead worker never delays another's
/// ping). Exits once the registry has been dropped.
pub(super) fn spawn(registry: Weak<Registry>) {
    let spawned = thread::Builder::new()
        .name("coordinator-liveness".to_string())
        .spawn(move || {
            while let Some(registry) = registry.upgrade() {
                let tick = registry.liveness().tick;
                for (worker_id, conn, deadline) in registry.take_due_pings(Instant::now()) {
                    let registry = Arc::clone(&registry);
                    thread::spawn(move || ping_one(&registry, worker_id, conn, deadline));
                }
                drop(registry);
                thread::sleep(tick);
            }
        });
    if let Err(e) = spawned {
        tracing::error!("coordinator: could not start the worker liveness thread: {e}");
    }
}

/// Pings one idle connection and either checks it back in (answered in time) or drops
/// it (silent until `deadline`, or broken).
fn ping_one(registry: &Registry, worker_id: u32, mut conn: Box<dyn WorkerConn>, deadline: Instant) {
    let nonce = registry.next_nonce();
    match await_pong(conn.as_mut(), nonce, deadline) {
        Ok(()) => {
            let _ = conn.set_timeouts(None, None);
            tracing::trace!("coordinator: worker #{worker_id} answered PING {nonce}");
            registry.check_in(worker_id, conn);
        }
        Err(why) => {
            drop(conn);
            registry.remove(worker_id, &why);
        }
    }
}

/// Sends `PING{nonce}` and reads until the matching `PONG`, all before `deadline`.
/// Anything else an idle worker sends is logged and skipped.
///
/// # Errors
///
/// Why the worker counts as dead: a write/read failure, EOF, or no `PONG` in time.
fn await_pong(mut conn: &mut dyn WorkerConn, nonce: u64, deadline: Instant) -> Result<(), String> {
    let remaining = || {
        let left = deadline.saturating_duration_since(Instant::now());
        (!left.is_zero()).then(|| left.max(Duration::from_millis(1)))
    };
    let silent = || "no traffic within the liveness deadline (no PONG)".to_string();

    let first = remaining().ok_or_else(silent)?;
    conn.set_timeouts(Some(first), Some(first))
        .map_err(|e| format!("could not arm the PING deadline: {e}"))?;
    indicatrix_net::messages::write_message(&mut conn, &ClientMessage::Ping { nonce })
        .map_err(|e| format!("PING failed: {e}"))?;
    loop {
        let left = remaining().ok_or_else(silent)?;
        conn.set_timeouts(Some(left), Some(left))
            .map_err(|e| format!("could not arm the PONG deadline: {e}"))?;
        let (event, _payload) = indicatrix_net::messages::read_stream_event(&mut conn)
            .map_err(|e| format!("no PONG ({e})"))?;
        match event {
            StreamEvent::Pong { nonce: answered } if answered == nonce => return Ok(()),
            other => tracing::debug!("coordinator: ignoring {other:?} from an idle worker"),
        }
    }
}
