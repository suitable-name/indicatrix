use super::{
    test_support::{curves, hdr_scene, still_job, video_job},
    *,
};

fn message(job: &RenderJobFile) -> String {
    job.validate().unwrap_err().to_string()
}

fn video_mut(job: &mut RenderJobFile) -> &mut TiltVideoJob {
    match &mut job.kind {
        JobKind::TiltVideo(video) => video,
        JobKind::Still(_) => unreachable!("the test job is a video"),
    }
}

#[test]
fn valid_jobs_pass() {
    assert_eq!(still_job().validate(), Ok(()));
    assert_eq!(video_job().validate(), Ok(()));
}

#[test]
fn refuses_a_wrong_format() {
    let mut job = still_job();
    job.format = 2;
    assert!(message(&job).contains("format 2"));
}

#[test]
fn refuses_a_bad_token() {
    for token in [
        "",
        "abc",
        "9F2C4A7D1E0B48C3A65D2F71C8E4B903",
        "9f2c4a7d1e0b48c3a65d2f71c8e4b90g",
    ] {
        let mut job = still_job();
        job.token = token.to_string();
        assert_eq!(
            message(&job),
            "The job token must be 32 lowercase hexadecimal characters."
        );
    }
}

#[test]
fn refuses_an_empty_output() {
    let mut job = still_job();
    job.output = "  ".to_string();
    assert_eq!(message(&job), "The job has no output location.");
}

#[test]
fn refuses_a_bad_size() {
    for (width, height) in [(15, 48), (64, 15), (8193, 48), (64, 8193)] {
        let mut job = still_job();
        job.scene.width = width;
        job.scene.height = height;
        assert_eq!(
            message(&job),
            "The picture size must be between 16 and 8192 pixels."
        );
    }
    let mut job = still_job();
    job.scene.width = 16;
    job.scene.height = 8192;
    assert_eq!(job.validate(), Ok(()));
}

#[test]
fn refuses_bad_samples() {
    for spp in [0, MAX_SPP + 1] {
        let mut job = still_job();
        job.kind = JobKind::Still(StillJob {
            samples_per_pixel: spp,
            color_space: JobColorSpace::Srgb,
            preset_label: String::new(),
        });
        assert_eq!(
            message(&job),
            "The samples per pixel must be between 1 and 32768."
        );
        let mut video = video_job();
        video_mut(&mut video).samples_per_pixel = spp;
        assert!(message(&video).contains("samples per pixel"));
    }
}

#[test]
fn refuses_bad_bounces() {
    for bounces in [0, MAX_BOUNCES + 1] {
        let mut job = still_job();
        job.scene.max_bounces = bounces;
        assert_eq!(message(&job), "The bounce limit must be between 1 and 128.");
    }
}

#[test]
fn refuses_bad_video_numbers() {
    let mut job = video_job();
    video_mut(&mut job).fps = 0;
    assert!(message(&job).contains("frame rate"));
    video_mut(&mut job).fps = MAX_FPS + 1;
    assert!(message(&job).contains("frame rate"));

    let mut job = video_job();
    video_mut(&mut job).total_frames = 0;
    assert!(message(&job).contains("between 1 and 18001 frames"));
    video_mut(&mut job).total_frames = MAX_FRAMES + 1;
    assert!(message(&job).contains("between 1 and 18001 frames"));

    for step in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut job = video_job();
        video_mut(&mut job).step_deg = step;
        assert_eq!(message(&job), "The tilt step must be a number above zero.");
    }
    let mut job = video_job();
    video_mut(&mut job).end_deg = f64::NAN;
    assert!(message(&job).contains("start and end angles"));
}

#[test]
fn refuses_short_curves_and_missing_curves() {
    let mut job = video_job();
    let mut short = curves();
    short.windowing.truncate(CURVE_POINTS - 1);
    video_mut(&mut job).curves = Some(short);
    assert!(message(&job).contains("at least 181 values"));

    let mut job = video_job();
    video_mut(&mut job).curves = None;
    assert!(message(&job).contains("needs the metric curves"));

    // No overlay and no curves is fine.
    video_mut(&mut job).overlay = OverlaySelection::default();
    assert_eq!(job.validate(), Ok(()));
}

#[test]
fn refuses_hdr_mismatches() {
    let mut job = video_job();
    job.hdr = None;
    assert!(message(&job).contains("does not name the map file"));

    let mut job = still_job();
    job.hdr = video_job().hdr;
    assert!(message(&job).contains("not lit by an HDR map"));

    let mut job = video_job();
    if let Some(hdr) = job.hdr.as_mut() {
        hdr.sha256_hex = "00".repeat(32);
    }
    assert!(message(&job).contains("does not match"));

    let (scene, source) = hdr_scene();
    let mut job = still_job();
    job.scene = scene;
    job.hdr = Some(source);
    assert_eq!(job.validate(), Ok(()));
}

#[test]
fn kind_words_and_frame_counts() {
    assert_eq!(still_job().kind_word(), "still");
    assert_eq!(still_job().frames_total(), 1);
    assert_eq!(video_job().kind_word(), "tilt_video");
    assert_eq!(video_job().frames_total(), 181);
}

#[test]
fn resolves_relative_paths_against_the_base() {
    let job = video_job();
    let base = Path::new("base");
    assert_eq!(job.output_path(base), base.join("frames"));
    assert_eq!(
        job.hdr_path(base),
        Some(base.join("assets").join("studio.hdr"))
    );
    assert_eq!(still_job().hdr_path(base), None);
    let absolute = std::env::temp_dir().join("x.png");
    let text = absolute.to_string_lossy().into_owned();
    assert_eq!(job.resolve(&text, base), absolute);
}

#[test]
fn cli_words_round_trip() {
    for choice in [
        ComputeChoice::Local,
        ComputeChoice::Remote,
        ComputeChoice::Both,
    ] {
        assert_eq!(
            ComputeChoice::from_cli_word(choice.cli_word()),
            Some(choice)
        );
    }
    for choice in [TransferChoice::FullData, TransferChoice::FinalPicture] {
        assert_eq!(
            TransferChoice::from_cli_word(choice.cli_word()),
            Some(choice)
        );
    }
    for engines in [LocalEngines::Cpu, LocalEngines::CpuGpu, LocalEngines::Gpu] {
        assert_eq!(
            LocalEngines::from_cli_word(engines.cli_word()),
            Some(engines)
        );
    }
    assert_eq!(
        LocalEngines::from_cli_word("CPUGPU"),
        Some(LocalEngines::CpuGpu)
    );
    assert_eq!(
        LocalEngines::from_cli_word("cpu-gpu"),
        Some(LocalEngines::CpuGpu)
    );
    assert_eq!(LocalEngines::from_cli_word("tpu"), None);
    assert_eq!(ComputeChoice::from_cli_word("all"), None);
    assert_eq!(TransferChoice::from_cli_word("raw"), None);
    assert_eq!(LocalEngines::default(), LocalEngines::CpuGpu);
}

#[test]
fn labels_and_overlay_count() {
    assert_eq!(JobColorSpace::Srgb.label(), "sRGB");
    assert_eq!(JobColorSpace::DisplayP3.label(), "Display P3");
    assert_eq!(JobColorSpace::Rec2020.label(), "Rec.2020");
    assert_eq!(OverlaySelection::default().count(), 0);
    let all = OverlaySelection {
        brilliance: true,
        windowing: true,
        extinction: true,
        tilt_brilliance: true,
        angle: true,
    };
    assert_eq!(all.count(), 5);
}

#[test]
fn tokens_have_the_right_shape_and_differ() {
    let first = new_job_token();
    let second = new_job_token();
    assert!(is_token(&first), "{first}");
    assert!(is_token(&second), "{second}");
    assert_ne!(first, second);
}

#[test]
fn diagnostic_options_roundtrip_and_old_jobs_default_to_clean_output() {
    let mut job = super::test_support::still_job();
    let clean = serde_json::to_value(&job).unwrap();
    assert!(clean.get("optical_diagnostics").is_none());
    let old: RenderJobFile = serde_json::from_value(clean).unwrap();
    assert!(old.optical_diagnostics.is_none());
    job.optical_diagnostics = Some(OpticalDiagnosticOptions {
        model_version: 1,
        layers: [true, false, true, true],
        opacity: 0.45,
        map_mode: false,
        high_contrast: true,
        scintillation: false,
    });
    let copy: RenderJobFile = serde_json::from_value(serde_json::to_value(&job).unwrap()).unwrap();
    assert_eq!(copy.optical_diagnostics, job.optical_diagnostics);
    job.optical_diagnostics.as_mut().unwrap().model_version = 2;
    assert!(job.validate().is_err());
}

#[test]
fn scintillation_defaults_off_is_written_only_when_on_and_round_trips() {
    let mut job = super::test_support::still_job();
    job.optical_diagnostics = Some(OpticalDiagnosticOptions {
        model_version: SUPPORTED_OPTICAL_MODEL_VERSION,
        layers: [true, false, false, false],
        opacity: 0.45,
        map_mode: false,
        high_contrast: false,
        scintillation: false,
    });
    // Off: no key at all, so a job written now is byte for byte a job written before.
    let value = serde_json::to_value(&job).unwrap();
    let options = value.get("optical_diagnostics").expect("diagnostics");
    assert!(options.get("scintillation").is_none(), "{options}");
    // A file that predates the key reads as off.
    let old: RenderJobFile = serde_json::from_value(value).unwrap();
    assert_eq!(old.optical_diagnostics, job.optical_diagnostics);
    assert!(!old.optical_diagnostics.unwrap().scintillation);
    // On: written, read back, still valid at the same model version.
    job.optical_diagnostics.as_mut().unwrap().scintillation = true;
    assert!(job.validate().is_ok());
    let value = serde_json::to_value(&job).unwrap();
    assert_eq!(
        value["optical_diagnostics"]["scintillation"],
        serde_json::Value::Bool(true)
    );
    let copy: RenderJobFile = serde_json::from_value(value).unwrap();
    assert_eq!(copy.optical_diagnostics, job.optical_diagnostics);
}

#[test]
fn diagnostics_serialize_after_the_format_header_and_use_the_supported_model_version() {
    let mut job = super::test_support::still_job();
    job.optical_diagnostics = Some(OpticalDiagnosticOptions {
        model_version: SUPPORTED_OPTICAL_MODEL_VERSION,
        layers: [true, false, false, false],
        opacity: 0.45,
        map_mode: false,
        high_contrast: false,
        scintillation: false,
    });
    assert!(job.validate().is_ok());
    let text = serde_json::to_string(&job).unwrap();
    let format_at = text.find("\"format\"").expect("format key");
    let diagnostics_at = text
        .find("\"optical_diagnostics\"")
        .expect("diagnostics key");
    assert!(
        format_at < diagnostics_at,
        "the format header must come before the optional diagnostics: {text}"
    );
}

/// A job file saved before frosted planes existed has no `frosted_planes` key in its scene: it
/// reads as unfrosted (and keeps its format version), and a job that frosts planes writes and
/// reads them back, through the job file codec.
#[test]
fn old_jobs_read_unfrosted_and_frosted_planes_round_trip() {
    let job = super::test_support::still_job();
    assert_eq!(job.scene.frosted_planes, Vec::<u32>::new());
    let mut value = serde_json::to_value(&job).unwrap();
    let scene = value
        .get_mut("scene")
        .and_then(serde_json::Value::as_object_mut)
        .expect("the job has a scene object");
    assert_eq!(
        scene.remove("frosted_planes"),
        Some(serde_json::json!([])),
        "the scene writes the (empty) list"
    );
    let old: RenderJobFile = serde_json::from_value(value).unwrap();
    assert!(
        old.scene.frosted_planes.is_empty(),
        "an old job is unfrosted"
    );
    assert_eq!(old, job, "and is otherwise the same job");
    assert_eq!(
        old.format, 1,
        "legacy Path-Traced jobs keep their original format"
    );

    let mut frosted = super::test_support::still_job();
    frosted.scene.frosted_planes = vec![3, 4, 30];
    let text = crate::codec::to_text(&frosted).unwrap();
    let back = crate::codec::from_text(&text).unwrap();
    assert_eq!(back.scene.frosted_planes, vec![3, 4, 30]);
    assert_eq!(back, frosted);
}

/// The text of a job as an older version wrote it: no `scene_backdrop_model` key and the
/// given card level in the scene.
fn legacy_text(card: f32) -> String {
    let mut value = serde_json::to_value(still_job()).unwrap();
    let object = value.as_object_mut().unwrap();
    assert_eq!(
        object.remove("scene_backdrop_model"),
        Some(serde_json::json!(BACKDROP_MODEL_TRAY)),
        "writers write the tray marker"
    );
    value["scene"]["backdrop"] = serde_json::json!(card);
    value.to_string()
}

#[test]
fn a_legacy_card_level_converts_to_the_tray_of_the_files_own_preset_and_pose() {
    use indicatrix::render_setup::Backdrop;
    let reference = still_job().scene;
    let tray = |choice: Backdrop| {
        choice.resolve(
            reference.lighting_preset,
            reference.light_yaw,
            reference.light_pitch,
        )
    };
    let grey = crate::codec::from_text(&legacy_text(0.23)).unwrap();
    assert_eq!(grey.scene_backdrop_model, BACKDROP_MODEL_TRAY);
    assert_eq!(
        grey.scene.backdrop.to_bits(),
        tray(Backdrop::Grey).to_bits()
    );
    assert!(grey.scene.backdrop > 0.0);
    let white = crate::codec::from_text(&legacy_text(8.0)).unwrap();
    assert_eq!(
        white.scene.backdrop.to_bits(),
        tray(Backdrop::White).to_bits()
    );
    assert!(white.scene.backdrop < 8.0, "no longer an 8.0 emitter");
    let as_lit = crate::codec::from_text(&legacy_text(0.0)).unwrap();
    assert_eq!(as_lit.scene.backdrop.to_bits(), 0.0_f32.to_bits());
    assert_eq!(as_lit.scene_backdrop_model, BACKDROP_MODEL_TRAY);
    let other = crate::codec::from_text(&legacy_text(0.5)).unwrap();
    assert_eq!(other.scene.backdrop.to_bits(), 0.5_f32.to_bits());
}

#[test]
fn the_conversion_follows_the_files_own_lighting() {
    use indicatrix::optics::raytracer::LightingPreset;
    let mut value: serde_json::Value = serde_json::from_str(&legacy_text(8.0)).unwrap();
    value["scene"]["lighting_preset"] = serde_json::to_value(LightingPreset::WhiteTray).unwrap();
    let job = crate::codec::from_text(&value.to_string()).unwrap();
    let expected = indicatrix::render_setup::Backdrop::White.resolve(
        LightingPreset::WhiteTray,
        job.scene.light_yaw,
        job.scene.light_pitch,
    );
    assert_eq!(job.scene.backdrop.to_bits(), expected.to_bits());
    let daylight = crate::codec::from_text(&legacy_text(8.0)).unwrap();
    assert_ne!(
        daylight.scene.backdrop.to_bits(),
        job.scene.backdrop.to_bits(),
        "a different preset resolves a different tray"
    );
}

#[test]
fn a_job_with_the_tray_marker_is_not_converted_again() {
    let mut job = still_job();
    job.scene.backdrop = 0.23;
    let back = crate::codec::from_text(&crate::codec::to_text(&job).unwrap()).unwrap();
    assert_eq!(back.scene.backdrop.to_bits(), 0.23_f32.to_bits());
    assert_eq!(back, job);
    let mut again = back.clone();
    again.upgrade_backdrop_model();
    assert_eq!(again, back, "the upgrade is idempotent");
}

#[test]
fn an_unknown_backdrop_model_is_refused() {
    let mut job = still_job();
    job.scene_backdrop_model = BACKDROP_MODEL_TRAY + 1;
    assert!(message(&job).contains("backdrop model 2"));
    job.scene_backdrop_model = BACKDROP_MODEL_LEGACY_CARD;
    assert_eq!(job.validate(), Ok(()), "the legacy marker is accepted");
}
