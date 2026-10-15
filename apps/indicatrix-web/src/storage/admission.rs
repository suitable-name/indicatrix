//! GPU admission records (the GPU track): a small record per adapter key under
//! `indicatrix.admission.<adapter>.v1` -- "this adapter compiled the kernel and may be
//! used" or "this adapter failed and must not be probed again". Stored in
//! `localStorage` so a verdict outlives the tab, with the `sessionStorage` fallback of
//! [`super::small_set`].
//!
//! The record's shape belongs to the caller; this module only moves text, or any
//! `serde` type as JSON.

use super::{keys, small_get, small_remove, small_set};
use serde::{Serialize, de::DeserializeOwned};

/// The stored record text of `adapter`, or `None` when absent.
#[must_use]
pub fn get(adapter: &str) -> Option<String> {
    small_get(&keys::admission_key(adapter))
}

/// Stores `json` as the record of `adapter`; `false` when no storage took it.
pub fn set(adapter: &str, json: &str) -> bool {
    small_set(&keys::admission_key(adapter), json)
}

/// Forgets the record of `adapter` (lets it be probed again).
pub fn remove(adapter: &str) {
    small_remove(&keys::admission_key(adapter));
}

/// The record of `adapter` parsed as `T`; `None` when absent or unparsable.
#[must_use]
pub fn get_json<T: DeserializeOwned>(adapter: &str) -> Option<T> {
    serde_json::from_str(&get(adapter)?).ok()
}

/// Stores `value` as JSON; `false` when it does not serialize or no storage took it.
pub fn set_json<T: Serialize>(adapter: &str, value: &T) -> bool {
    serde_json::to_string(value).is_ok_and(|json| set(adapter, &json))
}
