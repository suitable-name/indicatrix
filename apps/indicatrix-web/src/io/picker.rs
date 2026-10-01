//! File > Open: `rfd`'s async picker (on wasm32 a hidden
//! `<input type="file" multiple>`), so a native pair can be selected together.

use super::{IncomingFile, open_files, oversize_message};
use crate::app::{
    Ctx,
    push::{MessageKind, show_message},
};

/// Every extension the app opens (`indicatrix_editor::files::InputFileKind`). One
/// combined filter: the browser's `accept` list matches the last extension only,
/// so `.indicatrix.toml` is offered as `.toml`.
const OPEN_EXTENSIONS: [&str; 5] = ["asc", "toml", "gem", "gcs", "hdr"];

/// Shows the picker and opens whatever the user chose; a dismissed picker is not
/// an error and says nothing. Each file's `File.size` is checked against the page's
/// limits before its bytes are read, and an oversize file is named and skipped.
pub fn pick_and_open(ctx: &Ctx) {
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let Some(handles) = rfd::AsyncFileDialog::new()
            .add_filter(
                "Designs (.asc, .indicatrix.toml, .gem, .gcs) and .hdr maps",
                &OPEN_EXTENSIONS,
            )
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
