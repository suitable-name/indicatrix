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
//! them (the native `.isdd` / legacy `.indicatrix` attachment stores them directly), and
//! [`CataloguePlanes::tools`] holds them resolved. Tiers that fail to resolve set
//! [`CataloguePlanes::concave_error`] instead of silently yielding a flat stone,
//! and the angle table (which has no concave rows) is never used for such a design.
//!
//! The previews, the tilt curves and the detail view draw and measure the stone WITH its
//! tools ([`CataloguePlanes::render_tools`]): a design file's own concave tiers placed
//! against its planes, the way the editor places them, and, for a record whose design file
//! carries none (an `.asc` keeps them as footnotes) or that has no design file at all, the
//! concave rows of its angle table ([`tools_from_table_rows`]; the rows hold the tool at
//! the cutting sheet's rounded precision, see `ConcaveTier::from_table_row`).
//!
//! The design file also decides what the stone is made of and how big it is:
//! [`CataloguePlanes::material`] (its material selection, with the RI and body-color
//! overrides), [`CataloguePlanes::custom_material`] (the file's own snapshot of a custom
//! material the vault may lack) and [`CataloguePlanes::girdle_diameter_mm`]. The library's
//! consumers draw and score a design in that material at that size, as the editor and the
//! exports do (`gui::batch::catalogue_material`).

use super::design::{AttachedDesign, design_from_attachment};
use glam::DVec3;
use indicatrix::geometry::{GpuFacetPlane, cuts::FacetSpec, tool::ToolPrimitive};
use indicatrix_cut_core::{
    Design,
    design::{ConcaveTableRow, ConcaveTier, ToolPlacements, resolve_tiers_on_planes},
    material::MaterialSelection,
    native::CustomMaterialSnapshot,
};
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
    /// The concave tools the stone is DRAWN and MEASURED with (the detail view, the
    /// preview and tilt-curve batches), placed against `planes` -- the whole arrangement,
    /// preform planes included -- exactly as the editor and the Compare window place them.
    /// For a design file with concave tiers these are
    /// [`Design::concave_tools_from_planes`]'s; for a record with none, the tools its angle
    /// table's concave rows describe ([`tools_from_table_rows`]). Empty for a planar design
    /// and when the tiers do not resolve (the flat stone is drawn then, as everywhere).
    /// Unlike [`Self::tools`], which the rough planner reads and which is empty on the
    /// angle-table path, this is never empty for a record whose concave tiers resolve.
    pub render_tools: Vec<ToolPrimitive>,
    /// `(concave tier, placement)` of each of [`Self::render_tools`].
    pub render_placements: ToolPlacements,
    /// The design file's whole material selection (`Design::material`): the species name,
    /// the refractive-index and body-color overrides, the absorption bands and the path
    /// scale. `Some` for every design file, though the selection may name no material
    /// (an imported schedule); `None` on the angle-table path, which carries none.
    pub material: Option<MaterialSelection>,
    /// The design file's girdle diameter in millimetres (`Design::girdle_diameter_mm`),
    /// kept only when it is finite and positive. `None` when the file sets none ("not set":
    /// the 7 mm calibration) and on the angle-table path.
    pub girdle_diameter_mm: Option<f64>,
    /// The file's own snapshot of the custom material it names, for a material no built-in
    /// preset answers to (`LoadedDesign::restorable_custom_material` of the native file).
    /// `None` when the material is a built-in,
    /// when the file names none or carried no snapshot, and on the angle-table path.
    pub custom_material: Option<CustomMaterialSnapshot>,
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
    resolve_catalogue_planes_with_frosting(full).0
}

/// [`resolve_catalogue_planes`] plus the ascending indices of the frosted planes.
///
/// The indices are into the returned [`CataloguePlanes::planes`]: the planes the design frosts
/// (the `Design::frosted_planes_from_solved` of the design read from the record, which applies
/// the record's stored frosting, [`FullDiagramRecord::frosting`], or what a `.gcs` attachment
/// states, or a design file's own). The form `SceneState::frosted_planes` and
/// `indicatrix::geometry::facet_finishes_with_frosted` take, so the previews and the detail
/// view can render the finish the editor shows.
///
/// Empty for a design without frosting and on the angle-table path (which states none: its
/// rows have no notation for it), so a caller that ignores it renders exactly as before.
#[must_use]
pub fn resolve_catalogue_planes_with_frosting(
    full: &FullDiagramRecord,
) -> (CataloguePlanes, Vec<u32>) {
    match design_file_planes(full) {
        Ok(Some((resolved, frosted))) => {
            debug!(
                "Catalogue design #{}: {} facet planes from its design file ({} frosted)",
                full.entry_id,
                resolved.planes.len(),
                frosted.len()
            );
            return (resolved, frosted);
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
    (angle_table_planes(full), Vec::new())
}

/// `full`'s planes from its design-file attachment, with the indices of the frosted ones.
/// `Ok(None)` when it has none; `Err` (a ready message) when it has one that does not read,
/// convert, parse or solve, or panics doing so.
fn design_file_planes(
    full: &FullDiagramRecord,
) -> Result<Option<(CataloguePlanes, Vec<u32>)>, String> {
    let guarded = crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(
        || {
            let AttachedDesign {
                loaded,
                custom_material,
            } = match design_from_attachment(full)? {
                Ok(attached) => attached,
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
                    let (render_tools, render_placements) =
                        design_render_tools(full, design, &halfspaces);
                    #[expect(
                        clippy::cast_possible_truncation,
                        reason = "f32 is the stored angle precision; a gear reference angle in degrees is in the normal float range"
                    )]
                    let gear_reference_angle = loaded.design.meta.gear_reference_angle as f32;
                    let frosted = design.frosted_planes_from_solved(&solved);
                    let planes = CataloguePlanes {
                        // Same sign flip as `state::design_to_gpu_planes`: `planes()`
                        // is `n . x <= m`, the tracer's plane is `n . x + d <= 0`.
                        planes: halfspaces
                            .into_iter()
                            .map(|(normal, offset)| {
                                #[expect(
                                    clippy::cast_possible_truncation,
                                    reason = "f32 is the GPU plane precision; offsets are in the normal float range"
                                )]
                                let d = -offset as f32;
                                GpuFacetPlane::new(normal.as_vec3(), d)
                            })
                            .collect(),
                        gear_teeth: loaded.design.meta.gear_teeth_abs(),
                        gear_reference_angle,
                        source: CataloguePlanesSource::DesignFile,
                        // `planes_offset` (what `planes()` prepends) returns the same
                        // count as `planes()`; only the +Y/-Y offsets differ.
                        preform_plane_count: loaded.design.preform.planes().len(),
                        tools,
                        concave_error,
                        render_tools,
                        render_placements,
                        material: Some(design.material.clone()),
                        girdle_diameter_mm: design
                            .girdle_diameter_mm
                            .filter(|mm| mm.is_finite() && *mm > 0.0),
                        custom_material,
                    };
                    (planes, frosted)
                })
                .map_err(|e| format!("its cutting instructions do not solve: {e}"));
            Some(resolved)
        },
    ));
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
    // The table's concave rows are tools on this stone (the planes above are the flat rows
    // only). `tools` stays empty: the planner reads it and treats an angle-table design as
    // the flat stone it measures.
    let (render_tools, render_placements) =
        if full.angle_settings.iter().any(|row| row.tool.is_some()) {
            let facets: Vec<(DVec3, f64)> = planes
                .iter()
                .copied()
                .map(GpuFacetPlane::to_halfspace_f64)
                .collect();
            table_tools(full, &facets, i32::try_from(gear_teeth).unwrap_or(96), 0.0)
        } else {
            (Vec::new(), Vec::new())
        };
    CataloguePlanes {
        planes,
        gear_teeth,
        gear_reference_angle: 0.0,
        source: CataloguePlanesSource::AngleTable,
        preform_plane_count: 0,
        tools: Vec::new(),
        concave_error: None,
        render_tools,
        render_placements,
        material: None,
        girdle_diameter_mm: None,
        custom_material: None,
    }
}

/// The tools `full`'s stone is drawn and measured with, for a design file solved to
/// `halfspaces` (the whole arrangement, preform planes first).
///
/// A design with concave tiers places them on the whole arrangement
/// ([`Design::concave_tools_from_planes`], the editor's rule); a tier set that does not
/// resolve draws the flat stone ([`CataloguePlanes::concave_error`] says why). A design with
/// none (an `.asc`, `.gem` or `.gcs`, whose concave tiers are footnotes) takes the concave
/// rows of the record's angle table, placed on the facet stone alone, as the angle-table path
/// places them.
fn design_render_tools(
    full: &FullDiagramRecord,
    design: &Design,
    halfspaces: &[(DVec3, f64)],
) -> (Vec<ToolPrimitive>, ToolPlacements) {
    if !design.concave_tiers.is_empty() {
        return design
            .concave_tools_from_planes(halfspaces)
            .unwrap_or_default();
    }
    if !full.angle_settings.iter().any(|row| row.tool.is_some()) {
        return (Vec::new(), Vec::new());
    }
    let preform = design.preform.planes().len().min(halfspaces.len());
    table_tools(
        full,
        &halfspaces[preform..],
        design.meta.gear_teeth,
        design.meta.gear_reference_angle,
    )
}

/// [`tools_from_table_rows`] over the concave rows of `full`'s angle table.
fn table_tools(
    full: &FullDiagramRecord,
    facets: &[(DVec3, f64)],
    gear_teeth: i32,
    gear_reference_angle: f64,
) -> (Vec<ToolPrimitive>, ToolPlacements) {
    let rows = full
        .angle_settings
        .iter()
        .filter(|row| row.tool.is_some())
        .map(|row| ConcaveTableRow {
            facet: &row.facet,
            angle: &row.angle,
            index: &row.index,
            notes: &row.notes,
            tool: row.tool.as_deref().unwrap_or_default(),
            tool_line: row.tool_line.as_deref().unwrap_or_default(),
        });
    tools_from_table_rows(rows, facets, gear_teeth, gear_reference_angle)
}

/// The tools the concave `rows` of a library angle table describe, placed on the flat stone.
///
/// `facets` (`n . x <= m`) is that stone, of an index gear with `gear_teeth` teeth and the
/// given reference angle -- the library's way of drawing and measuring a design that has no
/// concave tiers of its own to resolve.
///
/// The rows keep the tools at the cutting sheet's rounded precision, so a tool is placed to
/// that precision (`ConcaveTier::from_table_row`). A row that cannot be read, or tiers that do
/// not resolve (see `resolve_tiers_on_planes`), leave no tools at all: the flat stone is drawn,
/// never a half-resolved set. The reason is logged.
#[must_use]
pub fn tools_from_table_rows<'a>(
    rows: impl IntoIterator<Item = ConcaveTableRow<'a>>,
    facets: &[(DVec3, f64)],
    gear_teeth: i32,
    gear_reference_angle: f64,
) -> (Vec<ToolPrimitive>, ToolPlacements) {
    let tiers: Result<Vec<ConcaveTier>, String> = rows
        .into_iter()
        .map(|row| ConcaveTier::from_table_row(&row))
        .collect();
    let tiers = match tiers {
        Ok(tiers) if tiers.is_empty() => return (Vec::new(), Vec::new()),
        Ok(tiers) => tiers,
        Err(reason) => {
            warn!(
                "A concave row of the angle table could not be read ({reason}); drawing the flat stone"
            );
            return (Vec::new(), Vec::new());
        }
    };
    resolve_tiers_on_planes(facets, &tiers, gear_teeth, gear_reference_angle).unwrap_or_else(
        |error| {
            warn!(
                "The angle table's concave rows do not resolve ({error}); drawing the flat stone"
            );
            (Vec::new(), Vec::new())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::batch::test_records::{angle_table_record, design_file_text, record_with};
    use indicatrix_cut_core::native::{DesignExtras, design_to_string};

    /// A design file hands back its whole material selection (the overrides included),
    /// its girdle diameter and its custom-material snapshot beside the planes.
    #[test]
    fn a_design_file_hands_back_its_material_its_size_and_its_custom_snapshot() {
        let snapshot = CustomMaterialSnapshot::new(1.74, 0.02, 0.0, None, "Cubic", "Isotropic");
        let selection = MaterialSelection {
            name: Some("My Garnet".to_string()),
            refractive_index_override: Some(1.75),
            ..MaterialSelection::none()
        }
        .with_body_color(Some([0.2, 0.4, 2.8]));
        let text = design_file_text(selection, Some(9.0), Some(&snapshot));
        let resolved = resolve_catalogue_planes(&record_with("garnet.isdd", text.into_bytes()));

        assert_eq!(resolved.source, CataloguePlanesSource::DesignFile);
        let material = resolved
            .material
            .expect("a design file has a material selection");
        assert_eq!(material.name.as_deref(), Some("My Garnet"));
        assert_eq!(material.refractive_index_override, Some(1.75));
        assert!(material.body_color_override.is_some());
        assert_eq!(resolved.girdle_diameter_mm, Some(9.0));
        assert_eq!(
            resolved.custom_material.map(|snapshot| snapshot.mean_ri),
            Some(1.74)
        );
    }

    /// A built-in material has no snapshot to restore, and a file with no girdle diameter
    /// is "not set".
    #[test]
    fn a_built_in_material_and_no_girdle_diameter_leave_those_fields_empty() {
        let selection = MaterialSelection {
            name: Some("Sapphire".to_string()),
            ..MaterialSelection::none()
        };
        let text = design_file_text(selection, None, None);
        let resolved = resolve_catalogue_planes(&record_with("sapphire.isdd", text.into_bytes()));

        assert_eq!(resolved.source, CataloguePlanesSource::DesignFile);
        assert_eq!(
            resolved
                .material
                .and_then(|material| material.name)
                .as_deref(),
            Some("Sapphire")
        );
        assert_eq!(resolved.girdle_diameter_mm, None);
        assert!(resolved.custom_material.is_none());
    }

    /// The angle-table rows a library import writes for `design`'s concave tiers
    /// (`apply_concave_rows`): the facet cells and the tool's two texts.
    fn concave_rows(design: &Design) -> Vec<indicatrix_vault::model::angle::AngleSetting> {
        design
            .concave_tiers
            .iter()
            .map(|tier| indicatrix_vault::model::angle::AngleSetting {
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
                tool_line: Some(tier.second_line_fields().join("  ")),
                ..Default::default()
            })
            .collect()
    }

    /// A design file with concave tiers resolves its render tools against the whole
    /// arrangement, the editor's and the Compare window's rule; the planner's `tools` stay
    /// on the facet stone and are left as they were.
    ///
    /// The reference is what the editor computes: the design the editor's loader reads from
    /// the attachment, solved in f64, its tools placed on that exact f64 arrangement
    /// (`Design::concave_tools_from_solved`). It is NOT recomputed from
    /// `CataloguePlanes::planes`: those are the tracer's f32 `n . x + d <= 0` planes (rounded,
    /// re-normalized), so tools placed on their widened copies differ from the editor's in the
    /// low f32 bits, and the library does not place tools on them.
    #[test]
    fn a_design_file_with_concave_tiers_has_render_tools() {
        let design = Design::concave_fixture();
        let text = design_to_string(&design, None, &DesignExtras::default()).expect("serializes");
        let record = record_with("fixture.isdd", text.into_bytes());
        let resolved = resolve_catalogue_planes(&record);

        assert_eq!(resolved.source, CataloguePlanesSource::DesignFile);
        assert!(resolved.concave_error.is_none());
        let editor_design = design_from_attachment(&record)
            .expect("the record has a design attachment")
            .unwrap_or_else(|e| panic!("the editor's loader reads it: {}", e.message))
            .loaded
            .design;
        let solved = editor_design.solve().expect("the fixture solves");
        let (tools, placements) = editor_design
            .concave_tools_from_solved(&solved)
            .expect("the fixture resolves");
        assert_eq!(
            resolved.render_tools.len(),
            design.concave_placement_count()
        );
        assert_eq!(resolved.render_tools.len(), tools.len());
        assert_eq!(resolved.render_placements, placements);
        assert_eq!(
            bytemuck::cast_slice::<ToolPrimitive, u8>(&resolved.render_tools),
            bytemuck::cast_slice::<ToolPrimitive, u8>(&tools),
            "the very tools the editor places"
        );
    }

    /// A planar design has no render tools, so the previews draw and measure it exactly as
    /// before.
    #[test]
    fn a_planar_design_has_no_render_tools() {
        let text = design_file_text(MaterialSelection::none(), None, None);
        let resolved = resolve_catalogue_planes(&record_with("flat.isdd", text.into_bytes()));
        assert!(resolved.render_tools.is_empty() && resolved.render_placements.is_empty());
        assert_eq!(
            resolve_catalogue_planes(&angle_table_record()).render_tools,
            [] as [indicatrix::geometry::ToolPrimitive; 0]
        );
    }

    /// A record with no design file but concave rows in its angle table draws and measures
    /// its tools; `tools` (the planner's) stays empty and `concave_error` unset.
    #[test]
    fn concave_rows_of_an_angle_table_become_render_tools() {
        let mut full = angle_table_record();
        full.angle_settings
            .extend(concave_rows(&Design::concave_fixture()));
        let resolved = resolve_catalogue_planes(&full);

        assert_eq!(resolved.source, CataloguePlanesSource::AngleTable);
        assert!(resolved.tools.is_empty() && resolved.concave_error.is_none());
        assert!(
            !resolved.render_tools.is_empty(),
            "the table's concave rows are tools of the stone"
        );
        assert_eq!(
            resolved.render_tools.len(),
            resolved.render_placements.len()
        );
    }

    /// A row that cannot be read leaves no tools: the flat stone, never a half-resolved set.
    #[test]
    fn an_unreadable_concave_row_draws_the_flat_stone() {
        let mut full = angle_table_record();
        let mut rows = concave_rows(&Design::concave_fixture());
        rows[0].tool_line = Some("not a tool line".to_owned());
        full.angle_settings.extend(rows);
        assert_eq!(
            resolve_catalogue_planes(&full).render_tools,
            [] as [indicatrix::geometry::ToolPrimitive; 0]
        );
    }

    /// The frosted planes of a catalogue design are the planes of the frosted tiers: the
    /// record's stored rows apply to the design read from its design file, a design without
    /// rows names none, and the angle-table path (no notation for frosting) names none.
    #[test]
    fn the_stored_frosting_names_the_frosted_planes_of_the_stone() {
        use crate::gui::batch::test_records::gem_record;
        use indicatrix_vault::model::frosting::StoredTierFrosting;

        let mut full = gem_record();
        let (polished, none) = resolve_catalogue_planes_with_frosting(&full);
        assert_eq!(polished.source, CataloguePlanesSource::DesignFile);
        assert!(none.is_empty(), "no rows: nothing is frosted");

        // The synthetic gem's tier 2 is its crown, eight facets.
        full.frosting = vec![StoredTierFrosting::whole(2)];
        let (frosted_stone, frosted) = resolve_catalogue_planes_with_frosting(&full);
        assert_eq!(
            frosted_stone.planes.len(),
            polished.planes.len(),
            "frosting changes no geometry"
        );
        assert_eq!(frosted.len(), 8, "{frosted:?}");
        assert!(
            frosted.windows(2).all(|pair| pair[0] < pair[1]),
            "ascending"
        );
        assert!(
            frosted.iter().all(|&index| {
                let index = index as usize;
                index >= frosted_stone.preform_plane_count && index < frosted_stone.planes.len()
            }),
            "facet planes of the stone: {frosted:?}"
        );
        assert_eq!(
            resolve_catalogue_planes(&full).planes.len(),
            frosted_stone.planes.len(),
            "the plain resolver is the same stone"
        );

        // The angle table states no frosting, so even stored rows name no plane there.
        let mut table = angle_table_record();
        table.frosting = vec![StoredTierFrosting::whole(0)];
        let (resolved, frosted) = resolve_catalogue_planes_with_frosting(&table);
        assert_eq!(resolved.source, CataloguePlanesSource::AngleTable);
        assert_eq!(frosted, [] as [u32; 0]);
    }

    /// The angle table carries no material, size or snapshot.
    #[test]
    fn the_angle_table_path_has_no_material_size_or_snapshot() {
        let resolved = resolve_catalogue_planes(&angle_table_record());
        assert_eq!(resolved.source, CataloguePlanesSource::AngleTable);
        assert!(resolved.material.is_none());
        assert_eq!(resolved.girdle_diameter_mm, None);
        assert!(resolved.custom_material.is_none());
    }
}
