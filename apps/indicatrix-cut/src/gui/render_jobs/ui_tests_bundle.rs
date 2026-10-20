use super::*;

/// The job file `relative` of the bundle, read back and decoded.
fn job_of(
    bundle: &crate::gui::render_jobs::script_export::Bundle,
    relative: &str,
) -> RenderJobFile {
    codec::from_text(std::str::from_utf8(text_of(bundle, relative)).unwrap()).unwrap()
}

#[test]
fn the_bundle_rewrites_paths_and_copies_each_hdr_map_once() {
    let hash = [7_u8; 32];
    let sha = hash_hex(&hash);
    let both = vec![ScriptFlavor::PowerShell, ScriptFlavor::Sh];
    let bundle = build_bundle(two_jobs(hash), &options(true, both)).unwrap();

    let names: Vec<PathBuf> = bundle.files.iter().map(|(path, _)| path.clone()).collect();
    assert_eq!(
        names,
        [
            Path::new("jobs").join("01-round-still.job.json"),
            Path::new("jobs").join("02-round-video.job.json"),
            PathBuf::from("run-render-jobs.ps1"),
            PathBuf::from("run-render-jobs.sh"),
        ]
    );

    // One HDR copy for the two jobs, into jobs/assets.
    assert_eq!(
        bundle.hdr_copies,
        [(
            PathBuf::from("/hdr/studio.hdr"),
            Path::new("jobs").join("assets").join(format!("{sha}.hdr")),
            sha.clone()
        )]
    );

    // Each job file reads back, with its paths relative to the jobs folder.
    let first = job_of(&bundle, "jobs/01-round-still.job.json");
    assert_eq!(first.output, "../renders/gem.png");
    assert_eq!(
        first.hdr.as_ref().unwrap().path,
        format!("assets/{sha}.hdr")
    );
    assert_eq!(first.token, TOKEN_A);
    let second = job_of(&bundle, "jobs/02-round-video.job.json");
    assert_eq!(second.output, "../renders/frames-45");
    assert_eq!(
        second.hdr.as_ref().unwrap().path,
        format!("assets/{sha}.hdr")
    );

    // Both scripts list both jobs, in order, with the right command.
    let ps1 = String::from_utf8_lossy(text_of(&bundle, "run-render-jobs.ps1")).into_owned();
    let sh = String::from_utf8_lossy(text_of(&bundle, "run-render-jobs.sh")).into_owned();
    for script in [&ps1, &sh] {
        let render = script
            .find("'render', 'jobs/01-round-still.job.json'")
            .or_else(|| script.find("run_job render 'jobs/01-round-still.job.json'"));
        let tilt = script
            .find("'tilt-video', 'jobs/02-round-video.job.json'")
            .or_else(|| script.find("run_job tilt-video 'jobs/02-round-video.job.json'"));
        assert!(render.is_some(), "{script}");
        assert!(tilt.is_some(), "{script}");
        assert!(render < tilt, "{script}");
    }
    assert!(text_of(&bundle, "run-render-jobs.ps1").starts_with(&[0xEF, 0xBB, 0xBF]));
    assert!(!text_of(&bundle, "run-render-jobs.sh").starts_with(&[0xEF, 0xBB, 0xBF]));
}

#[test]
fn one_flavor_writes_one_script_and_outputs_stay_when_not_beside() {
    let bundle = build_bundle(
        two_jobs([7_u8; 32]),
        &options(false, vec![ScriptFlavor::Sh]),
    )
    .unwrap();
    let scripts: Vec<PathBuf> = bundle
        .files
        .iter()
        .map(|(path, _)| path.clone())
        .filter(|path| path.extension().is_some_and(|e| e == "sh" || e == "ps1"))
        .collect();
    assert_eq!(scripts, [PathBuf::from("run-render-jobs.sh")]);

    let first = job_of(&bundle, "jobs/01-round-still.job.json");
    assert_eq!(first.output, "/renders/gem.png");
}

#[test]
fn two_outputs_with_one_name_stay_apart_beside_the_script() {
    let jobs = vec![
        file(TOKEN_A, "One", "/a/gem.png", None, still_kind()),
        file(TOKEN_B, "Two", "/b/GEM.png", None, still_kind()),
    ];
    let bundle = build_bundle(jobs, &options(true, vec![ScriptFlavor::Sh])).unwrap();
    let outputs: Vec<String> = ["01-one", "02-two"]
        .iter()
        .map(|slug| job_of(&bundle, &format!("jobs/{slug}.job.json")).output)
        .collect();
    assert_eq!(outputs, ["../renders/gem.png", "../renders/GEM (2).png"]);
}

#[test]
fn a_bundle_of_no_jobs_is_refused() {
    let error = build_bundle(Vec::new(), &options(false, vec![ScriptFlavor::Sh])).unwrap_err();
    assert_eq!(error, "There are no unfinished jobs to export.");
}

#[test]
fn the_bundle_folder_is_named_by_date_and_time() {
    assert_eq!(
        bundle_folder_name((2026, 10, 6, 14, 3)),
        "render-jobs-2026-10-06-1403"
    );
    assert_eq!(
        bundle_folder_name((2027, 1, 12, 0, 59)),
        "render-jobs-2027-01-12-0059"
    );
}

#[test]
fn writing_a_bundle_copies_the_map_checks_its_hash_and_never_reuses_a_folder() {
    let base = std::env::temp_dir().join(format!(
        "indicatrix-cut-script-export-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).unwrap();
    let map = base.join("studio.hdr");
    let bytes = b"not really an hdr map".to_vec();
    std::fs::write(&map, &bytes).unwrap();
    let hash = content_hash(&bytes);
    let map_text = map.to_string_lossy().into_owned();
    let make_jobs = || {
        vec![file(
            TOKEN_A,
            "Round still",
            "/renders/gem.png",
            Some((&map_text, hash)),
            still_kind(),
        )]
    };
    let now = 1_791_295_380; // 2026-10-06 14:03:00 UTC (day 20732, 14 h 3 min)
    let options = options(false, vec![ScriptFlavor::Sh]);

    let (folder, count) = write_bundle(&base, make_jobs(), &options, now).unwrap();
    assert_eq!(count, 1);
    assert_eq!(
        folder.file_name().unwrap().to_string_lossy(),
        "render-jobs-2026-10-06-1403"
    );
    assert!(folder.join("run-render-jobs.sh").is_file());
    assert!(
        folder
            .join("jobs")
            .join("01-round-still.job.json")
            .is_file()
    );
    let copied = folder
        .join("jobs")
        .join("assets")
        .join(format!("{}.hdr", hash_hex(&hash)));
    assert_eq!(std::fs::read(&copied).unwrap(), bytes);

    // The same minute again makes a second folder instead of writing into the first.
    let (second, _) = write_bundle(&base, make_jobs(), &options, now).unwrap();
    assert_eq!(
        second.file_name().unwrap().to_string_lossy(),
        "render-jobs-2026-10-06-1403 (2)"
    );

    // A map that changed since the job was added is refused, and nothing is left behind.
    std::fs::write(&map, b"changed").unwrap();
    let error = write_bundle(&base, make_jobs(), &options, now).unwrap_err();
    assert!(error.contains("changed since the job was added"), "{error}");
    assert!(!base.join("render-jobs-2026-10-06-1403 (3)").exists());

    let _ = std::fs::remove_dir_all(&base);
}

// ---- J4-T4: "Add to Queue" helpers ----------------------------------------------------

#[test]
fn still_outputs_are_reserved_against_the_disk_the_queue_and_each_other() {
    let dir = Path::new("/out");
    let names = [
        "gem.png".to_string(),
        "gem.png".to_string(),
        "other.png".to_string(),
        "gem.png".to_string(),
    ];
    // `other.png` is already the output of a queued job; `gem.png` exists on disk.
    let taken = [dir.join("other.png")];
    let on_disk = dir.join("gem.png");
    let planned = plan_still_outputs(dir, &names, &taken, &|path| path == on_disk);
    assert_eq!(
        planned,
        [
            dir.join("gem (2).png"),
            dir.join("gem (3).png"),
            dir.join("other (2).png"),
            dir.join("gem (4).png"),
        ]
    );

    // With nothing in the way, every name is kept.
    let free = plan_still_outputs(
        dir,
        &["a.png".to_string(), "b.png".to_string()],
        &[],
        &|_| false,
    );
    assert_eq!(free, [dir.join("a.png"), dir.join("b.png")]);
}

#[test]
fn still_labels_and_summaries_use_the_plain_wording() {
    assert_eq!(
        still_label("Barion Oval", "", 1920, 1080),
        "Barion Oval \u{b7} Current View \u{b7} 1920\u{d7}1080"
    );
    assert_eq!(
        still_label("Barion Oval", "Studio", 3840, 2160),
        "Barion Oval \u{b7} Studio \u{b7} 3840\u{d7}2160"
    );
    let params = ExportParams {
        engine: indicatrix::renderer::engine::EngineSpec::default(),
        width: 1920,
        height: 1080,
        samples_per_pixel: 1024,
        max_bounces: 12,
    };
    assert_eq!(
        still_summary(&params, "sRGB", "Daylight"),
        "1920 \u{d7} 1080 \u{b7} 1024 samples \u{b7} 12 bounces \u{b7} sRGB \u{b7} Daylight"
    );
}

#[test]
fn video_labels_and_summaries_use_the_plain_wording() {
    assert_eq!(
        video_label("Barion Oval", 45.0),
        "Barion Oval \u{b7} tilt video \u{b7} axis 45\u{b0}"
    );
    let facts = VideoFacts {
        start_deg: -90.0,
        end_deg: 90.0,
        step_deg: 1.0,
        frames: 181,
        fps: 30,
        width: 1280,
        height: 720,
        samples: 256,
    };
    assert_eq!(
        video_summary(&facts),
        "-90\u{b0} to 90\u{b0} in 1\u{b0} steps \u{b7} 181 frames \u{b7} 30 fps \u{b7} 1280 \u{d7} 720 \u{b7} 256 samples"
    );
    let fine = VideoFacts {
        step_deg: 0.5,
        start_deg: 0.0,
        end_deg: 45.5,
        frames: 92,
        ..facts
    };
    assert!(video_summary(&fine).starts_with("0\u{b0} to 45.5\u{b0} in 0.5\u{b0} steps"));
}
