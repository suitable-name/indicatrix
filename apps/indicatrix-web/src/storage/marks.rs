//! Per-stone cutting marks (Cutting Mode): one JSON blob per stone key under
//! `indicatrix.marks.<stone>.v1` in `localStorage`, falling back to `sessionStorage`
//! where that is blocked (see [`super::small_set`]).
//!
//! The blob's shape belongs to the caller; this module only moves text, or any
//! `serde` type as JSON.

use super::{keys, small_get, small_remove, small_set};
use serde::{Serialize, de::DeserializeOwned};

/// The stored marks text of `stone`, or `None` when absent.
#[must_use]
pub fn get(stone: &str) -> Option<String> {
    small_get(&keys::marks_key(stone))
}

/// Stores `json` as the marks of `stone`; `false` when no storage took it.
pub fn set(stone: &str, json: &str) -> bool {
    small_set(&keys::marks_key(stone), json)
}

/// Forgets the marks of `stone`.
pub fn remove(stone: &str) {
    small_remove(&keys::marks_key(stone));
}

/// The marks of `stone` parsed as `T`; `None` when absent or when the stored text does
/// not parse (the entry is then left alone: the caller may overwrite it).
#[must_use]
pub fn get_json<T: DeserializeOwned>(stone: &str) -> Option<T> {
    serde_json::from_str(&get(stone)?).ok()
}

/// Stores `value` as JSON; `false` when it does not serialize or no storage took it.
pub fn set_json<T: Serialize>(stone: &str, value: &T) -> bool {
    serde_json::to_string(value).is_ok_and(|json| set(stone, &json))
}
