/// A `CustomMaterialSpec` carrying a `color_recipe`, pinned like the messages above:
/// the name is a length-prefixed string, each `f64` eight little-endian bytes, an
/// `Option` a 0/1 tag before its payload, `recipe_json` a length-prefixed string and
/// `fallback_rgb` a fixed array without a length. It travels inside
/// `SceneSpec::custom_materials`, so a change here is a protocol change too. Version 16
/// appended the crystal system and optical character (empty strings, one `00` length byte
/// each) and the biaxial delta (`None`, one `00` tag byte).
#[test]
fn custom_material_with_color_recipe_wire_format_is_pinned() {
    let spec = crate::scene::CustomMaterialSpec {
        name: "Ruby".to_string(),
        mean_ri: 1.5,
        dispersion_delta: 0.25,
        birefringence_delta: 0.0,
        absorption_rgb: Some([0.5, 1.0, 2.0]),
        color_recipe: Some(indicatrix_formats::native::ColorRecipeDto {
            recipe_json: "{}".to_string(),
            fallback_rgb: [0.5, 1.0, 2.0],
        }),
        absorption_bands: Vec::new(),
        crystal_system: String::new(),
        optical_character: String::new(),
        biaxial_delta_beta_alpha: None,
    };
    // "Ruby" = 04 + 4 bytes; 1.5, 0.25, 0.0 as f64; Some = 01 then [0.5, 1.0, 2.0];
    // Some = 01, "{}" = 02 7B 7D, then fallback_rgb [0.5, 1.0, 2.0]; no bands = 00; the two
    // empty names = 00 00; no biaxial delta = 00.
    assert_eq!(
        hex(&postcard::to_allocvec(&spec).expect("encodes")),
        "04 52 75 62 79 00 00 00 00 00 00 F8 3F 00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 00 00 01 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 01 02 7B 7D 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 00 00 00 00"
    );
}

#[test]
fn the_optimize_and_retarget_messages_round_trip() {
    for message in [
        ToWorker::Solve {
            job_id: 6,
            design_toml: "[design]".to_string(),
            request: SolveRequest::Optimize {
                params: OptimizeParams {
                    only_tiers: Some(vec![1, 4]),
                    ..OptimizeParams::default()
                },
                custom_materials: vec![indicatrix::optics::materials::GemMaterial::new_custom(
                    "Garnet 1.74",
                    1.74,
                    0.024,
                    0.0,
                    [0.1, 0.0, 0.2],
                )],
            },
        },
        ToWorker::Solve {
            job_id: 7,
            design_toml: "[design]".to_string(),
            request: SolveRequest::Retarget {
                params: RetargetParams {
                    target: MaterialSelectionData {
                        name: Some("Quartz".to_string()),
                        refractive_index_override: Some(1.55),
                        ..MaterialSelectionData::default()
                    },
                    crown_fraction: 0.5,
                    scale_crown_by_ratio: true,
                    crown_follows_pavilion: false,
                    mode: RetargetModeData::Optimize,
                    optimize: OptimizeParams::default(),
                },
                custom_materials: Vec::new(),
            },
        },
    ] {
        round_trip_to(&message);
    }
}

#[test]
fn the_optimize_and_retarget_answers_round_trip() {
    // Built through `From<&AnchorChange>`: `solve::analysis` is private, so the wire type
    // itself is not nameable here.
    let anchor = indicatrix_editor::retarget::AnchorChange {
        tier_index: 2,
        old_mast: 0.6,
        new_mast: 0.625,
    };
    for message in [
        FromWorker::SolveResult {
            job_id: 6,
            response: SolveResponse::Optimized(OptimizeResultData {
                provenance: None,
                before: [1.0, 2.0, 3.0],
                before_score: 4.5,
                before_yield_loss_pct: 30.0,
                after: [0.5, 2.5, 3.5],
                after_score: 4.0,
                after_yield_loss_pct: 31.0,
                evaluations: 42,
                changes: vec![AngleChangeData {
                    index: 3,
                    from_deg: 40.75,
                    to_deg: 41.0,
                }],
                cancelled: false,
                polish_evaluations: 5,
                polish_improvement: 0.25,
                defaulted_ri: Some(1.54),
            }),
            elapsed_ms: 900.0,
        },
        FromWorker::SolveResult {
            job_id: 7,
            response: SolveResponse::Retargeted(RetargetResultData {
                rows: vec![RetargetRowData {
                    tier_index: 2,
                    block: BlockData::Pavilion,
                    name: "P1".to_string(),
                    old_angle: -40.75,
                    new_angle: -42.0,
                    margin_deg: 1.5,
                    risk: RiskData::Marginal,
                }],
                notes: vec!["note".to_string()],
                anchored_errors: vec!["#1 \"C1\"".to_string()],
                solve_error: String::new(),
                anchors: std::iter::once(&anchor).map(Into::into).collect(),
                validity_headline: "Valid: girdle 6.0 % (was 6.0 %)".to_string(),
            }),
            elapsed_ms: 12.0,
        },
        FromWorker::SolveResult {
            job_id: 8,
            response: SolveResponse::AnalysisFailed {
                message: "no anchor".to_string(),
                missing_anchor: true,
            },
            elapsed_ms: 1.0,
        },
    ] {
        round_trip_from(&message);
    }
}

#[test]
fn the_metrics_and_tilt_messages_round_trip() {
    let spec = SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.45,
            distance: 2.4,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.85,
            0.95,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 12,
        width: 64,
        height: 48,
        hdr_id: None,
    };
    let lit_by_map = SceneSpec {
        hdr_id: Some(3),
        ..spec.clone()
    };
    for message in [
        ToWorker::Solve {
            job_id: 9,
            design_toml: String::new(),
            request: SolveRequest::Metrics {
                params: MetricsParams::from_scene(&spec),
            },
        },
        ToWorker::Solve {
            job_id: 9,
            design_toml: String::new(),
            request: SolveRequest::Metrics {
                params: MetricsParams::from_scene(&lit_by_map),
            },
        },
        ToWorker::Solve {
            job_id: 10,
            design_toml: String::new(),
            request: SolveRequest::Tilt {
                params: TiltParams::from_scene(&spec),
            },
        },
    ] {
        round_trip_to(&message);
    }
    let axis = |azimuth_deg: f32| TiltAxisData {
        azimuth_deg,
        brilliance: vec![1.0; 181],
        extinction: vec![2.5; 181],
        windowing: vec![-0.0; 181],
    };
    for scored_under in [ScoredUnder::Preset(5), ScoredUnder::HdrMap(3)] {
        round_trip_from(&FromWorker::SolveResult {
            job_id: 9,
            response: SolveResponse::Metrics(MetricsResultData {
                brilliance_pct: 61.25,
                fire_index: 1.5,
                scintillation_pct: 40.0,
                windowing_pct: 8.75,
                extinction_pct: 14.0,
                scored_under,
            }),
            elapsed_ms: 40.0,
        });
    }
    round_trip_from(&FromWorker::SolveResult {
        job_id: 10,
        response: SolveResponse::TiltCurves(TiltResultData {
            axes: vec![axis(0.0), axis(45.0), axis(90.0), axis(135.0)],
        }),
        elapsed_ms: 3000.0,
    });
}

/// The HDR map's place in the metrics messages, pinned: `hdr_id` is the last field of the
/// request (the message ends with it) and `scored_under` the last of the result.
#[test]
fn the_metrics_hdr_fields_are_pinned() {
    let spec = |hdr_id| SceneSpec {
        planes: Vec::new(),
        finishes: FinishSpec::AllPolished,
        material: MaterialSpec::catalogue("Diamond"),
        camera: CameraSpec {
            yaw: 0.6,
            pitch: 0.45,
            distance: 2.4,
        },
        lighting: LightingSpec::new(
            LightingPreset::LightTent,
            1.0,
            0.85,
            0.95,
            Backdrop::Grey,
            16.0,
        ),
        max_bounces: 12,
        width: 64,
        height: 48,
        hdr_id,
    };
    let request = |hdr_id| {
        encode_to_worker(&ToWorker::Solve {
            job_id: 9,
            design_toml: String::new(),
            request: SolveRequest::Metrics {
                params: MetricsParams::from_scene(&spec(hdr_id)),
            },
        })
        .expect("encodes")
    };
    // `Option<u64>`: a tag byte, then the varint (300 = AC 02).
    let named = request(Some(300));
    assert_eq!(named[named.len() - 3..], [0x01, 0xAC, 0x02]);
    assert_eq!(request(None).last(), Some(&0x00));

    // SolveResult = variant 8, job 9; Metrics = response variant 6; five f32s; then
    // `ScoredUnder`: HdrMap = variant 1 with id 300 (AC 02), Preset = variant 0 with the
    // zigzag varint of -1 (01); then the f64 elapsed time.
    let result = |scored_under| {
        hex(&encode_from_worker(&FromWorker::SolveResult {
            job_id: 9,
            response: SolveResponse::Metrics(MetricsResultData {
                brilliance_pct: 1.0,
                fire_index: 2.0,
                scintillation_pct: 0.5,
                windowing_pct: 0.0,
                extinction_pct: 4.0,
                scored_under,
            }),
            elapsed_ms: 0.0,
        })
        .expect("encodes"))
    };
    let body = "06 00 00 80 3F 00 00 00 40 00 00 00 3F 00 00 00 00 00 00 80 40";
    let tail = "00 00 00 00 00 00 00 00";
    assert_eq!(
        result(ScoredUnder::HdrMap(300)),
        format!("08 09 {body} 01 AC 02 {tail}")
    );
    assert_eq!(
        result(ScoredUnder::Preset(-1)),
        format!("08 09 {body} 00 01 {tail}")
    );
}

#[test]
fn garbage_is_an_error_not_a_panic() {
    assert!(decode_to_worker(&[0xff, 0xff, 0xff]).is_err());
    assert!(decode_from_worker(&[]).is_err());
}

/// The v12 band fields pinned: `body_color_bands` is a varint length and then each row as
/// three little-endian `f32`s, `absorption_path_scale_override` a 0/1 tag before its payload.
/// `DesignMaterialOverrides` is `refractive_index_override` (`None` = 00), `body_color_override`
/// (`None` = 00), then the two new fields; an old message without bands writes `00 00` for them.
#[test]
fn the_band_fields_wire_format_is_pinned_and_round_trips() {
    let overrides = crate::scene::DesignMaterialOverrides {
        body_color_bands: vec![[1.0, 2.0, 3.0]],
        absorption_path_scale_override: Some(2.0),
        ..crate::scene::DesignMaterialOverrides::default()
    };
    let bytes = postcard::to_allocvec(&overrides).expect("encodes");
    assert_eq!(
        hex(&bytes),
        "00 00 01 00 00 80 3F 00 00 00 40 00 00 40 40 01 00 00 00 40"
    );
    let back: crate::scene::DesignMaterialOverrides =
        postcard::from_bytes(&bytes).expect("decodes");
    assert_eq!(back, overrides);
    assert_eq!(
        hex(
            &postcard::to_allocvec(&crate::scene::DesignMaterialOverrides::default())
                .expect("encodes")
        ),
        "00 00 00 00"
    );

    // The retarget target carries the same two fields after its four older ones.
    let target = MaterialSelectionData {
        body_color_bands: vec![[460.0, 45.0, 0.25]],
        absorption_path_scale_override: Some(1.5),
        ..MaterialSelectionData::default()
    };
    let bytes = postcard::to_allocvec(&target).expect("encodes");
    let back: MaterialSelectionData = postcard::from_bytes(&bytes).expect("decodes");
    assert_eq!(back, target);
    let selection = indicatrix_cut_core::MaterialSelection::from(back);
    assert_eq!(
        selection.body_color_bands_override,
        Some(vec![[460.0, 45.0, 0.25]])
    );
    assert_eq!(selection.absorption_path_scale_override, Some(1.5));
    assert_eq!(MaterialSelectionData::from(&selection), target);
}

/// The body-colour job crosses the page/Worker boundary both ways (request variant 5, response
/// variant 8, each the last of its enum).
#[test]
fn the_body_colour_job_round_trips_and_is_the_last_variant() {
    use crate::solve::{BodyColorParams, BodyColorResultData};
    let request = ToWorker::Solve {
        job_id: 9,
        design_toml: String::new(),
        request: SolveRequest::BodyColor {
            params: BodyColorParams {
                lch: [50.0, 30.0, 265.0],
                path_mm: 5.0,
            },
        },
    };
    round_trip_to(&request);
    let bytes = postcard::to_allocvec(&SolveRequest::BodyColor {
        params: BodyColorParams {
            lch: [0.0; 3],
            path_mm: 0.0,
        },
    })
    .expect("encodes");
    assert_eq!(bytes[0], 5, "appended after Tilt (variant 4)");

    let answer = FromWorker::SolveResult {
        job_id: 9,
        response: SolveResponse::BodyColor(BodyColorResultData {
            lch: [50.0, 30.0, 265.0],
            path_mm: 5.0,
            delta_e: 0.5,
            reachable: true,
            bands: vec![[460.0, 45.0, 0.25]],
            triple: [0.1, 0.2, 0.3],
            swatches: vec![[0.2, 0.3, 0.9]],
        }),
        elapsed_ms: 150.0,
    };
    round_trip_from(&answer);
    let bytes = postcard::to_allocvec(&SolveResponse::BodyColor(BodyColorResultData {
        lch: [0.0; 3],
        path_mm: 0.0,
        delta_e: 0.0,
        reachable: false,
        bands: Vec::new(),
        triple: [0.0; 3],
        swatches: Vec::new(),
    }))
    .expect("encodes");
    assert_eq!(bytes[0], 8, "appended after TiltCurves (variant 7)");
}
