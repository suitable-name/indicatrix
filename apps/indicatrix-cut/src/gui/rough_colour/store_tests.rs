//! Tests of the rough-colour storage API against an in-memory vault. Deliberately outside
//! `gui::rough_plan`: nothing here starts the planner.

use super::store::{
    AdoptRequest, HostMaterial, adopt_stone, attach_job_zoning, attach_zoning, cached_views,
    choose_and_save_poses, clear_pose_choices, delete_job_zoning, delete_material_zoning,
    delete_rough_colour, delete_view_photos, load_material_zoning, load_pose_choices,
    load_rough_colour, load_view_photos, planner_stone_material, plans_with_rough_colour,
    posed_stone_pose, prune_orphans, save_job_zoning, save_layout_pose_choices,
    save_material_zoning, save_relative_material_zoning, save_rough_colour, save_view_photos,
    with_stored_zoning,
};
use crate::gui::optics::crystal_optics::gem_material_from_row;
use glam::DVec3;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        fluorescence::Fluorescence,
        materials::{AbsorptionUnit, GemMaterial},
        raytracer::LightingPreset,
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    },
};
use indicatrix_cut_core::rough_plan::{
    Axis, CutOrder, CutPlan, PlacedStone, RoughLayout,
    fit::StonePose,
    photometry::{PixelMask, ResampledImage, WorkingGrid, flag},
    zoned_plan::{
        DesignPlacement, PoseGoal, RoughColour, StonePlacement, ZONING_FORMAT_VERSION, encode_zoned,
    },
};
use indicatrix_net::{SceneState, scene::SceneEnvironment};
use indicatrix_vault::{db::sqlite::Database, model::zoning::RoughColourRow};

fn db() -> Database {
    Database::new(Some(":memory:")).expect("in-memory vault")
}

fn absorber(centre_nm: f32, peak: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        centre_nm, 40.0, peak,
    )]))
}

/// A pale rind with a red rod along x through `(y, z) = (-3, 0)`, 3 mm in radius.
fn watermelon() -> ZonedAbsorption {
    let mut zoned = ZonedAbsorption::new(absorber(480.0, 0.05));
    zoned.zones.push(Zone {
        shape: ZoneShape::CoaxialCylinder {
            axis_point: DVec3::new(0.0, -3.0, 0.0),
            axis_dir: DVec3::X,
            r_in: 0.0,
            r_out: 3.0,
        },
        absorption: absorber(600.0, 0.9),
    });
    zoned
}

fn colour(zoned: ZonedAbsorption) -> RoughColour {
    RoughColour {
        zoned,
        fit_json: "{\"fit\":true}".to_string(),
        version: ZONING_FORMAT_VERSION,
        created: 1_700_000_123,
    }
}

fn pose(mm_per_unit: f64) -> StonePose {
    StonePose {
        center_mm: [0.0; 3],
        axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        mm_per_unit,
    }
}

fn cube_stone() -> PlacedStone {
    PlacedStone {
        entry_id: 1,
        piece_origin_mm: [-3.0; 3],
        piece_size_mm: [6.0; 3],
        stone_size_mm: [6.0; 3],
        table_axis: Axis::Y,
        carat: 1.0,
        volume_mm3: 100.0,
        pose: pose(1.0),
    }
}

fn layout(stones: usize, exact_fit: bool) -> RoughLayout {
    RoughLayout {
        cut_order: CutOrder::Xyz,
        stones: vec![cube_stone(); stones],
        cut_plan: CutPlan { slabs: Vec::new() },
        total_carat: 1.0,
        total_volume_mm3: 100.0,
        yield_fraction: 0.5,
        exact_fit,
    }
}

// ---- rough colour ----------------------------------------------------------------------------

#[test]
fn a_rough_colour_round_trips_and_is_listed_and_deleted() {
    let db = db();
    assert_eq!(load_rough_colour(&db, 4).unwrap(), None);
    let stored = colour(watermelon());
    save_rough_colour(&db, 4, &stored).unwrap();
    assert_eq!(load_rough_colour(&db, 4).unwrap(), Some(stored));
    assert_eq!(plans_with_rough_colour(&db).unwrap().len(), 1);
    delete_rough_colour(&db, 4).unwrap();
    assert_eq!(load_rough_colour(&db, 4).unwrap(), None);
}

#[test]
fn an_unusable_or_newer_rough_colour_row_is_an_error_not_a_panic() {
    let db = db();
    db.save_rough_colour(&RoughColourRow {
        plan_id: 1,
        zoned_json: "{ not json".to_string(),
        fit_json: "{}".to_string(),
        version: ZONING_FORMAT_VERSION,
        created: 0,
    })
    .unwrap();
    let message = format!("{:#}", load_rough_colour(&db, 1).unwrap_err());
    assert!(message.contains("saved plan 1"), "{message}");

    db.save_rough_colour(&RoughColourRow {
        plan_id: 2,
        zoned_json: encode_zoned(&watermelon()).unwrap(),
        fit_json: "{}".to_string(),
        version: ZONING_FORMAT_VERSION + 1,
        created: 0,
    })
    .unwrap();
    assert!(format!("{:#}", load_rough_colour(&db, 2).unwrap_err()).contains("newer"));
}

// ---- photos ----------------------------------------------------------------------------------

fn view_image() -> ResampledImage {
    let mut mask = PixelMask::new(2, 2);
    mask.set(0, 1, flag::SATURATED);
    ResampledImage {
        grid: WorkingGrid {
            width: 2,
            height: 2,
            origin: [3.5, 4.25],
            scale: 2.0,
        },
        values: vec![
            [0.5, 0.25, 0.125],
            [1.0, 0.0, 0.5],
            [0.75, 0.75, 0.75],
            [0.0; 3],
        ],
        variance: vec![[1e-4; 3], [f32::INFINITY; 3], [2e-4; 3], [3e-4; 3]],
        coverage: vec![1.0, 0.5, 1.0, 0.0],
        mask,
    }
}

#[test]
fn cached_photos_round_trip_per_view_and_delete_with_the_plan() {
    let db = db();
    assert_eq!(load_view_photos(&db, 3, 0).unwrap(), None);
    save_view_photos(&db, 3, 2, &view_image()).unwrap();
    save_view_photos(&db, 3, 5, &view_image()).unwrap();
    assert_eq!(cached_views(&db, 3).unwrap(), vec![2, 5]);
    assert_eq!(load_view_photos(&db, 3, 2).unwrap(), Some(view_image()));
    assert_eq!(load_view_photos(&db, 3, 3).unwrap(), None);
    assert_eq!(delete_view_photos(&db, 3).unwrap(), 10);
    assert_eq!(cached_views(&db, 3).unwrap(), [] as [u32; 0]);

    save_view_photos(&db, 3, 2, &view_image()).unwrap();
    delete_rough_colour(&db, 3).unwrap();
    assert_eq!(cached_views(&db, 3).unwrap(), [] as [u32; 0]);
}

// ---- pose choice -----------------------------------------------------------------------------

#[test]
fn the_pose_choice_is_stored_per_stone_and_read_back() {
    let db = db();
    let two = layout(2, false);
    let zoned = watermelon();

    // The default keeps the planner's poses and stores nothing.
    let kept = choose_and_save_poses(&db, 8, 0, &two, &zoned, PoseGoal::KeepCanonical).unwrap();
    assert_eq!(kept, vec![0, 0]);
    assert!(load_pose_choices(&db, 8).unwrap().is_empty());

    // "Best colour" for the core turns both stones over.
    let chosen = choose_and_save_poses(&db, 8, 1, &two, &zoned, PoseGoal::MostOfZone(1)).unwrap();
    assert_eq!(chosen, vec![1, 1]);
    let stored = load_pose_choices(&db, 8).unwrap();
    assert_eq!(stored.len(), 2);
    assert_eq!(stored.get(&(1, 0)), Some(&1));

    let stone = cube_stone();
    let turned = posed_stone_pose(&stored, 1, 0, &stone);
    assert!(
        (turned.axes[1][1] + 1.0).abs() < 1e-12,
        "the table faces down now"
    );
    // A stone of another layout, or one without a choice, keeps the planner's pose.
    assert_eq!(posed_stone_pose(&stored, 0, 0, &stone), stone.pose);

    // An exact single-stone fit has a unique pose.
    let exact = layout(1, true);
    let unique = choose_and_save_poses(&db, 9, 0, &exact, &zoned, PoseGoal::MostOfZone(1)).unwrap();
    assert_eq!(unique, vec![0]);

    // Going back to the default clears the layout's choices.
    choose_and_save_poses(&db, 8, 1, &two, &zoned, PoseGoal::KeepCanonical).unwrap();
    assert!(load_pose_choices(&db, 8).unwrap().is_empty());
    assert!(save_layout_pose_choices(&db, 8, 0, &[0, 4]).is_err());
    assert_eq!(clear_pose_choices(&db, 8).unwrap(), 0);
}

// ---- planner preview -------------------------------------------------------------------------

#[test]
fn the_planner_previews_a_stone_with_the_plans_zones_in_its_frame() {
    let db = db();
    let placement = StonePlacement {
        pose: pose(1.0),
        design: DesignPlacement::IDENTITY,
    };
    let host = GemMaterial::diamond();
    assert!(
        planner_stone_material(&db, 6, &host, &placement)
            .unwrap()
            .is_none()
    );
    save_rough_colour(&db, 6, &colour(watermelon())).unwrap();
    let material = planner_stone_material(&db, 6, &host, &placement)
        .unwrap()
        .expect("the plan has a colour");
    assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
    assert_eq!(material.zoning.as_ref().unwrap().zones.len(), 1);
}

// ---- adopt, and resolution re-attaching the zones --------------------------------------------

fn request<'a>(rough: &'a ZonedAbsorption, host: HostMaterial<'a>) -> AdoptRequest<'a> {
    AdoptRequest {
        rough_name: "Melon",
        rough_zoned: rough,
        placement: StonePlacement {
            pose: pose(2.0),
            design: DesignPlacement::IDENTITY,
        },
        stone_width_mm: 7.0,
        host,
    }
}

#[test]
fn adopting_a_stone_creates_the_material_row_and_the_zoning_row() {
    let db = db();
    let rough = watermelon();
    let host = GemMaterial::by_name("Quartz").unwrap_or_else(GemMaterial::diamond);
    let outcome = adopt_stone(&db, &request(&rough, HostMaterial::BuiltIn(&host))).unwrap();
    assert_eq!(outcome.material_name, "Melon colour");
    assert!(!outcome.replaced);
    assert!((outcome.stone_width_mm - 7.0).abs() < 1e-12);

    // The ordinary row carries the BASE zone as band rows, so a build that ignores zones shows it.
    let rows = db.get_custom_materials().unwrap();
    let row = rows
        .iter()
        .find(|r| r.name == "Melon colour")
        .expect("material row");
    assert!(row.color_recipe_json.is_none());
    let bands = row.absorption_bands_json.as_deref().expect("band rows");
    assert!(bands.contains("480"), "{bands}");
    assert_ne!(
        gem_material_from_row(row).absorption.o_ray,
        [] as [indicatrix::optics::absorption::AbsorptionBand; 0]
    );

    // The zones are in the side table, in mm, not relative to the stone.
    let stored = load_material_zoning(&db, "melon COLOUR")
        .unwrap()
        .expect("zoning row");
    assert!(!stored.relative_to_stone);
    assert_eq!(Some(&stored.zoning), outcome.material.zoning.as_ref());
    assert_eq!(outcome.material.absorption_unit, AbsorptionUnit::PerMm);
    assert!((outcome.material.absorption_path_scale - 2.0).abs() < 1e-6);

    // Adopting again replaces both rows.
    let again = adopt_stone(&db, &request(&rough, HostMaterial::BuiltIn(&host))).unwrap();
    assert!(again.replaced);
    assert_eq!(db.get_custom_materials().unwrap().len(), 1);
}

#[test]
fn adopting_from_a_catalogue_host_copies_its_optics() {
    let db = db();
    let rough = watermelon();
    let host_material = GemMaterial::by_name("Quartz").unwrap_or_else(GemMaterial::diamond);
    let first = adopt_stone(&db, &request(&rough, HostMaterial::BuiltIn(&host_material))).unwrap();
    let host_row = db
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|r| r.name == first.material_name)
        .unwrap();

    let mut second_request = request(&rough, HostMaterial::Custom(&host_row));
    second_request.rough_name = "Second";
    let second = adopt_stone(&db, &second_request).unwrap();
    assert_eq!(second.material_name, "Second colour");
    let copy = db
        .get_custom_materials()
        .unwrap()
        .into_iter()
        .find(|r| r.name == "Second colour")
        .unwrap();
    assert!((copy.refractive_index - host_row.refractive_index).abs() < 1e-6);
    assert_eq!(copy.specific_gravity, host_row.specific_gravity);
    assert_eq!(copy.crystal_system, host_row.crystal_system);
}

#[test]
fn adopting_refuses_a_bad_width_and_an_unusable_pose_and_writes_nothing() {
    let db = db();
    let rough = watermelon();
    let host = GemMaterial::diamond();
    let mut bad_width = request(&rough, HostMaterial::BuiltIn(&host));
    bad_width.stone_width_mm = 0.0;
    assert!(adopt_stone(&db, &bad_width).is_err());
    let mut bad_pose = request(&rough, HostMaterial::BuiltIn(&host));
    bad_pose.placement.pose.mm_per_unit = 0.0;
    assert!(adopt_stone(&db, &bad_pose).is_err());
    assert_eq!(
        db.get_custom_materials().unwrap(),
        [] as [indicatrix_vault::model::material::CustomMaterialRow; 0]
    );
    assert_eq!(
        db.all_material_zonings().unwrap(),
        [] as [indicatrix_vault::model::zoning::MaterialZoningRow; 0]
    );

    // A rough without a usable name still adopts, under the stem "Rough".
    let mut unnamed = request(&rough, HostMaterial::BuiltIn(&host));
    unnamed.rough_name = " ";
    assert_eq!(
        adopt_stone(&db, &unnamed).unwrap().material_name,
        "Rough colour"
    );
}

#[test]
fn material_resolution_puts_the_stored_zones_back_on_the_custom_materials() {
    let db = db();
    let rough = watermelon();
    let host = GemMaterial::by_name("Quartz").unwrap_or_else(GemMaterial::diamond);
    let outcome = adopt_stone(&db, &request(&rough, HostMaterial::BuiltIn(&host))).unwrap();
    // A second, plain custom material in the same list.
    let mut plain_row = db.get_custom_materials().unwrap()[0].clone();
    plain_row.name = "Plain".to_string();
    plain_row.absorption_bands_json = None;
    db.save_custom_material(&indicatrix_vault::db::sqlite::CustomMaterialParams {
        name: &plain_row.name,
        refractive_index: plain_row.refractive_index,
        dispersion: plain_row.dispersion,
        birefringence: plain_row.birefringence,
        absorption_rgb: plain_row.absorption_rgb,
        crystal_system: plain_row.crystal_system.as_deref(),
        optical_character: plain_row.optical_character.as_deref(),
        biaxial_delta_beta_alpha: plain_row.biaxial_delta_beta_alpha,
        per_axis_dispersion_json: None,
        specific_gravity: plain_row.specific_gravity,
        color_recipe_json: None,
        dispersion_model_json: plain_row.dispersion_model_json.as_deref(),
        absorption_bands_json: None,
    })
    .unwrap();

    let rows = db.get_custom_materials().unwrap();
    let mut list: Vec<GemMaterial> = rows.iter().map(gem_material_from_row).collect();
    assert!(
        list.iter().all(|m| m.zoning.is_none()),
        "a plain row load has no zones"
    );
    assert_eq!(attach_zoning(&db, &mut list, 7.0), 1);
    let zoned = list.iter().find(|m| m.name == "Melon colour").unwrap();
    assert_eq!(zoned.zoning.as_ref(), outcome.material.zoning.as_ref());
    assert_eq!(zoned.absorption_unit, AbsorptionUnit::PerMm);
    let plain = list.iter().find(|m| m.name == "Plain").unwrap();
    assert!(plain.zoning.is_none());

    // Deleting the zones makes the material plain again.
    assert_eq!(delete_material_zoning(&db, "Melon colour").unwrap(), 1);
    let mut again: Vec<GemMaterial> = rows.iter().map(gem_material_from_row).collect();
    assert_eq!(attach_zoning(&db, &mut again, 7.0), 0);
}

#[test]
fn a_relative_library_material_scales_with_the_stone_width() {
    let db = db();
    // Geometry for a 6 mm stone, stored relative to the stone.
    save_relative_material_zoning(&db, "Library melon", &watermelon(), 6.0).unwrap();
    let stored = load_material_zoning(&db, "Library melon").unwrap().unwrap();
    assert!(stored.relative_to_stone);

    let mut base = GemMaterial::diamond();
    base.name = "Library melon".to_string();
    let radius = |material: &GemMaterial| match &material.zoning.as_ref().unwrap().zones[0].shape {
        ZoneShape::CoaxialCylinder { r_out, .. } => *r_out,
        other => panic!("{other:?}"),
    };
    let at_six = with_stored_zoning(&db, base.clone(), 6.0);
    let at_twelve = with_stored_zoning(&db, base.clone(), 12.0);
    assert!((radius(&at_six) - 3.0).abs() < 1e-9);
    assert!((radius(&at_twelve) - 6.0).abs() < 1e-9);
    // An unset width means the default stone width (7 mm).
    let default_width = with_stored_zoning(&db, base, 0.0);
    assert!((radius(&default_width) - 3.5).abs() < 1e-9);

    assert!(save_relative_material_zoning(&db, "Bad", &watermelon(), 0.0).is_err());
}

#[test]
fn a_damaged_zoning_row_leaves_the_material_plain() {
    let db = db();
    db.save_material_zoning(&indicatrix_vault::model::zoning::MaterialZoningRow {
        material_name: "Broken".to_string(),
        zoned_json: "[]".to_string(),
        relative_to_stone: false,
        version: ZONING_FORMAT_VERSION,
    })
    .unwrap();
    let mut material = GemMaterial::diamond();
    material.name = "Broken".to_string();
    assert!(with_stored_zoning(&db, material, 7.0).zoning.is_none());
    assert!(load_material_zoning(&db, "Broken").is_err());
    save_material_zoning(&db, "Fine", &watermelon(), false).unwrap();
    assert!(load_material_zoning(&db, "Fine").unwrap().is_some());
}

// ---- render jobs -----------------------------------------------------------------------------

fn scene(material: GemMaterial) -> SceneState {
    SceneState {
        width: 64,
        height: 48,
        yaw: 0.4,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material,
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

#[test]
fn a_queued_jobs_zones_are_saved_with_it_and_reattached_when_it_runs() {
    let db = db();
    let zoned_material = GemMaterial::diamond().with_zoning(watermelon());
    assert!(save_job_zoning(&db, 11, &zoned_material).unwrap());
    // A plain material stores nothing.
    assert!(!save_job_zoning(&db, 12, &GemMaterial::diamond()).unwrap());

    // The job's scene comes back from its frozen text without zones (serde skips them) ...
    let mut thawed = scene(GemMaterial::diamond());
    assert!(thawed.material.zoning.is_none());
    // ... and gets them back.
    assert!(attach_job_zoning(&db, 11, &mut thawed).unwrap());
    assert_eq!(thawed.material.zoning, zoned_material.zoning);
    // A job without a row renders as it is.
    let mut other = scene(GemMaterial::diamond());
    assert!(!attach_job_zoning(&db, 12, &mut other).unwrap());
    assert!(other.material.zoning.is_none());

    // A row that cannot be read fails the job instead of rendering it without its zones.
    db.save_render_job_zoning(13, "not zones").unwrap();
    let mut broken = scene(GemMaterial::diamond());
    let message = format!("{:#}", attach_job_zoning(&db, 13, &mut broken).unwrap_err());
    assert!(message.contains("render job 13"), "{message}");

    assert_eq!(delete_job_zoning(&db, 11).unwrap(), 1);
}

#[test]
fn pruning_drops_the_rows_of_deleted_owners_only() {
    let db = db();
    let plan = db.save_rough_plan("Plan", 1, "{}", "s", 1).unwrap();
    save_rough_colour(&db, plan, &colour(watermelon())).unwrap();
    save_rough_colour(&db, plan + 100, &colour(watermelon())).unwrap();
    save_job_zoning(&db, 77, &GemMaterial::diamond().with_zoning(watermelon())).unwrap();
    assert_eq!(prune_orphans(&db).unwrap(), 2);
    assert_eq!(
        plans_with_rough_colour(&db)
            .unwrap()
            .into_iter()
            .collect::<Vec<_>>(),
        vec![plan]
    );
}
