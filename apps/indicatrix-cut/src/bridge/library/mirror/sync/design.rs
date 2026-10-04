//! Fetches, saves and (on an update) invalidates the stale cache of exactly one
//! mirrored design.

use crate::bridge::library::mirror::options::{LibraryTransport, MirrorCounts, MirrorOptions};
use indicatrix_net::library::{AngleSettingWire, DesignRecord, LibraryRequest, LibraryResponse};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        angle::AngleSetting, detail::FacetingDiagramDetail, entry::FacetingDiagramEntry,
        file::AttachedFile, mirror::MirrorState,
    },
};
use std::sync::{Arc, Mutex, PoisonError};
use tracing::warn;

/// How [`sync_one_design`] handled exactly one design -- distinguishes an ordinary
/// save from the local-row guard below, since only the former counts toward
/// [`MirrorCounts::new_count`]/[`MirrorCounts::updated_count`].
pub(super) enum SyncedDesign {
    /// `entry`/`detail`/mirror state were saved (new or updated).
    Saved,
    /// This design's `url` already names a local row this sync has never mirrored --
    /// see [`sync_one_design`]'s own doc comment. Nothing was written locally.
    SkippedLocalConflict,
}

/// Fetches and saves exactly one design: `FetchDesign`, then every non-oversized
/// attachment's bytes, then one atomic local `save_design` (entry and detail together)
/// and `upsert_mirror_state` -- all network I/O happens before the first local write, so a
/// failure partway through never touches the database (see the mirror module's own doc
/// comment's "Cancellation" section, which this same all-network-then-all-local
/// ordering also backs).
///
/// # The local-row guard: never overwrite a local row
///
/// A remote design's `url` can collide with a row this database already has that was
/// NEVER mirrored from any remote -- a worker serving its own `local://` import, or a
/// hand-imported `.asc` that happens to land on the same synthetic URL. Before saving,
/// [`is_unmirrored_local_row`] checks whether a local row already exists for
/// `design.url` and, if so, whether it is actually a previously-mirrored row. A
/// pre-existing row with no mirror state is left completely untouched --
/// [`SyncedDesign::SkippedLocalConflict`] -- matching the mirror module's own
/// "additive/update-only" rule. Only checked when `is_new` is true: when it's `false`,
/// `super::pass::run_mirror_sync` already found a mirror-state row for this URL itself,
/// so the row is known-mirrored without a second lookup here. That pass also skips a
/// tombstoned state (a locally deleted design) before ever calling this function, so an
/// `!is_new` call is always for a design the user has not deleted.
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
) -> Result<SyncedDesign, ()> {
    let design = match transport.request(&LibraryRequest::FetchDesign {
        entry_id: summary.entry_id,
    }) {
        Ok(LibraryResponse::Design(d)) => *d,
        _ => return Err(()),
    };

    let attached_files = fetch_attachments(transport, &design, options, counts)?;
    let entry = FacetingDiagramEntry {
        title: design.title.clone(),
        url: design.url.clone(),
        design_id: design.design_id.clone().unwrap_or_default(),
    };
    let detail = build_detail(&design, attached_files, summary.concave_tiers);

    let db = db.lock().unwrap_or_else(PoisonError::into_inner);

    // Local-row guard: never overwrite a local row this sync has never mirrored -- see this
    // function's own doc comment.
    if is_new && is_unmirrored_local_row(&db, &design.url) {
        warn!(
            "Mirror sync: leaving local entry ({}) untouched -- it was never mirrored \
             from this or any remote (local-row guard)",
            design.url
        );
        return Ok(SyncedDesign::SkippedLocalConflict);
    }

    // One transaction for entry and detail: a failure between the two must not leave a
    // detail-less row that the local-row guard above would then skip forever.
    let Ok(id) = db.save_design(&entry, &detail, source_id) else {
        return Err(());
    };
    // An UPDATED (not new) design just had its detail row replaced with fresh
    // geometry/metadata -- any cached preview thumbnail/tilt curves rendered from the
    // OLD geometry are now stale and must be invalidated, mirroring
    // `gui::library::local::import`'s own re-import handling (`import/mod.rs`'s
    // `is_collision` branch) rather than leaving performance filters and thumbnails
    // silently describing the previous version of the design.
    if !is_new {
        if let Err(e) = db.delete_preview_images(id) {
            warn!(
                "Mirror sync: failed to invalidate stale preview cache for entry #{id} \
                 ({}): {e}",
                design.url
            );
        }
        if let Err(e) = db.delete_tilt_curves(id) {
            warn!(
                "Mirror sync: failed to invalidate stale tilt-curve cache for entry \
                 #{id} ({}): {e}",
                design.url
            );
        }
        if let Err(e) = db.delete_solid_extents(id) {
            warn!(
                "Mirror sync: failed to invalidate stale solid-extents cache for entry \
                 #{id} ({}): {e}",
                design.url
            );
        }
    }
    // Only recorded once the local write above actually succeeded -- a design that
    // failed to save is never marked as synced, so the next sync retries it rather than
    // silently treating a failed write as done. See this function's own doc comment.
    let _ = db.upsert_mirror_state(&MirrorState {
        url: design.url.clone(),
        source_id: source_id.to_string(),
        summary_version: summary.version,
        design_version: design.version,
        deleted_locally: false,
    });
    drop(db);

    Ok(SyncedDesign::Saved)
}

/// Fetches every one of `design`'s attachments whose advertised size doesn't exceed
/// `options.max_attachment_bytes`, tallying skips/fetches/bytes into `counts` as it
/// goes -- pulled out of [`sync_one_design`] purely to keep that function's length
/// down; see its own doc comment for the full per-design fetch/save shape this is one
/// step of.
fn fetch_attachments(
    transport: &impl LibraryTransport,
    design: &DesignRecord,
    options: MirrorOptions,
    counts: &mut MirrorCounts,
) -> Result<Vec<AttachedFile>, ()> {
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
                // Vanished server-side between FetchDesign and FetchAttachment -- save
                // the rest of the design without this one file rather than failing it
                // outright.
            }
            _ => return Err(()),
        }
    }
    Ok(attached_files)
}

/// Builds the [`FacetingDiagramDetail`] `save_diagram_detail` will fully replace the
/// existing row with (see that method's own doc comment) -- pulled out of
/// [`sync_one_design`] purely to keep that function's length down.
fn build_detail(
    design: &DesignRecord,
    attached_files: Vec<AttachedFile>,
    concave_tiers: u32,
) -> FacetingDiagramDetail {
    let angle_settings_table: Vec<AngleSetting> =
        design.angle_settings.iter().map(to_angle_setting).collect();
    // The wire carries no placement count; a concave row's index text lists them.
    let concave_facets = angle_settings_table
        .iter()
        .filter(|a| a.tool.is_some())
        .map(|a| a.index.split(',').filter(|i| !i.trim().is_empty()).count() as u32)
        .sum();
    FacetingDiagramDetail {
        page_url: design.page_url.clone(),
        diagram_image_name: design.diagram_image_name.clone(),
        diagram_image_data: design.diagram_image_data.clone(),
        angle_settings_table,
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
        // Added to `DesignRecord` under `PROTOCOL_VERSION` v15 -- same "must be mapped
        // here or a re-sync blanks it" reasoning as the eight fields above. This now
        // sets every `FacetingDiagramDetail` field there is, so no `..default()` remains.
        source_citation: design.source_citation.clone(),
        pdf_file: design.pdf_file.clone(),
        gem_file: design.gem_file.clone(),
        shape_category: design.shape_category.clone(),
        concave_tiers,
        concave_facets,
    }
}

/// Whether `url` already names a local row that was NEVER put there by a mirror sync
/// (no `library_mirror_state` entry for it) -- the local-row guard's own predicate, pulled
/// out so [`sync_one_design`]'s doc comment has one thing to point at. `false` for a
/// brand-new URL (nothing to conflict with) and `false` for a row `library_mirror_state`
/// confirms was mirrored before (safe to update).
fn is_unmirrored_local_row(db: &Database, url: &str) -> bool {
    let Ok(Some(_)) = db.diagram_entry_id_for_url(url) else {
        return false;
    };
    db.get_mirror_state(url).ok().flatten().is_none()
}

fn to_angle_setting(a: &AngleSettingWire) -> AngleSetting {
    AngleSetting {
        order_index: a.order_index,
        facet: a.facet.clone(),
        angle: a.angle.clone(),
        index: a.index.clone(),
        notes: a.notes.clone(),
        // The wire keeps the formatted tool line only; its first column is the tool code.
        tool: a
            .tool_line
            .as_deref()
            .and_then(|line| line.split_whitespace().next())
            .map(str::to_owned),
        tool_line: a.tool_line.clone(),
    }
}
