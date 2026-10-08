//! The protocol's round trips and the pinned wire bytes.

use super::*;
use crate::{
    scene::{CameraSpec, FinishSpec, LightingSpec, MaterialSpec, planes_to_data},
    solve::{
        AngleChangeData, BlockData, MaterialSelectionData, MetricsParams, MetricsResultData,
        OptimizeParams, OptimizeResultData, RetargetModeData, RetargetParams, RetargetResultData,
        RetargetRowData, RiskData, ScoredUnder, SolveOutcome, SolvedTierData, StrategyData,
        TierWarning, TiltAxisData, TiltParams, TiltResultData,
    },
};
use indicatrix::{
    geometry::cuts::StandardGemCuts, optics::raytracer::LightingPreset, render_setup::Backdrop,
};

fn round_trip_to(message: &ToWorker) {
    let bytes = encode_to_worker(message).expect("encodes");
    assert_eq!(&decode_to_worker(&bytes).expect("decodes"), message);
}

fn round_trip_from(message: &FromWorker) {
    let bytes = encode_from_worker(message).expect("encodes");
    assert_eq!(&decode_from_worker(&bytes).expect("decodes"), message);
}

#[test]
fn every_page_to_worker_message_round_trips() {
    let spec = SceneSpec {
        planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
        finishes: FinishSpec::PerFacet(vec![true, false]),
        material: MaterialSpec {
            custom_materials: vec![indicatrix::optics::materials::GemMaterial::new_custom(
                "Garnet 1.74",
                1.74,
                0.024,
                0.0,
                [0.1, 0.0, 0.2],
            )],
            ..MaterialSpec::catalogue("Sapphire")
        },
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
        max_bounces: 12,
        width: 640,
        height: 480,
        hdr_id: Some(3),
    };
    for message in [
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION,
            role: WorkerRole::Render,
            worker_index: 2,
        },
        ToWorker::SetScene { scene_id: 9, spec },
        ToWorker::HdrMap {
            id: 3,
            bytes: vec![1, 2, 3, 255],
        },
        ToWorker::ClearHdr,
        ToWorker::TraceChunk {
            scene_id: 9,
            first_pixel: 1,
            stride: 4,
            sample_offset: 17,
            spp: 8,
        },
        ToWorker::Solve {
            job_id: 5,
            design_toml: "[design]".to_string(),
            request: SolveRequest::Solve,
        },
        ToWorker::Cancel { job_id: 5 },
        ToWorker::WatchCancel {
            job_id: 5,
            url: "blob:https://example.org/1-2".to_string(),
        },
        ToWorker::Picture {
            scene_id: 9,
            sample_count: 64,
            sums: vec![[1.0, 2.5, -0.0]],
            kind: PictureKind::Png {
                color_space: 1,
                denoise: true,
            },
        },
    ] {
        round_trip_to(&message);
    }
}

#[test]
fn every_worker_to_page_message_round_trips() {
    for message in [
        FromWorker::Loaded {
            protocol_version: PROTOCOL_VERSION,
        },
        FromWorker::Ready {
            role: WorkerRole::Solve,
            worker_index: 0,
        },
        FromWorker::ChunkResult {
            scene_id: 9,
            first_pixel: 1,
            stride: 4,
            sample_offset: 17,
            spp: 8,
            sums: vec![[0.5, -0.0, f32::MAX]],
            elapsed_ms: 187.25,
        },
        FromWorker::ChunkDropped {
            scene_id: 9,
            first_pixel: 1,
            sample_offset: 17,
        },
        FromWorker::SceneError {
            scene_id: 9,
            message: "no".to_string(),
        },
        FromWorker::HdrLoaded {
            id: 3,
            width: 64,
            height: 32,
        },
        FromWorker::HdrError {
            id: 3,
            message: "bad".to_string(),
        },
        FromWorker::Progress {
            job_id: 5,
            message: "half".to_string(),
            fraction: Some(0.5),
        },
        FromWorker::SolveResult {
            job_id: 5,
            response: SolveResponse::Solved(SolveOutcome {
                solved: Some(vec![SolvedTierData {
                    mast: 1.25,
                    strategy: StrategyData::JointGroup,
                    detail: "vertex".to_string(),
                }]),
                error: None,
                status_text: "Closed".to_string(),
                status_is_problem: false,
                warnings: vec![TierWarning {
                    tier_index: 3,
                    text: "small".to_string(),
                }],
                planes: Vec::new(),
                too_many_planes: false,
                tier_count: 1,
            }),
            elapsed_ms: 3.5,
        },
        FromWorker::Error {
            message: "oops".to_string(),
        },
        FromWorker::Picture {
            scene_id: 9,
            sample_count: 64,
            kind: PictureKind::DenoisedLive,
            bytes: vec![0, 1, 2, 255],
            elapsed_ms: 812.5,
        },
        FromWorker::PictureFailed {
            scene_id: 9,
            kind: PictureKind::DenoisedLive,
            message: "no scene".to_string(),
        },
        FromWorker::ChunkAborted {
            scene_id: 9,
            first_pixel: 1,
            sample_offset: 17,
        },
    ] {
        round_trip_from(&message);
    }
}

/// Space-separated upper-case hex, the form the pinned bytes below are written in.
fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The bytes of one message of each direction, pinned. postcard writes an enum as its
/// variant index, integers as LEB128 varints, floats as little-endian IEEE-754, a
/// string or `Vec` as a varint length and then its items, and a fixed array without a
/// length. A Worker script cached from another build reads the same bytes
/// differently, which is what [`PROTOCOL_VERSION`] guards: any change here is a
/// protocol change, so it must bump that constant and update these bytes.
#[test]
fn wire_format_is_pinned() {
    assert_eq!(
        PROTOCOL_VERSION, 15,
        "the bytes below were pinned for version 15"
    );

    // TraceChunk = variant 4; scene id 300 = varint AC 02.
    let trace = ToWorker::TraceChunk {
        scene_id: 300,
        first_pixel: 1,
        stride: 4,
        sample_offset: 17,
        spp: 8,
    };
    assert_eq!(
        hex(&encode_to_worker(&trace).expect("encodes")),
        "04 AC 02 01 04 11 08"
    );
    // WatchCancel = variant 8; "blob:x" is 6 bytes.
    let watch = ToWorker::WatchCancel {
        job_id: 5,
        url: "blob:x".to_string(),
    };
    assert_eq!(
        hex(&encode_to_worker(&watch).expect("encodes")),
        "08 05 06 62 6C 6F 62 3A 78"
    );
    // Init = variant 0, then version 15, role Render = 0, worker index 2.
    let init = ToWorker::Init {
        protocol_version: PROTOCOL_VERSION,
        role: WorkerRole::Render,
        worker_index: 2,
    };
    assert_eq!(
        hex(&encode_to_worker(&init).expect("encodes")),
        "00 0F 00 02"
    );

    // ChunkResult = variant 2; one [0.5, 1.0, 2.0] sum; 187.25 ms = 0x4067680000000000.
    let result = FromWorker::ChunkResult {
        scene_id: 9,
        first_pixel: 1,
        stride: 4,
        sample_offset: 17,
        spp: 8,
        sums: vec![[0.5, 1.0, 2.0]],
        elapsed_ms: 187.25,
    };
    assert_eq!(
        hex(&encode_from_worker(&result).expect("encodes")),
        "02 09 01 04 11 08 01 00 00 00 3F 00 00 80 3F 00 00 00 40 00 00 00 00 00 68 67 40"
    );
    // ChunkAborted = variant 12, the last one.
    let aborted = FromWorker::ChunkAborted {
        scene_id: 9,
        first_pixel: 1,
        sample_offset: 17,
    };
    assert_eq!(
        hex(&encode_from_worker(&aborted).expect("encodes")),
        "0C 09 01 11"
    );
    // Loaded = variant 0, then the version.
    let loaded = FromWorker::Loaded {
        protocol_version: PROTOCOL_VERSION,
    };
    assert_eq!(hex(&encode_from_worker(&loaded).expect("encodes")), "00 0F");
}

/// `LightingSpec` pinned: preset index and backdrop index as zigzag varints, then the
/// three `f32`s and the appended v11 `head_shadow_deg` (16.0 = `00 00 80 41`). The
/// first preset of the combo (the light tent) is index 0.
#[test]
fn lighting_spec_wire_format_is_pinned() {
    let spec = LightingSpec::new(
        LightingPreset::LightTent,
        1.0,
        0.5,
        0.25,
        Backdrop::Grey,
        16.0,
    );
    assert_eq!(
        hex(&postcard::to_allocvec(&spec).expect("encodes")),
        "00 00 00 80 3F 00 00 00 3F 00 00 80 3E 02 00 00 80 41"
    );
}

/// A Retarget message from before `crown_follows_pavilion` existed (as a self-describing
/// format would carry it) reads the field as `true`, the default: the crown follows the
/// pavilion. The postcard wire itself has no defaults, which is why the field bumped
/// [`PROTOCOL_VERSION`].
#[test]
fn a_retarget_message_without_the_crown_follow_field_reads_it_as_true() {
    let params = RetargetParams {
        target: MaterialSelectionData::default(),
        crown_fraction: 0.25,
        scale_crown_by_ratio: false,
        crown_follows_pavilion: false,
        mode: RetargetModeData::Shift,
        optimize: OptimizeParams::default(),
    };
    let mut json = serde_json::to_value(&params).expect("serialises");
    let object = json.as_object_mut().expect("a struct is an object");
    assert_eq!(
        object.remove("crown_follows_pavilion"),
        Some(serde_json::Value::Bool(false))
    );
    let old: RetargetParams = serde_json::from_value(json).expect("an old message still reads");
    assert!(old.crown_follows_pavilion);
    assert!((old.crown_fraction - 0.25).abs() < 1e-12);
}

/// A `CustomMaterialSpec` carrying a `color_recipe`, pinned like the messages above:
/// the name is a length-prefixed string, each `f64` eight little-endian bytes, an
/// `Option` a 0/1 tag before its payload, `recipe_json` a length-prefixed string and
/// `fallback_rgb` a fixed array without a length. It travels inside
/// `SceneSpec::custom_materials`, so a change here is a protocol change too.
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
    };
    // "Ruby" = 04 + 4 bytes; 1.5, 0.25, 0.0 as f64; Some = 01 then [0.5, 1.0, 2.0];
    // Some = 01, "{}" = 02 7B 7D, then fallback_rgb [0.5, 1.0, 2.0].
    assert_eq!(
        hex(&postcard::to_allocvec(&spec).expect("encodes")),
        "04 52 75 62 79 00 00 00 00 00 00 F8 3F 00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 00 00 01 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 01 02 7B 7D 00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 F0 3F 00 00 00 00 00 00 00 40 00"
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
    for message in [
        FromWorker::SolveResult {
            job_id: 6,
            response: SolveResponse::Optimized(OptimizeResultData {
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
