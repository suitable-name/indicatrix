//! The one-shot `render` subcommand.

use crate::cli::{Command, ComputeMode, RenderArgs, parse};
use std::path::PathBuf;

#[test]
fn parses_a_well_formed_render_command() {
    let argv = [
        "render",
        "--scene",
        "scene.json",
        "--out",
        "out.png",
        "--width",
        "3840",
        "--height",
        "2160",
        "--samples",
        "4096",
        "--threads",
        "8",
    ]
    .map(String::from);
    let cmd = parse(&argv).unwrap();
    assert_eq!(
        cmd,
        Command::Render(RenderArgs {
            scene: PathBuf::from("scene.json"),
            out: PathBuf::from("out.png"),
            width: 3840,
            height: 2160,
            samples: 4096,
            threads: 8,
            compute_mode: ComputeMode::Hybrid,
        })
    );
}

#[test]
fn render_threads_defaults_to_zero_meaning_auto() {
    let argv = [
        "render",
        "--scene",
        "s.json",
        "--out",
        "o.png",
        "--width",
        "8",
        "--height",
        "8",
        "--samples",
        "4",
    ]
    .map(String::from);
    let Command::Render(args) = parse(&argv).unwrap() else {
        panic!("expected Render")
    };
    assert_eq!(args.threads, 0);
}

/// Base argv for `render`'s required flags, shared by the `--only-*`/`compute_mode` tests.
fn render_argv_base() -> Vec<String> {
    [
        "render",
        "--scene",
        "s.json",
        "--out",
        "o.png",
        "--width",
        "8",
        "--height",
        "8",
        "--samples",
        "4",
    ]
    .map(String::from)
    .to_vec()
}

#[test]
fn render_compute_mode_defaults_to_hybrid_when_neither_only_flag_is_given() {
    let Command::Render(args) = parse(&render_argv_base()).unwrap() else {
        panic!("expected Render")
    };
    assert_eq!(args.compute_mode, ComputeMode::Hybrid);
}

#[test]
fn render_only_cpu_flag_parses_to_only_cpu() {
    let mut argv = render_argv_base();
    argv.push("--only-cpu".to_string());
    let Command::Render(args) = parse(&argv).unwrap() else {
        panic!("expected Render")
    };
    assert_eq!(args.compute_mode, ComputeMode::OnlyCpu);
}

#[cfg(feature = "gpu")]
#[test]
fn render_only_gpu_flag_parses_to_only_gpu_on_a_gpu_build() {
    let mut argv = render_argv_base();
    argv.push("--only-gpu".to_string());
    let Command::Render(args) = parse(&argv).unwrap() else {
        panic!("expected Render")
    };
    assert_eq!(args.compute_mode, ComputeMode::OnlyGpu);
}

/// Without the `gpu` feature there's no GPU path to satisfy `--only-gpu`, so it must
/// error rather than silently behave like `--only-cpu` or `Hybrid`.
#[cfg(not(feature = "gpu"))]
#[test]
fn render_only_gpu_flag_is_rejected_without_the_gpu_feature() {
    let mut argv = render_argv_base();
    argv.push("--only-gpu".to_string());
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("--only-gpu"), "{err}");
    assert!(err.contains("gpu"), "{err}");
}

/// Passing both flags is a parse error, not silent last-wins, regardless of build.
#[test]
fn render_only_gpu_and_only_cpu_together_is_a_parse_error() {
    let mut argv = render_argv_base();
    argv.push("--only-gpu".to_string());
    argv.push("--only-cpu".to_string());
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("mutually exclusive"), "{err}");

    // Order must not matter.
    let mut argv = render_argv_base();
    argv.push("--only-cpu".to_string());
    argv.push("--only-gpu".to_string());
    assert!(parse(&argv).is_err());
}

/// Repeating the same flag is not a conflict -- only two different `--only-*` flags are.
#[test]
fn render_repeating_the_same_only_flag_is_not_an_error() {
    let mut argv = render_argv_base();
    argv.push("--only-cpu".to_string());
    argv.push("--only-cpu".to_string());
    let Command::Render(args) = parse(&argv).unwrap() else {
        panic!("expected Render")
    };
    assert_eq!(args.compute_mode, ComputeMode::OnlyCpu);
}

#[test]
fn render_rejects_missing_required_flags() {
    let argv = ["render", "--scene", "s.json"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn render_rejects_a_negative_width_at_the_parse_layer() {
    let argv = [
        "render",
        "--scene",
        "s.json",
        "--out",
        "o.png",
        "--width",
        "-100",
        "--height",
        "8",
        "--samples",
        "4",
    ]
    .map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn render_rejects_a_non_numeric_samples_value() {
    let argv = [
        "render",
        "--scene",
        "s.json",
        "--out",
        "o.png",
        "--width",
        "8",
        "--height",
        "8",
        "--samples",
        "lots",
    ]
    .map(String::from);
    assert!(parse(&argv).is_err());
}
