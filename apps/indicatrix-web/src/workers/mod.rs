//! The page's one Worker pool (`indicatrix_web_core::host::WorkerPool`): render
//! Workers plus the solve Worker, created lazily when the first design appears
//! ([`pool`]) and shared by the solve path (`crate::app::solve`) and the renderer
//! (`crate::render`).
//!
//! The pool lives here, in a thread-local, not in `WebApp`: its methods run their
//! callbacks synchronously, and those callbacks borrow `WebApp`, so the pool must be
//! reachable without holding a `WebApp` borrow. The handles are `Rc`s, so [`pool`]
//! hands out cheap clones.

use indicatrix_web_core::host::WorkerPool;
use std::cell::RefCell;

thread_local! {
    static POOL: RefCell<Option<WorkerPool>> = const { RefCell::new(None) };
    /// Why the one creation attempt failed; returned by every later call.
    static FAILURE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The pool if it has been created.
#[must_use]
pub fn existing() -> Option<WorkerPool> {
    POOL.with(|pool| pool.borrow().clone())
}

/// The pool, created on first use: `clamp(hardwareConcurrency - 1, 1, 8)` render
/// Workers and one solve Worker, loading in the background (calls made before they
/// are ready are queued by the pool).
///
/// # Errors
///
/// Why the Workers could not be started (the loader script is missing, or Workers are
/// blocked); the same error is returned on every later call without retrying.
pub fn pool() -> Result<WorkerPool, String> {
    if let Some(pool) = existing() {
        return Ok(pool);
    }
    if let Some(error) = FAILURE.with(|f| f.borrow().clone()) {
        return Err(error);
    }
    match WorkerPool::new() {
        Ok(pool) => {
            POOL.with(|slot| *slot.borrow_mut() = Some(pool.clone()));
            Ok(pool)
        }
        Err(error) => {
            let error = format!("The compute workers could not start: {error}");
            FAILURE.with(|f| *f.borrow_mut() = Some(error.clone()));
            Err(error)
        }
    }
}
