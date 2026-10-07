//! The design detail panel: building its model from a design (local or remote),
//! editing metadata, exporting attachments, and reconstructing the 3D viewport's
//! planes -- split into [`local_load`]/[`remote_load`] (the two model-build
//! sources), [`planes`] (their shared plane/material reconstruction), [`shared`]
//! (formatting and angle-classification helpers used by both), [`metadata`]
//! (the metadata editor callback) and [`attachments`] (attachment export).

mod attachments;
mod local_load;
mod metadata;
mod planes;
mod remote_load;
mod row_cursor;
mod shared;
#[cfg(test)]
mod tests;

pub use attachments::{export_diagram_file, export_diagram_file_via_source};
pub use metadata::setup_save_metadata_callback;
pub use planes::reconstruct_planes;
pub use shared::{clear_current_detail_display, setup_filtered_row_count_callback};

use crate::{
    MainWindow,
    bridge::{library::source::LibrarySource, render_thread::RenderContext},
    gui::solid_preview::preview_state::SolidPreviewState,
};
use indicatrix_vault::db::sqlite::Database;
use std::sync::{Arc, Mutex};

/// Dispatches to [`local_load::load_diagram_detail`] (the LOCAL database lookup,
/// unchanged -- see this crate's requirement that local behaviour stay
/// byte-for-byte identical) or [`remote_load::load_diagram_detail_remote`],
/// depending on which library is currently active.
/// Every call site calls this rather than the local loader directly,
/// so `entry_id` is always interpreted against the SAME library it was listed
/// from (a remote entry id and a local row id occupy independent id spaces -- see
/// `bridge::library_mirror`'s module doc comment on identity -- so this dispatch is
/// what keeps a remote-listed id from ever being looked up against the local database
/// by mistake).
///
/// `preview_state` is only used by the REMOTE branch -- see
/// [`remote_load::load_diagram_detail_remote`]'s own doc comment for why: the local
/// branch writes `render_ctx.active_planes` synchronously, before this function
/// returns, so its own Live-Render-tab Solid-mode resubmit happens at the call site
/// instead (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`).
pub fn load_diagram_detail_via_source(
    ui: &MainWindow,
    db_mutex: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    preview_state: &Arc<SolidPreviewState>,
) {
    let current = source
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    match current {
        LibrarySource::Local => local_load::load_diagram_detail(ui, db_mutex, render_ctx, entry_id),
        LibrarySource::Remote(worker) => {
            // Bumped here, before the request is even built, so a second selection
            // (of the same or a different row, remote or local) made before this
            // fetch's reply lands is what invalidates it -- see
            // `remote_load::load_diagram_detail_remote`'s own doc comment.
            let seq = crate::gui::library::search::bump_search_seq();
            remote_load::load_diagram_detail_remote(
                ui,
                worker,
                render_ctx,
                entry_id,
                preview_state,
                Arc::clone(source),
                seq,
            );
        }
    }
}
