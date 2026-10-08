//! Scene parity: [`OwnedScene::build`] against the desktop's own recipe, field by field.
//!
//! The desktop's builder is private (`apps/indicatrix-cut/src/bridge/render_thread`),
//! so [`desktop_recipe`] reproduces it with the same public `indicatrix` calls in the
//! same order: `resolve_material_with_override` (falling back to diamond, which the
//! desktop's suspension flag makes unreachable), the `StoneWidthCache` adapter's
//! measure-only-when-on rule, `GirdleFinishCache`'s `girdle_facet_finishes`,
//! `Camera::new(yaw, pitch, distance, 42.0)` and the environment formula.

use super::*;
use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::raytracer::build_plane_soa,
    render_setup::{MaterialOverrides, apply_material_overrides, needs_model_width},
    renderer::cpu_frame::trace_pixels_interleaved,
};

/// One desktop settings combination, in the desktop's own types.
struct DesktopSettings {
    material_name: &'static str,
    custom_materials: Vec<GemMaterial>,
    /// `Some(selection)` = the viewport is linked to the design.
    linked_selection: Option<MaterialSelection>,
    overrides: MaterialOverrides,
    girdle_frosted: bool,
    yaw: f32,
    pitch: f32,
    distance: f32,
    preset: LightingPreset,
    exposure: f32,
    light_yaw: f32,
    light_pitch: f32,
    backdrop: Backdrop,
}

struct DesktopScene {
    material: GemMaterial,
    finishes: Vec<FacetFinish>,
    camera: Camera,
    environment: EnvironmentSource<'static>,
}

/// The desktop render loop's scene assembly for `s`, step for step.
fn desktop_recipe(s: &DesktopSettings, planes: &[GpuFacetPlane]) -> DesktopScene {
    let materials = GemMaterial::all_materials();
    // `inspector::sync_viewport_material_link`.
    let material_override = s.linked_selection.as_ref().and_then(|selection| {
        traced_gem_material(
            s.material_name,
            selection,
            &EditorMaterialLookup::new(&s.custom_materials),
        )
    });
    // `context::materials::resolve_material_and_quality`.
    let material = resolve_material_with_override(
        &materials,
        &s.custom_materials,
        material_override.as_ref(),
        s.material_name,
    )
    .unwrap_or_else(GemMaterial::diamond);
    let model_width = if needs_model_width(&material) {
        measure_model_width(planes)
    } else {
        None
    };
    let material = apply_material_overrides(material, &s.overrides, model_width);
    let finishes = if s.girdle_frosted {
        girdle_facet_finishes(planes)
    } else {
        Vec::new()
    };
    let camera = Camera::new(s.yaw, s.pitch, s.distance, 42.0);
    let environment = s
        .preset
        .studio(s.exposure, s.light_yaw, s.light_pitch)
        .with_backdrop(s.backdrop.level());
    DesktopScene {
        material,
        finishes,
        camera,
        environment,
    }
}

/// The web spec for the same settings.
fn web_spec(s: &DesktopSettings, planes: &[GpuFacetPlane], custom: Vec<GemMaterial>) -> SceneSpec {
    SceneSpec {
        planes: planes_to_data(planes),
        finishes: if s.girdle_frosted {
            FinishSpec::FrostedGirdle
        } else {
            FinishSpec::AllPolished
        },
        material: MaterialSpec {
            name: s.material_name.to_string(),
            custom_materials: custom,
            linked_design: s
                .linked_selection
                .as_ref()
                .map(DesignMaterialOverrides::from_selection),
            overrides: MaterialOverridesSpec {
                inclusion_sigma_s: s.overrides.inclusion_sigma_s,
                c_axis_override: s.overrides.c_axis_override.map(<[f32; 3]>::from),
                edge_rounding_radius: s.overrides.edge_rounding_radius,
                stone_width_mm: s.overrides.stone_width_mm,
            },
        },
        camera: CameraSpec {
            yaw: s.yaw,
            pitch: s.pitch,
            distance: s.distance,
        },
        lighting: LightingSpec::new(
            s.preset,
            s.exposure,
            s.light_yaw,
            s.light_pitch,
            s.backdrop,
            16.0,
        ),
        max_bounces: 6,
        width: 12,
        height: 8,
        hdr_id: None,
    }
}

fn vec_bits(v: glam::Vec3) -> [u32; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

fn assert_camera_eq(a: &Camera, b: &Camera) {
    assert_eq!(vec_bits(a.origin), vec_bits(b.origin), "camera origin");
    assert_eq!(vec_bits(a.forward), vec_bits(b.forward), "camera forward");
    assert_eq!(vec_bits(a.right), vec_bits(b.right), "camera right");
    assert_eq!(vec_bits(a.up), vec_bits(b.up), "camera up");
    assert_eq!(a.fov_tan.to_bits(), b.fov_tan.to_bits(), "camera fov");
}

fn assert_environment_eq(a: EnvironmentSource<'_>, b: EnvironmentSource<'_>) {
    match (a, b) {
        (
            EnvironmentSource::Studio {
                preset: pa,
                exposure: ea,
                light_yaw: ya,
                light_pitch: qa,
                backdrop: ba,
                ..
            },
            EnvironmentSource::Studio {
                preset: pb,
                exposure: eb,
                light_yaw: yb,
                light_pitch: qb,
                backdrop: bb,
                ..
            },
        ) => {
            assert_eq!(pa, pb, "preset");
            assert_eq!(
                [ea, ya, qa, ba].map(f32::to_bits),
                [eb, yb, qb, bb].map(f32::to_bits),
                "exposure / light yaw / light pitch / backdrop"
            );
        }
        (EnvironmentSource::HdrMap(ma), EnvironmentSource::HdrMap(mb)) => {
            assert!(std::ptr::eq(ma, mb), "different HDR maps");
        }
        _ => panic!("studio vs HDR environment"),
    }
}

fn trace_bits(scene: &FrameScene<'_>) -> Vec<[u32; 3]> {
    let soa = build_plane_soa(scene.planes);
    trace_pixels_interleaved(scene, &soa, 0, 1, 0, 2)
        .into_iter()
        .map(vec_bits)
        .collect()
}

/// Builds both sides and compares every field, then traces a tiny frame through both.
fn check_parity(s: &DesktopSettings, custom: Vec<GemMaterial>) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let desktop = desktop_recipe(s, &planes);
    let spec = web_spec(s, &planes, custom);
    let web = OwnedScene::build(&spec, None).expect("the web scene builds");

    assert_eq!(web.material(), &desktop.material, "material");
    assert_eq!(
        format!("{:?}", web.material()),
        format!("{:?}", desktop.material),
        "material (debug dump)"
    );
    assert_eq!(web.finishes(), desktop.finishes.as_slice(), "finishes");
    assert_eq!(web.planes(), planes.as_slice(), "planes");
    assert_camera_eq(web.camera(), &desktop.camera);
    let web_frame = web.frame_scene();
    assert_environment_eq(web_frame.environment, desktop.environment);

    let desktop_frame = FrameScene {
        camera: &desktop.camera,
        width: spec.width,
        height: spec.height,
        planes: &planes,
        facet_finishes: &desktop.finishes,
        material: &desktop.material,
        max_bounces: spec.max_bounces,
        environment: desktop.environment,
    };
    assert_eq!(
        trace_bits(&web_frame),
        trace_bits(&desktop_frame),
        "traced pixels"
    );
}

fn no_overrides() -> MaterialOverrides {
    MaterialOverrides {
        inclusion_sigma_s: 0.0,
        c_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: 0.0,
    }
}

#[test]
fn catalogue_diamond_under_the_default_light_tent_matches_the_desktop() {
    check_parity(
        &DesktopSettings {
            material_name: "Diamond",
            custom_materials: Vec::new(),
            linked_selection: None,
            overrides: no_overrides(),
            girdle_frosted: false,
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.2,
            preset: LightingPreset::LightTent,
            exposure: 1.0,
            light_yaw: 0.4,
            light_pitch: 0.35,
            backdrop: Backdrop::Grey,
        },
        Vec::new(),
    );
}

#[test]
fn linked_sapphire_with_every_override_and_a_frosted_girdle_matches_the_desktop() {
    let selection = MaterialSelection {
        name: Some("Sapphire".to_string()),
        refractive_index_override: Some(1.77),
        body_color_override: Some([0.8, 0.6, 0.1]),
        ..MaterialSelection::default()
    };
    check_parity(
        &DesktopSettings {
            material_name: "Sapphire",
            custom_materials: Vec::new(),
            linked_selection: Some(selection),
            overrides: MaterialOverrides {
                inclusion_sigma_s: 0.3,
                c_axis_override: Some(glam::Vec3::new(0.0, 0.6, 0.8)),
                edge_rounding_radius: 0.01,
                stone_width_mm: 6.5,
            },
            girdle_frosted: true,
            yaw: -1.1,
            pitch: 0.9,
            distance: 5.0,
            preset: LightingPreset::Daylight,
            exposure: 1.4,
            light_yaw: 1.2,
            light_pitch: 0.7,
            backdrop: Backdrop::White,
        },
        Vec::new(),
    );
}

#[test]
fn a_custom_material_wins_over_the_catalogue_like_the_desktop() {
    let spec = CustomMaterialSpec {
        name: "Garnet 1.74".to_string(),
        mean_ri: 1.74,
        dispersion_delta: 0.024,
        birefringence_delta: 0.0,
        absorption_rgb: None,
        color_recipe: None,
        absorption_bands: Vec::new(),
    };
    let desktop_custom = vec![spec.to_gem_material()];
    let web_custom = desktop_custom.clone();
    check_parity(
        &DesktopSettings {
            material_name: "Garnet 1.74",
            custom_materials: desktop_custom,
            linked_selection: Some(MaterialSelection::default()),
            overrides: MaterialOverrides {
                c_axis_override: Some(glam::Vec3::X),
                ..no_overrides()
            },
            girdle_frosted: false,
            yaw: 2.0,
            pitch: -0.4,
            distance: 3.5,
            preset: LightingPreset::IsoHemisphere,
            exposure: 0.8,
            light_yaw: 0.0,
            light_pitch: 0.0,
            backdrop: Backdrop::AsLit,
        },
        web_custom,
    );
}

#[test]
fn a_custom_material_spec_keeps_its_snapshots_body_color() {
    let snapshot = CustomMaterialSnapshot::new(1.74, 0.024, 0.0, None, "Cubic", "Isotropic")
        .with_absorption_rgb(Some([0.25, 0.5, 1.5]));
    let spec = CustomMaterialSpec::from_snapshot("Garnet 1.74", &snapshot);
    assert_eq!(spec.absorption_rgb, Some([0.25, 0.5, 1.5]));
    let colored = spec.to_gem_material();
    let expected = GemMaterial::new_custom("Garnet 1.74", 1.74, 0.024, 0.0, [0.25, 0.5, 1.5]);
    assert_eq!(colored.absorption, expected.absorption);
    let clear = CustomMaterialSpec {
        absorption_rgb: None,
        ..spec
    }
    .to_gem_material();
    assert_ne!(colored.absorption, clear.absorption);
}

#[test]
fn an_hdr_scene_shares_the_map_and_needs_it() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let map = Arc::new(EnvironmentMap::uniform(8, 4, [0.5, 0.4, 0.3]));
    let mut spec = SceneSpec {
        planes: planes_to_data(&planes),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.35,
            distance: 4.2,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.4,
            0.35,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 4,
        width: 4,
        height: 4,
        hdr_id: Some(7),
    };
    assert_eq!(
        OwnedScene::build(&spec, None).err(),
        Some(SceneError::MissingHdr { id: 7 })
    );
    let scene = OwnedScene::build(&spec, Some(Arc::clone(&map))).expect("builds with its map");
    assert_environment_eq(
        scene.frame_scene().environment,
        EnvironmentSource::HdrMap(&map),
    );

    // A map handed to a studio scene is ignored.
    spec.hdr_id = None;
    let studio = OwnedScene::build(&spec, Some(map)).expect("builds");
    assert_environment_eq(
        studio.frame_scene().environment,
        spec.lighting.studio_environment(),
    );
}

#[test]
fn bad_specs_are_errors_not_substitutions() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let good = SceneSpec {
        planes: planes_to_data(&planes),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.0,
            pitch: 0.0,
            distance: 4.0,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.0,
            0.0,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 4,
        width: 4,
        height: 4,
        hdr_id: None,
    };
    let unknown = SceneSpec {
        material: MaterialSpec::catalogue("Unobtainium"),
        ..good.clone()
    };
    assert_eq!(
        OwnedScene::build(&unknown, None).err(),
        Some(SceneError::UnknownMaterial {
            name: "Unobtainium".to_string()
        })
    );
    let empty = SceneSpec {
        planes: Vec::new(),
        ..good.clone()
    };
    assert_eq!(
        OwnedScene::build(&empty, None).err(),
        Some(SceneError::NoPlanes)
    );
    let zero = SceneSpec {
        width: 0,
        ..good.clone()
    };
    assert!(matches!(
        OwnedScene::build(&zero, None),
        Err(SceneError::BadFrameSize { .. })
    ));
    let too_many = SceneSpec {
        finishes: FinishSpec::PerFacet(vec![true; planes.len() + 1]),
        ..good
    };
    assert!(matches!(
        OwnedScene::build(&too_many, None),
        Err(SceneError::TooManyFinishes { .. })
    ));
}

#[test]
fn planes_survive_the_plain_data_round_trip_bit_for_bit() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let back: Vec<GpuFacetPlane> = planes_to_data(&planes)
        .into_iter()
        .map(Into::into)
        .collect();
    assert_eq!(back, planes);
}

#[test]
fn a_linked_designs_bands_reach_the_traced_material() {
    let selection = MaterialSelection::none().with_body_color_bands(
        Some([0.2, 0.4, 0.6]),
        Some(vec![[460.0, 45.0, 0.25]]),
        Some(2.0),
    );
    let linked = DesignMaterialOverrides::from_selection(&selection);
    assert_eq!(linked.body_color_bands, vec![[460.0, 45.0, 0.25]]);
    let spec = MaterialSpec {
        name: "Quartz".to_string(),
        custom_materials: Vec::new(),
        linked_design: Some(linked.clone()),
        overrides: MaterialOverridesSpec::default(),
    };
    let traced = resolve_scene_material(&spec, &[]).expect("resolves");
    assert!((traced.absorption_path_scale - 2.0).abs() < 1e-6);

    // The same design without its bands is coloured by the triple alone.
    let triple_only = MaterialSpec {
        linked_design: Some(DesignMaterialOverrides {
            body_color_bands: Vec::new(),
            ..linked
        }),
        ..spec
    };
    let plain = resolve_scene_material(&triple_only, &[]).expect("resolves");
    assert_ne!(traced.absorption, plain.absorption);
}

#[test]
fn a_custom_materials_bands_colour_the_restored_material() {
    let spec = CustomMaterialSpec {
        name: "Banded".to_string(),
        mean_ri: 1.74,
        dispersion_delta: 0.024,
        birefringence_delta: 0.0,
        absorption_rgb: Some([0.25, 0.5, 1.5]),
        color_recipe: None,
        absorption_bands: vec![[460.0, 45.0, 0.25]],
    };
    let banded = spec.to_gem_material();
    let triple_only = CustomMaterialSpec {
        absorption_bands: Vec::new(),
        ..spec
    }
    .to_gem_material();
    assert_ne!(banded.absorption, triple_only.absorption);
}
