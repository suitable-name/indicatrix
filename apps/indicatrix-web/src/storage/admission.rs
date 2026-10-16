//! GPU admission records (the GPU track): a small record per adapter key under
//! `indicatrix.admission.<adapter>.v1` -- "this adapter compiled the kernel and may be
//! used" or "this adapter failed and must not be probed again". Stored in
//! `localStorage` so a verdict outlives the tab, with the `sessionStorage` fallback of
//! [`super::small_set`].
//!
//! Holds the typed [`AdmissionRecord`] (§6.5) with load/save/forget functions and
//! key scanning for crash markers left behind in the [`AdmissionState::Probing`] state.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use super::{keys, small_get, small_remove, small_set};

/// The admission record of one adapter, tracking its compile verdict and compile times.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AdmissionRecord {
    pub state: AdmissionState,
    pub per_kernel_ms: BTreeMap<String, f64>,
    pub max_ms: f64,
    pub total_ms: f64,
    pub reason: Option<String>,
    pub when: String,
}

/// The lifecycle state of a GPU adapter's admission.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdmissionState {
    Unknown,
    Probing,
    Ok,
    Failed,
}

impl Default for AdmissionRecord {
    fn default() -> Self {
        Self {
            state: AdmissionState::Unknown,
            per_kernel_ms: BTreeMap::new(),
            max_ms: 0.0,
            total_ms: 0.0,
            reason: None,
            when: String::new(),
        }
    }
}

impl Default for AdmissionState {
    fn default() -> Self {
        Self::Unknown
    }
}

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

/// Pure helper: returns the first key whose JSON value parses into an [`AdmissionRecord`]
/// in the [`AdmissionState::Probing`] state.
#[must_use]
pub fn find_probing_key<'a, I>(entries: I) -> Option<String>
where
    I: IntoIterator<Item = (&'a str, &'a str)>,
{
    let prefix = format!("{}{}.", keys::PREFIX, keys::ADMISSION_NAME);
    for (key, value) in entries {
        if key.starts_with(&prefix) {
            if let Ok(record) = serde_json::from_str::<AdmissionRecord>(value) {
                if record.state == AdmissionState::Probing {
                    return Some(key.to_string());
                }
            }
        }
    }
    None
}

/// Scans a `web_sys::Storage` object for any admission record in [`AdmissionState::Probing`].
fn scan_storage_for_probing(storage: &web_sys::Storage) -> Option<String> {
    let len = storage.length().ok()?;
    let prefix = format!("{}{}.", keys::PREFIX, keys::ADMISSION_NAME);
    for i in 0..len {
        if let Ok(Some(key)) = storage.key(i) {
            if key.starts_with(&prefix) {
                if let Ok(Some(value)) = storage.get_item(&key) {
                    if let Ok(record) = serde_json::from_str::<AdmissionRecord>(&value) {
                        if record.state == AdmissionState::Probing {
                            return Some(key);
                        }
                    }
                }
            }
        }
    }
    None
}

/// Scans storage for any admission record in the [`AdmissionState::Probing`] state
/// (a crash marker left behind if the browser or GPU process crashed during probing).
///
/// Returns the matching storage key, or `None` if no record is probing.
#[must_use]
pub fn any_probing() -> Option<String> {
    if let Some(window) = web_sys::window() {
        if let Ok(Some(storage)) = window.local_storage() {
            if let Some(key) = scan_storage_for_probing(&storage) {
                return Some(key);
            }
        }
        if let Ok(Some(storage)) = window.session_storage() {
            if let Some(key) = scan_storage_for_probing(&storage) {
                return Some(key);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn admission_state_serde_lowercase() {
        assert_eq!(
            serde_json::to_string(&AdmissionState::Unknown).unwrap(),
            "\"unknown\""
        );
        assert_eq!(
            serde_json::to_string(&AdmissionState::Probing).unwrap(),
            "\"probing\""
        );
        assert_eq!(
            serde_json::to_string(&AdmissionState::Ok).unwrap(),
            "\"ok\""
        );
        assert_eq!(
            serde_json::to_string(&AdmissionState::Failed).unwrap(),
            "\"failed\""
        );

        assert_eq!(
            serde_json::from_str::<AdmissionState>("\"unknown\"").unwrap(),
            AdmissionState::Unknown
        );
        assert_eq!(
            serde_json::from_str::<AdmissionState>("\"probing\"").unwrap(),
            AdmissionState::Probing
        );
        assert_eq!(
            serde_json::from_str::<AdmissionState>("\"ok\"").unwrap(),
            AdmissionState::Ok
        );
        assert_eq!(
            serde_json::from_str::<AdmissionState>("\"failed\"").unwrap(),
            AdmissionState::Failed
        );
    }

    #[test]
    fn admission_record_round_trip() {
        let mut per_kernel = BTreeMap::new();
        per_kernel.insert("ew_extend".to_string(), 12.5);
        per_kernel.insert("ew_surface".to_string(), 8.3);

        let record = AdmissionRecord {
            state: AdmissionState::Ok,
            per_kernel_ms: per_kernel,
            max_ms: 12.5,
            total_ms: 20.8,
            reason: None,
            when: "2026-10-16T12:00:00Z".to_string(),
        };

        let json = serde_json::to_string(&record).expect("serializes");
        let decoded: AdmissionRecord = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(decoded, record);
    }

    #[test]
    fn admission_record_with_reason_and_probing() {
        let record = AdmissionRecord {
            state: AdmissionState::Failed,
            per_kernel_ms: BTreeMap::new(),
            max_ms: 0.0,
            total_ms: 0.0,
            reason: Some("compile timeout".to_string()),
            when: "2026-10-16T12:05:00Z".to_string(),
        };

        let json = serde_json::to_string(&record).expect("serializes");
        let decoded: AdmissionRecord = serde_json::from_str(&json).expect("deserializes");
        assert_eq!(decoded.state, AdmissionState::Failed);
        assert_eq!(decoded.reason.as_deref(), Some("compile timeout"));
    }

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

    #[test]
    fn find_probing_key_detects_probing_record() {
        let ok_rec = AdmissionRecord {
            state: AdmissionState::Ok,
            ..AdmissionRecord::default()
        };
        let probing_rec = AdmissionRecord {
            state: AdmissionState::Probing,
            ..AdmissionRecord::default()
        };
        let ok_json = serde_json::to_string(&ok_rec).unwrap();
        let probing_json = serde_json::to_string(&probing_rec).unwrap();

        let entries = [
            ("indicatrix.settings.v1", "{}"),
            ("indicatrix.admission.adapter_a.v1", ok_json.as_str()),
            ("indicatrix.admission.adapter_b.v1", probing_json.as_str()),
        ];

        let found = find_probing_key(entries.iter().copied());
        assert_eq!(found.as_deref(), Some("indicatrix.admission.adapter_b.v1"));
    }

    #[test]
    fn find_probing_key_returns_none_when_no_probing() {
        let ok_rec = AdmissionRecord {
            state: AdmissionState::Ok,
            ..AdmissionRecord::default()
        };
        let failed_rec = AdmissionRecord {
            state: AdmissionState::Failed,
            reason: Some("crash".into()),
            ..AdmissionRecord::default()
        };
        let entries = [
            (
                "indicatrix.admission.a.v1",
                serde_json::to_string(&ok_rec).unwrap(),
            ),
            (
                "indicatrix.admission.b.v1",
                serde_json::to_string(&failed_rec).unwrap(),
            ),
        ];
        assert_eq!(
            find_probing_key(entries.iter().map(|(k, v)| (*k, v.as_str()))),
            None
        );
    }
}
