use super::*;

#[test]
fn axis_label_for_index_reads_the_profile_azimuth_table() {
    assert_eq!(axis_label_for_index(0), "0");
    assert_eq!(axis_label_for_index(1), "45");
    assert_eq!(axis_label_for_index(-1), "0");
    assert_eq!(axis_label_for_index(999), "0");
}

#[test]
fn the_compute_pill_maps_to_the_engines_and_is_local_without_a_remote() {
    assert_eq!(compute_target_of(0, true), ComputeTarget::LocalOnly);
    assert_eq!(compute_target_of(1, true), ComputeTarget::RemoteOnly);
    assert_eq!(compute_target_of(2, true), ComputeTarget::Both);
    // An out-of-range index falls back to today's behaviour.
    assert_eq!(compute_target_of(7, true), ComputeTarget::Both);
    // No remote configured: the pill is hidden, so even a stale "Remote only" renders locally.
    for index in 0..3 {
        assert_eq!(compute_target_of(index, false), ComputeTarget::LocalOnly);
    }
}

#[test]
fn colorspace_label_covers_every_variant() {
    assert_eq!(colorspace_label(ColorSpace::Srgb), "sRGB");
    assert_eq!(colorspace_label(ColorSpace::DisplayP3), "Display P3");
    assert_eq!(colorspace_label(ColorSpace::Rec2020), "Rec.2020");
    assert_eq!(colorspace_label(ColorSpace::AcesCg), "ACEScg");
}

/// `build_video_template_context` -- the function `live_template_context` feeds
/// the preview from -- must carry the real design and material all the way
/// through the rendered folder name, for a design/material pair that is
/// nothing like the hard-coded sample, so the "Resolves to" preview never
/// falls back to the fixed placeholder while a real design is open.
#[test]
fn preview_reflects_the_real_open_design_and_material_not_the_hard_coded_sample() {
    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let settings = VideoTemplateSettings {
        axis_index: 0,
        fps: 30,
        width: 1920,
        height: 1080,
        spp: 64,
        bounces: 12,
        color_space: ColorSpace::Srgb,
        step_deg: 1.0,
        start_deg: -90.0,
        end_deg: 90.0,
        total_frames: 181,
    };
    let (ctx, extras) = build_video_template_context(
        "40.014 Eleets",
        "Sample Designer",
        "Round",
        "1.71",
        "Spinel",
        &scene,
        settings,
    );
    assert_eq!(ctx.design, "40.014 Eleets");
    assert_eq!(ctx.material, "Spinel");

    let name = template::resolve_folder_name(template::DEFAULT_VIDEO_TEMPLATE, &ctx, &extras);
    assert!(
        name.contains("40.014 Eleets"),
        "expected the real design name in {name:?}"
    );
    assert!(
        name.contains("Spinel"),
        "expected the real material in {name:?}"
    );
    assert!(
        !name.contains("Sample Design"),
        "must not fall back to the hard-coded sample in {name:?}"
    );
}

/// A CPU end-to-end smoke test covering the whole pipeline this module wires up
/// (frame pose list, per-frame render, overlay, PNG write) at 128x128 with a
/// handful of frames. Drives `run::render_all_frames`'s pieces directly (not
/// through the Slint callback plumbing, which needs a live `MainWindow`) so it
/// can run in a headless `cargo test` process.
#[test]
fn end_to_end_cpu_render_at_128x128_with_a_handful_of_frames_and_overlay() {
    use crate::bridge::render_thread::RenderContext;
    use std::sync::{Mutex, atomic::AtomicBool};

    let scene =
        SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves");
    let start_deg = -2.0;
    let end_deg = 2.0;
    let step_deg = 2.0; // 3 frames: -2, 0, 2
    let total_frames = params::frame_count(start_deg, end_deg, step_deg);
    assert_eq!(total_frames, 3);

    let dir = std::env::temp_dir().join(format!("tilt_video_e2e_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let curves = metrics::MetricCurves {
        brilliance: vec![50.0; 181],
        windowing: vec![5.0; 181],
        extinction: vec![5.0; 181],
    };
    let selection = metrics::MetricSelection {
        brilliance: true,
        angle: true,
        ..Default::default()
    };
    let cancel = AtomicBool::new(false);
    let config = render::VideoComputeConfig {
        remote: crate::bridge::export_thread::RemoteSelection::local_only(),
        local_compute: crate::settings::LocalComputeTarget::Cpu,
    };
    let gpu = indicatrix::renderer::gpu_backend::GpuBackend::disabled();
    let mut carry = crate::bridge::export_thread::AccumulationCarry::default();

    for index in 0..total_frames {
        let tilt_deg = params::frame_angle_deg(start_deg, end_deg, step_deg, index, total_frames);
        let (cam_yaw, cam_pitch) =
            crate::gui::tilt::tilt_hover_preview::camera_pose_for_axis_tilt(0, tilt_deg);
        let outcome = render::render_frame_rgba(
            &scene,
            128,
            128,
            1,
            cam_yaw,
            cam_pitch,
            ColorSpace::Srgb,
            &config,
            &gpu,
            &mut carry,
            &cancel,
            |_| {},
        );
        let render::FrameOutcome::Rendered(mut rgba) = outcome else {
            panic!("expected a rendered frame");
        };
        assert_eq!(rgba.len(), 128 * 128 * 4);
        let readings = metrics::readings_for_frame(selection, &curves, tilt_deg);
        overlay::draw_overlay(&mut rgba, 128, 128, &readings);

        let path = dir.join(params::frame_file_name(index + 1, total_frames));
        let image = image::RgbaImage::from_raw(128, 128, rgba).expect("buffer matches dimensions");
        image.save(&path).expect("frame must save");
        assert!(path.exists());
        let saved = image::open(&path).expect("saved frame must be readable");
        assert_eq!((saved.width(), saved.height()), (128, 128));
    }
    assert!(!cancel.load(std::sync::atomic::Ordering::Relaxed));

    let _ = std::fs::remove_dir_all(&dir);
}
