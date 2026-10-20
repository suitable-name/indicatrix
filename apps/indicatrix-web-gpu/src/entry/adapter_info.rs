//! What the browser itself says about the GPU (`GPUAdapterInfo`), read once at `Init`.
//!
//! Split out of `lib.rs` to keep that file under the 800-line limit; nothing here is shared
//! with the rest of the worker except `timeout_race`.

use wasm_bindgen::JsCast;

use super::{RaceResult, timeout_race};

/// What the browser says about the GPU (`GPUAdapterInfo`); empty strings when unknown.
#[derive(Default)]
pub(super) struct BrowserAdapterInfo {
    pub(super) vendor: String,
    pub(super) architecture: String,
    pub(super) device: String,
    pub(super) description: String,
}

/// Reads a string property of a JS object; empty when missing or not a string.
fn js_string(object: &wasm_bindgen::JsValue, name: &str) -> String {
    js_sys::Reflect::get(object, &wasm_bindgen::JsValue::from_str(name))
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_default()
}

/// Asks the browser for `navigator.gpu.requestAdapter()` once more and reads its `info`.
///
/// wgpu 30 reports vendor and device 0 on WebGPU and only the description, so the raw
/// `GPUAdapterInfo` is read here. It may be a different adapter than the one wgpu holds
/// (the browser's default pick) on a machine with two GPUs. Any JS failure gives `None`.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn browser_adapter_info() -> Option<BrowserAdapterInfo> {
    use wasm_bindgen::JsValue;

    let global = js_sys::global();
    let navigator = js_sys::Reflect::get(&global, &JsValue::from_str("navigator")).ok()?;
    let gpu = js_sys::Reflect::get(&navigator, &JsValue::from_str("gpu")).ok()?;
    if gpu.is_undefined() || gpu.is_null() {
        return None;
    }
    let request = js_sys::Reflect::get(&gpu, &JsValue::from_str("requestAdapter"))
        .ok()?
        .dyn_into::<js_sys::Function>()
        .ok()?;
    let promise = request
        .call0(&gpu)
        .ok()?
        .dyn_into::<js_sys::Promise>()
        .ok()?;
    let RaceResult::Completed(adapter) =
        timeout_race(wasm_bindgen_futures::JsFuture::from(promise), 2000.0).await
    else {
        return None;
    };
    let adapter = adapter.ok()?;
    if adapter.is_undefined() || adapter.is_null() {
        return None;
    }
    let info = js_sys::Reflect::get(&adapter, &JsValue::from_str("info")).ok()?;
    if info.is_undefined() || info.is_null() {
        return None;
    }
    Some(BrowserAdapterInfo {
        vendor: js_string(&info, "vendor"),
        architecture: js_string(&info, "architecture"),
        device: js_string(&info, "device"),
        description: js_string(&info, "description"),
    })
}

/// `preferred` unless it is empty.
pub(super) fn non_empty_or(preferred: String, fallback: String) -> String {
    if preferred.is_empty() {
        fallback
    } else {
        preferred
    }
}
