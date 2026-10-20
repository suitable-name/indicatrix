use indicatrix::optics::{
    materials::{CrystalSystem, GemMaterial},
    raytracer::LightingPreset,
};

use super::*;

fn spec_with(name: &str) -> SceneSpec {
    SceneSpec {
        material: MaterialSpec::catalogue(name),
        ..self_check_scene()
    }
}

#[test]
fn cubic_and_glass_materials_are_eligible() {
    for name in ["Diamond", "Spinel"] {
        assert_eq!(
            gpu_ineligibility_reason(&spec_with(name), 0, None, false),
            None,
            "{name} is cubic"
        );
    }
}

#[test]
fn birefringent_materials_are_refused() {
    for name in ["Zircon", "Topaz", "Sapphire"] {
        assert_eq!(
            gpu_ineligibility_reason(&spec_with(name), 0, None, false),
            Some(REASON_ANISOTROPIC),
            "{name} is birefringent"
        );
    }
}

#[test]
fn unknown_material_is_unresolved() {
    assert_eq!(
        gpu_ineligibility_reason(&spec_with("No Such Stone"), 0, None, false),
        Some(REASON_UNRESOLVED)
    );
}

#[test]
fn zoned_isotropic_material_is_refused() {
    let mut zoned = GemMaterial::ametrine();
    zoned.name = "Zoned Cubic".to_string();
    zoned.crystal_system = CrystalSystem::Cubic;
    zoned.birefringence_delta = 0.0;
    zoned.biaxial_delta_beta_alpha = None;
    assert!(zoned.zoning.is_some(), "the built-in Ametrine is zoned");
    let mut spec = spec_with("Zoned Cubic");
    spec.material.custom_materials = vec![zoned];
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, None, false),
        Some(REASON_ZONED)
    );
}

#[test]
fn fluorescent_flag_is_refused() {
    assert_eq!(
        gpu_ineligibility_reason(&spec_with("Diamond"), 0, None, true),
        Some(REASON_FLUORESCENT)
    );
}

#[test]
fn uv_lamp_lighting_is_refused() {
    let uv = LightingPreset::ALL
        .iter()
        .copied()
        .find(|preset| preset.is_uv_lamp())
        .expect("the preset list has a UV lamp");
    let mut spec = spec_with("Diamond");
    spec.lighting = LightingSpec::new(uv, 1.0, 0.0, 0.0, Backdrop::AsLit, 16.0);
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, None, false),
        Some(REASON_UV_LAMP)
    );
}

#[test]
fn hdr_map_must_fit_the_storage_binding() {
    let mut spec = spec_with("Diamond");
    spec.hdr_id = Some(7);
    // 16 bytes per texel.
    assert_eq!(
        gpu_ineligibility_reason(&spec, 1_000 * 16 - 1, Some(1_000), false),
        Some(REASON_HDR_TOO_BIG)
    );
    assert_eq!(
        gpu_ineligibility_reason(&spec, 1_000 * 16, Some(1_000), false),
        None
    );
    // An unknown limit (the adapter is not up yet) or no map never blocks.
    assert_eq!(
        gpu_ineligibility_reason(&spec, 0, Some(u64::MAX), false),
        None
    );
    assert_eq!(gpu_ineligibility_reason(&spec, 16, None, false), None);
    // A map the scene does not name does not count.
    let plain = spec_with("Diamond");
    assert_eq!(
        gpu_ineligibility_reason(&plain, 16, Some(1_000), false),
        None
    );
}

/// Concave stones go to the GPU up to the kernels' tool limit and are the CPU's beyond it; the
/// reason names the limit and never hides a material reason.
#[test]
fn a_concave_stone_is_eligible_up_to_the_tool_limit() {
    use indicatrix::geometry::tool::GPU_MAX_TOOL_PRIMITIVES;

    let diamond = spec_with("Diamond");
    for tools in [0, 1, GPU_MAX_TOOL_PRIMITIVES] {
        assert_eq!(
            gpu_ineligibility_reason_with_tools(&diamond, tools, 0, None, false),
            None,
            "{tools} tools"
        );
    }
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&diamond, GPU_MAX_TOOL_PRIMITIVES + 1, 0, None, false),
        Some(REASON_TOO_MANY_TOOLS)
    );
    // The material reasons keep their precedence.
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&spec_with("Zircon"), 1_000, 0, None, false),
        Some(REASON_ANISOTROPIC)
    );
    assert_eq!(
        gpu_ineligibility_reason_with_tools(&diamond, 1, 0, None, true),
        Some(REASON_FLUORESCENT)
    );
}

/// Without tools the new entry point is the old one, for every reason the old one gives.
#[test]
fn the_tool_aware_reason_equals_the_plain_one_without_tools() {
    for name in ["Diamond", "Spinel", "Zircon", "Sapphire", "No Such Stone"] {
        let spec = spec_with(name);
        for fluorescent in [false, true] {
            assert_eq!(
                gpu_ineligibility_reason_with_tools(&spec, 0, 0, None, fluorescent),
                gpu_ineligibility_reason(&spec, 0, None, fluorescent),
                "{name}, fluorescent {fluorescent}"
            );
        }
    }
}

/// The number in the reason text is the kernels' limit.
#[test]
fn the_too_many_tools_reason_names_the_limit() {
    let limit = indicatrix::geometry::tool::GPU_MAX_TOOL_PRIMITIVES.to_string();
    assert_eq!(
        REASON_TOO_MANY_TOOLS.matches(limit.as_str()).count(),
        2,
        "{REASON_TOO_MANY_TOOLS}"
    );
}

#[test]
fn the_self_check_scene_is_eligible_and_small() {
    let scene = self_check_scene();
    assert_eq!((scene.width, scene.height), (16, 16));
    assert_eq!(gpu_ineligibility_reason(&scene, 0, None, false), None);
}

#[test]
fn fnv_matches_known_vectors() {
    assert_eq!(fnv1a_64(""), 0xcbf2_9ce4_8422_2325);
    assert_eq!(fnv1a_64("a"), 0xaf63_dc4c_8601_ec8c);
    assert_eq!(fnv1a_64("foobar"), 0x8594_4171_f739_67e8);
}

#[test]
fn chrome_major_is_read_from_chrome_chromium_and_edge() {
    let chrome = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.7390.55 Safari/537.36";
    let edge = format!("{chrome} Edg/141.0.3537.57");
    assert_eq!(chrome_major_from_user_agent(chrome), "141");
    assert_eq!(chrome_major_from_user_agent(&edge), "141");
    assert_eq!(
        chrome_major_from_user_agent("Mozilla/5.0 Firefox/130.0"),
        "0"
    );
    assert_eq!(chrome_major_from_user_agent("Chrome/abc.1"), "0");
    assert_eq!(chrome_major_from_user_agent(""), "0");
}

fn facts() -> AdapterFacts<'static> {
    AdapterFacts {
        vendor: "0000",
        architecture: "",
        device: "0000",
        description: "",
        backend: "BrowserWebGpu",
        max_storage_buffers_per_stage: 10,
        max_storage_binding: 2_147_483_644,
        max_buffer: 4_294_967_296,
    }
}

#[test]
fn adapter_key_is_stable_and_well_formed() {
    let key = adapter_key(&facts(), "141", 0x1234_u64);
    assert_eq!(key, adapter_key(&facts(), "141", 0x1234_u64));
    assert!(key.starts_with("indicatrix.admission."));
    assert_eq!(key.rsplit_once('.').map(|(_, version)| version), Some("v1"));
}

#[test]
fn adapter_key_separates_gpus_that_report_the_same_empty_identity() {
    let base = adapter_key(&facts(), "141", 1_u64);
    let other_limits = AdapterFacts {
        max_storage_binding: 134_217_728,
        ..facts()
    };
    let other_buffer = AdapterFacts {
        max_buffer: 268_435_456,
        ..facts()
    };
    let other_count = AdapterFacts {
        max_storage_buffers_per_stage: 8,
        ..facts()
    };
    let named = AdapterFacts {
        description: "NVIDIA RTX A6000",
        ..facts()
    };
    for different in [other_limits, other_buffer, other_count, named] {
        assert_ne!(adapter_key(&different, "141", 1_u64), base);
    }
}

#[test]
fn adapter_key_changes_with_browser_and_kernel_set() {
    let base = adapter_key(&facts(), "141", 1_u64);
    assert_ne!(adapter_key(&facts(), "142", 1_u64), base);
    assert_ne!(adapter_key(&facts(), "141", 2_u64), base);
}

#[test]
fn admission_state_serde_is_lowercase() {
    for (state, text) in [
        (AdmissionState::Unknown, "\"unknown\""),
        (AdmissionState::Probing, "\"probing\""),
        (AdmissionState::Ok, "\"ok\""),
        (AdmissionState::Failed, "\"failed\""),
    ] {
        assert_eq!(serde_json::to_string(&state).unwrap(), text);
        assert_eq!(serde_json::from_str::<AdmissionState>(text).unwrap(), state);
    }
    assert_eq!(AdmissionState::default(), AdmissionState::Unknown);
}

#[test]
fn admission_record_round_trips() {
    let mut per_kernel = BTreeMap::new();
    per_kernel.insert("Extend".to_string(), 12.5);
    per_kernel.insert("Surface".to_string(), 8.3);
    let record = AdmissionRecord {
        state: AdmissionState::Failed,
        per_kernel_ms: per_kernel,
        max_ms: 12.5,
        total_ms: 20.8,
        reason: Some("compile timeout".to_string()),
        when: "2026-10-16T12:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&record).unwrap();
    assert_eq!(
        serde_json::from_str::<AdmissionRecord>(&json).unwrap(),
        record
    );
}

fn record_json(state: AdmissionState) -> String {
    serde_json::to_string(&AdmissionRecord {
        state,
        ..AdmissionRecord::default()
    })
    .unwrap()
}

#[test]
fn probing_scan_finds_every_crash_marker_and_ignores_other_keys() {
    let ok = record_json(AdmissionState::Ok);
    let probing = record_json(AdmissionState::Probing);
    let entries = [
        ("indicatrix.settings.v1", probing.as_str()),
        ("indicatrix.admission.a.v1", ok.as_str()),
        ("indicatrix.admission.b.v1", probing.as_str()),
        ("indicatrix.admission.c.v1", "not json"),
        ("indicatrix.admission.d.v1", probing.as_str()),
    ];
    assert_eq!(
        find_probing_key(entries.iter().copied()).as_deref(),
        Some("indicatrix.admission.b.v1")
    );
    assert_eq!(
        probing_keys(entries.iter().copied()),
        vec![
            "indicatrix.admission.b.v1".to_string(),
            "indicatrix.admission.d.v1".to_string()
        ]
    );
}

#[test]
fn probing_scan_is_empty_without_markers() {
    let ok = record_json(AdmissionState::Ok);
    let failed = record_json(AdmissionState::Failed);
    let entries = [
        ("indicatrix.admission.a.v1", ok.as_str()),
        ("indicatrix.admission.b.v1", failed.as_str()),
    ];
    assert_eq!(find_probing_key(entries.iter().copied()), None);
    assert_eq!(probing_keys(entries.iter().copied()).len(), 0);
}

#[test]
fn kernel_order_has_no_duplicates() {
    let mut names = ISOTROPIC_KERNELS_ORDER.to_vec();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), ISOTROPIC_KERNELS_ORDER.len());
}
