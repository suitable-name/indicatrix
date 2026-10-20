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
        ToWorker::GpuCompile {
            class: 1,
            kernels: vec!["ew_extend".to_string(), "ew_surface".to_string()],
            soft_ms: 5000,
            hard_ms: 20000,
        },
        ToWorker::TraceFullPass {
            scene_id: 9,
            pass_id: 3,
            sample_offset: 17,
            spp: 8,
        },
    ] {
        round_trip_to(&message);
    }
}

/// One of each page-bound message that is not a GPU event.
fn render_and_solve_messages() -> Vec<FromWorker> {
    vec![
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
    ]
}

/// One of each page-bound GPU event.
fn gpu_messages() -> Vec<FromWorker> {
    vec![
        FromWorker::GpuAdapter {
            vendor: "NVIDIA".to_string(),
            architecture: "Ada".to_string(),
            device: "RTX 4090".to_string(),
            description: "GeForce".to_string(),
            backend: "Vulkan".to_string(),
            max_storage_buffers_per_stage: 8,
            max_storage_binding: 134_217_728,
            max_buffer: 268_435_456,
        },
        FromWorker::GpuUnavailable {
            reason: "no adapter".to_string(),
        },
        FromWorker::GpuCompiled {
            kernel: "ew_extend".to_string(),
            class: 1,
            ms: 124.5,
        },
        FromWorker::GpuCompileFailed {
            kernel: "ew_surface".to_string(),
            class: 1,
            reason: "syntax error".to_string(),
            ms: 12.0,
        },
        FromWorker::FullPassResult {
            scene_id: 9,
            pass_id: 3,
            sample_offset: 17,
            spp: 8,
            sums: vec![[0.5, -0.0, f32::MAX]],
            elapsed_ms: 25.5,
        },
        FromWorker::GpuLost {
            reason: "device reset".to_string(),
        },
    ]
}

#[test]
fn every_worker_to_page_message_round_trips() {
    for message in render_and_solve_messages()
        .into_iter()
        .chain(gpu_messages())
    {
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

/// The bytes of one page-to-Worker message of each kind, pinned.
///
/// postcard writes an enum as its variant index, integers as LEB128 varints, floats as
/// little-endian IEEE-754, a string or `Vec` as a varint length and then its items, and a
/// fixed array without a length. A Worker script cached from another build reads the same
/// bytes differently, which is what [`PROTOCOL_VERSION`] guards: any change here is a
/// protocol change, so it must bump that constant and update these bytes.
#[test]
fn wire_format_is_pinned_to_worker() {
    assert_eq!(
        PROTOCOL_VERSION, 24,
        "the bytes below were pinned for version 24"
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
    // Init = variant 0, then version 24 (0x18), role Render = 0, worker index 2.
    let init = ToWorker::Init {
        protocol_version: PROTOCOL_VERSION,
        role: WorkerRole::Render,
        worker_index: 2,
    };
    assert_eq!(
        hex(&encode_to_worker(&init).expect("encodes")),
        "00 18 00 02"
    );
    // Init with role Gpu = 2, worker index 0.
    let init_gpu = ToWorker::Init {
        protocol_version: PROTOCOL_VERSION,
        role: WorkerRole::Gpu,
        worker_index: 0,
    };
    assert_eq!(
        hex(&encode_to_worker(&init_gpu).expect("encodes")),
        "00 18 02 00"
    );
    // GpuCompile = variant 9.
    let gpu_compile = ToWorker::GpuCompile {
        class: 1,
        kernels: vec!["a".to_string()],
        soft_ms: 10,
        hard_ms: 20,
    };
    assert_eq!(
        hex(&encode_to_worker(&gpu_compile).expect("encodes")),
        "09 01 01 01 61 0A 14"
    );
    // TraceFullPass = variant 10.
    let trace_full = ToWorker::TraceFullPass {
        scene_id: 9,
        pass_id: 2,
        sample_offset: 17,
        spp: 8,
    };
    assert_eq!(
        hex(&encode_to_worker(&trace_full).expect("encodes")),
        "0A 09 02 11 08"
    );
}

/// The bytes of one Worker-to-page message of each kind, pinned (see
/// [`wire_format_is_pinned_to_worker`]).
#[test]
fn wire_format_is_pinned_from_worker() {
    assert_eq!(
        PROTOCOL_VERSION, 24,
        "the bytes below were pinned for version 24"
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
    // ChunkAborted = variant 12.
    let aborted = FromWorker::ChunkAborted {
        scene_id: 9,
        first_pixel: 1,
        sample_offset: 17,
    };
    assert_eq!(
        hex(&encode_from_worker(&aborted).expect("encodes")),
        "0C 09 01 11"
    );
    // GpuAdapter = variant 13.
    let gpu_adapter = FromWorker::GpuAdapter {
        vendor: "v".to_string(),
        architecture: "a".to_string(),
        device: "d".to_string(),
        description: String::new(),
        backend: "b".to_string(),
        max_storage_buffers_per_stage: 8,
        max_storage_binding: 16,
        max_buffer: 32,
    };
    assert_eq!(
        hex(&encode_from_worker(&gpu_adapter).expect("encodes")),
        "0D 01 76 01 61 01 64 00 01 62 08 10 20"
    );
    // GpuUnavailable = variant 14.
    let gpu_unavail = FromWorker::GpuUnavailable {
        reason: "no".to_string(),
    };
    assert_eq!(
        hex(&encode_from_worker(&gpu_unavail).expect("encodes")),
        "0E 02 6E 6F"
    );
    // GpuCompiled = variant 15.
    let gpu_compiled = FromWorker::GpuCompiled {
        kernel: "k".to_string(),
        class: 1,
        ms: 1.0,
    };
    assert_eq!(
        hex(&encode_from_worker(&gpu_compiled).expect("encodes")),
        "0F 01 6B 01 00 00 00 00 00 00 F0 3F"
    );
    // GpuCompileFailed = variant 16.
    let gpu_failed = FromWorker::GpuCompileFailed {
        kernel: "k".to_string(),
        class: 1,
        reason: "fail".to_string(),
        ms: 1.0,
    };
    assert_eq!(
        hex(&encode_from_worker(&gpu_failed).expect("encodes")),
        "10 01 6B 01 04 66 61 69 6C 00 00 00 00 00 00 F0 3F"
    );
    // FullPassResult = variant 17, the last one before GpuLost.
    let full_pass_result = FromWorker::FullPassResult {
        scene_id: 9,
        pass_id: 2,
        sample_offset: 17,
        spp: 8,
        sums: vec![[0.5, 1.0, 2.0]],
        elapsed_ms: 1.0,
    };
    assert_eq!(
        hex(&encode_from_worker(&full_pass_result).expect("encodes")),
        "11 09 02 11 08 01 00 00 00 3F 00 00 80 3F 00 00 00 40 00 00 00 00 00 00 F0 3F"
    );
    // GpuLost = variant 18, the last one.
    let gpu_lost = FromWorker::GpuLost {
        reason: "lost".to_string(),
    };
    assert_eq!(
        hex(&encode_from_worker(&gpu_lost).expect("encodes")),
        "12 04 6C 6F 73 74"
    );
    // Loaded = variant 0, then the version 24 (0x18): the one Worker-to-page message that
    // carries the version, so a bump changes this byte too.
    let loaded = FromWorker::Loaded {
        protocol_version: PROTOCOL_VERSION,
    };
    assert_eq!(hex(&encode_from_worker(&loaded).expect("encodes")), "00 18");
}

/// `LightingSpec` pinned: preset index and backdrop index as zigzag varints, then the
/// three `f32`s, the appended v11 `head_shadow_deg` (16.0 = `00 00 80 41`) and the appended
/// v17 `surface_glare` (the default 1.0 = `00 00 80 3F`). The first preset of the combo (the
/// light tent) is index 0.
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
        "00 00 00 80 3F 00 00 00 3F 00 00 80 3E 02 00 00 80 41 00 00 80 3F"
    );
    // A dialled-in 0.5 (`00 00 00 3F`) replaces only the appended field.
    assert_eq!(
        hex(&postcard::to_allocvec(&spec.with_surface_glare(0.5)).expect("encodes")),
        "00 00 00 80 3F 00 00 00 3F 00 00 80 3E 02 00 00 80 41 00 00 00 3F"
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

include!("tests_continued.rs");
