//! GPU admission records (the GPU track): a small record per adapter key under
//! `indicatrix.admission.<adapter>.v1` -- "this adapter compiled the kernel and may be
//! used" or "this adapter failed and must not be probed again". Stored in
//! `localStorage` (remembered in this browser, per graphics card, browser version and
//! shader build) with the `sessionStorage` fallback of [`super::small_set`].
//!
//! The typed record and the crash-marker scan are pure and live in
//! `indicatrix_web_core::gpu_admission` (native tests); this module reads and writes them
//! through the small-key policy and scans the two storages for crash markers left behind
//! in the [`AdmissionState::Probing`] state.

use serde::{Serialize, de::DeserializeOwned};

use indicatrix_web_core::gpu_admission::probing_keys;
pub use indicatrix_web_core::gpu_admission::{AdmissionRecord, AdmissionState};

use super::{keys, small_get, small_remove, small_set};

/// Normalizes `adapter_or_key` into a full storage key.
///
/// If `adapter_or_key` already begins with [`keys::PREFIX`], it is returned unchanged;
/// otherwise [`keys::admission_key`] formats it into `indicatrix.admission.<adapter>.v1`.
#[must_use]
pub fn resolve_key(adapter_or_key: &str) -> String {
    if adapter_or_key.starts_with(keys::PREFIX) {
        adapter_or_key.to_string()
    } else {
        keys::admission_key(adapter_or_key)
    }
}

/// The stored record text of `adapter_or_key`, or `None` when absent.
#[must_use]
pub fn get(adapter_or_key: &str) -> Option<String> {
    small_get(&resolve_key(adapter_or_key))
}

/// Stores `json` as the record of `adapter_or_key`; `false` when no storage took it.
pub fn set(adapter_or_key: &str, json: &str) -> bool {
    small_set(&resolve_key(adapter_or_key), json)
}

/// Forgets the record of `adapter_or_key` (lets it be probed again).
pub fn remove(adapter_or_key: &str) {
    small_remove(&resolve_key(adapter_or_key));
}

/// The record of `adapter_or_key` parsed as `T`; `None` when absent or unparsable.
#[must_use]
pub fn get_json<T: DeserializeOwned>(adapter_or_key: &str) -> Option<T> {
    serde_json::from_str(&get(adapter_or_key)?).ok()
}

/// Stores `value` as JSON; `false` when it does not serialize or no storage took it.
pub fn set_json<T: Serialize>(adapter_or_key: &str, value: &T) -> bool {
    serde_json::to_string(value).is_ok_and(|json| set(adapter_or_key, &json))
}

/// Loads the [`AdmissionRecord`] of `adapter_or_key`, or `None` when absent or unparsable.
#[must_use]
pub fn load(adapter_or_key: &str) -> Option<AdmissionRecord> {
    get_json(adapter_or_key)
}

/// Stores `record` as the admission record of `adapter_or_key`; `false` when storage failed.
pub fn save(adapter_or_key: &str, record: &AdmissionRecord) -> bool {
    set_json(adapter_or_key, record)
}

/// Forgets the admission record of `adapter_or_key` (lets it be probed again).
pub fn forget(adapter_or_key: &str) {
    remove(adapter_or_key);
}

/// Every admission key (in either storage) whose record is in the
/// [`AdmissionState::Probing`] state: crash markers left behind if the browser or GPU
/// process crashed during a test or a compile.
#[must_use]
pub fn all_probing() -> Vec<String> {
    let mut found = Vec::new();
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            found.extend(scan_storage_for_probing(&storage));
        }
        if let Ok(Some(storage)) = window.session_storage() {
            found.extend(scan_storage_for_probing(&storage));
        }
    }
    found.sort();
    found.dedup();
    found
}

/// The probing admission keys of one `web_sys::Storage`.
fn scan_storage_for_probing(storage: &web_sys::Storage) -> Vec<String> {
    let Ok(len) = storage.length() else {
        return Vec::new();
    };
    let mut entries: Vec<(String, String)> = Vec::new();
    for i in 0..len {
        if let Ok(Some(key)) = storage.key(i)
            && let Ok(Some(value)) = storage.get_item(&key)
        {
            entries.push((key, value));
        }
    }
    probing_keys(entries.iter().map(|(k, v)| (k.as_str(), v.as_str())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_key_handles_both_adapter_and_full_key() {
        assert_eq!(
            resolve_key("intel_arc"),
            "indicatrix.admission.intel_arc.v1"
        );
        assert_eq!(
            resolve_key("indicatrix.admission.custom.v1"),
            "indicatrix.admission.custom.v1"
        );
    }
}
