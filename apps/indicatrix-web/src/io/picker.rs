//! File > Open: `rfd`'s async picker (on wasm32 a hidden
//! `<input type="file" multiple>`), so several files can be selected together.

use super::{IncomingFile, open_files, oversize_message};
use crate::app::{
    Ctx,
    push::{MessageKind, show_message},
};
use indicatrix_web_core::open_route::OPEN_EXTENSIONS;

/// The filter label: the design file first, then the other formats still read.
const FILTER_LABEL: &str = "Designs (.isdd, .indicatrix, .asc, .gem, .gcs) and .hdr Maps";

/// Shows the picker and opens whatever the user chose; a dismissed picker is not
/// an error and says nothing. Each file's `File.size` is checked against the page's
/// limits before its bytes are read, and an oversize file is named and skipped.
pub fn pick_and_open(ctx: &Ctx) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let Some(handles) = rfd::AsyncFileDialog::new()
            // One combined filter (`OPEN_EXTENSIONS` lists `.isdd` first, then the
            // legacy `.indicatrix`).
            .add_filter(FILTER_LABEL, &OPEN_EXTENSIONS)
            .pick_files()
            .await
        else {
            return;
        };
        let mut files = Vec::with_capacity(handles.len());
        for handle in handles {
            let name = handle.file_name();
            if let Some(message) = oversize_message(&name, handle.inner().size()) {
                show_message(&ctx, MessageKind::Error, &message);
                continue;
            }
            files.push(IncomingFile {
                name,
                bytes: handle.read().await,
            });
        }
        if !files.is_empty() {
            open_files(&ctx, files);
        }
    });
}
