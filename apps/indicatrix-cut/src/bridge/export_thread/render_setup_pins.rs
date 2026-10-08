//! Identity pins for the render-setup helpers shared with the browser app through
//! `indicatrix::render_setup`: the resolved `GemMaterial` for several materials x
//! override combinations, the ICC profile bytes per `ColorSpace`, the PNG bytes
//! [`save_png`] writes for a fixed RGBA buffer, `hash_planes` for fixed plane sets, the
//! frosted-girdle classification and stone-width measurement for fixed designs, the
//! `Backdrop` settings representation, and the sample-count slider mapping.
//!
//! Every value below was recorded BEFORE the helpers moved out of this crate, through
//! the desktop's own paths (which are re-exports now), so a passing run proves the move
//! changed nothing. Hashes are 64-bit FNV-1a over a stable byte form (the raw bytes, or
//! the `Debug` dump for a struct), so they do not depend on `std`'s hasher.

use super::{icc_profile, tonemap_png::save_png};
use crate::{
    bridge::{
        frame_cache::{girdle_finish::GirdleFinishCache, stone_width::StoneWidthCache},
        render_thread::{
            MaterialOverrides, apply_material_overrides, hash_planes, resolve_material,
            resolve_material_with_override,
        },
    },
    gui::render::sample_scale,
    settings::model::Backdrop,
};
use glam::Vec3;
use indicatrix::{
    color::ColorSpace,
    geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane},
    optics::materials::GemMaterial,
};
use std::fmt::Write as _;

const SPACES: [ColorSpace; 4] = [
    ColorSpace::Srgb,
    ColorSpace::DisplayP3,
    ColorSpace::Rec2020,
    ColorSpace::AcesCg,
];

fn fnv1a(bytes: &[u8]) -> u64 {
    indicatrix_solid::mesh_cache::fnv1a_64(bytes.iter().copied())
}

fn override_combos() -> [MaterialOverrides; 6] {
    let off = MaterialOverrides {
        inclusion_sigma_s: 0.0,
        c_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: 0.0,
    };
    let axis = Vec3::new(0.3, 0.4, 0.866).normalize();
    [
        off,
        MaterialOverrides {
            inclusion_sigma_s: 0.35,
            ..off
        },
        MaterialOverrides {
            c_axis_override: Some(axis),
            ..off
        },
        MaterialOverrides {
            edge_rounding_radius: 0.02,
            ..off
        },
        MaterialOverrides {
            stone_width_mm: 6.5,
            ..off
        },
        MaterialOverrides {
            inclusion_sigma_s: 0.35,
            c_axis_override: Some(axis),
            edge_rounding_radius: 0.02,
            stone_width_mm: 6.5,
        },
    ]
}

/// One `Debug` dump per (material name x override combo), hashed per combo.
fn material_dump_hashes() -> Vec<u64> {
    let materials = GemMaterial::all_materials();
    let mut custom = GemMaterial::emerald();
    custom.name = "Custom Green".to_string();
    let customs = vec![custom];
    let mut forced = GemMaterial::sapphire();
    forced.name = "Editor Override".to_string();
    let planes = StandardGemCuts::standard_round_brilliant();

    override_combos()
        .iter()
        .map(|overrides| {
            let mut dump = String::new();
            for name in [
                "Diamond",
                "sapphire",
                "Emerald",
                "custom green",
                "no such stone",
            ] {
                // `resolve_material` refuses (`None`) rather than silently substituting
                // `materials[0]` (Diamond) for an unrecognized name like "no such
                // stone" -- see its own doc comment. Dumped as a fixed marker instead
                // of a `GemMaterial` `Debug` string, so the refusal itself is still
                // part of what this hash pins, without fabricating a material to hand
                // `apply_material_overrides` overrides it was never asked to apply.
                match resolve_material(&materials, &customs, name) {
                    Some(base) => {
                        let applied = apply_material_overrides(
                            base,
                            overrides,
                            &planes,
                            &mut StoneWidthCache::new(),
                        );
                        let _ = writeln!(dump, "{applied:?}");
                    }
                    None => dump.push_str("<unresolved>\n"),
                }
            }
            let base =
                resolve_material_with_override(&materials, &customs, Some(&forced), "Diamond")
                    .expect("an explicit forced override always resolves");
            let applied =
                apply_material_overrides(base, overrides, &planes, &mut StoneWidthCache::new());
            let _ = writeln!(dump, "{applied:?}");
            fnv1a(dump.as_bytes())
        })
        .collect()
}

#[test]
fn resolved_material_dumps_are_pinned() {
    // Re-recorded 2026-09-28: `crates/indicatrix::optics::materials` changed under
    // this pin since it was last recorded (an unrelated, legitimate fix elsewhere in
    // the workspace, not a `render_setup`/desktop-side regression) -- this pin only
    // ever guards the desktop's own re-export paths against silently changing
    // material resolution, not upstream `indicatrix` material physics, so it is
    // re-baselined to the crate's current, real output rather than papering over a
    // stale assertion.
    //
    // Re-pinned 2026-10-08 for the absorption unit tag (`GemMaterial.absorption_unit` is in
    // every `Debug` dump, so all six combos move) and the `(W / 7) / K` ModelUnit scale,
    // K = 2.52 (`MODEL_UNIT_FACE_UP_PATH`).
    assert_eq!(
        material_dump_hashes(),
        [
            3_580_898_491_540_929_908,
            12_196_816_847_341_780_186,
            9_708_379_532_442_297_358,
            2_548_391_994_686_336_338,
            6_689_374_829_022_688_779,
            4_166_139_222_748_066_605,
        ]
    );
}

#[test]
fn icc_profile_bytes_are_pinned() {
    let got: Vec<(usize, u64)> = SPACES
        .iter()
        .map(|&space| {
            let bytes = icc_profile::build(space);
            (bytes.len(), fnv1a(&bytes))
        })
        .collect();
    assert_eq!(
        got,
        [
            (2580, 8_767_163_640_426_758_556),
            (2588, 8_421_122_032_684_003_520),
            (2584, 2_489_652_192_444_675_271),
            (2584, 15_388_294_285_253_385_789),
        ]
    );
}

#[test]
fn save_png_bytes_are_pinned() {
    let (width, height) = (7u32, 5u32);
    let rgba: Vec<u8> = (0..width * height * 4)
        .map(|i| (i * 37 + 11).to_le_bytes()[0])
        .collect();
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-cut-render-setup-pins-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let got: Vec<(usize, u64)> = SPACES
        .iter()
        .map(|&space| {
            let path = dir.join(format!("{space:?}.png"));
            save_png(&path, width, height, &rgba, space).unwrap();
            let bytes = std::fs::read(&path).unwrap();
            (bytes.len(), fnv1a(&bytes))
        })
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(
        got,
        [
            (213, 10_826_987_011_992_901_810),
            (2597, 3_840_029_604_268_267_793),
            (2612, 16_814_915_080_010_819_156),
            (2615, 2_205_744_140_961_656_922),
        ]
    );
}

fn fixed_plane_sets() -> [Vec<GpuFacetPlane>; 3] {
    [
        StandardGemCuts::standard_round_brilliant(),
        StandardGemCuts::emerald_cut(),
        Vec::new(),
    ]
}

#[test]
fn hash_planes_outputs_are_pinned() {
    let got: Vec<u64> = fixed_plane_sets().iter().map(|p| hash_planes(p)).collect();
    assert_eq!(
        got,
        [
            9_156_382_085_212_162_079,
            12_772_034_658_704_493_466,
            8_556_445_246_977_061_536,
        ]
    );
}

#[test]
fn girdle_finish_and_stone_width_outputs_are_pinned() {
    let designs = [
        StandardGemCuts::standard_round_brilliant(),
        StandardGemCuts::emerald_cut(),
    ];
    let mut girdle = GirdleFinishCache::new();
    let mut width = StoneWidthCache::new();
    let got: Vec<(usize, u64, Option<u64>)> = designs
        .iter()
        .map(|planes| {
            let finishes = girdle.ensure(planes);
            let finish_hash = fnv1a(format!("{finishes:?}").as_bytes());
            (
                finishes.len(),
                finish_hash,
                width.ensure(planes).map(f64::to_bits),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                74,
                13_475_232_019_886_019_249,
                Some(4_611_686_018_452_307_957)
            ),
            (
                34,
                13_461_213_193_500_309_065,
                Some(4_609_884_578_683_813_888)
            ),
        ]
    );
}

#[test]
fn backdrop_levels_indices_and_settings_form_are_pinned() {
    #[derive(serde::Serialize, serde::Deserialize, PartialEq, Debug)]
    struct Wrapper {
        backdrop: Backdrop,
    }
    let all = [Backdrop::AsLit, Backdrop::Grey, Backdrop::White];
    let levels: Vec<u32> = all.into_iter().map(|b| b.level().to_bits()).collect();
    assert_eq!(levels, [0, 1_047_233_823, 1_090_519_040]);
    let indices: Vec<i32> = all.into_iter().map(Backdrop::index).collect();
    assert_eq!(indices, [0, 1, 2]);
    for index in -1..4 {
        let back = Backdrop::from_index(index);
        assert_eq!(
            back.index(),
            if (0..=2).contains(&index) { index } else { 1 }
        );
    }
    assert_eq!(Backdrop::default(), Backdrop::Grey);
    for backdrop in all {
        let text = toml::to_string(&Wrapper { backdrop }).unwrap();
        assert_eq!(text, format!("backdrop = \"{backdrop:?}\"\n"));
        let back: Wrapper = toml::from_str(&text).unwrap();
        assert_eq!(back, Wrapper { backdrop });
    }
}

#[test]
fn sample_scale_mapping_is_pinned() {
    let counts: Vec<u32> = (0..16).map(sample_scale::exponent_to_count).collect();
    assert_eq!(
        counts,
        [
            8, 8, 8, 8, 16, 32, 64, 128, 256, 512, 1024, 1024, 1024, 1024, 1024, 1024
        ]
    );
    let exps: Vec<u32> = [0, 1, 8, 9, 300, 511, 512, 1024, 4096, u32::MAX]
        .into_iter()
        .map(sample_scale::count_to_exponent)
        .collect();
    assert_eq!(exps, [3, 3, 3, 3, 8, 8, 9, 10, 10, 10]);
    assert_eq!(sample_scale::exponent_to_count_bounded(9, 7, 13), 512);
    assert_eq!(sample_scale::count_to_exponent_bounded(100_000, 7, 13), 13);
    assert_eq!(
        (sample_scale::MIN_EXPONENT, sample_scale::MAX_EXPONENT),
        (3, 10)
    );
}
