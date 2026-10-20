//! The material a catalogue design is drawn and scored in by the library's batches
//! (cached previews, tilt curves), decided by the one rule the editor's viewport and the
//! exports use.
//!
//! # The rule
//!
//! 1. A design whose file names a material gets THAT material
//!    ([`design_file_material`]): looked up as the editor looks it up
//!    (`indicatrix_editor::material_lookup`: the vault's custom materials over the
//!    built-ins), with the file's refractive-index and body-color overrides applied on
//!    top; a custom material the vault lacks is rebuilt from the snapshot the design file
//!    carries. The name is persisted into `diagram_previews.preview_material`,
//!    OVERWRITING (the file is the authority).
//! 2. Otherwise -- no name, or a name that resolves nowhere (the viewport would refuse
//!    such a design; a batch has to render SOMETHING) -- the refractive-index pick of
//!    `super::material_choice::ensure_balanced_material`, unchanged and write-once.
//! 3. Either way the material is SIZED by the design's girdle diameter
//!    (`indicatrix::render_setup::material_for_stone`; `0.0` is "not set", the 7 mm
//!    calibration), measured on the design's whole plane list, preform planes included,
//!    exactly as the live loop and the exports measure it.
//!
//! The library detail view applies the same first step
//! (`gui::library::detail::planes::apply_catalogue_material`), so a design's thumbnail,
//! its tilt curves, its detail view and its exports all show one stone.

use super::{material_choice::ensure_balanced_material, preview::RI_MATCH_TOLERANCE};
use crate::gui::editor::CataloguePlanes;
use indicatrix::{optics::materials::GemMaterial, render_setup};
use indicatrix_cut_core::{
    material::MaterialSelection,
    native::{CustomMaterialSnapshot, gem_material_from_custom_snapshot},
};
use indicatrix_editor::material_lookup::{EditorMaterialLookup, traced_gem_material};
use indicatrix_vault::{
    db::sqlite::Database,
    model::{material_match::RiPresetCandidate, preview::PreviewMaterialSource},
};
use std::sync::{Mutex, PoisonError};
use tracing::debug;

/// How a catalogue design's traced material was decided: `DesignFile` when the design's
/// file names it (the editor's own rule), `RefractiveIndex` for the balanced pick among
/// the materials that fit the design's index, persisted write-once.
pub type CatalogueMaterialSource = PreviewMaterialSource;

/// A catalogue design's traced material, sized, and how it was decided.
#[derive(Debug, Clone)]
pub struct CatalogueMaterial {
    /// The material to trace, already sized for the design's girdle diameter.
    pub material: GemMaterial,
    /// The name persisted as the design's preview material: the design file's material
    /// name, or the refractive-index pick's.
    pub name: String,
    /// How [`Self::name`] was decided.
    pub source: CatalogueMaterialSource,
    /// The girdle diameter in millimetres the material was sized with; `0.0` is "not
    /// set". The fingerprint of the cached result names it.
    pub girdle_mm: f64,
}

/// The material a design file names, as the editor resolves it, and its name: `selection`'s
/// name looked up in `custom` (case-insensitively, over the built-ins), with the
/// selection's refractive-index and body-color overrides applied; or, when nothing
/// answers to the name but the file carried `snapshot` of that custom material, the
/// material rebuilt from it, with the same overrides.
///
/// The material is NOT sized: the viewport sizes it every frame from the stone width,
/// the batches by [`catalogue_material`].
///
/// `None` when the selection names no material, or a name that resolves nowhere and has
/// no snapshot: the editor refuses such a design, and a caller falls back to the next
/// rule instead.
#[must_use]
pub fn design_file_material(
    selection: &MaterialSelection,
    snapshot: Option<&CustomMaterialSnapshot>,
    custom: &[GemMaterial],
) -> Option<(String, GemMaterial)> {
    let name = selection
        .name
        .as_deref()
        .filter(|name| !name.trim().is_empty())?;
    let lookup = EditorMaterialLookup::new(custom);
    let material = traced_gem_material(name, selection, &lookup).or_else(|| {
        snapshot.map(|snapshot| {
            selection.apply_overrides(gem_material_from_custom_snapshot(name, snapshot))
        })
    })?;
    Some((name.to_string(), material))
}

/// The material `resolved`'s design is traced in by a batch, sized, with its name and
/// provenance persisted into the design's `diagram_previews` row. See this module's doc
/// comment for the rule.
///
/// `custom` is the vault's custom materials ([`load_custom_materials`], loaded once per
/// batch); `candidates` and `target_ri` feed the refractive-index pick of the second
/// rule. Persistence: a design-file material is written over whatever the row held
/// (`Database::set_preview_material`, with the size and `DesignFile`); the
/// refractive-index pick stays write-once (`Database::ensure_preview_material_by`), after
/// a name an earlier design-file resolution left behind is forgotten
/// (`Database::clear_design_preview_material`), and only the size is recorded beside it.
///
/// `Ok(None)` when the refractive-index pick finds nothing that fits (no candidates, or
/// the persisted name is not a built-in material), as before.
///
/// # Errors
///
/// The database error of the persisting call.
pub fn catalogue_material(
    db: &Database,
    entry_id: i64,
    resolved: &CataloguePlanes,
    custom: &[GemMaterial],
    candidates: &[RiPresetCandidate],
    target_ri: f64,
) -> anyhow::Result<Option<CatalogueMaterial>> {
    let girdle_mm = resolved.girdle_diameter_mm.unwrap_or(0.0);
    let from_file = resolved.material.as_ref().and_then(|selection| {
        design_file_material(selection, resolved.custom_material.as_ref(), custom)
    });
    if let Some((name, material)) = from_file {
        db.set_preview_material(
            entry_id,
            &name,
            resolved.girdle_diameter_mm,
            CatalogueMaterialSource::DesignFile,
        )?;
        return Ok(Some(CatalogueMaterial {
            material: sized(material, girdle_mm, resolved),
            name,
            source: CatalogueMaterialSource::DesignFile,
            girdle_mm,
        }));
    }
    if let Some(name) = resolved
        .material
        .as_ref()
        .and_then(|selection| selection.name.as_deref())
    {
        debug!(
            entry_id,
            material = name,
            "the design file's material resolves nowhere; using the refractive-index pick"
        );
    }

    // A name taken from the design file on an earlier run must not outlive the file's
    // naming it: the pick below is write-once and would return it.
    db.clear_design_preview_material(entry_id)?;
    let Some(name) = ensure_balanced_material(
        db,
        entry_id,
        target_ri,
        candidates,
        RI_MATCH_TOLERANCE,
        &resolved.planes,
    )?
    else {
        return Ok(None);
    };
    let Some(material) = GemMaterial::by_name(&name) else {
        return Ok(None);
    };
    db.set_preview_girdle_mm(entry_id, resolved.girdle_diameter_mm)?;
    Ok(Some(CatalogueMaterial {
        material: sized(material, girdle_mm, resolved),
        name,
        source: CatalogueMaterialSource::RefractiveIndex,
        girdle_mm,
    }))
}

/// `material` sized for a stone of `girdle_mm` (`0.0`: not set) on `resolved`'s planes.
///
/// The WHOLE plane list goes in, preform planes included: that is what the live loop
/// (`RenderContext::active_planes`, the editor's `design_to_gpu_planes`) and the exports
/// measure the stone width from, and the pick of
/// [`ensure_balanced_material`] has always scored on the same list.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the render material precision; a girdle width in mm is in the normal float range"
)]
fn sized(material: GemMaterial, girdle_mm: f64, resolved: &CataloguePlanes) -> GemMaterial {
    render_setup::material_for_stone(material, girdle_mm as f32, &resolved.planes)
}

/// The vault's custom materials, as the viewport holds them in `RenderContext::
/// custom_materials` (`gui::main_window::state::apply_custom_materials`): the rows
/// converted to materials and their stored zones attached at the
/// default stone width. Read once per batch, so every design of the batch resolves a
/// material name against the same list.
#[must_use]
pub fn load_custom_materials(db: &Mutex<Database>) -> Vec<GemMaterial> {
    let guard = db.lock().unwrap_or_else(PoisonError::into_inner);
    let rows = guard.get_custom_materials().unwrap_or_default();
    let (materials, _specific_gravity) =
        crate::gui::custom_material_startup::initial_custom_materials_and_specific_gravity(&rows);
    let zoned = with_zones(&guard, materials);
    drop(guard);
    zoned
}

/// `materials` with their stored colour zones attached, so a zoned custom material resolves as
/// the viewport resolves it.
fn with_zones(db: &Database, mut materials: Vec<GemMaterial>) -> Vec<GemMaterial> {
    crate::gui::rough_colour::store::attach_zoning(db, &mut materials, 0.0);
    materials
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::editor::CataloguePlanesSource;
    use indicatrix::geometry::{GpuFacetPlane, cuts::StandardGemCuts};
    use indicatrix_vault::model::{entry::FacetingDiagramEntry, preview::PreviewMaterialState};

    /// An in-memory database with one design.
    fn db_with_one_entry() -> (Database, i64) {
        let db = Database::new(Some(":memory:")).expect("an in-memory database opens");
        let entry_id = db
            .save_diagram_entry(
                &FacetingDiagramEntry {
                    title: "Catalogue Material".to_string(),
                    url: "local://catalogue-material.asc".to_string(),
                    design_id: String::new(),
                },
                "local-import",
            )
            .expect("the entry saves");
        (db, entry_id)
    }

    fn planes() -> Vec<GpuFacetPlane> {
        StandardGemCuts::standard_round_brilliant()
    }

    /// A design-file resolution of the standard round brilliant with the given material,
    /// size and snapshot.
    fn resolved(
        material: Option<MaterialSelection>,
        girdle_diameter_mm: Option<f64>,
        custom_material: Option<CustomMaterialSnapshot>,
    ) -> CataloguePlanes {
        CataloguePlanes {
            planes: planes(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            source: CataloguePlanesSource::DesignFile,
            preform_plane_count: 0,
            tools: Vec::new(),
            concave_error: None,
            render_tools: Vec::new(),
            render_placements: Vec::new(),
            material,
            girdle_diameter_mm,
            custom_material,
        }
    }

    fn named(name: &str) -> MaterialSelection {
        MaterialSelection {
            name: Some(name.to_string()),
            ..MaterialSelection::none()
        }
    }

    /// One candidate, so the refractive-index pick needs no raytraced scoring.
    fn quartz_only() -> [RiPresetCandidate; 1] {
        [RiPresetCandidate {
            name: "Quartz".to_string(),
            refractive_index: 1.545,
        }]
    }

    fn garnet() -> GemMaterial {
        GemMaterial::new_custom("My Garnet", 1.74, 0.02, 0.0, [0.0; 3])
    }

    fn state(db: &Database, entry_id: i64) -> PreviewMaterialState {
        db.get_preview_material_state(entry_id)
            .expect("the state reads")
    }

    /// A design naming Sapphire, with a body-color override and a girdle diameter of 9 mm,
    /// is traced as Sapphire with the overridden absorption, sized for 9 mm, and recorded
    /// as a design-file material.
    #[test]
    fn a_design_file_material_is_traced_with_its_overrides_at_its_size() {
        let (db, entry_id) = db_with_one_entry();
        let yellow = [0.2_f32, 0.4, 2.8];
        let selection = named("Sapphire").with_body_color(Some(yellow));
        let resolved = resolved(Some(selection.clone()), Some(9.0), None);

        let got = catalogue_material(&db, entry_id, &resolved, &[], &[], 1.76)
            .unwrap()
            .expect("Sapphire resolves");

        assert_eq!(got.name, "Sapphire");
        assert_eq!(got.source, CatalogueMaterialSource::DesignFile);
        assert_eq!(got.girdle_mm, 9.0);
        let traced = selection.apply_overrides(GemMaterial::by_name("Sapphire").unwrap());
        assert_eq!(got.material.absorption, traced.absorption);
        assert_ne!(
            got.material.absorption,
            GemMaterial::by_name("Sapphire").unwrap().absorption,
            "the body color replaced the species' own absorption"
        );
        assert_eq!(
            got.material,
            render_setup::material_for_stone(traced, 9.0, &planes())
        );
        assert_eq!(
            state(&db, entry_id),
            PreviewMaterialState {
                name: Some("Sapphire".to_string()),
                girdle_mm: Some(9.0),
                source: PreviewMaterialSource::DesignFile,
            }
        );
    }

    /// The refractive-index override flattens the dispersion at the typed value, as it
    /// does in the viewport.
    #[test]
    fn the_refractive_index_override_applies() {
        let (db, entry_id) = db_with_one_entry();
        let selection = MaterialSelection {
            refractive_index_override: Some(1.66),
            ..named("Quartz")
        };
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(selection), None, None),
            &[],
            &[],
            1.54,
        )
        .unwrap()
        .expect("Quartz resolves");
        for lambda in [450.0_f32, 589.3, 700.0] {
            assert!((f64::from(got.material.dispersion.evaluate(lambda)) - 1.66).abs() < 1e-4);
        }
    }

    /// A design naming a custom material the vault has uses that material (custom
    /// over built-in), unsized when the file sets no girdle diameter.
    #[test]
    fn a_design_naming_a_custom_material_uses_the_vaults_one() {
        let (db, entry_id) = db_with_one_entry();
        let custom = [garnet()];
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(named("my garnet")), None, None),
            &custom,
            &[],
            1.54,
        )
        .unwrap()
        .expect("the custom material resolves");

        assert_eq!(got.name, "my garnet");
        assert_eq!(got.source, CatalogueMaterialSource::DesignFile);
        assert_eq!(got.girdle_mm, 0.0);
        assert_eq!(
            got.material,
            render_setup::material_for_stone(garnet(), 0.0, &planes())
        );
        assert_eq!(state(&db, entry_id).girdle_mm, None);
    }

    /// A material the vault lacks is rebuilt from the design file's snapshot, with the
    /// selection's overrides on top.
    #[test]
    fn a_missing_custom_material_is_rebuilt_from_the_snapshot() {
        let (db, entry_id) = db_with_one_entry();
        let snapshot = CustomMaterialSnapshot::new(1.74, 0.02, 0.0, None, "Cubic", "Isotropic");
        let selection = MaterialSelection {
            refractive_index_override: Some(1.80),
            ..named("My Garnet")
        };
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(selection.clone()), Some(8.0), Some(snapshot.clone())),
            &[],
            &[],
            1.54,
        )
        .unwrap()
        .expect("the snapshot restores the material");

        assert_eq!(got.name, "My Garnet");
        assert_eq!(got.source, CatalogueMaterialSource::DesignFile);
        let expected =
            selection.apply_overrides(gem_material_from_custom_snapshot("My Garnet", &snapshot));
        assert_eq!(
            got.material,
            render_setup::material_for_stone(expected, 8.0, &planes())
        );
        assert!((f64::from(got.material.dispersion.evaluate(450.0)) - 1.80).abs() < 1e-4);

        // The vault's own material wins over the snapshot when both exist.
        let (db, entry_id) = db_with_one_entry();
        let vault_garnet = GemMaterial::new_custom("My Garnet", 1.70, 0.0, 0.0, [0.0; 3]);
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(named("My Garnet")), None, Some(snapshot)),
            std::slice::from_ref(&vault_garnet),
            &[],
            1.54,
        )
        .unwrap()
        .expect("resolves");
        assert_eq!(
            got.material,
            render_setup::material_for_stone(vault_garnet, 0.0, &planes())
        );
    }

    /// A design that names no material falls back to the refractive-index pick, unsized,
    /// exactly as the batches resolved every design before: the persisted name, the
    /// material and the width are what `ensure_balanced_material` and `material_for_stone`
    /// at width 0 gave.
    #[test]
    fn a_design_without_a_material_name_keeps_the_refractive_index_pick() {
        let (db, entry_id) = db_with_one_entry();
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(MaterialSelection::none()), None, None),
            &[],
            &quartz_only(),
            1.544,
        )
        .unwrap()
        .expect("Quartz fits");

        assert_eq!(got.name, "Quartz");
        assert_eq!(got.source, CatalogueMaterialSource::RefractiveIndex);
        assert_eq!(got.girdle_mm, 0.0);
        assert_eq!(
            got.material,
            render_setup::material_for_stone(
                GemMaterial::by_name("Quartz").unwrap(),
                0.0,
                &planes()
            )
        );
        assert_eq!(
            state(&db, entry_id),
            PreviewMaterialState {
                name: Some("Quartz".to_string()),
                girdle_mm: None,
                source: PreviewMaterialSource::RefractiveIndex,
            }
        );

        // The angle-table path has no material selection at all: the same answer.
        let (db, entry_id) = db_with_one_entry();
        let angle_table = CataloguePlanes {
            source: CataloguePlanesSource::AngleTable,
            ..resolved(None, None, None)
        };
        let again = catalogue_material(&db, entry_id, &angle_table, &[], &quartz_only(), 1.544)
            .unwrap()
            .expect("Quartz fits");
        assert_eq!(again.material, got.material);
        assert_eq!(again.name, got.name);
    }

    /// A name that resolves nowhere and has no snapshot does not refuse the design (the
    /// viewport would): the refractive-index pick renders it.
    #[test]
    fn a_name_that_resolves_nowhere_falls_through_to_the_pick() {
        let (db, entry_id) = db_with_one_entry();
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(named("Deleted Custom Garnet")), None, None),
            &[],
            &quartz_only(),
            1.544,
        )
        .unwrap()
        .expect("the pick renders it");
        assert_eq!(got.name, "Quartz");
        assert_eq!(got.source, CatalogueMaterialSource::RefractiveIndex);
    }

    /// The pick is sized by the file's girdle diameter too, and the size is recorded
    /// beside the pick (which stays write-once).
    #[test]
    fn the_pick_is_sized_by_the_files_girdle_diameter() {
        let (db, entry_id) = db_with_one_entry();
        let got = catalogue_material(
            &db,
            entry_id,
            &resolved(Some(MaterialSelection::none()), Some(9.0), None),
            &[],
            &quartz_only(),
            1.544,
        )
        .unwrap()
        .expect("Quartz fits");

        assert_eq!(got.source, CatalogueMaterialSource::RefractiveIndex);
        assert_eq!(got.girdle_mm, 9.0);
        assert_eq!(
            got.material,
            render_setup::material_for_stone(
                GemMaterial::by_name("Quartz").unwrap(),
                9.0,
                &planes()
            )
        );
        assert_eq!(
            state(&db, entry_id),
            PreviewMaterialState {
                name: Some("Quartz".to_string()),
                girdle_mm: Some(9.0),
                source: PreviewMaterialSource::RefractiveIndex,
            }
        );
    }

    /// The design file is the authority: a later resolution overwrites the material an
    /// earlier one persisted, and a design whose file stops naming one forgets it and
    /// takes the pick.
    #[test]
    fn the_design_file_overwrites_and_a_dropped_name_is_forgotten() {
        let (db, entry_id) = db_with_one_entry();
        let candidates = quartz_only();
        let named_sapphire = resolved(Some(named("Sapphire")), Some(9.0), None);
        let first = catalogue_material(&db, entry_id, &named_sapphire, &[], &candidates, 1.544)
            .unwrap()
            .expect("resolves");
        assert_eq!(first.name, "Sapphire");

        let named_ruby = resolved(Some(named("Ruby")), None, None);
        let second = catalogue_material(&db, entry_id, &named_ruby, &[], &candidates, 1.544)
            .unwrap()
            .expect("resolves");
        assert_eq!(second.name, "Ruby");
        assert_eq!(state(&db, entry_id).name.as_deref(), Some("Ruby"));
        assert_eq!(state(&db, entry_id).girdle_mm, None);

        let unnamed = resolved(Some(MaterialSelection::none()), None, None);
        let third = catalogue_material(&db, entry_id, &unnamed, &[], &candidates, 1.544)
            .unwrap()
            .expect("the pick resolves");
        assert_eq!(third.name, "Quartz");
        assert_eq!(third.source, CatalogueMaterialSource::RefractiveIndex);
        assert_eq!(
            state(&db, entry_id).source,
            PreviewMaterialSource::RefractiveIndex
        );
    }

    /// `design_file_material` is the shared first rule, also the library detail view's.
    #[test]
    fn design_file_material_names_nothing_for_an_unnamed_or_unresolvable_selection() {
        assert!(design_file_material(&MaterialSelection::none(), None, &[]).is_none());
        assert!(design_file_material(&named("   "), None, &[]).is_none());
        assert!(design_file_material(&named("Nowhere At All"), None, &[]).is_none());
        let (name, material) = design_file_material(&named("Quartz"), None, &[]).unwrap();
        assert_eq!(name, "Quartz");
        assert_eq!(material.name, "Quartz");
    }
}
