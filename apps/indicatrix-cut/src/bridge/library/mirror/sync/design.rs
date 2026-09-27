//! Fetches, saves and (on an update) invalidates the stale cache of exactly one
//! mirrored design.

use crate::bridge::library::mirror::options::{LibraryTransport, MirrorCounts, MirrorOptions};
use indicatrix_net::library::{AngleSettingWire, LibraryRequest, LibraryResponse};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        angle::AngleSetting, detail::FacetDiagramDetail, entry::FacetDiagramEntry,
        file::AttachedFile, mirror::MirrorState,
    },
};
use std::sync::{Arc, Mutex, PoisonError};
use tracing::warn;

/// Fetches and saves exactly one design: `FetchDesign`, then every non-oversized
/// attachment's bytes, then one local `save_diagram_entry` + `save_diagram_detail` +
/// `upsert_mirror_state` -- all network I/O happens before the first local write, so a
/// failure partway through never touches the database (see the mirror module's own doc
/// comment's "Cancellation" section, which this same all-network-then-all-local
/// ordering also backs).
///
/// `Err(())` on any failure (fetch, or local save) -- the caller only needs to know
/// pass/fail to update [`MirrorCounts::failed`]; the specific reason isn't surfaced
/// per-design (this sync covers up to a thousand designs at once -- see the mirror
/// module doc comment's protocol-limitation note -- so a per-design error UI would be
/// noise; a failed design is simply retried on the next sync, same as one skipped for
/// looking unchanged is not).
///
/// `is_new` (from `super::pass::run_mirror_sync`'s own `existing_state.is_none()`)
/// decides whether an UPDATED design's now-stale cached preview images/tilt curves
/// are invalidated after the save below -- see that call site's own comment.
pub(super) fn sync_one_design(
    db: &Arc<Mutex<Database>>,
    transport: &impl LibraryTransport,
    source_id: &str,
    options: MirrorOptions,
    summary: &indicatrix_net::library::DesignSummary,
    is_new: bool,
    counts: &mut MirrorCounts,
) -> Result<(), ()> {
    let design = match transport.request(&LibraryRequest::FetchDesign {
        entry_id: summary.entry_id,
    }) {
        Ok(LibraryResponse::Design(d)) => *d,
        _ => return Err(()),
    };

    let mut attached_files = Vec::with_capacity(design.attachments.len());
    for meta in &design.attachments {
        if meta.size > options.max_attachment_bytes {
            counts.attachments_skipped_too_large += 1;
            continue;
        }
        match transport.request(&LibraryRequest::FetchAttachment {
            attachment_id: meta.id,
        }) {
            Ok(LibraryResponse::Attachment { name, content }) => {
                counts.attachment_bytes_fetched += content.len() as u64;
                counts.attachments_fetched += 1;
                attached_files.push(AttachedFile {
                    name,
                    url: meta.url.clone(),
                    content,
                });
            }
            Ok(LibraryResponse::NotFound) => {
                // Vanished server-side between FetchDesign and FetchAttachment --
                // save the rest of the design without this one file rather than
                // failing it outright.
            }
            _ => return Err(()),
        }
    }

    let entry = FacetDiagramEntry {
        title: design.title.clone(),
        url: design.url.clone(),
        design_id: design.design_id.clone().unwrap_or_default(),
    };
    let detail = FacetDiagramDetail {
        page_url: design.page_url.clone(),
        diagram_image_name: design.diagram_image_name.clone(),
        diagram_image_data: design.diagram_image_data.clone(),
        angle_settings_table: design.angle_settings.iter().map(to_angle_setting).collect(),
        attached_files,
        competition_diagram: design.competition_diagram.clone(),
        lw_ratio: design.lw_ratio.clone(),
        refractive_index: design.refractive_index.clone(),
        index_gear: design.index_gear.clone(),
        volume: design.volume.clone(),
        facets_count: design.facets_count.clone(),
        shape: design.shape.clone(),
        designer_info: design.designer_info.clone(),
        // `DesignRecord` carries these eight fields as of `PROTOCOL_VERSION` v6 (see
        // that constant's doc comment). They must be mapped here: `save_diagram_detail`
        // DELETES and re-inserts the whole detail row (`entries.rs`'s own doc comment),
        // so leaving one out would blank it on every sync, even for a design that
        // already had it from a prior local import/edit.
        hw_ratio: design.hw_ratio.clone(),
        tw_ratio: design.tw_ratio.clone(),
        uw_ratio: design.uw_ratio.clone(),
        pw_ratio: design.pw_ratio.clone(),
        cw_ratio: design.cw_ratio.clone(),
        symmetry_order: design.symmetry_order.clone(),
        mirror_symmetry: design.mirror_symmetry,
        designer: design.designer.clone(),
        ..FacetDiagramDetail::default()
    };

    let local_entry_id = {
        let db = db.lock().unwrap_or_else(PoisonError::into_inner);
        let Ok(id) = db.save_diagram_entry(&entry, source_id) else {
            return Err(());
        };
        if db.save_diagram_detail(&detail, id).is_err() {
            return Err(());
        }
        // An UPDATED (not new) design just had its detail row replaced with fresh
        // geometry/metadata -- any cached preview thumbnail/tilt curves rendered from
        // the OLD geometry are now stale and must be invalidated, mirroring
        // `gui::library::local::import`'s own re-import handling (`import/mod.rs`'s
        // `is_collision` branch) rather than leaving performance filters and
        // thumbnails silently describing the previous version of the design.
        if !is_new {
            if let Err(e) = db.delete_preview_images(id) {
                warn!(
                    "Mirror sync: failed to invalidate stale preview cache for entry \
                     #{id} ({}): {e}",
                    design.url
                );
            }
            if let Err(e) = db.delete_tilt_curves(id) {
                warn!(
                    "Mirror sync: failed to invalidate stale tilt-curve cache for \
                     entry #{id} ({}): {e}",
                    design.url
                );
            }
        }
        // Only recorded once the local write above actually succeeded -- a design
        // that failed to save is never marked as synced, so the next sync retries
        // it rather than silently treating a failed write as done. See this
        // function's own doc comment.
        let _ = db.upsert_mirror_state(&MirrorState {
            url: design.url.clone(),
            source_id: source_id.to_string(),
            summary_version: summary.version,
            design_version: design.version,
        });
        id
    };
    let _ = local_entry_id;

    Ok(())
}

fn to_angle_setting(a: &AngleSettingWire) -> AngleSetting {
    AngleSetting {
        order_index: a.order_index,
        facet: a.facet.clone(),
        angle: a.angle.clone(),
        index: a.index.clone(),
        notes: a.notes.clone(),
    }
}
