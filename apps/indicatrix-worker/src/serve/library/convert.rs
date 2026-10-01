//! Wire conversions between `indicatrix_net::library`'s wire types and
//! `indicatrix_vault`'s local storage-layer types, in both directions: request-side
//! (`from_*`) validates and narrows a wire filter into the vault's own
//! [`RangeFilter`]/[`PerformanceFilter`]; response-side (`to_*`) builds a
//! [`DesignSummary`]/[`DesignRecord`] from a vault row, stamping in the summary's
//! content-hash `version` and the revision-token `design_version`/`version` via
//! `super::version_hash`.

use indicatrix_net::{
    library::{
        AngleSettingWire, AttachedFileMeta, AttributeRangesWire, DesignRecord, DesignSummary,
        LibraryResponse, PerformanceAggregateWire, PerformanceBoundWire, PerformanceFilterWire,
        PerformanceMetricWire, RangeFilterWire,
    },
    messages::ErrorMsg,
};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{
        entry::{DiagramListItem, FullDiagramMeta},
        filter::{AttributeRanges, RangeFilter},
        performance::{
            PerformanceAggregate, PerformanceBound, PerformanceFilter, PerformanceMetric,
        },
    },
};

use super::{
    LIBRARY_VALIDATION_ERROR_CODE,
    version_hash::{Revision, hash_summary, revision_token},
};

/// Maps a wire [`RangeFilterWire`] onto the local, storage-layer [`RangeFilter`].
///
/// `Err` carries a ready-to-return [`LibraryResponse::Error`] -- this can only fail on a
/// [`PerformanceFilterWire`] entry, via [`PerformanceFilter::new`], which validates
/// `tilt_radius_deg` into `0.0..=90.0` (the one place that knows what a valid radius
/// is). The specific out-of-range value is logged server-side but not echoed to the peer.
pub(super) fn from_range_wire(r: &RangeFilterWire) -> Result<RangeFilter, LibraryResponse> {
    let mut performance = Vec::with_capacity(r.performance.len());
    for wire_filter in &r.performance {
        performance.push(from_performance_filter_wire(wire_filter).map_err(|e| {
            tracing::warn!("library request rejected: {e:#}");
            LibraryResponse::Error(ErrorMsg {
                code: LIBRARY_VALIDATION_ERROR_CODE,
                message: "invalid tilt-performance filter in search request".to_string(),
                // The library protocol has no request_id/epoch to be stale against.
                request_id: None,
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

pub(super) const fn to_ranges_wire(r: AttributeRanges) -> AttributeRangesWire {
    AttributeRangesWire {
        ri: r.ri,
        lw_ratio: r.lw_ratio,
        volume: r.volume,
        facets: r.facets,
    }
}

/// Builds the summary of one search row. [`DesignSummary::design_version`] is left all
/// zero: the vault's list row carries no revision stamp, so the caller fills it with
/// [`stamp_design_versions`] once the whole reply is built.
pub(super) fn to_summary(item: &DiagramListItem) -> DesignSummary {
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
        design_version: [0u8; 32],
    };
    summary.version = hash_summary(&summary);
    summary
}

/// Fills [`DesignSummary::design_version`] on every row with the design's revision token:
/// one primary-key lookup of the revision stamp per row, never a record load. A row
/// whose stamp cannot be read keeps the all-zero token, which a client treats as "changed".
pub(super) fn stamp_design_versions(summaries: &mut [DesignSummary], db: &Database) {
    for summary in summaries {
        summary.design_version = revision_token(&summary.url, Revision::read(db, summary.entry_id));
    }
}

/// Builds the full record of one design. `revision` must have been read BEFORE `meta`
/// was loaded, so an edit landing in between leaves the token older than the content
/// (one harmless extra re-fetch) rather than newer (a missed edit).
pub(super) fn to_record(
    meta: &FullDiagramMeta,
    preview_material: Option<String>,
    revision: Revision,
) -> DesignRecord {
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

    DesignRecord {
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
        source_citation: meta.source_citation.clone(),
        pdf_file: meta.pdf_file.clone(),
        gem_file: meta.gem_file.clone(),
        shape_category: meta.shape_category.clone(),
        version: revision_token(&meta.url, revision),
    }
}
