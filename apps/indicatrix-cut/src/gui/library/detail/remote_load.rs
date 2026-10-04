//! Builds the detail pane and 3D viewport from a design fetched off a REMOTE
//! worker's library.

use super::{
    planes::{ReconstructedPlanesInput, apply_reconstructed_planes},
    shared::{catalogue_material_guess, format_optional_proportion, sides_from_rows},
};
use crate::{
    AngleItem, DiagramDetailData, FileItem, LibraryModel, MainWindow, TiltModel, ViewportModel,
    bridge::{
        library::source::{self as library_source, LibrarySource},
        render_thread::RenderContext,
    },
    gui::{
        render::camera_lighting::resubmit_live_solid,
        solid_preview::preview_state::SolidPreviewState,
    },
    settings::WorkerSettings,
};
use indicatrix_net::library::{DesignRecord, LibraryRequest, LibraryResponse};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::sync::{Arc, Mutex};
use tracing::error;

/// The remote counterpart of `super::local_load::load_diagram_detail`: fetches one
/// design's full record over the network (`LibraryRequest::FetchDesign`, off the UI
/// thread -- see `bridge::library_source`'s module doc comment) and applies it to the
/// SAME `DiagramDetailData`/`AngleItem`/`FileItem`/render-context fields the local path
/// populates, via [`apply_design_record_to_ui`], so the detail panel and 3D viewport
/// behave identically regardless of source.
///
/// [`FileItem::size_str`] is built from the attachment's advertised
/// [`indicatrix_net::library::AttachedFileMeta::size`] here (never its content -- the library
/// protocol deliberately never inlines attachment bytes into a `FetchDesign` reply, see
/// `indicatrix_net::library`'s module doc comment's "Attachments" section); the actual bytes
/// are fetched lazily, only if the user exports that specific file (see
/// `super::attachments::export_diagram_file_via_source`).
///
/// `preview_state` is forwarded to [`apply_design_record_to_ui`], not used here
/// directly -- it's only needed once the design's record (and its planes) actually
/// arrive, inside the completion closure below.
///
/// # Staleness guard
///
/// `source`/`seq` guard against exactly this bug: a slow
/// `FetchDesign` reply landing after the cutter has already selected a different row,
/// or switched away from this remote worker entirely (back to Local, or to another
/// remote), must never re-bind `LibraryModel.selected_entry_id`/`current_detail` to
/// stale remote data -- `local::organize`'s ignore/delete callbacks guard only on
/// `source.is_remote()`, so a stale reply that slipped through here would hand them a
/// REMOTE entry id while `source` (correctly) still reads Local, and they would act on
/// the local database with it. `seq` is captured by the caller
/// (`load_diagram_detail_via_source`) BEFORE this request is even built, and `source`
/// is re-read fresh at reply time -- see [`reply_is_current`] for the actual
/// (pure, unit-tested) predicate.
pub(super) fn load_diagram_detail_remote(
    ui: &MainWindow,
    worker: WorkerSettings,
    render_ctx: &Arc<Mutex<RenderContext>>,
    entry_id: i64,
    preview_state: &Arc<SolidPreviewState>,
    source: Arc<Mutex<LibrarySource>>,
    seq: u64,
) {
    let render_ctx = render_ctx.clone();
    let preview_state = Arc::clone(preview_state);
    let request_source = LibrarySource::Remote(worker.clone());
    library_source::spawn_library_request(
        ui.as_weak(),
        worker,
        LibraryRequest::FetchDesign { entry_id },
        move |ui, result| {
            let current_source = source
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone();
            if !reply_is_current(
                crate::gui::library::search::is_current_search(seq),
                &request_source,
                &current_source,
            ) {
                return;
            }
            match result {
                Ok(LibraryResponse::Design(record)) => {
                    apply_design_record_to_ui(ui, &render_ctx, &record, &preview_state);
                }
                Ok(LibraryResponse::NotFound) => {
                    ui.global::<LibraryModel>().set_status_message(
                        "Diagram detail not found on the remote library.".into(),
                    );
                }
                Ok(_) => {
                    ui.global::<LibraryModel>().set_status_message(
                        "Unexpected reply fetching remote diagram detail.".into(),
                    );
                }
                Err(e) => {
                    error!("Remote FetchDesign failed: {e}");
                    ui.global::<LibraryModel>()
                        .set_status_message(format!("Error loading remote detail: {e}").into());
                }
            }
        },
    );
}

/// Whether a remote-detail reply is still safe to apply to the UI: `false` once a
/// newer request has superseded this one (`seq_is_current` false), or the active
/// library source has since moved away from `request_source` (switched to local, or to
/// a different remote worker) even if `seq` itself is still current -- e.g. a switch
/// implemented as a straight assignment rather than through the same `SEARCH_SEQ`
/// counter. Pulled out as a pure function (no thread-local, no `Arc<Mutex<_>>`) so both
/// ways a reply can go stale are unit-tested directly, without a live network round
/// trip or Slint event loop -- see the tests below.
#[must_use]
fn reply_is_current(
    seq_is_current: bool,
    request_source: &LibrarySource,
    current_source: &LibrarySource,
) -> bool {
    seq_is_current && request_source == current_source
}

fn apply_design_record_to_ui(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    record: &DesignRecord,
    preview_state: &Arc<SolidPreviewState>,
) {
    let is_local = record.url.starts_with("local://");
    let detail_data = DiagramDetailData {
        id: record.entry_id as i32,
        title: record.title.clone().into(),
        url: record.url.clone().into(),
        designer: record.designer_info.clone().unwrap_or_default().into(),
        shape: record.shape.clone().unwrap_or_default().into(),
        gear: record.index_gear.clone().unwrap_or_default().into(),
        facets: record.facets_count.clone().unwrap_or_default().into(),
        lw_ratio: format_optional_proportion(record.lw_ratio.as_deref()).into(),
        material_guess: catalogue_material_guess(record.refractive_index.as_deref()).into(),
        ri: record.refractive_index.clone().unwrap_or_default().into(),
        volume: format_optional_proportion(record.volume.as_deref()).into(),
        competition: record
            .competition_diagram
            .clone()
            .unwrap_or_default()
            .into(),
        image_name: record.diagram_image_name.clone().unwrap_or_default().into(),
        has_image: record.diagram_image_data.is_some(),
        is_local,
        // `indicatrix_net::library::DesignRecord` carries these as of `PROTOCOL_VERSION`
        // v6 (see that constant's doc comment for the full story), formatted the same
        // way as the local path via the same [`format_optional_proportion`], so remote
        // and local render identically. Metadata editing itself stays local-only (gated
        // on `detail.is_local` in `detail_header.slint`, same as rename/shape already
        // are) -- these chips are populated for a remote design, but not editable
        // there.
        hw_ratio: format_optional_proportion(record.hw_ratio.as_deref()).into(),
        cw_ratio: format_optional_proportion(record.cw_ratio.as_deref()).into(),
        pw_ratio: format_optional_proportion(record.pw_ratio.as_deref()).into(),
        symmetry_order: record.symmetry_order.clone().unwrap_or_default().into(),
        mirror_symmetry: record.mirror_symmetry.unwrap_or(false),
    };
    ui.global::<LibraryModel>().set_current_detail(detail_data);
    // The remote counterpart of `load_diagram_detail`'s own cached-material read, via
    // `DesignRecord::preview_material` (added in `PROTOCOL_VERSION` v5). An absent
    // value means "nothing generated yet, nothing to be stale against" -- the same
    // thing `cached_curve_material_is_stale` documents as "never stale" -- so the Tilt
    // Performance dialog's "re-render with the current material?" offer only appears
    // once a remote design actually has a cached material to compare against.
    ui.global::<TiltModel>()
        .set_cached_curve_material(record.preview_material.clone().unwrap_or_default().into());
    // Provenance is a local-catalogue-only concept (same as tags) -- a remote-browsed
    // design never gets the "Derived from" status note `load_diagram_detail` (the
    // local route) sets.

    // See `apply_reconstructed_planes`'s call site in `load_diagram_detail`
    // for the local path's identical treatment -- `sides_from_rows` is shared by
    // both so the two can never disagree about a design's own sides.
    let sides = sides_from_rows(
        record
            .angle_settings
            .iter()
            .map(|a| (a.facet.as_str(), a.angle.as_str(), a.index.as_str())),
    );
    let angle_items: Vec<AngleItem> = record
        .angle_settings
        .iter()
        .zip(sides)
        .map(|(a, side)| AngleItem {
            order_idx: a.order_index as i32,
            side,
            facet: a.facet.clone().into(),
            angle: a.angle.trim_start_matches('-').to_string().into(),
            index_val: a.index.clone().into(),
            notes: a.notes.clone().into(),
            second_line: a.tool_line.clone().unwrap_or_default().into(),
        })
        .collect();
    ui.global::<LibraryModel>()
        .set_current_angles(ModelRc::new(VecModel::from(angle_items.clone())));

    let file_items: Vec<FileItem> = record
        .attachments
        .iter()
        .map(|f| {
            let size_kb = f.size as f64 / 1024.0;
            FileItem {
                name: f.name.clone().into(),
                url: f.url.clone().into(),
                size_str: format!("{size_kb:.1} KB").into(),
            }
        })
        .collect();
    ui.global::<LibraryModel>()
        .set_current_files(ModelRc::new(VecModel::from(file_items)));
    ui.global::<LibraryModel>()
        .set_selected_entry_id(record.entry_id as i32);

    apply_reconstructed_planes(
        ui,
        render_ctx,
        record.entry_id,
        ReconstructedPlanesInput {
            shape: record.shape.as_deref(),
            index_gear: record.index_gear.as_deref(),
            angle_items: &angle_items,
            refractive_index: record.refractive_index.as_deref(),
            // The remote counterpart of `load_diagram_detail`'s own
            // `cached_curve_material` -- `record.preview_material` is exactly the
            // value this function already read into `TiltModel.cached_curve_material`
            // a few lines up.
            preview_material: record.preview_material.as_deref(),
            // A remote `DesignRecord` never carries attachment
            // BYTES (see this function's own doc comment) -- there is no real `.asc`
            // text here to resolve a `Design` from, so this route stays on the
            // placeholder reconstruction. Fixing that needs a wire-protocol change
            // (`indicatrix-net`/`indicatrix-worker`), out of scope for this crate
            // alone.
            real_design: None,
        },
    );

    // The remote counterpart of the local selection callback's own resubmit
    // (`gui::library::diagram_list::setup_diagram_selection_and_export_callbacks`) --
    // needed here specifically because THIS path is async: the local branch's planes
    // land synchronously before its caller returns, but this closure is where a
    // remote design's planes actually arrive. Only fires when the Live Render tab is
    // both showing (`render_view_tab == 0`) and in Solid mode (`live_view_mode == 0`)
    // -- the same condition the local path's own resubmit checks, so a remote
    // selection redraws the solid immediately instead of leaving the previous
    // design's stale raster on screen until the next camera drag.
    if ui.get_render_view_tab() == 0 && ui.global::<ViewportModel>().get_live_view_mode() == 0 {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        resubmit_live_solid(ui, &ctx, preview_state);
    }
}

#[cfg(test)]
mod reply_is_current_tests {
    use super::{LibrarySource, reply_is_current};
    use crate::settings::WorkerSettings;

    fn worker(address: &str) -> WorkerSettings {
        WorkerSettings {
            address: address.to_string(),
            ..WorkerSettings::default()
        }
    }

    #[test]
    fn current_seq_and_matching_remote_source_is_current() {
        let source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(reply_is_current(true, &source, &source));
    }

    #[test]
    fn a_superseded_seq_is_never_current_even_with_a_matching_source() {
        let source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(!reply_is_current(false, &source, &source));
    }

    #[test]
    fn switching_to_local_before_the_reply_lands_is_never_current() {
        let request_source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(!reply_is_current(
            true,
            &request_source,
            &LibrarySource::Local
        ));
    }

    #[test]
    fn switching_to_a_different_remote_worker_before_the_reply_lands_is_never_current() {
        let request_source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        let current_source = LibrarySource::Remote(worker("10.0.0.6:9443"));
        assert!(!reply_is_current(true, &request_source, &current_source));
    }

    #[test]
    fn a_stale_seq_and_a_moved_source_together_are_still_never_current() {
        let request_source = LibrarySource::Remote(worker("10.0.0.5:9443"));
        assert!(!reply_is_current(
            false,
            &request_source,
            &LibrarySource::Local
        ));
    }
}
