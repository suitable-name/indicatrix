use super::{
    Command, ExportFormat, JobCommandKind, MaterialArg, Mode, RenderJobArgs, ReportFormat, Topic,
    parse,
    types::{LIGHTING_NAMES, PRESET_NAMES},
};
use indicatrix::optics::LightingPreset;
use indicatrix_cut_core::{CANONICAL_LIGHTING_PRESET, ObjectivePreset};
use indicatrix_editor::{
    optimize_view::{DEFAULT_BUDGET, DEFAULT_CANDIDATES, DEFAULT_STARTS},
    retarget::CrownShift,
};
use indicatrix_render_jobs::{ComputeChoice, LocalEngines, TransferChoice};
use std::path::PathBuf;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn parsed(line: &str) -> Command {
    parse(&argv(line)).unwrap_or_else(|e| panic!("{line:?} should parse: {e}"))
}

fn error(line: &str) -> String {
    parse(&argv(line)).expect_err("this line should not parse")
}

#[test]
fn no_words_and_help_words_give_help() {
    assert_eq!(parse(&[]), Ok(Command::Help(Topic::Root)));
    assert_eq!(parsed("--help"), Command::Help(Topic::Root));
    assert_eq!(parsed("help sweep"), Command::Help(Topic::Sweep));
    assert_eq!(parsed("retarget --help"), Command::Help(Topic::Retarget));
    // Help wins over a missing design.
    assert_eq!(parsed("metrics --tilt -h"), Command::Help(Topic::Metrics));
    assert_eq!(parsed("--version"), Command::Version);
}

#[test]
fn an_unknown_command_names_the_choices() {
    let message = error("frobnicate x.asc");
    assert!(
        message.contains("unknown command \"frobnicate\""),
        "{message}"
    );
    assert!(message.contains("retarget"), "{message}");
}

#[test]
fn info_takes_a_design_and_its_flags() {
    let Command::Info(args) = parsed("info stone.asc --json --db lib.sqlite --out r.json") else {
        panic!("expected info");
    };
    assert_eq!(args.design, PathBuf::from("stone.asc"));
    assert!(args.json);
    assert_eq!(args.db, Some(PathBuf::from("lib.sqlite")));
    assert_eq!(args.out, Some(PathBuf::from("r.json")));
}

#[test]
fn a_design_is_required_and_only_one_is_taken() {
    assert!(error("info").contains("needs a design file"));
    assert!(error("info a.asc b.asc").contains("unexpected extra argument \"b.asc\""));
    assert!(error("solve --json").contains("needs a design file"));
}

#[test]
fn an_unknown_flag_names_the_command() {
    assert_eq!(
        error("solve a.asc --frob"),
        "unknown flag \"--frob\" for \"solve\" (see --help)"
    );
    assert_eq!(
        error("export a.asc --tilt"),
        "unknown flag \"--tilt\" for \"export\" (see --help)"
    );
}

#[test]
fn a_flag_without_its_value_is_named() {
    assert_eq!(
        error("solve a.asc --out"),
        "--out needs a value (see --help)"
    );
    assert_eq!(
        error("metrics a.asc --ri"),
        "--ri needs a value (see --help)"
    );
}

#[test]
fn solve_takes_an_output() {
    let Command::Solve(args) = parsed("solve a.asc --out a.indicatrix") else {
        panic!("expected solve");
    };
    assert_eq!(args.out, Some(PathBuf::from("a.indicatrix")));
    assert!(!args.json);
}

#[test]
fn metrics_defaults_and_flags() {
    let Command::Metrics(plain) = parsed("metrics a.indicatrix") else {
        panic!("expected metrics");
    };
    assert_eq!(plain.format, ReportFormat::Text);
    assert!(!plain.tilt);
    assert_eq!(plain.material, None);
    assert_eq!(plain.lighting, CANONICAL_LIGHTING_PRESET);

    let Command::Metrics(args) =
        parsed("metrics a.indicatrix --material Sapphire --tilt --csv --lighting tent")
    else {
        panic!("expected metrics");
    };
    assert_eq!(
        args.material,
        Some(MaterialArg::Named("Sapphire".to_string()))
    );
    assert!(args.tilt);
    assert_eq!(args.format, ReportFormat::Csv);
    assert_eq!(args.lighting, LightingPreset::LightTent);
}

#[test]
fn metrics_refuses_two_formats_and_two_materials() {
    assert!(error("metrics a.asc --json --csv").contains("not both"));
    assert!(error("metrics a.asc --material Ruby --ri 1.77").contains("give one of"));
    assert!(error("metrics a.asc --ri 1.7 --ri 1.8").contains("give it once"));
}

#[test]
fn a_refractive_index_must_be_above_one() {
    assert!(error("metrics a.asc --ri 1.0").contains("above 1"));
    assert!(error("metrics a.asc --ri 0.5").contains("above 1"));
    assert!(error("metrics a.asc --ri 9").contains("at most"));
    assert!(error("metrics a.asc --ri abc").contains("needs a number"));
    let Command::Metrics(args) = parsed("metrics a.asc --ri 1.76") else {
        panic!("expected metrics");
    };
    assert_eq!(args.material, Some(MaterialArg::Ri(1.76)));
}

#[test]
fn a_lighting_name_is_checked() {
    let message = error("metrics a.asc --lighting sunset");
    assert!(message.contains("unknown lighting \"sunset\""), "{message}");
    assert!(message.contains("dome"), "{message}");
    assert_eq!(
        message.contains("uv365"),
        cfg!(feature = "physical-color"),
        "{message}"
    );
    for (name, preset) in LIGHTING_NAMES {
        let Command::Metrics(args) = parsed(&format!("metrics a.asc --lighting {name}")) else {
            panic!("expected metrics");
        };
        assert_eq!(args.lighting, preset);
    }
}

#[test]
fn validate_flags() {
    let Command::Validate(args) = parsed("validate a.asc --json --ri 1.54") else {
        panic!("expected validate");
    };
    assert!(args.json);
    assert_eq!(args.material, Some(MaterialArg::Ri(1.54)));
}

#[test]
fn optimize_defaults_match_the_optimize_tab() {
    let Command::Optimize(args) = parsed("optimize a.indicatrix") else {
        panic!("expected optimize");
    };
    assert_eq!(args.preset, ObjectivePreset::Balanced);
    assert_eq!(args.budget, DEFAULT_BUDGET);
    assert_eq!(args.budget, 800);
    assert_eq!(args.starts, DEFAULT_STARTS);
    assert_eq!(args.starts, 8);
    assert_eq!(args.seed, 0);
    assert_eq!(args.candidates, DEFAULT_CANDIDATES);
    assert!(!args.vary_anchored);
    assert_eq!(args.lighting, CANONICAL_LIGHTING_PRESET);
}

#[test]
fn optimize_flags() {
    let Command::Optimize(args) = parsed(
        "optimize a.indicatrix --preset low-windowing --budget 50 --seed 7 \
         --vary-anchored --candidates 2 --out best.indicatrix",
    ) else {
        panic!("expected optimize");
    };
    assert_eq!(args.preset, ObjectivePreset::LowWindowing);
    assert_eq!(args.budget, 50);
    assert_eq!(args.seed, 7);
    assert!(args.vary_anchored);
    assert_eq!(args.candidates, 2);
    assert_eq!(args.out, Some(PathBuf::from("best.indicatrix")));
}

#[test]
fn optimize_starts_flag_is_parsed_and_checked() {
    let Command::Optimize(args) = parsed("optimize a.indicatrix --starts 3") else {
        panic!("expected optimize");
    };
    assert_eq!(args.starts, 3);
    let Command::Optimize(one) = parsed("optimize a.indicatrix --starts 1") else {
        panic!("expected optimize");
    };
    assert_eq!(one.starts, 1);
    let Command::Optimize(most) = parsed("optimize a.indicatrix --starts 32") else {
        panic!("expected optimize");
    };
    assert_eq!(most.starts, 32);
    assert!(error("optimize a.asc --starts 0").contains("whole number"));
    assert!(error("optimize a.asc --starts 33").contains("at most 32"));
    assert!(error("optimize a.asc --starts 2.5").contains("whole number"));
    assert!(error("optimize a.asc --starts").contains("--starts needs a value"));
}

#[test]
fn optimize_checks_its_numbers_and_names() {
    assert!(error("optimize a.asc --preset speedy").contains("unknown preset"));
    assert!(error("optimize a.asc --budget 0").contains("at least 1"));
    assert!(error("optimize a.asc --seed -3").contains("whole number"));
    assert!(error("optimize a.asc --candidates 0").contains("from 1 to"));
    assert!(error("optimize a.asc --candidates 9").contains("from 1 to"));
    for (name, preset) in PRESET_NAMES {
        let Command::Optimize(args) = parsed(&format!("optimize a.asc --preset {name}")) else {
            panic!("expected optimize");
        };
        assert_eq!(args.preset, preset);
    }
    for (name, preset) in [
        ("lighten-dark", ObjectivePreset::LightenDark),
        ("intensify-pale", ObjectivePreset::IntensifyPale),
    ] {
        let Command::Optimize(args) = parsed(&format!("optimize a.asc --preset {name}")) else {
            panic!("expected optimize");
        };
        assert_eq!(args.preset, preset);
    }
}

#[test]
fn retarget_needs_a_target() {
    assert!(error("retarget a.asc").contains("--material NAME or --ri N"));
    let Command::Retarget(args) = parsed("retarget a.asc --material Sapphire") else {
        panic!("expected retarget");
    };
    assert_eq!(args.target, MaterialArg::Named("Sapphire".to_string()));
    assert_eq!(args.mode, Mode::Shift);
    assert_eq!(args.crown, CrownShift::default());
    assert!(
        args.crown.follow_pavilion,
        "the crown follows the pavilion by default"
    );
    assert_eq!(args.range_deg, None);
    assert_eq!(args.budget, None);
    assert!(
        args.keep_look,
        "the search keeps the design's look by default"
    );
}

#[test]
fn retarget_no_keep_look_turns_the_shape_penalty_off() {
    let Command::Retarget(args) = parsed("retarget a.asc --ri 1.7 --mode optimize --no-keep-look")
    else {
        panic!("expected retarget");
    };
    assert!(!args.keep_look);
}

#[test]
fn retarget_crown_policy_and_mode() {
    let Command::Retarget(args) = parsed(
        "retarget a.asc --ri 1.7681 --crown-fraction 0.33 --mode optimize --range 10 \
         --budget 100 --preset brilliance --out out.indicatrix",
    ) else {
        panic!("expected retarget");
    };
    assert_eq!(args.target, MaterialArg::Ri(1.7681));
    assert_eq!(args.mode, Mode::Optimize);
    assert_eq!(args.crown.fraction, 0.33);
    assert!(!args.crown.scale_by_ratio);
    assert!(
        !args.crown.follow_pavilion,
        "an explicit fraction replaces the default rule"
    );
    assert_eq!(args.range_deg, Some(10.0));
    assert_eq!(args.budget, Some(100));
    assert_eq!(args.preset, ObjectivePreset::Brilliance);

    let Command::Retarget(ratio) = parsed("retarget a.asc --ri 1.7 --crown-ratio") else {
        panic!("expected retarget");
    };
    assert!(ratio.crown.scale_by_ratio);
    assert!(!ratio.crown.follow_pavilion);

    let Command::Retarget(follow) = parsed("retarget a.asc --ri 1.7 --crown-follow") else {
        panic!("expected retarget");
    };
    assert_eq!(follow.crown, CrownShift::default());
}

#[test]
fn retarget_checks_its_flags() {
    assert!(error("retarget a.asc --ri 1.7 --mode fast").contains("unknown mode"));
    assert!(error("retarget a.asc --ri 1.7 --crown-fraction 1.5").contains("0 to 1"));
    assert!(
        error("retarget a.asc --ri 1.7 --crown-fraction 0.5 --crown-ratio").contains("give one of")
    );
    assert!(error("retarget a.asc --ri 1.7 --crown-follow --crown-ratio").contains("give one of"));
    assert!(
        error("retarget a.asc --ri 1.7 --crown-fraction 0.5 --crown-follow")
            .contains("give one of")
    );
    assert!(error("retarget a.asc --ri 1.7 --range 0").contains("above 0"));
    assert!(error("retarget a.asc --ri 1.7 --range 90").contains("at most"));
}

#[test]
fn sweep_takes_negative_angles_as_values() {
    let Command::Sweep(args) = parsed(
        "sweep a.indicatrix --tier P1 --from -43 --to -39.5 --step 0.5 --tilt --csv rows.csv",
    ) else {
        panic!("expected sweep");
    };
    assert_eq!(args.tier, "P1");
    assert_eq!(args.from_deg, -43.0);
    assert_eq!(args.to_deg, -39.5);
    assert_eq!(args.step_deg, 0.5);
    assert!(args.tilt);
    assert_eq!(args.csv, Some(PathBuf::from("rows.csv")));
}

#[test]
fn sweep_names_what_is_missing() {
    assert!(error("sweep a.asc --from 1 --to 2 --step 1").contains("--tier NAME"));
    assert!(error("sweep a.asc --tier P1 --to 2 --step 1").contains("--from ANGLE"));
    assert!(error("sweep a.asc --tier P1 --from 1 --step 1").contains("--to ANGLE"));
    assert!(error("sweep a.asc --tier P1 --from 1 --to 2").contains("--step DEGREES"));
    assert!(error("sweep a.asc --tier P1 --from x --to 2 --step 1").contains("needs a number"));
}

#[test]
fn export_needs_a_format_and_an_output() {
    assert!(error("export a.asc --format asc").contains("needs --out FILE"));
    assert!(error("export a.asc --out thing").contains("needs --format"));
    assert!(error("export a.asc --format pdf --out x").contains("unknown format"));
    let Command::Export(args) = parsed("export a.asc --format gcs --out x.dat") else {
        panic!("expected export");
    };
    assert_eq!(args.format, ExportFormat::Gcs);
    assert_eq!(args.out, PathBuf::from("x.dat"));
}

#[test]
fn export_can_infer_the_format_from_the_output_name() {
    for (name, format) in [
        ("a.asc", ExportFormat::Asc),
        ("a.INDICATRIX", ExportFormat::Indicatrix),
        ("a.gcs", ExportFormat::Gcs),
        ("a.html", ExportFormat::Html),
        ("a.htm", ExportFormat::Html),
    ] {
        let Command::Export(args) = parsed(&format!("export in.gem --out {name}")) else {
            panic!("expected export");
        };
        assert_eq!(args.format, format, "{name}");
    }
    // An explicit format beats the extension.
    let Command::Export(args) = parsed("export in.gem --format html --out a.asc") else {
        panic!("expected export");
    };
    assert_eq!(args.format, ExportFormat::Html);
}

fn render_args(line: &str) -> RenderJobArgs {
    match parsed(line) {
        Command::Render(args) | Command::TiltVideo(args) => args,
        other => panic!("expected a render command, got {other:?}"),
    }
}

#[test]
fn render_defaults_to_both_engines_and_the_jobs_own_choices() {
    let args = render_args("render a.job.json");
    assert_eq!(args.kind, JobCommandKind::Render);
    assert_eq!(args.job, PathBuf::from("a.job.json"));
    assert_eq!(args.local, LocalEngines::CpuGpu);
    assert_eq!(args.out, None);
    assert_eq!(args.compute, None);
    assert_eq!(args.transfer, None);
    assert_eq!(args.remote, None);
    assert_eq!(args.cert_dir, None);
    assert!(!args.contribute_local && !args.restart && !args.quiet);
    assert!(matches!(parsed("render a.job.json"), Command::Render(_)));
    assert!(matches!(
        parsed("tilt-video a.job.json"),
        Command::TiltVideo(_)
    ));
}

#[test]
fn render_takes_every_flag() {
    let args = render_args(
        "render a.job.json --out pic.png --local gpu --remote box:7878 --cert-dir certs \
         --compute both --transfer final --contribute-local --quiet",
    );
    assert_eq!(
        args,
        RenderJobArgs {
            kind: JobCommandKind::Render,
            job: PathBuf::from("a.job.json"),
            out: Some(PathBuf::from("pic.png")),
            local: LocalEngines::Gpu,
            compute: Some(ComputeChoice::Both),
            transfer: Some(TransferChoice::FinalPicture),
            remote: Some("box:7878".to_string()),
            cert_dir: Some(PathBuf::from("certs")),
            contribute_local: true,
            restart: false,
            quiet: true,
        }
    );
}

#[test]
fn tilt_video_takes_every_flag() {
    let args = render_args(
        "tilt-video v.job.json --out-dir frames --restart --local cpu --compute local \
         --transfer full --quiet",
    );
    assert_eq!(
        args,
        RenderJobArgs {
            kind: JobCommandKind::TiltVideo,
            job: PathBuf::from("v.job.json"),
            out: Some(PathBuf::from("frames")),
            local: LocalEngines::Cpu,
            compute: Some(ComputeChoice::Local),
            transfer: Some(TransferChoice::FullData),
            remote: None,
            cert_dir: None,
            contribute_local: false,
            restart: true,
            quiet: true,
        }
    );
}

#[test]
fn the_local_engines_have_three_words() {
    for (word, engines) in [
        ("cpu", LocalEngines::Cpu),
        ("gpu", LocalEngines::Gpu),
        ("cpu+gpu", LocalEngines::CpuGpu),
    ] {
        assert_eq!(
            render_args(&format!("render a.job.json --local {word}")).local,
            engines,
            "{word}"
        );
    }
    assert!(error("render a.job.json --local tpu").contains("unknown engines \"tpu\""));
    assert!(error("render a.job.json --compute some").contains("unknown compute choice"));
    assert!(error("render a.job.json --transfer some").contains("unknown transfer choice"));
}

#[test]
fn a_flag_of_the_other_render_command_is_refused_and_the_command_named() {
    let message = error("tilt-video v.job.json --out pic.png");
    assert!(message.contains("--out is for \"render\""), "{message}");
    assert!(message.contains("--out-dir"), "{message}");
    for flag in ["--out-dir frames", "--restart"] {
        let message = error(&format!("render a.job.json {flag}"));
        assert!(message.contains("is for \"tilt-video\""), "{message}");
        assert!(message.contains("\"render\""), "{message}");
    }
    assert_eq!(
        error("render a.job.json --frob"),
        "unknown flag \"--frob\" for \"render\" (see --help)"
    );
}

#[test]
fn a_render_command_needs_exactly_one_job_file_and_every_value() {
    assert!(error("render").contains("needs a job file"));
    assert!(error("tilt-video --quiet").contains("needs a job file"));
    assert!(
        error("render a.job.json b.job.json").contains("unexpected extra argument \"b.job.json\"")
    );
    assert_eq!(
        error("render a.job.json --out"),
        "--out needs a value (see --help)"
    );
    assert_eq!(
        error("render a.job.json --local"),
        "--local needs a value (see --help)"
    );
    assert_eq!(
        error("render a.job.json --remote"),
        "--remote needs a value (see --help)"
    );
}

#[test]
fn an_empty_remote_is_refused() {
    let words = vec![
        "render".to_string(),
        "a.job.json".to_string(),
        "--remote".to_string(),
        String::new(),
        "--cert-dir".to_string(),
        "certs".to_string(),
    ];
    let message = parse(&words).expect_err("an empty address");
    assert!(message.contains("--remote needs HOST:PORT"), "{message}");
}

#[test]
fn remote_and_cert_dir_go_together() {
    assert!(
        error("render a.job.json --remote box:7878")
            .contains("--remote and --cert-dir go together")
    );
    assert!(
        error("tilt-video v.job.json --cert-dir certs")
            .contains("--remote and --cert-dir go together")
    );
}

#[test]
fn a_remote_compute_choice_needs_a_remote() {
    for choice in ["remote", "both"] {
        let message = error(&format!("render a.job.json --compute {choice}"));
        assert_eq!(message, "--compute remote needs --remote and --cert-dir");
        let args = render_args(&format!(
            "render a.job.json --compute {choice} --remote box:7878 --cert-dir certs"
        ));
        assert!(args.remote.is_some());
    }
    // Local needs no remote.
    assert!(
        render_args("render a.job.json --compute local")
            .remote
            .is_none()
    );
}

#[test]
fn the_help_pages_of_the_render_commands() {
    assert_eq!(parsed("render --help"), Command::Help(Topic::Render));
    assert_eq!(parsed("help tilt-video"), Command::Help(Topic::TiltVideo));
    assert_eq!(
        parsed("tilt-video a.job.json -h"),
        Command::Help(Topic::TiltVideo)
    );
    let root = crate::help::text(Topic::Root);
    assert!(root.contains("render"), "{root}");
    assert!(root.contains("tilt-video"), "{root}");
    assert!(crate::help::text(Topic::Render).contains("--out FILE.png"));
    assert!(crate::help::text(Topic::TiltVideo).contains("--restart"));
    let message = error("frobnicate x");
    assert!(message.contains("render or tilt-video"), "{message}");
}
