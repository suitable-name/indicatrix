//! `sessionStorage`: same-tab reload survival. It holds the autosaved current design
//! (the fast path of a reload) and is the fallback for the small keys when
//! `localStorage` is blocked. Gone when the tab closes.
//!
//! Failures are swallowed exactly like [`super::local`]'s.

fn storage() -> Option<web_sys::Storage> {
    web_sys::window()?.session_storage().ok().flatten()
}

/// Reads `key`, or `None` when absent or storage is unavailable.
#[must_use]
pub fn get(key: &str) -> Option<String> {
    storage()?.get_item(key).ok().flatten()
}

/// Writes `value` under `key`; `false` when storage is unavailable or full.
pub fn set(key: &str, value: &str) -> bool {
    storage().is_some_and(|s| s.set_item(key, value).is_ok())
}

/// Removes `key` (a missing key or unavailable storage is not an error).
pub fn remove(key: &str) {
    if let Some(storage) = storage() {
        let _ = storage.remove_item(key);
    }
}
