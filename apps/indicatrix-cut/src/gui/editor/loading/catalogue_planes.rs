//! The one resolution of "the facet planes of a catalogue record", shared by every
//! consumer that shows or measures a stored design without opening it in the
//! editor: the library detail view's 3D preview, the preview batch (local and
//! remote lanes) and the tilt-curve batch (local and remote lanes).
//!
//! The design file comes first: the attachment `super::design`'s
//! `design_from_attachment` picks (the first `.asc`, else `.gem`, else `.gcs`,
//! converted to `.asc` cutting instructions), loaded as a `Design` and solved into
//! its planes. The angle table is only the fallback, used when the record has no
//! design file or the file does not read, convert, parse or solve. The fallback
//! is `gui::library::detail::reconstruct_planes` over the angle-settings rows --
//! the geometry both batches always built before -- not the zero-mast placeholder
//! schedule `design_from_full_record` hands the editor, whose facet planes all
//! pass through the origin.
//!
//! A design's concave tiers (curved-tool cuts) come with it: the design file keeps
//! them (the native `.indicatrix` attachment stores them directly, and
//! `load_paired` restores them from the sidecar's stash), and
//! [`CataloguePlanes::tools`] holds them resolved. Tiers that fail to resolve set
//! [`CataloguePlanes::concave_error`] instead of silently yielding a flat stone,
//! and the angle table (which has no concave rows) is never used for such a design.

use super::design::design_from_attachment;
use indicatrix::geometry::{GpuFacetPlane, cuts::FacetSpec, tool::ToolPrimitive};
use indicatrix_vault::model::entry::FullDiagramRecord;
use tracing::{debug, warn};

/// Where [`resolve_catalogue_planes`] took a record's planes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CataloguePlanesSource {
    /// The record's design-file attachment (`.asc`, else `.gem`, else `.gcs`).
    DesignFile,
    /// The angle-settings table: the record has no design file, or it did not
    /// read, convert, parse or solve.
    AngleTable,
}

/// A catalogue record's facet planes and index-gear settings, as
/// [`resolve_catalogue_planes`] resolved them.
#[derive(Debug, Clone)]
pub struct CataloguePlanes {
    /// The facet planes in the tracer's `n . x + d <= 0` convention.
    pub planes: Vec<GpuFacetPlane>,
    /// The index gear's tooth count.
    pub gear_teeth: u32,
    /// The index gear's reference angle, in teeth (`0.0` from the angle table,
    /// which records none).
    pub gear_reference_angle: f32,
    /// Which source the planes came from.
    pub source: CataloguePlanesSource,
    /// How many of the leading `planes` are the design's preform planes rather
    /// than facet planes (`Design::planes` lists the preform's first). `0` on the
    /// angle-table path, which has no preform. The rough planner measures
    /// `planes[preform_plane_count..]` alone.
    pub preform_plane_count: usize,
    /// The design's concave tiers resolved to tool primitives, placed on the FACET
    /// stone (`planes[preform_plane_count..]`, see
    /// `Design::facet_geometry_from_solved`). Empty for a planar design, for the
    /// angle-table path, and when `concave_error` is set.
    pub tools: Vec<ToolPrimitive>,
    /// Why the design's concave tiers could not be resolved, when they could not.
    /// `planes` are then the flat facets only, which is NOT the stone the design
    /// describes: a consumer that measures or plans must skip the design rather
    /// than treat it as flat. (Display-only consumers just draw the flat planes.)
    pub concave_error: Option<String>,
}

/// Resolves `full`'s facet planes: the design file first, the angle table only as
/// the fallback. See this module's doc comment for the rule and its consumers.
///
/// Reading, converting, parsing and solving the design file all run inside
/// `gui::library::local::catch_file_panic`, so a design file that panics anywhere
/// in that chain falls back to the angle table like one that fails cleanly. Logs
/// the source used for every record at debug level, and a design file that could
/// not be used at warn level.
///
/// Blocking: it parses the design file and runs a full meet-point solve (0.01 ms to
/// 5.9 s), and owns no cancel point. A caller that answers a user action calls it on
/// a worker thread, as the library detail view does; the batches already run it on
/// theirs.
#[must_use]
pub fn resolve_catalogue_planes(full: &FullDiagramRecord) -> CataloguePlanes {
    match design_file_planes(full) {
        Ok(Some(resolved)) => {
            debug!(
                "Catalogue design #{}: {} facet planes from its design file",
                full.entry_id,
                resolved.planes.len()
            );
            return resolved;
        }
        Ok(None) => debug!(
            "Catalogue design #{}: no design file, planes from the angle table",
            full.entry_id
        ),
        Err(e) => warn!(
            "Catalogue design #{}: the design file could not be used ({e}); planes from \
             the angle table",
            full.entry_id
        ),
    }
    angle_table_planes(full)
}

/// `full`'s planes from its design-file attachment. `Ok(None)` when it has none;
/// `Err` (a ready message) when it has one that does not read, convert, parse or
/// solve, or panics doing so.
fn design_file_planes(full: &FullDiagramRecord) -> Result<Option<CataloguePlanes>, String> {
    let guarded =
        crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(|| {
            let loaded = match design_from_attachment(full)? {
                Ok(loaded) => loaded,
                Err(e) => return Some(Err(e.message)),
            };
            let design = &loaded.design;
            let resolved = design
                .solve()
                .map(|solved| {
                    // `Design::planes` is `solve` then `planes_from_solved`; solving
                    // once here lets the concave tools reuse the masts.
                    let halfspaces = design.planes_from_solved(&solved);
                    // A planar design skips the resolver entirely, so it plans
                    // exactly as before.
                    let (tools, concave_error) = if design.concave_tiers.is_empty() {
                        (Vec::new(), None)
                    } else {
                        match design.facet_geometry_from_solved(&solved) {
                            Ok((_, tools)) => (tools, None),
                            Err(e) => (Vec::new(), Some(e.to_string())),
                        }
                    };
                    CataloguePlanes {
                        // Same sign flip as `state::design_to_gpu_planes`: `planes()`
                        // is `n . x <= m`, the tracer's plane is `n . x + d <= 0`.
                        planes: halfspaces
                            .into_iter()
                            .map(|(normal, offset)| {
                                GpuFacetPlane::new(normal.as_vec3(), -offset as f32)
                            })
                            .collect(),
                        gear_teeth: loaded.design.meta.gear_teeth_abs(),
                        gear_reference_angle: loaded.design.meta.gear_reference_angle as f32,
                        source: CataloguePlanesSource::DesignFile,
                        // `planes_offset` (what `planes()` prepends) returns the same
                        // count as `planes()`; only the +Y/-Y offsets differ.
                        preform_plane_count: loaded.design.preform.planes().len(),
                        tools,
                        concave_error,
                    }
                })
                .map_err(|e| format!("its cutting instructions do not solve: {e}"));
            Some(resolved)
        }));
    match guarded {
        Ok(result) => result.transpose(),
        Err(panic_msg) => Err(format!("internal error: {panic_msg}")),
    }
}

/// `full`'s planes rebuilt from its angle-settings rows by
/// `gui::library::detail::reconstruct_planes`, with the gear tooth count the
/// detail view has always shown for an angle-table design (96 when the record has
/// no usable one) and no reference angle.
fn angle_table_planes(full: &FullDiagramRecord) -> CataloguePlanes {
    let facet_specs: Vec<FacetSpec> = full
        .angle_settings
        .iter()
        // A concave row is a tool placement, not a flat facet plane.
        .filter(|a| a.tool.is_none())
        .map(|a| FacetSpec {
            facet: a.facet.clone(),
            angle: a.angle.clone(),
            index: a.index.clone(),
            notes: a.notes.clone(),
        })
        .collect();
    let planes = crate::gui::library::detail::reconstruct_planes(
        full.shape.as_deref(),
        full.index_gear.as_deref(),
        &facet_specs,
    );
    let gear_teeth = full
        .index_gear
        .as_deref()
        .and_then(|g| g.trim().parse::<u32>().ok())
        .filter(|&g| g > 0)
        .unwrap_or(96);
    CataloguePlanes {
        planes,
        gear_teeth,
        gear_reference_angle: 0.0,
        source: CataloguePlanesSource::AngleTable,
        preform_plane_count: 0,
        tools: Vec::new(),
        concave_error: None,
    }
}
