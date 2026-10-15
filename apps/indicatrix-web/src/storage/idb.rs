//! A minimal IndexedDB wrapper for the autosaved current design.
//!
//! One database (`indicatrix`, version 1), one object store (`designs`), one record
//! under the key `"current"`: a JS object `{ bytes: Uint8Array, meta: string }` where
//! `bytes` is the `.isdd` design file text (attachments included) and `meta` a
//! JSON [`DesignMeta`]. IndexedDB has no `sessionStorage`-style size ceiling, so the
//! whole file goes in.
//!
//! Everything is async (`wasm-bindgen-futures`) and every failure -- no IndexedDB
//! (private mode in some browsers), a blocked open, quota, an aborted transaction --
//! comes back as `Err(String)` for the caller to log; nothing here shows the user
//! anything. A write resolves when its transaction has completed, not merely when the
//! request was queued.

use js_sys::{Object, Promise, Reflect, Uint8Array};
use serde::{Deserialize, Serialize};
use wasm_bindgen::{JsCast, JsValue, closure::Closure};
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbRequest, IdbTransaction, IdbTransactionMode};

/// The database name.
pub const DB_NAME: &str = "indicatrix";
/// The schema version (bump only with an `onupgradeneeded` migration).
pub const DB_VERSION: u32 = 1;
/// The object store holding designs.
pub const STORE_DESIGNS: &str = "designs";
/// The key of the autosaved current design.
pub const CURRENT_KEY: &str = "current";

/// The small record stored beside the design bytes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DesignMeta {
    /// The design's title (`[meta] title`, else its display name).
    pub title: String,
    /// When it was written, milliseconds since the Unix epoch (`Date.now()`).
    pub saved_at_ms: f64,
    /// `EditorSession::current_generation` at write time.
    pub generation: u64,
    /// The design's recorded `.asc` name, if any.
    #[serde(default)]
    pub design_name: Option<String>,
    /// Whether the design had changes no save had captured.
    #[serde(default)]
    pub unsaved: bool,
}

/// What [`load_design`] reads back.
#[derive(Debug, Clone)]
pub struct StoredDesign {
    /// The `.isdd` file bytes (UTF-8 text).
    pub bytes: Vec<u8>,
    /// The meta record (default-filled when the stored one was unreadable).
    pub meta: Option<DesignMeta>,
}

/// A JS error value as text.
fn js_text(value: &JsValue) -> String {
    value
        .as_string()
        .or_else(|| {
            Reflect::get(value, &JsValue::from_str("message"))
                .ok()
                .and_then(|m| m.as_string())
        })
        .unwrap_or_else(|| "unknown IndexedDB error".to_string())
}

/// The failure text of a request that fired `error`.
fn request_error_text(request: &IdbRequest) -> String {
    request
        .error()
        .ok()
        .flatten()
        .map_or_else(|| "IndexedDB request failed".to_string(), |e| e.message())
}

/// Resolves with the request's result once it succeeds.
async fn request_result(request: &IdbRequest) -> Result<JsValue, String> {
    let promise = Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        let ok_request = request.clone();
        let err_request = request.clone();
        let on_success = Closure::once_into_js(move || {
            let value = ok_request.result().unwrap_or(JsValue::UNDEFINED);
            let _ = resolve.call1(&JsValue::NULL, &value);
        });
        let on_error = Closure::once_into_js(move || {
            let text = JsValue::from_str(&request_error_text(&err_request));
            let _ = reject.call1(&JsValue::NULL, &text);
        });
        request.set_onsuccess(Some(on_success.unchecked_ref()));
        request.set_onerror(Some(on_error.unchecked_ref()));
    });
    JsFuture::from(promise).await.map_err(|e| js_text(&e))
}

/// Resolves once the transaction has completed (data durable), rejects on error or abort.
async fn transaction_done(tx: &IdbTransaction) -> Result<(), String> {
    let promise = Promise::new(&mut |resolve: js_sys::Function, reject: js_sys::Function| {
        let on_complete = Closure::once_into_js(move || {
            let _ = resolve.call0(&JsValue::NULL);
        });
        let err_tx = tx.clone();
        let reject_error = reject.clone();
        let on_error = Closure::once_into_js(move || {
            let text = err_tx.error().map_or_else(
                || "IndexedDB transaction failed".to_string(),
                |e| e.message(),
            );
            let _ = reject_error.call1(&JsValue::NULL, &JsValue::from_str(&text));
        });
        let on_abort = Closure::once_into_js(move || {
            let _ = reject.call1(
                &JsValue::NULL,
                &JsValue::from_str("IndexedDB transaction aborted"),
            );
        });
        tx.set_oncomplete(Some(on_complete.unchecked_ref()));
        tx.set_onerror(Some(on_error.unchecked_ref()));
        tx.set_onabort(Some(on_abort.unchecked_ref()));
    });
    JsFuture::from(promise)
        .await
        .map(|_| ())
        .map_err(|e| js_text(&e))
}

/// Opens (creating on first use) the `indicatrix` database.
async fn open_db() -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .ok_or_else(|| "no window".to_string())?
        .indexed_db()
        .map_err(|e| js_text(&e))?
        .ok_or_else(|| "IndexedDB is unavailable".to_string())?;
    let open = factory
        .open_with_u32(DB_NAME, DB_VERSION)
        .map_err(|e| js_text(&e))?;
    let upgrade_request = open.clone();
    let on_upgrade = Closure::once_into_js(move || {
        if let Ok(result) = upgrade_request.result() {
            let db: IdbDatabase = result.unchecked_into();
            // Version 1 is the first schema: the store never exists yet.
            let _ = db.create_object_store(STORE_DESIGNS);
        }
    });
    open.set_onupgradeneeded(Some(on_upgrade.unchecked_ref()));
    let result = request_result(&open).await?;
    result
        .dyn_into::<IdbDatabase>()
        .map_err(|_| "IndexedDB open returned no database".to_string())
}

/// Writes the autosaved design (replacing the previous one).
///
/// # Errors
///
/// IndexedDB is unavailable, the open was blocked, the value could not be built, or the
/// transaction failed (quota included).
pub async fn save_design(bytes: &[u8], meta: &DesignMeta) -> Result<(), String> {
    let meta_json = serde_json::to_string(meta).map_err(|e| e.to_string())?;
    let value = Object::new();
    Reflect::set(&value, &"bytes".into(), &Uint8Array::from(bytes)).map_err(|e| js_text(&e))?;
    Reflect::set(&value, &"meta".into(), &JsValue::from_str(&meta_json))
        .map_err(|e| js_text(&e))?;

    let db = open_db().await?;
    let outcome = async {
        let tx = db
            .transaction_with_str_and_mode(STORE_DESIGNS, IdbTransactionMode::Readwrite)
            .map_err(|e| js_text(&e))?;
        let store = tx.object_store(STORE_DESIGNS).map_err(|e| js_text(&e))?;
        store
            .put_with_key(&value, &JsValue::from_str(CURRENT_KEY))
            .map_err(|e| js_text(&e))?;
        transaction_done(&tx).await
    }
    .await;
    db.close();
    outcome
}

/// Reads the autosaved design, `Ok(None)` when there is none.
///
/// # Errors
///
/// IndexedDB is unavailable or the read failed. A record that is present but not shaped
/// like ours is an `Err` too (the caller treats it as "no usable design").
pub async fn load_design() -> Result<Option<StoredDesign>, String> {
    let db = open_db().await?;
    let outcome = async {
        let tx = db
            .transaction_with_str_and_mode(STORE_DESIGNS, IdbTransactionMode::Readonly)
            .map_err(|e| js_text(&e))?;
        let store = tx.object_store(STORE_DESIGNS).map_err(|e| js_text(&e))?;
        let request = store
            .get(&JsValue::from_str(CURRENT_KEY))
            .map_err(|e| js_text(&e))?;
        request_result(&request).await
    }
    .await;
    db.close();
    let record = outcome?;
    if record.is_undefined() || record.is_null() {
        return Ok(None);
    }
    let bytes = Reflect::get(&record, &"bytes".into())
        .map_err(|e| js_text(&e))?
        .dyn_into::<Uint8Array>()
        .map_err(|_| "stored design has no byte array".to_string())?
        .to_vec();
    let meta = Reflect::get(&record, &"meta".into())
        .ok()
        .and_then(|m| m.as_string())
        .and_then(|text| serde_json::from_str::<DesignMeta>(&text).ok());
    Ok(Some(StoredDesign { bytes, meta }))
}

/// Deletes the autosaved design.
///
/// # Errors
///
/// IndexedDB is unavailable or the transaction failed.
pub async fn clear_design() -> Result<(), String> {
    let db = open_db().await?;
    let outcome = async {
        let tx = db
            .transaction_with_str_and_mode(STORE_DESIGNS, IdbTransactionMode::Readwrite)
            .map_err(|e| js_text(&e))?;
        let store = tx.object_store(STORE_DESIGNS).map_err(|e| js_text(&e))?;
        store
            .delete(&JsValue::from_str(CURRENT_KEY))
            .map_err(|e| js_text(&e))?;
        transaction_done(&tx).await
    }
    .await;
    db.close();
    outcome
}
