//! Serves `indicatrix_net::library`'s read-only design-library protocol.
//!
//! Backed by a `indicatrix_vault::db::sqlite::Database` -- the shared handler both a
//! library-only build and a `worker` build call for `ClientMessage::Library`.
//!
//! [`handle_request`] is the one entry point: given one `LibraryRequest` and the
//! `Database` opened at startup, it returns exactly one `LibraryResponse`
//! (request/response, never streamed). It never panics on a database error or an
//! unknown id: a query failure becomes `LibraryResponse::Error` (logged in full
//! server-side, reported to the peer only as a generic message), and a
//! `FetchDesign`/`FetchAttachment` for an unmatched id becomes `LibraryResponse::NotFound`.
//!
//! # Versioning: a content hash computed here, not stored in the database
//!
//! `DesignSummary::version`/`DesignRecord::version` are SHA-256 hashes computed at
//! response time (see [`hash_summary`]/[`hash_record`]) over exactly the fields that
//! response carries, in a fixed order, each length-prefixed so adjacent fields can
//! never collide, and each `Option` tagged present/absent before its value.
//! `indicatrix_vault`'s schema has no `updated_at`/revision column to read instead.

use indicatrix_net::{
    library::{
        AngleSettingWire, AttachedFileMeta, AttributeRangesWire, DesignRecord, DesignSummary,
        LibraryRequest, LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire,
        PerformanceFilterWire, PerformanceMetricWire, RangeFilterWire, SortOrderWire,
    },
    messages::ErrorMsg,
};
use indicatrix_vault::{
    db::sqlite::{Database, DisplayFilters, SEARCH_RESULT_CAP, SortOrder},
    model::{
        entry::{DiagramListItem, FullDiagramMeta},
        filter::{AttributeRanges, RangeFilter},
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};
use sha2::{Digest, Sha256};

/// `<- ERROR` code for a [`LibraryRequest`] this worker refused before touching the
/// database (currently only an out-of-range `tilt_radius_deg`, see [`from_range_wire`]).
///
/// Distinct from [`LIBRARY_ERROR_CODE`] so a client can, in principle, tell "malformed
/// request" apart from "server hit a problem".
pub const LIBRARY_VALIDATION_ERROR_CODE: u32 = 5;

/// `<- ERROR` code for a `LibraryRequest` that failed on this worker's side.
///
/// Distinct from `crate::serve::connection`'s render-specific codes (1-3) so a client
/// can tell a library failure apart from a render/tilt-curves one.
pub const LIBRARY_ERROR_CODE: u32 = 4;

/// Handles one [`LibraryRequest`] against `db`, producing exactly one [`LibraryResponse`].
/// See the module doc comment.
#[must_use]
pub fn handle_request(request: &LibraryRequest, db: &Database) -> LibraryResponse {
    match request {
        LibraryRequest::Search {
            query,
            shape_filter,
            gear_filter,
            range,
            order,
            tag_filter,
        } => search(
            db,
            query,
            shape_filter,
            gear_filter,
            range,
            *order,
            tag_filter.as_deref(),
        ),
        LibraryRequest::FilterOptions => filter_options(db),
        LibraryRequest::FetchDesign { entry_id } => match db.get_diagram_full_meta(*entry_id) {
            Ok(Some(meta)) => {
                // A failed preview lookup degrades to `None` rather than failing the
                // whole fetch; worst case is the re-render offer not appearing.
                let preview_material = db
                    .get_preview_images(*entry_id)
                    .ok()
                    .and_then(|p| p.material);
                LibraryResponse::Design(Box::new(to_record(&meta, preview_material)))
            }
            Ok(None) => LibraryResponse::NotFound,
            Err(e) => db_error("get_diagram_full_meta", &e),
        },
        LibraryRequest::FetchAttachment { attachment_id } => {
            match db.get_attachment_content(*attachment_id) {
                Ok(Some((name, content))) => LibraryResponse::Attachment { name, content },
                Ok(None) => LibraryResponse::NotFound,
                Err(e) => db_error("get_attachment_content", &e),
            }
        }
        LibraryRequest::SearchPage {
            query,
            shape_filter,
            gear_filter,
            range,
            cursor,
        } => search_page(db, query, shape_filter, gear_filter, range, *cursor),
        LibraryRequest::FetchDesignSource { entry_id } => design_source(db, *entry_id),
    }
}

/// Handles [`LibraryRequest::Search`]: resolves `order`/`tag_filter` and
/// calls `Database::search_diagrams_display`, so a remote search honours the same
/// sort order and tag-chip restriction a local one does -- rather than the
/// catalogue-order-only `search_diagrams_with_performance_exclusions`, which ignores both.
///
/// `tag_filter` is resolved by NAME via [`Database::tag_id_by_name`] (see
/// [`LibraryRequest::Search`]'s own doc comment for why a name crosses the wire, never
/// an id): a name this worker's catalogue has never seen resolves to `None` -- no
/// restriction -- the same tolerance `apps/indicatrix-cut`'s own tag-chip lookup has for
/// an unknown tag, rather than an error over an absent chip.
///
/// `local_only`/`id_filter` on [`DisplayFilters`] are always `false`/`None` here: both
/// name concepts local to the client's own database (see [`LibraryRequest::Search`]'s
/// doc comment) that never cross the wire.
fn search(
    db: &Database,
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilterWire,
    order: SortOrderWire,
    tag_filter: Option<&str>,
) -> LibraryResponse {
    let range = match from_range_wire(range) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    let tag_filter = match tag_filter.map(|name| db.tag_id_by_name(name)) {
        Some(Ok(id)) => id,
        Some(Err(e)) => return db_error("tag_id_by_name", &e),
        None => None,
    };
    let filters = DisplayFilters {
        order: sort_order_from_wire(order),
        local_only: false,
        tag_filter,
        id_filter: None,
    };
    match db.search_diagrams_display(query, shape_filter, gear_filter, &range, filters) {
        Ok(result) => LibraryResponse::SearchResults {
            items: result.items.iter().map(to_summary).collect(),
            // Capped at SEARCH_RESULT_CAP (1000), so this cast never truncates.
            excluded_for_missing_curves: result.excluded_for_missing_curves as u32,
        },
        Err(e) => db_error("search_diagrams_display", &e),
    }
}

/// Maps a wire [`SortOrderWire`] onto the vault's local [`SortOrder`]. A plain
/// `match`, not a shared derive, since the two types deliberately live in different
/// crates -- see [`SortOrderWire`]'s own doc comment.
const fn sort_order_from_wire(order: SortOrderWire) -> SortOrder {
    match order {
        SortOrderWire::CatalogueOrder => SortOrder::CatalogueOrder,
        SortOrderWire::Title => SortOrder::Title,
        SortOrderWire::Newest => SortOrder::Newest,
        SortOrderWire::RecentlyEdited => SortOrder::RecentlyEdited,
    }
}

/// Handles [`LibraryRequest::FetchDesignSource`]: finds `entry_id`'s real attached
/// `.asc` file (the same "first attachment whose name ends in `.asc`" rule the client's
/// own local load path uses, `gui::editor::loading::design_from_full_record`) and
/// returns its exact text, decoded as UTF-8 lossily -- matching that same local path so
/// a design loads identically whether it came from this worker or a local file.
///
/// Three distinct outcomes, matching [`LibraryRequest::FetchDesign`]'s own convention
/// for `NotFound` plus one new case:
/// - No such `entry_id` at all -> [`LibraryResponse::NotFound`] (same as `FetchDesign`).
/// - `entry_id` exists but has no `.asc` attachment -> [`LibraryResponse::DesignSourceNotAvailable`]
///   (a design with only a reconstructed, placeholder schedule has no real file bytes to
///   send -- see that variant's own doc comment).
/// - A real `.asc` attachment exists -> [`LibraryResponse::DesignSource`] with its name
///   and text.
///
/// No bespoke size cap here: a `.asc` schedule is always a small text file, and this
/// reply is bounded the same way [`LibraryResponse::Attachment`] already is, by the
/// transport-level `indicatrix_net::framing::MAX_FRAME_LEN`, not a second check
/// duplicated per response type.
fn design_source(db: &Database, entry_id: i64) -> LibraryResponse {
    let meta = match db.get_diagram_full_meta(entry_id) {
        Ok(Some(meta)) => meta,
        Ok(None) => {
            tracing::debug!("FetchDesignSource: entry {entry_id} not found");
            return LibraryResponse::NotFound;
        }
        Err(e) => return db_error("get_diagram_full_meta", &e),
    };
    let Some(attachment) = meta
        .attached_files
        .iter()
        .find(|f| f.name.to_lowercase().ends_with(".asc"))
    else {
        tracing::debug!("FetchDesignSource: entry {entry_id} has no .asc attachment");
        return LibraryResponse::DesignSourceNotAvailable;
    };
    match db.get_attachment_content(attachment.id) {
        Ok(Some((name, content))) => {
            tracing::debug!(
                "FetchDesignSource: entry {entry_id} serving attachment {} ('{name}', {} bytes)",
                attachment.id,
                content.len()
            );
            LibraryResponse::DesignSource {
                entry_id,
                file_name: name,
                asc_text: String::from_utf8_lossy(&content).into_owned(),
            }
        }
        // The metadata query just found this attachment; content going missing here
        // would mean a concurrent delete raced this request -- treat it the same as
        // "no .asc attachment" rather than a hard error.
        Ok(None) => {
            tracing::debug!(
                "FetchDesignSource: entry {entry_id}'s .asc attachment {} vanished concurrently",
                attachment.id
            );
            LibraryResponse::DesignSourceNotAvailable
        }
        Err(e) => db_error("get_attachment_content", &e),
    }
}

/// Handles [`LibraryRequest::SearchPage`]: one keyset-paginated page of
/// `Database::search_diagrams_page`, converted to [`LibraryResponse::SearchResultsPage`].
///
/// Pages at [`SEARCH_RESULT_CAP`] rows. `next_cursor` is `Some` (the last row's
/// `entry_id`) exactly when the page came back full -- cheap-but-not-exact (an
/// occasional harmless extra empty final page) rather than a second COUNT query.
///
/// `excluded_for_missing_curves` is always `0` here: `Database` has no page-scoped
/// counterpart of `search_diagrams_with_performance_exclusions`. Known limitation, not
/// an oversight -- harmless today since this variant's only real caller
/// (`library_mirror`'s exhaustive walk) never sets a performance filter.
fn search_page(
    db: &Database,
    query: &str,
    shape_filter: &str,
    gear_filter: &str,
    range: &RangeFilterWire,
    cursor: Option<i64>,
) -> LibraryResponse {
    let range = match from_range_wire(range) {
        Ok(r) => r,
        Err(resp) => return resp,
    };
    match db.search_diagrams_page(
        query,
        shape_filter,
        gear_filter,
        &range,
        cursor,
        SEARCH_RESULT_CAP,
    ) {
        Ok(items) => {
            let page_full = items.len() == usize::try_from(SEARCH_RESULT_CAP).unwrap_or(usize::MAX);
            let next_cursor = if page_full {
                items.last().map(|i| i.id)
            } else {
                None
            };
            LibraryResponse::SearchResultsPage {
                results: items.iter().map(to_summary).collect(),
                next_cursor,
                // See this function's own doc comment for why this is always `0`.
                excluded_for_missing_curves: 0,
            }
        }
        Err(e) => db_error("search_diagrams_page", &e),
    }
}

fn filter_options(db: &Database) -> LibraryResponse {
    let shapes = match db.get_unique_shapes() {
        Ok(v) => v,
        Err(e) => return db_error("get_unique_shapes", &e),
    };
    let gears = match db.get_unique_gears() {
        Ok(v) => v,
        Err(e) => return db_error("get_unique_gears", &e),
    };
    let ranges = match db.get_attribute_ranges() {
        Ok(v) => v,
        Err(e) => return db_error("get_attribute_ranges", &e),
    };
    LibraryResponse::FilterOptions {
        shapes,
        gears,
        ranges: to_ranges_wire(ranges),
    }
}

/// Logs the real reason server-side and returns a generic [`LibraryResponse::Error`] --
/// the peer never sees the underlying database error text.
fn db_error(op: &str, e: &anyhow::Error) -> LibraryResponse {
    tracing::warn!("library request failed ({op}): {e:#}");
    LibraryResponse::Error(ErrorMsg {
        code: LIBRARY_ERROR_CODE,
        message: "internal error serving the design library".to_string(),
    })
}

/// Maps a wire [`RangeFilterWire`] onto the local, storage-layer [`RangeFilter`].
///
/// `Err` carries a ready-to-return [`LibraryResponse::Error`] -- this can only fail on a
/// [`PerformanceFilterWire`] entry, via [`PerformanceFilter::new`], which validates
/// `tilt_radius_deg` into `0.0..=90.0` (the one place that knows what a valid radius
/// is). The specific out-of-range value is logged server-side but not echoed to the peer.
fn from_range_wire(r: &RangeFilterWire) -> Result<RangeFilter, LibraryResponse> {
    let mut performance = Vec::with_capacity(r.performance.len());
    for wire_filter in &r.performance {
        performance.push(from_performance_filter_wire(wire_filter).map_err(|e| {
            tracing::warn!("library request rejected: {e:#}");
            LibraryResponse::Error(ErrorMsg {
                code: LIBRARY_VALIDATION_ERROR_CODE,
                message: "invalid tilt-performance filter in search request".to_string(),
            })
        })?);
    }

    Ok(RangeFilter {
        ri_min: r.ri_min,
        ri_max: r.ri_max,
        lw_min: r.lw_min,
        lw_max: r.lw_max,
        volume_min: r.volume_min,
        volume_max: r.volume_max,
        facets_min: r.facets_min,
        facets_max: r.facets_max,
        ri_tolerance: r.ri_tolerance,
        include_ignored: r.include_ignored,
        performance,
    })
}

/// Maps one wire [`PerformanceFilterWire`] onto a local
/// `indicatrix_vault::model::performance::PerformanceFilter`, via
/// [`PerformanceFilter::new`] (see [`from_range_wire`] for where validation happens).
///
/// # Errors
///
/// Returns [`indicatrix_vault::model::performance::InvalidTiltRadius`] when
/// `wire.tilt_radius_deg` is outside `0.0..=90.0`.
fn from_performance_filter_wire(
    wire: &PerformanceFilterWire,
) -> Result<PerformanceFilter, indicatrix_vault::model::performance::InvalidTiltRadius> {
    let metric = match wire.metric {
        PerformanceMetricWire::Brilliance => PerformanceMetric::Brilliance,
        PerformanceMetricWire::Extinction => PerformanceMetric::Extinction,
        PerformanceMetricWire::Windowing => PerformanceMetric::Windowing,
    };
    let bound = match wire.bound {
        PerformanceBoundWire::AtMost(t) => PerformanceBound::AtMost(t),
        PerformanceBoundWire::AtLeast(t) => PerformanceBound::AtLeast(t),
    };
    let aggregate = match wire.aggregate {
        PerformanceAggregateWire::Worst => PerformanceAggregate::Worst,
        PerformanceAggregateWire::Mean => PerformanceAggregate::Mean,
    };
    PerformanceFilter::new(metric, bound, wire.tilt_radius_deg, aggregate)
}

const fn to_ranges_wire(r: AttributeRanges) -> AttributeRangesWire {
    AttributeRangesWire {
        ri: r.ri,
        lw_ratio: r.lw_ratio,
        volume: r.volume,
        facets: r.facets,
    }
}

fn to_summary(item: &DiagramListItem) -> DesignSummary {
    let mut summary = DesignSummary {
        entry_id: item.id,
        title: item.title.clone(),
        url: item.url.clone(),
        design_id: item.design_id.clone(),
        shape: item.shape.clone(),
        index_gear: item.index_gear.clone(),
        facets_count: item.facets_count.clone(),
        designer_info: item.designer_info.clone(),
        lw_ratio: item.lw_ratio.clone(),
        refractive_index: item.refractive_index.clone(),
        volume: item.volume.clone(),
        competition_diagram: item.competition_diagram.clone(),
        ignored: item.ignored,
        version: [0u8; 32],
    };
    summary.version = hash_summary(&summary);
    summary
}

fn to_record(meta: &FullDiagramMeta, preview_material: Option<String>) -> DesignRecord {
    let angle_settings = meta
        .angle_settings
        .iter()
        .map(|a| AngleSettingWire {
            order_index: a.order_index,
            facet: a.facet.clone(),
            angle: a.angle.clone(),
            index: a.index.clone(),
            notes: a.notes.clone(),
        })
        .collect();
    let attachments = meta
        .attached_files
        .iter()
        .map(|f| AttachedFileMeta {
            id: f.id,
            name: f.name.clone(),
            url: f.url.clone(),
            size: u64::try_from(f.size).unwrap_or(0),
        })
        .collect();

    let mut record = DesignRecord {
        entry_id: meta.entry_id,
        title: meta.title.clone(),
        url: meta.url.clone(),
        design_id: meta.design_id.clone(),
        page_url: meta.page_url.clone(),
        diagram_image_name: meta.diagram_image_name.clone(),
        diagram_image_data: meta.diagram_image_data.clone(),
        competition_diagram: meta.competition_diagram.clone(),
        lw_ratio: meta.lw_ratio.clone(),
        refractive_index: meta.refractive_index.clone(),
        index_gear: meta.index_gear.clone(),
        volume: meta.volume.clone(),
        facets_count: meta.facets_count.clone(),
        shape: meta.shape.clone(),
        designer_info: meta.designer_info.clone(),
        angle_settings,
        attachments,
        preview_material,
        hw_ratio: meta.hw_ratio.clone(),
        tw_ratio: meta.tw_ratio.clone(),
        uw_ratio: meta.uw_ratio.clone(),
        pw_ratio: meta.pw_ratio.clone(),
        cw_ratio: meta.cw_ratio.clone(),
        symmetry_order: meta.symmetry_order.clone(),
        mirror_symmetry: meta.mirror_symmetry,
        designer: meta.designer.clone(),
        version: [0u8; 32],
    };
    record.version = hash_record(&record);
    record
}

fn hash_str(hasher: &mut Sha256, s: &str) {
    hasher.update((s.len() as u64).to_le_bytes());
    hasher.update(s.as_bytes());
}

fn hash_opt_str(hasher: &mut Sha256, s: Option<&str>) {
    match s {
        Some(s) => {
            hasher.update([1u8]);
            hash_str(hasher, s);
        }
        None => hasher.update([0u8]),
    }
}

fn hash_opt_bytes(hasher: &mut Sha256, b: Option<&[u8]>) {
    match b {
        Some(b) => {
            hasher.update([1u8]);
            hasher.update((b.len() as u64).to_le_bytes());
            hasher.update(b);
        }
        None => hasher.update([0u8]),
    }
}

/// [`hash_opt_str`]'s counterpart for [`DesignRecord::mirror_symmetry`]: same presence
/// tag before the payload byte, so `Some(false)` still hashes differently from `None`.
fn hash_opt_bool(hasher: &mut Sha256, b: Option<bool>) {
    match b {
        Some(b) => hasher.update([1u8, u8::from(b)]),
        None => hasher.update([0u8]),
    }
}

/// SHA-256 over every [`DesignSummary`] field except [`DesignSummary::version`] itself.
fn hash_summary(s: &DesignSummary) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(s.entry_id.to_le_bytes());
    hash_str(&mut hasher, &s.title);
    hash_str(&mut hasher, &s.url);
    hash_opt_str(&mut hasher, s.design_id.as_deref());
    hash_opt_str(&mut hasher, s.shape.as_deref());
    hash_opt_str(&mut hasher, s.index_gear.as_deref());
    hash_opt_str(&mut hasher, s.facets_count.as_deref());
    hash_opt_str(&mut hasher, s.designer_info.as_deref());
    hash_opt_str(&mut hasher, s.lw_ratio.as_deref());
    hash_opt_str(&mut hasher, s.refractive_index.as_deref());
    hash_opt_str(&mut hasher, s.volume.as_deref());
    hash_opt_str(&mut hasher, s.competition_diagram.as_deref());
    hasher.update([u8::from(s.ignored)]);
    hasher.finalize().into()
}

/// SHA-256 over every [`DesignRecord`] field except [`DesignRecord::version`] itself --
/// including each attachment's metadata (id/name/url/size), never content.
fn hash_record(r: &DesignRecord) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(r.entry_id.to_le_bytes());
    hash_str(&mut hasher, &r.title);
    hash_str(&mut hasher, &r.url);
    hash_opt_str(&mut hasher, r.design_id.as_deref());
    hash_str(&mut hasher, &r.page_url);
    hash_opt_str(&mut hasher, r.diagram_image_name.as_deref());
    hash_opt_bytes(&mut hasher, r.diagram_image_data.as_deref());
    hash_opt_str(&mut hasher, r.competition_diagram.as_deref());
    hash_opt_str(&mut hasher, r.lw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.refractive_index.as_deref());
    hash_opt_str(&mut hasher, r.index_gear.as_deref());
    hash_opt_str(&mut hasher, r.volume.as_deref());
    hash_opt_str(&mut hasher, r.facets_count.as_deref());
    hash_opt_str(&mut hasher, r.shape.as_deref());
    hash_opt_str(&mut hasher, r.designer_info.as_deref());
    hash_opt_str(&mut hasher, r.preview_material.as_deref());
    hash_opt_str(&mut hasher, r.hw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.tw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.uw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.pw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.cw_ratio.as_deref());
    hash_opt_str(&mut hasher, r.symmetry_order.as_deref());
    hash_opt_bool(&mut hasher, r.mirror_symmetry);
    hash_opt_str(&mut hasher, r.designer.as_deref());
    hasher.update((r.angle_settings.len() as u64).to_le_bytes());
    for a in &r.angle_settings {
        hasher.update(a.order_index.to_le_bytes());
        hash_str(&mut hasher, &a.facet);
        hash_str(&mut hasher, &a.angle);
        hash_str(&mut hasher, &a.index);
        hash_str(&mut hasher, &a.notes);
    }
    hasher.update((r.attachments.len() as u64).to_le_bytes());
    for f in &r.attachments {
        hasher.update(f.id.to_le_bytes());
        hash_str(&mut hasher, &f.name);
        hash_str(&mut hasher, &f.url);
        hasher.update(f.size.to_le_bytes());
    }
    hasher.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_vault::model::{
        detail::FacetDiagramDetail, entry::FacetDiagramEntry, file::AttachedFile,
    };

    /// Builds a fresh, populated temp database (read-write) and returns the path;
    /// callers reopen it `Database::open_read_only`. Tests never touch
    /// `facet_diagrams.sqlite`, only their own throwaway temp files.
    fn populated_temp_db() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "indicatrix-worker-library-test-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_str = path.to_str().unwrap();
        let db = Database::new(Some(path_str)).unwrap();

        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Round Brilliant".to_string(),
                    url: "https://example.test/diagram/1".to_string(),
                    design_id: "RB-1".to_string(),
                },
                "facetdiagrams.org",
            )
            .unwrap();

        let mut detail = FacetDiagramDetail {
            page_url: "https://example.test/diagram/1".to_string(),
            shape: Some("Round".to_string()),
            refractive_index: Some("2.417".to_string()),
            attached_files: vec![AttachedFile {
                name: "schedule.pdf".to_string(),
                url: "https://example.test/schedule.pdf".to_string(),
                content: vec![1, 2, 3, 4, 5],
            }],
            ..Default::default()
        };
        detail.angle_settings_table = vec![indicatrix_vault::model::angle::AngleSetting {
            order_index: 0,
            facet: "P1".to_string(),
            angle: "41.0".to_string(),
            index: "96".to_string(),
            notes: String::new(),
        }];
        db.save_diagram_detail(&detail, entry_id).unwrap();

        path
    }

    #[test]
    fn search_returns_the_seeded_design_with_a_version_hash() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(
            &LibraryRequest::Search {
                query: "Round".to_string(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                order: SortOrderWire::default(),
                tag_filter: None,
            },
            &db,
        );
        let LibraryResponse::SearchResults {
            items,
            excluded_for_missing_curves,
        } = response
        else {
            panic!("expected SearchResults, got {response:?}");
        };
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].title, "Round Brilliant");
        assert_ne!(items[0].version, [0u8; 32]);
        assert!(!items[0].ignored);
        assert_eq!(
            excluded_for_missing_curves, 0,
            "no performance filter was active, so nothing can have been excluded for lacking curves"
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn fetch_design_returns_metadata_only_never_attachment_content() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FetchDesign { entry_id: 1 }, &db);
        let LibraryResponse::Design(record) = response else {
            panic!("expected Design, got {response:?}");
        };
        assert_eq!(record.attachments.len(), 1);
        assert_eq!(record.attachments[0].name, "schedule.pdf");
        assert_eq!(record.attachments[0].size, 5);
        assert_eq!(record.angle_settings.len(), 1);
        assert_ne!(record.version, [0u8; 32]);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// `DesignRecord::preview_material` must carry the preset name a design's cached
    /// previews were generated with, so a remote client can run the same stale-material
    /// check the local path does. Asserts a genuinely stored value, not `None`, since
    /// `None` would pin nothing.
    #[test]
    fn fetch_design_carries_the_stored_preview_material() {
        let path = populated_temp_db();
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();

        // One candidate within tolerance -> chosen outright, no RNG draw, so this
        // fixture is deterministic.
        let candidates = [indicatrix_vault::model::material_match::RiPresetCandidate {
            name: "Diamond".to_string(),
            refractive_index: 2.417,
        }];
        let stored = db
            .ensure_preview_material(1, 2.417, &candidates, 0.01, &mut || 0.0)
            .unwrap();
        assert_eq!(stored.as_deref(), Some("Diamond"));

        let response = handle_request(&LibraryRequest::FetchDesign { entry_id: 1 }, &db);
        let LibraryResponse::Design(record) = response else {
            panic!("expected Design, got {response:?}");
        };
        assert_eq!(record.preview_material.as_deref(), Some("Diamond"));

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// The ratio/symmetry/designer fields on `DesignRecord` must carry the same values
    /// a local lookup sees. Builds its own fixture (rather than `populated_temp_db`,
    /// which leaves them `None`) with every field set to a distinct, non-default value,
    /// so this pins that each survives `to_record`/the wire, not merely that it exists.
    #[test]
    fn fetch_design_carries_the_stored_ratio_and_symmetry_fields() {
        let path = std::env::temp_dir().join(format!(
            "indicatrix-worker-library-ratio-test-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_str = path.to_str().unwrap();
        let db = Database::new(Some(path_str)).unwrap();

        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Round Brilliant".to_string(),
                    url: "https://example.test/diagram/1".to_string(),
                    design_id: "RB-1".to_string(),
                },
                "facetdiagrams.org",
            )
            .unwrap();
        let detail = FacetDiagramDetail {
            page_url: "https://example.test/diagram/1".to_string(),
            shape: Some("Round".to_string()),
            refractive_index: Some("2.417".to_string()),
            hw_ratio: Some("1.0".to_string()),
            tw_ratio: Some("0.53".to_string()),
            uw_ratio: Some("0.16".to_string()),
            pw_ratio: Some("0.43".to_string()),
            cw_ratio: Some("0.14".to_string()),
            symmetry_order: Some("8".to_string()),
            mirror_symmetry: Some(true),
            designer: Some("Capps, Jerry".to_string()),
            ..Default::default()
        };
        db.save_diagram_detail(&detail, entry_id).unwrap();
        drop(db);

        let db = Database::open_read_only(path_str).unwrap();
        let response = handle_request(&LibraryRequest::FetchDesign { entry_id }, &db);
        let LibraryResponse::Design(record) = response else {
            panic!("expected Design, got {response:?}");
        };
        assert_eq!(record.hw_ratio.as_deref(), Some("1.0"));
        assert_eq!(record.tw_ratio.as_deref(), Some("0.53"));
        assert_eq!(record.uw_ratio.as_deref(), Some("0.16"));
        assert_eq!(record.pw_ratio.as_deref(), Some("0.43"));
        assert_eq!(record.cw_ratio.as_deref(), Some("0.14"));
        assert_eq!(record.symmetry_order.as_deref(), Some("8"));
        assert_eq!(record.mirror_symmetry, Some(true));
        assert_eq!(record.designer.as_deref(), Some("Capps, Jerry"));
        assert_ne!(record.version, [0u8; 32]);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn fetch_design_for_an_unknown_id_is_not_found() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FetchDesign { entry_id: 999 }, &db);
        assert_eq!(response, LibraryResponse::NotFound);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn fetch_attachment_returns_exactly_that_attachments_bytes() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FetchAttachment { attachment_id: 1 }, &db);
        assert_eq!(
            response,
            LibraryResponse::Attachment {
                name: "schedule.pdf".to_string(),
                content: vec![1, 2, 3, 4, 5],
            }
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn fetch_attachment_for_an_unknown_id_is_not_found() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FetchAttachment { attachment_id: 999 }, &db);
        assert_eq!(response, LibraryResponse::NotFound);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// `FilterOptions` serves `Database::get_unique_shapes`, the union of the seeded
    /// canonical vocabulary and the shapes actually present in the library -- a remote
    /// client gets the full picker list, exactly as a local one does.
    #[test]
    fn filter_options_reports_the_seeded_shape_alongside_the_canonical_vocabulary() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FilterOptions, &db);
        let LibraryResponse::FilterOptions { shapes, .. } = response else {
            panic!("expected FilterOptions, got {response:?}");
        };

        assert!(
            shapes.contains(&"Round".to_string()),
            "the served library's own shape must still be reported, got {shapes:?}"
        );
        assert!(
            shapes.contains(&"Marquise".to_string()),
            "the seeded canonical vocabulary must reach a remote client too, got {shapes:?}"
        );
        // "Round" is both seeded and present on the fixture -- must dedupe.
        assert_eq!(
            shapes.iter().filter(|s| *s == "Round").count(),
            1,
            "a shape in both the vocabulary and the data must appear once, got {shapes:?}"
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn two_designs_with_different_fields_get_different_version_hashes() {
        let path = populated_temp_db();
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        db.save_diagram_entry(
            &FacetDiagramEntry {
                title: "Emerald Cut".to_string(),
                url: "https://example.test/diagram/2".to_string(),
                design_id: "EC-1".to_string(),
            },
            "facetdiagrams.org",
        )
        .unwrap();
        drop(db);

        let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                order: SortOrderWire::default(),
                tag_filter: None,
            },
            &ro,
        );
        let LibraryResponse::SearchResults { items, .. } = response else {
            panic!("expected SearchResults, got {response:?}");
        };
        assert_eq!(items.len(), 2);
        assert_ne!(items[0].version, items[1].version);

        drop(ro);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn search_page_returns_no_next_cursor_when_the_page_is_not_full() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(
            &LibraryRequest::SearchPage {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                cursor: None,
            },
            &db,
        );
        let LibraryResponse::SearchResultsPage {
            results,
            next_cursor,
            excluded_for_missing_curves,
        } = response
        else {
            panic!("expected SearchResultsPage, got {response:?}");
        };
        assert_eq!(results.len(), 1);
        assert_eq!(
            next_cursor, None,
            "one row is far short of a full page -- there is nothing more to fetch"
        );
        assert_eq!(
            excluded_for_missing_curves, 0,
            "no performance filter was active on this request"
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn search_page_cursor_excludes_rows_already_returned_by_an_earlier_page() {
        let path = populated_temp_db();
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        let second_entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Emerald Cut".to_string(),
                    url: "https://example.test/diagram/2".to_string(),
                    design_id: "EC-1".to_string(),
                },
                "facetdiagrams.org",
            )
            .unwrap();
        drop(db);

        let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();

        // First page with no cursor sees both designs.
        let first = handle_request(
            &LibraryRequest::SearchPage {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                cursor: None,
            },
            &ro,
        );
        let LibraryResponse::SearchResultsPage {
            results: first_results,
            ..
        } = first
        else {
            panic!("expected SearchResultsPage, got {first:?}");
        };
        assert_eq!(first_results.len(), 2);
        let first_id = first_results[0].entry_id;

        // Re-requesting with that row's id as cursor must see only what came after it.
        let second = handle_request(
            &LibraryRequest::SearchPage {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                cursor: Some(first_id),
            },
            &ro,
        );
        let LibraryResponse::SearchResultsPage {
            results: second_results,
            next_cursor,
            ..
        } = second
        else {
            panic!("expected SearchResultsPage, got {second:?}");
        };
        assert_eq!(second_results.len(), 1);
        assert_eq!(second_results[0].entry_id, second_entry_id);
        assert_eq!(next_cursor, None);

        drop(ro);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn search_with_an_out_of_range_tilt_radius_is_rejected_without_touching_the_database() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire {
                    performance: vec![PerformanceFilterWire {
                        metric: PerformanceMetricWire::Windowing,
                        bound: PerformanceBoundWire::AtMost(20.0),
                        tilt_radius_deg: 91.0, // out of range
                        aggregate: PerformanceAggregateWire::Worst,
                    }],
                    ..RangeFilterWire::default()
                },
                order: SortOrderWire::default(),
                tag_filter: None,
            },
            &db,
        );
        let LibraryResponse::Error(err) = response else {
            panic!("expected Error, got {response:?}");
        };
        assert_eq!(err.code, LIBRARY_VALIDATION_ERROR_CODE);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn search_with_a_valid_performance_filter_reaches_the_database() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        // A valid-but-unsatisfiable filter must reach the database, not be rejected.
        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire {
                    performance: vec![PerformanceFilterWire {
                        metric: PerformanceMetricWire::Brilliance,
                        bound: PerformanceBoundWire::AtLeast(0.0),
                        tilt_radius_deg: 45.0,
                        aggregate: PerformanceAggregateWire::Worst,
                    }],
                    ..RangeFilterWire::default()
                },
                order: SortOrderWire::default(),
                tag_filter: None,
            },
            &db,
        );
        let LibraryResponse::SearchResults {
            items,
            excluded_for_missing_curves,
        } = response
        else {
            panic!("expected SearchResults, got {response:?}");
        };
        assert!(
            items.is_empty(),
            "the seeded design has no tilt curves, so it can never satisfy an active \
             performance filter"
        );
        // The design matched the otherwise-unfiltered search but was excluded for
        // lacking tilt curves; a remote caller must be told so, not just see "no match".
        assert_eq!(
            excluded_for_missing_curves, 1,
            "the seeded design was excluded specifically for lacking tilt curves"
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// [`LibraryRequest::FetchDesignSource`] against the seeded fixture's real attached
    /// `.asc` file (`populated_temp_db` names it `schedule.pdf`, NOT `.asc` -- see the
    /// dedicated fixture built below) returns that attachment's exact text.
    #[test]
    fn fetch_design_source_returns_the_attached_asc_files_exact_text() {
        let path = std::env::temp_dir().join(format!(
            "indicatrix-worker-library-source-test-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path_str = path.to_str().unwrap();
        let db = Database::new(Some(path_str)).unwrap();

        let entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Round Brilliant".to_string(),
                    url: "https://example.test/diagram/1".to_string(),
                    design_id: "RB-1".to_string(),
                },
                "facetdiagrams.org",
            )
            .unwrap();
        let asc_text = "GemCad 5.0\nR1  c 41.0 96\n";
        let detail = FacetDiagramDetail {
            page_url: "https://example.test/diagram/1".to_string(),
            shape: Some("Round".to_string()),
            attached_files: vec![AttachedFile {
                name: "round-brilliant.asc".to_string(),
                url: "https://example.test/round-brilliant.asc".to_string(),
                content: asc_text.as_bytes().to_vec(),
            }],
            ..Default::default()
        };
        db.save_diagram_detail(&detail, entry_id).unwrap();
        drop(db);

        let ro = Database::open_read_only(path_str).unwrap();
        let response = handle_request(&LibraryRequest::FetchDesignSource { entry_id }, &ro);
        assert_eq!(
            response,
            LibraryResponse::DesignSource {
                entry_id,
                file_name: "round-brilliant.asc".to_string(),
                asc_text: asc_text.to_string(),
            }
        );

        drop(ro);
        std::fs::remove_file(&path).ok();
    }

    /// A design with SOME attachment but no `.asc` among them (the `populated_temp_db`
    /// fixture: only `schedule.pdf`) has no genuine source text to send --
    /// `DesignSourceNotAvailable`, not `NotFound` (the entry itself is real).
    #[test]
    fn fetch_design_source_for_a_design_with_no_asc_attachment_is_not_available() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FetchDesignSource { entry_id: 1 }, &db);
        assert_eq!(response, LibraryResponse::DesignSourceNotAvailable);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// An `entry_id` with no matching row at all is `NotFound`, distinct from
    /// `DesignSourceNotAvailable` (which means the entry exists but lacks a `.asc`).
    #[test]
    fn fetch_design_source_for_an_unknown_entry_is_not_found() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(&LibraryRequest::FetchDesignSource { entry_id: 999 }, &db);
        assert_eq!(response, LibraryResponse::NotFound);

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// A design marked ignored locally must come back from a `Search` reply with
    /// `DesignSummary::ignored == true`.
    #[test]
    fn search_reports_an_ignored_design_as_ignored_when_include_ignored_is_set() {
        let path = populated_temp_db();
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        db.set_diagram_ignored(1, true).unwrap();
        drop(db);

        let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire {
                    include_ignored: true,
                    ..RangeFilterWire::default()
                },
                order: SortOrderWire::default(),
                tag_filter: None,
            },
            &ro,
        );
        let LibraryResponse::SearchResults { items, .. } = response else {
            panic!("expected SearchResults, got {response:?}");
        };
        assert_eq!(items.len(), 1);
        assert!(items[0].ignored, "the design was just marked ignored");

        drop(ro);
        std::fs::remove_file(&path).ok();
    }

    /// A `Search` with `order: Title` must return results sorted by title,
    /// not the catalogue's insertion (id) order -- `populated_temp_db` seeds "Round
    /// Brilliant" first, so two more titles are added here specifically out of
    /// alphabetical order to prove the sort, not just pass by coincidence.
    #[test]
    fn search_with_title_order_returns_titles_sorted() {
        let path = populated_temp_db();
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        for (title, design_id) in [("Zircon Cut", "ZC-1"), ("Asscher Cut", "AC-1")] {
            db.save_diagram_entry(
                &FacetDiagramEntry {
                    title: title.to_string(),
                    url: format!("https://example.test/diagram/{design_id}"),
                    design_id: design_id.to_string(),
                },
                "facetdiagrams.org",
            )
            .unwrap();
        }
        drop(db);

        let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                order: SortOrderWire::Title,
                tag_filter: None,
            },
            &ro,
        );
        let LibraryResponse::SearchResults { items, .. } = response else {
            panic!("expected SearchResults, got {response:?}");
        };
        let titles: Vec<&str> = items.iter().map(|i| i.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Asscher Cut", "Round Brilliant", "Zircon Cut"],
            "results must come back alphabetically by title, not insertion order"
        );

        drop(ro);
        std::fs::remove_file(&path).ok();
    }

    /// A `Search` whose `tag_filter` names a tag one seeded design carries
    /// (and the other does not) must restrict the results to that design only.
    #[test]
    fn search_with_tag_filter_restricts_to_designs_carrying_that_tag() {
        let path = populated_temp_db();
        let db = Database::new(Some(path.to_str().unwrap())).unwrap();
        let tagged_entry_id = db
            .save_diagram_entry(
                &FacetDiagramEntry {
                    title: "Emerald Cut".to_string(),
                    url: "https://example.test/diagram/2".to_string(),
                    design_id: "EC-1".to_string(),
                },
                "facetdiagrams.org",
            )
            .unwrap();
        db.add_tag_to_entry(tagged_entry_id, "Favorites").unwrap();
        drop(db);

        let ro = Database::open_read_only(path.to_str().unwrap()).unwrap();
        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                order: SortOrderWire::default(),
                tag_filter: Some("Favorites".to_string()),
            },
            &ro,
        );
        let LibraryResponse::SearchResults { items, .. } = response else {
            panic!("expected SearchResults, got {response:?}");
        };
        assert_eq!(
            items.len(),
            1,
            "only the tagged design should match, not the seeded untagged one too"
        );
        assert_eq!(items[0].entry_id, tagged_entry_id);
        assert_eq!(items[0].title, "Emerald Cut");

        drop(ro);
        std::fs::remove_file(&path).ok();
    }

    /// A `tag_filter` naming a tag this worker's catalogue has never seen
    /// must resolve to no restriction (every design matches), the same tolerance
    /// `apps/indicatrix-cut`'s own tag-chip lookup has for an unknown tag -- not an
    /// error, and not "match nothing".
    #[test]
    fn search_with_an_unknown_tag_name_applies_no_restriction() {
        let path = populated_temp_db();
        let db = Database::open_read_only(path.to_str().unwrap()).unwrap();

        let response = handle_request(
            &LibraryRequest::Search {
                query: String::new(),
                shape_filter: "All".to_string(),
                gear_filter: "All".to_string(),
                range: RangeFilterWire::default(),
                order: SortOrderWire::default(),
                tag_filter: Some("Nonexistent Tag".to_string()),
            },
            &db,
        );
        let LibraryResponse::SearchResults { items, .. } = response else {
            panic!("expected SearchResults, got {response:?}");
        };
        assert_eq!(
            items.len(),
            1,
            "an unknown tag name must not exclude the seeded design"
        );

        drop(db);
        std::fs::remove_file(&path).ok();
    }

    /// [`sort_order_from_wire`] maps all four [`SortOrderWire`] variants to the matching
    /// [`SortOrder`]. A pure function, so unlike the sibling `search_with_*`
    /// tests above (which additionally prove `Title` drives a real sorted query) this
    /// needs no database fixture -- it just pins the mapping itself, including the two
    /// variants (`Newest`, `RecentlyEdited`) no integration test above exercises.
    #[test]
    fn sort_order_from_wire_maps_every_variant() {
        assert_eq!(
            sort_order_from_wire(SortOrderWire::CatalogueOrder),
            SortOrder::CatalogueOrder
        );
        assert_eq!(sort_order_from_wire(SortOrderWire::Title), SortOrder::Title);
        assert_eq!(
            sort_order_from_wire(SortOrderWire::Newest),
            SortOrder::Newest
        );
        assert_eq!(
            sort_order_from_wire(SortOrderWire::RecentlyEdited),
            SortOrder::RecentlyEdited
        );
    }
}
