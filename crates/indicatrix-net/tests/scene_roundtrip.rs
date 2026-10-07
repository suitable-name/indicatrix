//! `SceneState` must round-trip through `postcard` unchanged -- including a material
//! with absorption bands (and biaxial data) and a full facet-plane set, since those are
//! exactly the fields a name/id-based scheme would have gotten wrong (see
//! `indicatrix_net::scene`'s module docs).

use indicatrix::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_net::SceneState;

fn sample_scene() -> SceneState {
    // Alexandrite has both absorption bands and biaxial_delta_beta_alpha = Some(..) --
    // the two nested-data cases most likely dropped by an incomplete derive.
    let material = GemMaterial::by_name("Alexandrite").expect("Alexandrite is a built-in material");
    assert!(
        !material.absorption.o_ray.is_empty(),
        "test material should actually exercise absorption bands"
    );
    assert!(
        material.biaxial_delta_beta_alpha.is_some(),
        "test material should actually exercise biaxial data"
    );

    let planes = StandardGemCuts::standard_round_brilliant();
    assert!(
        planes.len() >= 57,
        "test should exercise a full facet-plane set"
    );

    SceneState {
        width: 640,
        height: 480,
        yaw: 0.37,
        pitch: -0.12,
        distance: 3.5,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.25,
        max_bounces: 12,
        lighting_preset: LightingPreset::RingLights,
        material,
        planes,
        girdle_frosted: false,
        backdrop: 0.0,
        environment: indicatrix_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: indicatrix::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
    }
}

#[test]
fn scene_state_round_trips_unchanged_through_postcard() {
    let scene = sample_scene();

    let bytes = postcard::to_allocvec(&scene).expect("SceneState must serialize");
    let decoded: SceneState = postcard::from_bytes(&bytes).expect("SceneState must deserialize");

    assert_eq!(
        scene, decoded,
        "SceneState must round-trip bit-for-bit through postcard"
    );
}

/// v19: a scene with concave tools round-trips, and a convex one (the empty `Vec`) still
/// does -- postcard has no way to omit a trailing field, so both must decode by count.
#[test]
fn scene_state_with_and_without_tools_round_trips_through_postcard() {
    use glam::Vec3;
    use indicatrix::geometry::ToolPrimitive;

    let convex = sample_scene();
    assert_eq!(convex.tools, Vec::<ToolPrimitive>::new());
    let convex_bytes = postcard::to_allocvec(&convex).unwrap();
    let decoded: SceneState = postcard::from_bytes(&convex_bytes).unwrap();
    assert_eq!(convex, decoded);

    let mut concave = sample_scene();
    concave.tools = vec![
        ToolPrimitive::ball(Vec3::new(0.0, 0.0, 0.4), 0.2),
        ToolPrimitive::cylinder(Vec3::new(0.1, 0.0, 0.3), Vec3::X, 0.05, 0.3),
    ];
    let concave_bytes = postcard::to_allocvec(&concave).unwrap();
    let decoded: SceneState = postcard::from_bytes(&concave_bytes).unwrap();
    assert_eq!(concave, decoded);
    assert!(
        concave_bytes.len() > convex_bytes.len(),
        "tools are a length-prefixed trailing Vec, so they only add bytes"
    );
}

/// v20: a scene with fluorescent emitters round-trips through postcard (and a non-fluorescent
/// one, which costs one zero length byte), and the cached sampling tables a trace builds
/// are not part of the wire form.
#[test]
fn scene_state_with_and_without_fluorescence_round_trips_through_postcard() {
    use indicatrix::optics::{
        absorption::AbsorptionBand,
        fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter},
    };

    let plain = sample_scene();
    assert!(plain.fluorescence.is_empty());
    let plain_bytes = postcard::to_allocvec(&plain).unwrap();
    let decoded: SceneState = postcard::from_bytes(&plain_bytes).unwrap();
    assert_eq!(plain, decoded);

    let mut glowing = sample_scene();
    glowing.lighting_preset = LightingPreset::UvLamp365;
    glowing.fluorescence = Fluorescence::new(vec![FluorescentEmitter {
        excitation: vec![
            AbsorptionBand::energy(410.0, 3500.0, 2.0),
            AbsorptionBand::energy(556.0, 2800.0, 2.0),
        ],
        emission: vec![
            EmissionBand::new(692.9, 1.0, 0.4),
            EmissionBand::new(694.3, 1.0, 0.6),
        ],
        quantum_yield: 0.9,
    }]);
    // Build the sampling tables, as a trace would: they must not change the wire bytes.
    let before = postcard::to_allocvec(&glowing).unwrap();
    let _ = glowing.fluorescence.pseudo_extinction(694.0);
    let bytes = postcard::to_allocvec(&glowing).unwrap();
    assert_eq!(before, bytes);
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(glowing, decoded);
    assert_eq!(decoded.lighting_preset, LightingPreset::UvLamp365);
    assert_eq!(decoded.fluorescence.emitters().len(), 1);
    assert!(
        bytes.len() > plain_bytes.len(),
        "emitters are a length-prefixed trailing Vec, so they only add bytes"
    );
    // The emitter list is followed only by `head_shadow_deg` (an f32, four bytes) on the wire.
    let tail = postcard::to_allocvec(&glowing.fluorescence).unwrap();
    let emitters_end = bytes.len() - 4;
    assert_eq!(
        &bytes[emitters_end - tail.len()..emitters_end],
        tail.as_slice()
    );
}

/// A v20 `SceneState` (no trailing `head_shadow_deg` field) decodes as v21 only by the
/// handshake's refusal: postcard cannot skip a missing trailing field, so the bytes of the
/// old layout run out. Pins that the decode fails rather than yielding a scene.
#[test]
fn a_v20_scene_byte_layout_does_not_decode_as_v21() {
    let scene = sample_scene();
    let v21 = postcard::to_allocvec(&scene).unwrap();
    // The v20 layout is the v21 one without the trailing head-shadow f32 (16.0 = 0, 0, 128, 65).
    assert_eq!(
        v21[v21.len() - 4..],
        16.0f32.to_le_bytes(),
        "the head shadow is four trailing bytes"
    );
    let v20 = &v21[..v21.len() - 4];
    assert!(postcard::from_bytes::<SceneState>(v20).is_err());
    // The v19 layout (also without the empty emitter list's zero byte) fails too.
    let v19 = &v21[..v21.len() - 5];
    assert!(postcard::from_bytes::<SceneState>(v19).is_err());
}

/// The wire carries `LightingPreset` as its derived-serde variant index, which is the
/// DECLARATION order (not the `ALL` / `index()` UI order). New presets are appended; this pins
/// every variant to its position so a reorder of the declaration is caught here.
#[test]
fn lighting_preset_postcard_index_is_the_declaration_order() {
    use LightingPreset::*;
    let declared = [
        Daylight,
        Incandescent,
        RingLights,
        DarkSpotlight,
        IsoHemisphere,
        LightTent,
        DaylightDome,
        UvLamp365,
        UvLamp395,
        DaylightSun,
        Aset,
        ShopLights,
        WindowDaylight,
        WhiteTray,
        IlluminantA,
    ];
    assert_eq!(declared.len(), LightingPreset::ALL.len());
    for (position, preset) in declared.into_iter().enumerate() {
        assert_eq!(
            postcard::to_allocvec(&preset).unwrap(),
            vec![u8::try_from(position).unwrap()],
            "{preset:?} must encode as its declaration position"
        );
        assert!(
            LightingPreset::ALL.contains(&preset),
            "{preset:?} is in ALL"
        );
    }
}

#[test]
fn scene_state_round_trip_preserves_every_lighting_preset() {
    for preset in LightingPreset::ALL {
        let mut scene = sample_scene();
        scene.lighting_preset = preset;

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.lighting_preset, preset);
    }
}

#[test]
fn scene_state_round_trip_preserves_a_diamond_with_empty_absorption() {
    // The opposite corner case from Alexandrite: empty absorption bands and
    // biaxial_delta_beta_alpha = None must round-trip too.
    let mut scene = sample_scene();
    scene.material = GemMaterial::diamond();
    assert_eq!(
        scene.material.absorption.o_ray,
        Vec::new(),
        "diamond should have no absorption bands"
    );
    assert!(scene.material.biaxial_delta_beta_alpha.is_none());

    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(scene, decoded);
}

#[test]
fn scene_state_round_trip_preserves_an_empty_plane_set() {
    let mut scene = sample_scene();
    scene.planes = Vec::new();

    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(scene, decoded);
    assert_eq!(decoded.planes, Vec::new());
}

/// `girdle_frosted` is the wire-format encoding of the viewer's frosted-girdle toggle --
/// both its values must round-trip, not just the default.
#[test]
fn scene_state_round_trip_preserves_girdle_frosted_both_ways() {
    for girdle_frosted in [false, true] {
        let mut scene = sample_scene();
        scene.girdle_frosted = girdle_frosted;

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.girdle_frosted, girdle_frosted);
        assert_eq!(scene, decoded);
    }
}

/// The v14 (Part E) environment round-trips both ways, and a `scene.json` written before
/// the field existed still loads as the studio rig (`#[serde(default)]`).
#[test]
fn scene_state_round_trip_preserves_the_environment_both_ways() {
    use indicatrix_net::scene::{HdrEnvironment, SceneEnvironment};
    let hdr = HdrEnvironment {
        content_hash: [0x5A; 32],
        width: 4096,
        height: 2048,
    };
    for environment in [SceneEnvironment::Studio, SceneEnvironment::Hdr(hdr)] {
        let mut scene = sample_scene();
        scene.environment = environment;
        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.environment, environment);
        assert_eq!(scene, decoded);
    }
    let mut scene = sample_scene();
    scene.environment = SceneEnvironment::Hdr(hdr);
    assert_eq!(scene.hdr(), Some(&hdr));
    scene.environment = SceneEnvironment::Studio;
    assert_eq!(scene.hdr(), None);
}

/// `surface_glare` (v18) round-trips for the default, the clamp ends and a dialled-in
/// value, and the serde default is the unscaled `1.0`.
#[test]
fn scene_state_round_trip_preserves_surface_glare() {
    assert_eq!(
        indicatrix_net::scene::default_surface_glare().to_bits(),
        1.0f32.to_bits()
    );
    assert_eq!(sample_scene().surface_glare.to_bits(), 1.0f32.to_bits());
    for glare in [1.0f32, 0.0, 0.35] {
        let mut scene = sample_scene();
        scene.surface_glare = glare;

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.surface_glare.to_bits(), glare.to_bits());
        assert_eq!(scene, decoded);
    }
}

/// `GemMaterial::absorption_path_scale` is embedded inside `SceneState::material`, not
/// a top-level field -- but the same round-trip risk applies, so both the default
/// (`1.0`) and a dialled-in scale must survive the wire unchanged.
#[test]
fn scene_state_round_trip_preserves_absorption_path_scale_both_default_and_scaled() {
    for scale in [1.0f32, 0.42, 2.75] {
        let mut scene = sample_scene();
        scene.material = scene.material.with_absorption_path_scale(scale);

        let bytes = postcard::to_allocvec(&scene).unwrap();
        let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(decoded.material.absorption_path_scale, scale);
        assert_eq!(scene, decoded);
    }
}

// NOTE on `#[serde(default)]` and postcard: unlike JSON, a postcard-encoded struct has
// no field names or "present/absent" signal on the wire, so a byte stream one field
// shorter than the current shape does NOT gracefully default it -- postcard reports
// `DeserializeUnexpectedEnd` instead (confirmed empirically: an earlier version of this
// test asserted the opposite and failed). `#[serde(default)]` on
// `SceneState::girdle_frosted` is for `indicatrix-worker::render_cmd`'s on-disk
// `scene.json` (via `serde_json`, self-describing), not this crate's postcard wire
// format -- the network path's cross-version protection is `PROTOCOL_VERSION`.

/// v21: the head-shadow radius round-trips through postcard.
#[test]
fn scene_state_head_shadow_round_trips_through_postcard() {
    let mut scene = sample_scene();
    scene.head_shadow_deg = 22.5;
    let bytes = postcard::to_allocvec(&scene).unwrap();
    let decoded: SceneState = postcard::from_bytes(&bytes).unwrap();
    assert_eq!(decoded.head_shadow_deg.to_bits(), 22.5f32.to_bits());
}
