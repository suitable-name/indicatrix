//! Drag-and-drop onto the page's `<canvas id="canvas">` (the element Slint renders
//! into): `dragover` is accepted (`preventDefault`, or the browser navigates to the
//! file instead), and `drop` reads every dropped file with `File.arrayBuffer()` -- after
//! checking `File.size` against the page's limits, so an oversize file is never copied
//! into memory -- and hands them to [`open_files`] together, exactly like a multi-file
//! pick.

use super::{IncomingFile, open_files, oversize_message};
use crate::app::{
    Ctx,
    diagnostics::console_warn,
    push::{MessageKind, show_message},
};
use wasm_bindgen::{JsCast, closure::Closure};

/// Reads every file of a drop (`File.arrayBuffer()`), then opens them together.
fn handle_drop(ctx: &Ctx, event: &web_sys::DragEvent) {
    event.prevent_default();
    let Some(list) = event.data_transfer().and_then(|dt| dt.files()) else {
        return;
    };
    let files: Vec<web_sys::File> = (0..list.length()).filter_map(|i| list.get(i)).collect();
    if files.is_empty() {
        return;
    }
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let mut incoming = Vec::with_capacity(files.len());
        for file in files {
            let name = file.name();
            if let Some(message) = oversize_message(&name, file.size()) {
                show_message(&ctx, MessageKind::Error, &message);
                continue;
            }
            match wasm_bindgen_futures::JsFuture::from(file.array_buffer()).await {
                Ok(buffer) => incoming.push(IncomingFile {
                    name,
                    bytes: js_sys::Uint8Array::new(&buffer).to_vec(),
                }),
                Err(_) => show_message(
                    &ctx,
                    MessageKind::Error,
                    &format!("Could not read the dropped file \"{name}\"."),
                ),
            }
        }
        if !incoming.is_empty() {
            open_files(&ctx, incoming);
        }
    });
}

/// Installs the `dragover`/`drop` listeners on `#canvas`. The closures live for
/// the page's lifetime, so they are leaked with `forget()`. A page without the
/// canvas (never the case with this app's `index.html`) just has no drop target.
pub fn install_drop_target(ctx: &Ctx) {
    let Some(canvas) = web_sys::window()
        .and_then(|w| w.document())
        .and_then(|d| d.get_element_by_id("canvas"))
    else {
        console_warn("no #canvas element; drag-and-drop is unavailable");
        return;
    };

    let on_dragover = Closure::wrap(Box::new(|event: web_sys::DragEvent| {
        event.prevent_default();
        if let Some(transfer) = event.data_transfer() {
            transfer.set_drop_effect("copy");
        }
    }) as Box<dyn FnMut(web_sys::DragEvent)>);
    let drop_ctx = ctx.clone();
    let on_drop = Closure::wrap(Box::new(move |event: web_sys::DragEvent| {
        handle_drop(&drop_ctx, &event);
    }) as Box<dyn FnMut(web_sys::DragEvent)>);

    let target: &web_sys::EventTarget = canvas.as_ref();
    let added = target
        .add_event_listener_with_callback("dragover", on_dragover.as_ref().unchecked_ref())
        .and_then(|()| {
            target.add_event_listener_with_callback("drop", on_drop.as_ref().unchecked_ref())
        });
    if added.is_err() {
        console_warn("could not install the drag-and-drop listeners");
    }
    on_dragover.forget();
    on_drop.forget();
}
