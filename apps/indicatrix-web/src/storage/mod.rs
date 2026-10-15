//! Storage that outlives the tab: `localStorage` for small keys (settings, guide state,
//! GPU admission records, cutting marks), IndexedDB for the autosaved current design,
//! and `sessionStorage` for the same-tab reload fast path and as the fallback where the
//! other two are unavailable.
//!
//! # Layers
//!
//! - [`local`] and [`session`]: thin, error-swallowing wrappers over the two
//!   `Storage` objects.
//! - [`small_get`] / [`small_set`] / [`small_remove`]: the policy for small keys.
//!   `localStorage` first; an entry only an earlier build left in `sessionStorage` is
//!   migrated once on its first read; with `localStorage` blocked everything behaves
//!   as it did when `sessionStorage` was the only store.
//! - [`idb`]: the autosaved design (bytes plus a meta record), async.
//! - [`marks`] and [`admission`]: typed-by-the-caller JSON blobs per stone and per GPU
//!   adapter, over the small-key policy.
//! - [`keys`]: the pure key builders (native-testable).
//!
//! Nothing here reports to the user; callers log (`app::diagnostics`) and carry on.

#![allow(
    dead_code,
    reason = "marks, admission and the IndexedDB clear/meta API are for later lanes"
)]

pub mod admission;
pub mod idb;
pub mod keys;
pub mod local;
pub mod marks;
pub mod session;

/// Reads a small key: `localStorage`, else `sessionStorage`.
///
/// A value found only in `sessionStorage` is the leftover of an earlier build (or of a
/// blocked `localStorage`); when `localStorage` accepts it, it is moved there (copied,
/// then removed from `sessionStorage`) so the migration happens once.
#[must_use]
pub fn small_get(key: &str) -> Option<String> {
    if let Some(value) = local::get(key) {
        return Some(value);
    }
    let value = session::get(key)?;
    if local::set(key, &value) {
        session::remove(key);
    }
    Some(value)
}

/// Writes a small key to `localStorage`, else to `sessionStorage`; `false` when neither
/// took it. A successful `localStorage` write drops any stale `sessionStorage` copy so a
/// later read cannot resurrect it.
pub fn small_set(key: &str, value: &str) -> bool {
    if local::set(key, value) {
        session::remove(key);
        return true;
    }
    session::set(key, value)
}

/// Removes a small key from both stores.
pub fn small_remove(key: &str) {
    local::remove(key);
    session::remove(key);
}
