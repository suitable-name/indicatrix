//! Measured metadata for a design about to be stored, and carrying hand-entered metadata
//! over when a file replaces an existing entry.
//!
//! A port of the desktop importer's `apply_measured_metadata` (proportions and the shape
//! from the solved facet planes) and `merge_reimport_metadata`, so a row written here
//! reads the same as one the desktop writes from the same file. The planes come from the
//! shared solver instead of the desktop's database-angle reconstruction; the numbers are
//! the same measure of the same solid.

use std::sync::atomic::AtomicBool;

use glam::DVec3;
use indicatrix::geometry::{GpuFacetPlane, girdle, stone_metrics::measure_solid};
use indicatrix_vault::{
    db::sqlite::DEFAULT_SHAPES,
    model::{detail::FacetingDiagramDetail, entry::FullDiagramRecord},
};
use indicatrix_web_core::solve::{SolveResponse, solve_design};

use super::design_source::design_of;

/// Fills the proportion fields and, when the file named none, the shape, from the
/// design's own solved planes. A design that does not read, solve or close leaves every
/// field as it was: a number that could not be measured is never invented.
pub(super) fn apply_measured_metadata(detail: &mut FacetingDiagramDetail) {
    if detail.angle_settings_table.is_empty() {
        return;
    }
    let Ok(design) = design_of(&detail.attached_files) else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let SolveResponse::Solved(outcome) = solve_design(&design, &cancel) else {
        return;
    };
    if outcome.solved.is_none() {
        return;
    }
    let planes: Vec<GpuFacetPlane> = outcome
        .planes
        .into_iter()
        .map(GpuFacetPlane::from)
        .collect();
    // `measure_solid` wants unit outward normals with `n.x <= m`; the viewport planes
    // satisfy `n.x + d = 0`, so `m = -d`.
    let measurable: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|plane| {
            (
                DVec3::new(
                    f64::from(plane.normal[0]),
                    f64::from(plane.normal[1]),
                    f64::from(plane.normal[2]),
                ),
                -f64::from(plane.d),
            )
        })
        .collect();
    let Some(metrics) = measure_solid(&measurable) else {
        return;
    };
    let width = metrics.width_axis;
    if width < 1e-9 {
        return;
    }
    detail.lw_ratio = Some((metrics.length_axis / width).to_string());
    detail.hw_ratio = Some((metrics.total_height / width).to_string());
    detail.cw_ratio = metrics
        .crown_height
        .map(|crown| (crown / width).to_string());
    detail.pw_ratio = metrics
        .pavilion_depth
        .map(|pavilion| (pavilion / width).to_string());
    // Dimensionless (Vol/W^3), as a printed diagram sheet states it.
    detail.volume = Some((metrics.volume / (width * width * width)).to_string());
    // A shape the file itself names outranks the girdle classification.
    detail.shape = detail
        .shape
        .take()
        .or_else(|| classify_shape(&planes, metrics.length_axis / width));
}

/// The shape the design's girdle outline makes unambiguous, `None` otherwise: a blank
/// shape is correctable by the user, a confidently wrong one would poison the filter.
///
/// By girdle facet count (the silhouette), with `lw` the measured length over width:
/// twelve or more sides and `lw <= 1.03` is Round; exactly 3, 4, 6 or 8 sides is
/// Triangle, Square, Hexagon or Octagon within the length-over-width bounds those
/// outlines measure; anything else is not guessed.
fn classify_shape(planes: &[GpuFacetPlane], lw: f64) -> Option<String> {
    const ROUND_LW_MAX: f64 = 1.03;
    const POLYGON_LW_MAX: f64 = 1.20;
    const ROUND_MIN_SIDES: usize = 12;

    let sides = girdle::classify_girdle_plane_indices(planes).len();
    let name = match sides {
        3 if lw <= POLYGON_LW_MAX => "Triangle",
        4 if lw <= ROUND_LW_MAX => "Square",
        6 if lw <= POLYGON_LW_MAX => "Hexagon",
        8 if lw <= ROUND_LW_MAX => "Octagon",
        n if n >= ROUND_MIN_SIDES && lw <= ROUND_LW_MAX => "Round",
        _ => return None,
    };
    DEFAULT_SHAPES
        .iter()
        .find(|shape| **shape == name)
        .map(|shape| (*shape).to_owned())
}

/// Gives the designer line a design file's `designer` and `source_citation` fields
/// leave empty, in the scraped catalogue's `Designer; Citation` form, so the list and
/// the free-text search find the designer.
pub(super) fn fill_designer_info(detail: &mut FacetingDiagramDetail) {
    if detail.designer_info.is_some() {
        return;
    }
    let designer = detail
        .designer
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    if designer.is_empty() {
        return;
    }
    let citation = detail
        .source_citation
        .as_deref()
        .map(str::trim)
        .unwrap_or_default();
    detail.designer_info = Some(if citation.is_empty() {
        designer.to_owned()
    } else {
        format!("{designer}; {citation}")
    });
}

/// Carries the hand-entered fields of the row being replaced into the fresh detail,
/// wherever the fresh one has none. The angle table and the attachments are never
/// carried: they are the point of the replacement.
pub(super) fn merge_existing_metadata(
    fresh: &mut FacetingDiagramDetail,
    existing: &FullDiagramRecord,
) {
    if fresh.page_url.is_empty() {
        fresh.page_url.clone_from(&existing.page_url);
    }
    fresh.diagram_image_name = fresh
        .diagram_image_name
        .take()
        .or_else(|| existing.diagram_image_name.clone());
    fresh.diagram_image_data = fresh
        .diagram_image_data
        .take()
        .or_else(|| existing.diagram_image_data.clone());
    carry(
        &mut fresh.competition_diagram,
        existing.competition_diagram.as_deref(),
    );
    carry(&mut fresh.lw_ratio, existing.lw_ratio.as_deref());
    carry(
        &mut fresh.refractive_index,
        existing.refractive_index.as_deref(),
    );
    carry(&mut fresh.index_gear, existing.index_gear.as_deref());
    carry(&mut fresh.volume, existing.volume.as_deref());
    carry(&mut fresh.facets_count, existing.facets_count.as_deref());
    carry(&mut fresh.shape, existing.shape.as_deref());
    carry(&mut fresh.designer_info, existing.designer_info.as_deref());
    carry(&mut fresh.hw_ratio, existing.hw_ratio.as_deref());
    carry(&mut fresh.tw_ratio, existing.tw_ratio.as_deref());
    carry(&mut fresh.uw_ratio, existing.uw_ratio.as_deref());
    carry(&mut fresh.pw_ratio, existing.pw_ratio.as_deref());
    carry(&mut fresh.cw_ratio, existing.cw_ratio.as_deref());
    carry(
        &mut fresh.symmetry_order,
        existing.symmetry_order.as_deref(),
    );
    fresh.mirror_symmetry = fresh.mirror_symmetry.or(existing.mirror_symmetry);
    carry(&mut fresh.designer, existing.designer.as_deref());
    carry(
        &mut fresh.source_citation,
        existing.source_citation.as_deref(),
    );
    carry(&mut fresh.pdf_file, existing.pdf_file.as_deref());
    carry(&mut fresh.gem_file, existing.gem_file.as_deref());
    carry(
        &mut fresh.shape_category,
        existing.shape_category.as_deref(),
    );
}

/// Keeps `fresh` when it holds a value, else takes `existing`'s.
fn carry(fresh: &mut Option<String>, existing: Option<&str>) {
    if fresh.is_none() {
        *fresh = existing.map(str::to_owned);
    }
}
