//! Browser downloads: a `Blob` over the bytes, an object URL, and a synthetic
//! `<a download>` click. The anchor is attached for the click (Firefox ignores a
//! click on a detached anchor) and removed at once; the object URL is revoked a
//! little later, once the browser has started reading it.

use indicatrix_formats::native::design::DESIGN_MIME_TYPE;
use std::time::Duration;
use wasm_bindgen::{JsCast, JsValue};

/// How long an object URL stays valid after its click.
const REVOKE_AFTER: Duration = Duration::from_secs(30);

/// MIME type of a `.asc` download.
pub const MIME_ASC: &str = "text/plain;charset=utf-8";
/// MIME type of a `.indicatrix` design file download.
pub const MIME_DESIGN: &str = DESIGN_MIME_TYPE;
/// MIME type of an older `.indicatrix.toml` sidecar download.
pub const MIME_TOML: &str = "application/toml;charset=utf-8";
/// MIME type of the cutting sheet.
pub const MIME_HTML: &str = "text/html;charset=utf-8";
/// MIME type of the diagram.
pub const MIME_PNG: &str = "image/png";

fn js_error(context: &str, value: &JsValue) -> String {
    format!(
        "{context}: {}",
        value.as_string().unwrap_or_else(|| format!("{value:?}"))
    )
}

/// Offers `bytes` to the user as the download `file_name`.
///
/// # Errors
///
/// A readable message when the page has no document/body or the browser refuses
/// to build the `Blob` or its URL.
pub fn download_bytes(file_name: &str, mime: &str, bytes: &[u8]) -> Result<(), String> {
    let window = web_sys::window().ok_or("No browser window to download from.")?;
    let document = window.document().ok_or("No document to download from.")?;
    let body = document
        .body()
        .ok_or("No document body to download from.")?;

    let parts = js_sys::Array::of1(&js_sys::Uint8Array::from(bytes));
    let options = web_sys::BlobPropertyBag::new();
    options.set_type(mime);
    let blob = web_sys::Blob::new_with_u8_array_sequence_and_options(&parts, &options)
        .map_err(|e| js_error("Could not build the download", &e))?;
    let url = web_sys::Url::create_object_url_with_blob(&blob)
        .map_err(|e| js_error("Could not create the download URL", &e))?;

    let anchor: web_sys::HtmlAnchorElement = document
        .create_element("a")
        .map_err(|e| js_error("Could not create the download link", &e))?
        .dyn_into()
        .map_err(|_| "The download link is not an anchor.".to_string())?;
    anchor.set_href(&url);
    anchor.set_download(file_name);
    body.append_child(&anchor)
        .map_err(|e| js_error("Could not attach the download link", &e))?;
    anchor.click();
    let _ = body.remove_child(&anchor);

    slint::Timer::single_shot(REVOKE_AFTER, move || {
        let _ = web_sys::Url::revoke_object_url(&url);
    });
    Ok(())
}
