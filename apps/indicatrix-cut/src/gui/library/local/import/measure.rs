//! Derives a design's measured proportions and shape from its own reconstructed
//! facet planes, and merges a re-import's fresh metadata over an existing catalogue
//! row without discarding hand-entered fields.

use crate::gui::library::detail::reconstruct_planes;
use glam::DVec3;
use indicatrix::geometry::{GpuFacetPlane, cuts::FacetSpec, girdle, stone_metrics};
use indicatrix_vault::{
    db::sqlite::DEFAULT_SHAPES,
    model::{angle::AngleSetting, detail::FacetingDiagramDetail, entry::FullDiagramRecord},
};

/// Fills in the proportion fields `indicatrix_vault::local::import_asc` always leaves
/// `None` -- it only reads the `.asc` file's header/tiers, which don't carry these.
/// Reconstructs the same 3D facet planes the viewport shows ([`reconstruct_planes`],
/// reused from `gui::detail::load_diagram_detail` rather than re-parsed) and measures
/// them with `stone_metrics::measure_solid`, measured at ~0.01% median error on L/W and
/// ~0.09% on Vol/W^3 against the real `.asc` corpus. `measure_solid` returns `None` for
/// a degenerate/unbounded arrangement, and every field below stays `None` rather than
/// fabricated when that happens -- never store a number this couldn't measure.
///
/// Values are written with plain `f64::to_string()`, matching `local::import_asc`'s own
/// convention one call site up. The real `facet_diagrams.sqlite`'s ratio/volume columns
/// are all REAL-affinity, so SQLite normalises whatever numeric text is written here
/// regardless of decimal-place convention -- it only has to parse as a plain number.
pub fn apply_measured_metadata(detail: &mut FacetingDiagramDetail) {
    measure_flat_planes(detail);
    apply_concave_rows(detail);
}

/// Records a design file's concave tiers on `detail`: one tool-carrying row per tier
/// appended to the angle table (facet line in the flat rows' own format, plus the
/// formatted tool line), and the `concave_tiers` / `concave_facets` counts the
/// library's `has_concave` filter and the sync stamp read. Read from the `.indicatrix`
/// attachment, the design of record, so every writer that attaches one (import, save
/// write-back) records the same thing. A design without concave tiers is left exactly
/// as it was (no rows, both counts `0`). Runs after the measurement, which must see
/// the flat rows only.
pub fn apply_concave_rows(detail: &mut FacetingDiagramDetail) {
    detail.angle_settings_table.retain(|row| row.tool.is_none());
    detail.concave_tiers = 0;
    detail.concave_facets = 0;
    let Some(position) = indicatrix_vault::local::native_design_attachment_position(
        detail.attached_files.iter().map(|f| f.name.as_str()),
    ) else {
        return;
    };
    let Ok(text) = std::str::from_utf8(&detail.attached_files[position].content) else {
        return;
    };
    // A planar file (the usual case) skips the full load.
    if !text.contains("[[concave_tiers]]") {
        return;
    }
    let Ok(loaded) = indicatrix_cut_core::native::design_from_str(text) else {
        return;
    };
    let first = detail.angle_settings_table.len();
    for (i, tier) in loaded.design.concave_tiers.iter().enumerate() {
        detail.angle_settings_table.push(AngleSetting {
            order_index: (first + i) as u32,
            facet: tier.name.clone(),
            angle: format!("{}\u{b0}", tier.angle_deg),
            index: tier
                .indices
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            notes: tier.instructions.clone(),
            tool: Some(tier.tool.code().to_owned()),
            tool_line: Some(indicatrix_editor::view_model::rows::concave_tool_line(tier)),
        });
        detail.concave_facets += tier.indices.len() as u32;
    }
    detail.concave_tiers = loaded.design.concave_tiers.len() as u32;
}

/// The measurement half of [`apply_measured_metadata`], over the flat rows.
fn measure_flat_planes(detail: &mut FacetingDiagramDetail) {
    // `reconstruct_planes` falls back to `standard_round_brilliant()` for no facet
    // specs -- right for the viewport, but measuring that fallback here would
    // attribute a fabricated design's proportions to this one.
    if detail.angle_settings_table.is_empty() {
        return;
    }

    let facet_specs: Vec<FacetSpec> = detail
        .angle_settings_table
        .iter()
        .filter(|a| a.tool.is_none())
        .map(|a| FacetSpec {
            facet: a.facet.clone(),
            angle: a.angle.clone(),
            index: a.index.clone(),
            notes: a.notes.clone(),
        })
        .collect();
    let planes = reconstruct_planes(None, detail.index_gear.as_deref(), &facet_specs);
    let dvec_planes: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|p| {
            (
                DVec3::new(
                    f64::from(p.normal[0]),
                    f64::from(p.normal[1]),
                    f64::from(p.normal[2]),
                ),
                // `measure_solid` wants unit outward normals with `n.x <= m`;
                // `GpuFacetPlane`'s convention is `n.x + d = 0` (see
                // `optics::raytracer::intersect`'s ray-plane test), i.e. `m = -d`.
                -f64::from(p.d),
            )
        })
        .collect();

    let Some(m) = stone_metrics::measure_solid(&dvec_planes) else {
        return;
    };
    let w = m.width_axis;
    if w < 1e-9 {
        return;
    }

    detail.lw_ratio = Some((m.length_axis / w).to_string());
    detail.hw_ratio = Some((m.total_height / w).to_string());
    detail.cw_ratio = m.crown_height.map(|c| (c / w).to_string());
    detail.pw_ratio = m.pavilion_depth.map(|p| (p / w).to_string());
    // Stored dimensionless (`Vol/W^3`, matching a printed diagram sheet), not the raw
    // mast-unit volume.
    detail.volume = Some((m.volume / (w * w * w)).to_string());

    // Classified from the SAME planes, so the girdle outline is measured rather than
    // inferred from the schedule's fold count.
    // A shape the file itself names (a design file's `[meta]`) outranks the girdle
    // classification, which only labels a design that arrives with no shape.
    detail.shape = detail
        .shape
        .take()
        .or_else(|| classify_shape(&planes, m.length_axis / w));
}

/// `Database::save_diagram_detail` fully REPLACES a design's
/// `diagram_details` row (see that method's own doc comment), so re-importing a
/// `.asc` whose filename collides with an existing row would, without merging, silently
/// wipe every hand-entered field the fresh parse doesn't itself produce -- `designer_info`,
/// a manually corrected `shape`, the competition-entry columns, and any proportion the
/// cutter typed in over `apply_measured_metadata`'s own measurement.
///
/// # The merge rule
///
/// For every field below: the freshly imported/measured value wins when it is
/// present (`Some`/non-empty) -- a re-import is the cutter saying "this file is the
/// current version", so a fresh geometry-derived shape or proportion should win over
/// a stale one. Only when the fresh parse left a field blank does the EXISTING row's
/// value survive. Since `local::import_asc` never populates `designer_info`,
/// `designer`, `source_citation`, `competition_diagram`, `pdf_file`, `gem_file`,
/// `shape_category`, `diagram_image_name`/`diagram_image_data` or `page_url` at all
/// (those are remote-scrape-only or hand-typed fields), this rule always preserves
/// them across a local re-import -- exactly the "never silently discard a cutter's
/// typed metadata" contract this function exists to uphold. `angle_settings_table` and
/// `attached_files` are deliberately NOT touched here: those two are the whole point
/// of a re-import and must always come from the fresh file.
pub fn merge_reimport_metadata(fresh: &mut FacetingDiagramDetail, existing: &FullDiagramRecord) {
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
    fresh.competition_diagram = fresh
        .competition_diagram
        .take()
        .or_else(|| existing.competition_diagram.clone());
    fresh.lw_ratio = fresh.lw_ratio.take().or_else(|| existing.lw_ratio.clone());
    fresh.refractive_index = fresh
        .refractive_index
        .take()
        .or_else(|| existing.refractive_index.clone());
    fresh.index_gear = fresh
        .index_gear
        .take()
        .or_else(|| existing.index_gear.clone());
    fresh.volume = fresh.volume.take().or_else(|| existing.volume.clone());
    fresh.facets_count = fresh
        .facets_count
        .take()
        .or_else(|| existing.facets_count.clone());
    fresh.shape = fresh.shape.take().or_else(|| existing.shape.clone());
    fresh.designer_info = fresh
        .designer_info
        .take()
        .or_else(|| existing.designer_info.clone());
    fresh.hw_ratio = fresh.hw_ratio.take().or_else(|| existing.hw_ratio.clone());
    fresh.tw_ratio = fresh.tw_ratio.take().or_else(|| existing.tw_ratio.clone());
    fresh.uw_ratio = fresh.uw_ratio.take().or_else(|| existing.uw_ratio.clone());
    fresh.pw_ratio = fresh.pw_ratio.take().or_else(|| existing.pw_ratio.clone());
    fresh.cw_ratio = fresh.cw_ratio.take().or_else(|| existing.cw_ratio.clone());
    fresh.symmetry_order = fresh
        .symmetry_order
        .take()
        .or_else(|| existing.symmetry_order.clone());
    fresh.mirror_symmetry = fresh.mirror_symmetry.or(existing.mirror_symmetry);
    fresh.designer = fresh.designer.take().or_else(|| existing.designer.clone());
    fresh.source_citation = fresh
        .source_citation
        .take()
        .or_else(|| existing.source_citation.clone());
    fresh.pdf_file = fresh.pdf_file.take().or_else(|| existing.pdf_file.clone());
    fresh.gem_file = fresh.gem_file.take().or_else(|| existing.gem_file.clone());
    fresh.shape_category = fresh
        .shape_category
        .take()
        .or_else(|| existing.shape_category.clone());
}

/// Assigns [`FacetingDiagramDetail::shape`], but only where the design's own girdle
/// outline makes the call unambiguous. Returns `None` otherwise -- a blank shape is
/// correctable by the user, whereas a confidently wrong one looks authoritative and
/// would quietly poison the library's shape filter.
///
/// Classifies by girdle facet count, not schedule fold count: a shape name describes
/// the silhouette, and `classify_girdle_plane_indices` returns every near-vertical
/// plane, so its length is the side count directly. Fold count fails here -- e.g. a
/// "Round Trichecker-12" fixture parses to `symmetry_order = 6` while being a round cut
/// built from six repeats, which a fold-count rule would call a Hexagon; its girdle has
/// far more than six facets, so this rule reads it correctly.
///
/// The rule, with `sides` the girdle facet count and `lw` the measured length/width
/// ratio (>= 1.0): `sides >= ROUND_MIN_SIDES` and `lw <= ROUND_LW_MAX` -> Round; exactly
/// 3/4/6/8 sides -> Triangle/Square/Hexagon/Octagon; anything else -> `None`. Square and
/// Octagon (side count a multiple of 4) measure `lw == 1.0` by construction, so they
/// get the tight `ROUND_LW_MAX`; Triangle and Hexagon carry an inherent
/// `2/sqrt(3) ~= 1.1547`, hence the looser `POLYGON_LW_MAX`. Elongated designs (Oval,
/// Rectangle, Marquise, Pear) are never guessed among -- distinguishing those needs
/// outline curvature this does not measure.
///
/// Looks the chosen name up in [`DEFAULT_SHAPES`] rather than returning the literal, so
/// a rename there can't leave this returning a label the vocabulary no longer
/// recognises.
pub(super) fn classify_shape(planes: &[GpuFacetPlane], lw: f64) -> Option<String> {
    const ROUND_LW_MAX: f64 = 1.03;
    const POLYGON_LW_MAX: f64 = 1.20;
    /// Below this, a many-sided outline is not confidently "round" -- a 10-sided
    /// outline is as plausibly a decagon as a coarse circle, so it gets no shape.
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
        .find(|s| **s == name)
        .map(|s| (*s).to_string())
}
