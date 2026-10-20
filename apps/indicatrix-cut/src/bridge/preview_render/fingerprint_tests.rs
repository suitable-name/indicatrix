use super::*;
use indicatrix::optics::raytracer::LightingPreset;

const PREVIEW: CacheKind = CacheKind::Preview {
    size: 160,
    spp: 256,
    max_bounces: 12,
};

/// The material of a refractive-index pick, unsized.
fn picked(name: &str) -> FingerprintMaterial<'_> {
    FingerprintMaterial::picked(Some(name))
}

/// The material of a design whose own file decided it.
fn from_file(name: &str, girdle_mm: f64) -> FingerprintMaterial<'_> {
    FingerprintMaterial {
        name: Some(name),
        girdle_mm,
        from_design_file: true,
        frosting: 0,
    }
}

/// The stored frosting of a design with the given whole-tier rows.
fn frosting_of(tiers: &[u32]) -> Vec<StoredTierFrosting> {
    tiers
        .iter()
        .map(|&t| StoredTierFrosting::whole(t))
        .collect()
}

#[test]
fn cache_fingerprint_is_stable_and_names_the_tracer_build() {
    let fingerprint = cache_fingerprint(PREVIEW, picked("Diamond"));
    assert_eq!(fingerprint, cache_fingerprint(PREVIEW, picked("Diamond")));
    assert!(fingerprint.contains(indicatrix::BUILD_ID), "{fingerprint}");
    assert!(
        cache_fingerprint(CacheKind::TiltCurves, picked("Diamond")).contains(indicatrix::BUILD_ID)
    );
}

#[test]
fn cache_fingerprint_changes_with_every_parameter_it_tracks() {
    let base = cache_fingerprint(PREVIEW, picked("Diamond"));
    let other_size = CacheKind::Preview {
        size: 128,
        spp: 256,
        max_bounces: 12,
    };
    let other_spp = CacheKind::Preview {
        size: 160,
        spp: 64,
        max_bounces: 12,
    };
    let other_bounces = CacheKind::Preview {
        size: 160,
        spp: 256,
        max_bounces: 8,
    };
    for changed in [
        cache_fingerprint(other_size, picked("Diamond")),
        cache_fingerprint(other_spp, picked("Diamond")),
        cache_fingerprint(other_bounces, picked("Diamond")),
        cache_fingerprint(PREVIEW, picked("Sapphire")),
        cache_fingerprint(PREVIEW, FingerprintMaterial::picked(None)),
        cache_fingerprint(CacheKind::TiltCurves, picked("Diamond")),
        cache_fingerprint(PREVIEW, from_file("Diamond", 0.0)),
        cache_fingerprint(PREVIEW, from_file("Diamond", 9.0)),
    ] {
        assert_ne!(changed, base);
    }
    assert!(base.contains(PREVIEW_LIGHTING_PRESET.label()), "{base}");
}

/// A design the refractive index decided and that has no stone size keeps the material
/// text its fingerprint had before the size and the source were tracked, byte for
/// byte (the tilt fingerprint wholly, the preview one after the scene and band
/// fields), so only designs whose file names a material or a size differ there.
#[test]
fn an_unsized_refractive_index_pick_keeps_the_old_fingerprint_exactly() {
    let backdrop = super::super::thumbnail_backdrop_name();
    let light = format!("{PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH}");
    let quartz_bands = material_bands_hash(&GemMaterial::by_name("Quartz").expect("built-in"));
    assert_eq!(
        cache_fingerprint(PREVIEW, picked("Quartz")),
        format!(
            "preview;tracer={};size=160;spp=256;bounces=12;lighting={};light={light};\
                 exposure={PREVIEW_EXPOSURE};shadow={PREVIEW_HEAD_SHADOW_DEG};\
                 backdrop={backdrop};tray={};glare={PREVIEW_SURFACE_GLARE};\
                 bands={quartz_bands:016x};material=Quartz",
            indicatrix::BUILD_ID,
            PREVIEW_LIGHTING_PRESET.label(),
            indicatrix::optics::raytracer::TRAY_MODEL_VERSION
        )
    );
    assert_eq!(
        cache_fingerprint(CacheKind::TiltCurves, picked("Quartz")),
        format!(
            "tilt;tracer={};lighting={};light={light};bands={quartz_bands:016x};\
                 material=Quartz",
            indicatrix::BUILD_ID,
            BATCH_TILT_LIGHTING_PRESET.label()
        )
    );
    assert!(
        cache_fingerprint(PREVIEW, FingerprintMaterial::picked(None)).ends_with(";material=none"),
        "no material picked yet"
    );
    // A row from before the size and source columns reads as exactly that.
    let legacy = PreviewMaterialState {
        name: Some("Quartz".to_string()),
        girdle_mm: None,
        source: PreviewMaterialSource::RefractiveIndex,
    };
    assert_eq!(
        cache_fingerprint(PREVIEW, FingerprintMaterial::from_state(&legacy)),
        cache_fingerprint(PREVIEW, picked("Quartz"))
    );
    // The solid stand-in names no material at all.
    assert_eq!(
        cache_fingerprint(CacheKind::SolidDraft { size: 160 }, picked("Quartz")),
        "solid-draft;size=160"
    );
}

/// A design whose file names a material or a girdle diameter appends them after the
/// material: the size with two decimals, and the source.
#[test]
fn a_design_files_size_and_source_are_appended_to_the_material() {
    assert!(
        cache_fingerprint(PREVIEW, from_file("Sapphire", 9.0))
            .ends_with(";material=Sapphire;size=9.00;source=design")
    );
    assert!(
        cache_fingerprint(CacheKind::TiltCurves, from_file("Sapphire", 10.874))
            .ends_with(";material=Sapphire;size=10.87;source=design")
    );
    // A file that names a material but no size.
    assert!(
        cache_fingerprint(PREVIEW, from_file("Sapphire", 0.0))
            .ends_with(";material=Sapphire;source=design")
    );
    // A size without the source cannot happen, but is still told apart.
    let sized = FingerprintMaterial {
        name: Some("Sapphire"),
        girdle_mm: 9.0,
        from_design_file: false,
        frosting: 0,
    };
    assert!(cache_fingerprint(PREVIEW, sized).ends_with(";material=Sapphire;size=9.00"));
    // Different sizes differ, a difference below the printed precision does not.
    assert_ne!(
        cache_fingerprint(PREVIEW, from_file("Sapphire", 9.0)),
        cache_fingerprint(PREVIEW, from_file("Sapphire", 9.5))
    );
    assert_eq!(
        cache_fingerprint(PREVIEW, from_file("Sapphire", 9.0)),
        cache_fingerprint(PREVIEW, from_file("Sapphire", 9.001))
    );
    // Not a size: negative, not finite.
    for unset in [-1.0, f64::NAN, f64::INFINITY] {
        assert_eq!(
            cache_fingerprint(PREVIEW, from_file("Sapphire", unset)),
            cache_fingerprint(PREVIEW, from_file("Sapphire", 0.0)),
            "{unset}"
        );
    }
}

/// The material a scan builds from a persisted row is the one the writer built from
/// the resolved design.
#[test]
fn the_material_of_a_persisted_state_is_the_one_the_writer_used() {
    let state = PreviewMaterialState {
        name: Some("Sapphire".to_string()),
        girdle_mm: Some(9.0),
        source: PreviewMaterialSource::DesignFile,
    };
    assert_eq!(
        FingerprintMaterial::from_state(&state),
        from_file("Sapphire", 9.0)
    );
    assert_eq!(
        FingerprintMaterial::from_state(&PreviewMaterialState::default()),
        FingerprintMaterial::picked(None)
    );
}

/// The tilt fingerprint names the scoring lighting preset by its stable label, so
/// curves stored under another preset (the ring lights before 2026-10-07) are stale.
#[test]
fn the_tilt_fingerprint_names_the_scoring_lighting_preset() {
    let fingerprint = cache_fingerprint(CacheKind::TiltCurves, picked("Diamond"));
    let diamond_bands = material_bands_hash(&GemMaterial::diamond());
    assert_eq!(
        fingerprint,
        format!(
            "tilt;tracer={};lighting={};light={PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH};\
                 bands={diamond_bands:016x};material=Diamond",
            indicatrix::BUILD_ID,
            BATCH_TILT_LIGHTING_PRESET.label()
        )
    );
    assert_ne!(
        BATCH_TILT_LIGHTING_PRESET.label(),
        LightingPreset::RingLights.label()
    );
    let old_ring_lights = fingerprint.replace(
        BATCH_TILT_LIGHTING_PRESET.label(),
        LightingPreset::RingLights.label(),
    );
    assert_ne!(fingerprint, old_ring_lights);
}

/// The tilt sweep has no image size or sample budget, so changing the preview
/// settings must not look like a reason to recompute every design's curves. (The
/// `size=` of a design file's stone size is not one of them: it comes after the
/// material, and an unsized material has none.)
#[test]
fn the_tilt_fingerprint_names_no_preview_setting() {
    let fingerprint = cache_fingerprint(CacheKind::TiltCurves, picked("Diamond"));
    for setting in ["size=", "spp=", "bounces="] {
        assert!(!fingerprint.contains(setting), "{fingerprint}");
    }
}

/// A preview rendered with another exposure, head shadow, backdrop or surface glare is
/// stale; the same scene reads the same.
#[test]
fn the_preview_fingerprint_changes_with_the_exposure_and_the_head_shadow() {
    let bands = built_in_bands_hash("Diamond");
    let base = fingerprint_with(PREVIEW, picked("Diamond"), PreviewScene::current(), bands);
    assert_eq!(
        base,
        fingerprint_with(PREVIEW, picked("Diamond"), PreviewScene::current(), bands)
    );
    assert_eq!(base, cache_fingerprint(PREVIEW, picked("Diamond")));
    let brighter = PreviewScene {
        exposure: PreviewScene::current().exposure * 1.5,
        ..PreviewScene::current()
    };
    let wider_shadow = PreviewScene {
        head_shadow_deg: PreviewScene::current().head_shadow_deg + 8.0,
        ..PreviewScene::current()
    };
    let other_backdrop = PreviewScene {
        backdrop: "black",
        ..PreviewScene::current()
    };
    let other_tray_model = PreviewScene {
        tray_model: PreviewScene::current().tray_model + 1,
        ..PreviewScene::current()
    };
    let more_glare = PreviewScene {
        surface_glare: PreviewScene::current().surface_glare + 0.5,
        ..PreviewScene::current()
    };
    for changed in [
        brighter,
        wider_shadow,
        other_backdrop,
        other_tray_model,
        more_glare,
    ] {
        assert_ne!(
            fingerprint_with(PREVIEW, picked("Diamond"), changed, bands),
            base
        );
    }
    assert!(base.contains("exposure="), "{base}");
    assert!(base.contains("shadow="), "{base}");
    // The default Thumbnail Backdrop (Grey); no test changes it.
    assert!(base.contains(";backdrop=grey;tray="), "{base}");
    assert!(base.contains(";glare="), "{base}");
}

/// The material band hash moves when a band of the material changes and not when
/// something that is not color data does.
#[test]
fn the_band_hash_changes_with_the_material_bands_and_with_nothing_else() {
    let ruby = GemMaterial::by_name("Ruby").expect("a built-in");
    let base = material_bands_hash(&ruby);
    assert_eq!(
        base,
        material_bands_hash(&GemMaterial::by_name("Ruby").expect("a built-in"))
    );

    let mut stronger = ruby.clone();
    stronger.absorption.o_ray[0].peak *= 1.1;
    assert_ne!(material_bands_hash(&stronger), base, "one band peak");

    let mut shifted = ruby.clone();
    shifted.absorption.e_ray[0].center_nm += 5.0;
    assert_ne!(material_bands_hash(&shifted), base, "one band centre");

    let mut fewer = ruby.clone();
    fewer.absorption.o_ray.pop();
    assert_ne!(material_bands_hash(&fewer), base, "a band removed");

    let mut renamed = ruby;
    renamed.name = "Another Name".to_owned();
    renamed.absorption_path_scale = 3.0;
    assert_eq!(
        material_bands_hash(&renamed),
        base,
        "the name and the size scale are not color data"
    );
}

/// A changed band set regenerates the preview; an unchanged one does not.
#[test]
fn the_preview_fingerprint_changes_when_a_materials_bands_change() {
    let ruby = GemMaterial::by_name("Ruby").expect("a built-in");
    let mut edited = ruby.clone();
    edited.absorption.o_ray[0].peak += 0.5;
    let with = |material: &GemMaterial| {
        fingerprint_with(
            PREVIEW,
            picked("Ruby"),
            PreviewScene::current(),
            Some(material_bands_hash(material)),
        )
    };
    let same_again = GemMaterial::by_name("Ruby").expect("a built-in");
    assert_eq!(with(&ruby), with(&same_again));
    assert_ne!(with(&ruby), with(&edited));
    // The production fingerprint of a built-in is the one of its own data.
    assert_eq!(cache_fingerprint(PREVIEW, picked("Ruby")), with(&ruby));
}

/// A built-in carries its band hash (looked up by exact name, any case); a custom
/// material or no material carries none; different species differ.
#[test]
fn only_a_built_in_name_carries_a_band_hash() {
    let ruby = material_bands_hash(&GemMaterial::by_name("Ruby").expect("built-in"));
    assert!(cache_fingerprint(PREVIEW, picked("Sapphire")).contains(";bands="));
    for spelling in ["Ruby", "ruby", "RUBY"] {
        assert!(
            cache_fingerprint(PREVIEW, picked(spelling)).contains(&format!(";bands={ruby:016x};")),
            "{spelling}"
        );
    }
    for none in [
        picked("My Garnet"),
        picked("Ruby Red Custom"),
        FingerprintMaterial::picked(None),
    ] {
        assert!(!cache_fingerprint(PREVIEW, none).contains("bands="));
    }
    assert_ne!(
        built_in_bands_hash("Ruby").expect("built-in"),
        built_in_bands_hash("Sapphire").expect("built-in")
    );
}

/// The tilt fingerprint does not name the preview scene: the sweep is scored under its
/// own preset, so a preview-scene change must not recompute curves. It does name the
/// band hash of a built-in, since the curves are computed through the absorption.
#[test]
fn the_tilt_fingerprint_names_the_band_hash_but_not_the_preview_scene() {
    let fingerprint = cache_fingerprint(CacheKind::TiltCurves, picked("Ruby"));
    for field in ["exposure=", "shadow=", "backdrop=", "glare="] {
        assert!(!fingerprint.contains(field), "{fingerprint}");
    }
    assert!(fingerprint.contains(";bands="), "{fingerprint}");
    // A custom material carries none, as in a preview fingerprint.
    assert!(!cache_fingerprint(CacheKind::TiltCurves, picked("My Garnet")).contains("bands="));
}

/// The tilt fingerprint changes when a material's bands change and is equal otherwise.
#[test]
fn the_tilt_fingerprint_changes_when_a_materials_bands_change() {
    let ruby = GemMaterial::by_name("Ruby").expect("a built-in");
    let mut edited = ruby.clone();
    edited.absorption.e_ray[0].peak += 0.5;
    let with = |material: &GemMaterial, scene: PreviewScene| {
        fingerprint_with(
            CacheKind::TiltCurves,
            picked("Ruby"),
            scene,
            Some(material_bands_hash(material)),
        )
    };
    let same_again = GemMaterial::by_name("Ruby").expect("a built-in");
    assert_eq!(
        with(&ruby, PreviewScene::current()),
        with(&same_again, PreviewScene::current())
    );
    assert_ne!(
        with(&ruby, PreviewScene::current()),
        with(&edited, PreviewScene::current())
    );
    // The preview scene settings do not move it.
    let brighter = PreviewScene {
        exposure: PreviewScene::current().exposure * 2.0,
        ..PreviewScene::current()
    };
    assert_eq!(with(&ruby, PreviewScene::current()), with(&ruby, brighter));
    // The production fingerprint of a built-in is the one of its own data.
    assert_eq!(
        cache_fingerprint(CacheKind::TiltCurves, picked("Ruby")),
        with(&ruby, PreviewScene::current())
    );
}

/// The frosting hash is `0` for none, never `0` for any frosting, and moves with the
/// tiers and the facet slots.
#[test]
fn the_frosting_hash_is_zero_for_none_and_tells_frostings_apart() {
    assert_eq!(frosting_hash(&[]), 0);
    let one_tier = frosting_hash(&frosting_of(&[1]));
    let other_tier = frosting_hash(&frosting_of(&[2]));
    let two_tiers = frosting_hash(&frosting_of(&[1, 2]));
    let facets = frosting_hash(&[StoredTierFrosting {
        tier: 1,
        facets: vec![0, 2],
    }]);
    let all = [one_tier, other_tier, two_tiers, facets];
    assert!(all.iter().all(|&hash| hash != 0));
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a, b);
        }
    }
    assert_eq!(one_tier, frosting_hash(&frosting_of(&[1])));
}

/// A frosted design never reuses an unfrosted cached picture or curve: frosting is part
/// of both fingerprints, differs by what is frosted, and is absent for a polished design.
#[test]
fn frosting_is_part_of_both_fingerprints() {
    let polished = picked("Sapphire");
    let whole = polished.with_frosting(frosting_hash(&frosting_of(&[1])));
    let other = polished.with_frosting(frosting_hash(&frosting_of(&[2])));
    for kind in [PREVIEW, CacheKind::TiltCurves] {
        let base = cache_fingerprint(kind, polished);
        let frosted = cache_fingerprint(kind, whole);
        assert_ne!(base, frosted, "{kind:?}");
        assert_ne!(frosted, cache_fingerprint(kind, other), "{kind:?}");
        assert_eq!(frosted, cache_fingerprint(kind, whole), "{kind:?}");
        assert!(frosted.contains(";frosting="), "{frosted}");
        assert!(!base.contains("frosting="), "{base}");
    }
    // `with_frosting(0)` is the polished design, byte for byte.
    assert_eq!(
        cache_fingerprint(PREVIEW, polished.with_frosting(0)),
        cache_fingerprint(PREVIEW, polished)
    );
    // The solid stand-in is flat shaded and names neither.
    assert_eq!(
        cache_fingerprint(CacheKind::SolidDraft { size: 160 }, whole),
        "solid-draft;size=160"
    );
}

#[test]
fn ray_drafts_never_satisfy_persistent_previews_and_quality_is_versioned() {
    let material = picked("Sapphire");
    let ray =
        |size, taps, fire| cache_fingerprint(CacheKind::RayPreview { size, taps, fire }, material);
    let original = ray(160, 8, true);
    assert!(original.starts_with(&format!(
        "ray-preview;v={};",
        indicatrix::renderer::whitted::WHITTED_VERSION
    )));
    assert_ne!(original, cache_fingerprint(PREVIEW, material));
    assert_ne!(
        original,
        cache_fingerprint(CacheKind::SolidDraft { size: 160 }, material)
    );
    assert_ne!(
        original,
        cache_fingerprint(
            CacheKind::RayDraft {
                size: 160,
                taps: 8,
                fire: true
            },
            material
        )
    );
    assert_ne!(original, ray(320, 8, true));
    assert_ne!(original, ray(160, 16, true));
    assert_ne!(original, ray(160, 8, false));
    assert_ne!(
        original,
        cache_fingerprint(
            CacheKind::RayPreview {
                size: 160,
                taps: 8,
                fire: true
            },
            from_file("Sapphire", 9.0)
        )
    );
}

#[test]
fn ray_draft_fingerprint_has_the_exact_versioned_contract() {
    let backdrop = super::super::thumbnail_backdrop_name();
    let fingerprint = fingerprint_with(
        CacheKind::RayDraft {
            size: 160,
            taps: 8,
            fire: true,
        },
        FingerprintMaterial::picked(None),
        PreviewScene::current(),
        None,
    );
    assert_eq!(
        fingerprint,
        format!(
            "ray-draft;v={};size=160;taps=8;fire=true;lighting={};light={PREVIEW_LIGHT_YAW}/{PREVIEW_LIGHT_PITCH};exposure={PREVIEW_EXPOSURE};shadow={PREVIEW_HEAD_SHADOW_DEG};backdrop={backdrop};tray={};glare={PREVIEW_SURFACE_GLARE};material=none",
            indicatrix::renderer::whitted::WHITTED_VERSION,
            PREVIEW_LIGHTING_PRESET.label(),
            indicatrix::optics::raytracer::TRAY_MODEL_VERSION
        )
    );
}
