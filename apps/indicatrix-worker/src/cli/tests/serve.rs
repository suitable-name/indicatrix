//! The coordinator's `serve` flags.

use crate::cli::{
    Command, ComputeMode, DEFAULT_BIND, DEFAULT_MAX_CONNECTIONS, DEFAULT_MAX_JOB_MEMORY_MIB,
    DEFAULT_MAX_PREAUTH_PER_IP, ServeArgs, parse,
};
use std::path::PathBuf;

fn default_serve_args() -> ServeArgs {
    ServeArgs {
        bind: DEFAULT_BIND.to_string(),
        threads: 0,
        allow_remote: false,
        ca: None,
        cert: None,
        key: None,
        allowlist: None,
        trust_any_client_cert: false,
        insecure_no_tls: false,
        compute_mode: ComputeMode::Hybrid,
        render: false,
        enroll_bind: None,
        no_enroll: false,
        worker_bind: None,
        worker_enroll_bind: None,
        no_workers: false,
        worker_allowlist: None,
        db: None,
        max_connections: DEFAULT_MAX_CONNECTIONS,
        max_preauth_per_ip: DEFAULT_MAX_PREAUTH_PER_IP,
        interactive_workers: 0,
        pin_interactive_worker: None,
        max_job_memory_mib: DEFAULT_MAX_JOB_MEMORY_MIB,
    }
}

#[test]
fn parses_serve_job_flags() {
    let argv = [
        "serve",
        "--interactive-workers",
        "2",
        "--max-job-memory-mib",
        "512",
        "--pin-interactive-worker",
        "worker:gpu-box",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            interactive_workers: 2,
            pin_interactive_worker: Some("gpu-box".to_string()),
            max_job_memory_mib: 512,
            ..default_serve_args()
        }))
    );
    assert!(parse(&["serve", "--interactive-workers", "many"].map(String::from)).is_err());
    assert!(parse(&["serve", "--pin-interactive-worker", "worker:"].map(String::from)).is_err());
}

#[test]
fn parses_a_well_formed_serve_command_with_defaults() {
    let argv = ["serve".to_string()];
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(default_serve_args()))
    );
}

#[test]
fn parses_serve_flags() {
    let argv = [
        "serve",
        "--bind",
        "0.0.0.0:9000",
        "--threads",
        "4",
        "--allow-remote",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            bind: "0.0.0.0:9000".to_string(),
            threads: 4,
            allow_remote: true,
            ..default_serve_args()
        }))
    );
}

#[test]
fn parses_serve_tls_flags() {
    let argv = [
        "serve",
        "--ca",
        "ca.pem",
        "--cert",
        "server.pem",
        "--key",
        "server.key",
        "--allowlist",
        "trusted.txt",
        "--trust-any-client-cert",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            ca: Some(PathBuf::from("ca.pem")),
            cert: Some(PathBuf::from("server.pem")),
            key: Some(PathBuf::from("server.key")),
            allowlist: Some(PathBuf::from("trusted.txt")),
            trust_any_client_cert: true,
            ..default_serve_args()
        }))
    );
}

#[test]
fn parses_serve_insecure_no_tls_flag() {
    let argv = ["serve", "--insecure-no-tls"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            insecure_no_tls: true,
            ..default_serve_args()
        }))
    );
}

#[test]
fn serve_compute_mode_defaults_to_hybrid_when_neither_only_flag_is_given() {
    let argv = ["serve".to_string()];
    let Command::Serve(args) = parse(&argv).unwrap() else {
        panic!("expected Serve")
    };
    assert_eq!(args.compute_mode, ComputeMode::Hybrid);
}

/// `--only-cpu` selects the own lane's engine, so it implies `--render`.
#[cfg(feature = "worker")]
#[test]
fn parses_serve_only_cpu_flag() {
    let argv = ["serve", "--only-cpu"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            compute_mode: ComputeMode::OnlyCpu,
            render: true,
            ..default_serve_args()
        }))
    );
}

/// Plain `serve` never renders; `--render` turns the own lane on (hybrid).
#[cfg(feature = "worker")]
#[test]
fn serve_renders_only_with_the_render_flag() {
    let Command::Serve(bare) = parse(&["serve".to_string()]).unwrap() else {
        panic!("expected Serve")
    };
    assert!(!bare.render);
    let argv = ["serve", "--render"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            render: true,
            ..default_serve_args()
        }))
    );
}

/// A library-only build has no render lane to turn on.
#[cfg(not(feature = "worker"))]
#[test]
fn serve_render_flags_are_rejected_without_the_worker_feature() {
    for flag in ["--render", "--only-cpu"] {
        let err = parse(&["serve".to_string(), flag.to_string()]).unwrap_err();
        assert!(err.contains("worker"), "{flag}: {err}");
    }
}

#[test]
fn parses_serve_worker_port_flags() {
    let argv = [
        "serve",
        "--worker-bind",
        "0.0.0.0:9880",
        "--worker-enroll-bind",
        "0.0.0.0:9881",
        "--worker-allowlist",
        "workers.txt",
        "--no-workers",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            worker_bind: Some("0.0.0.0:9880".to_string()),
            worker_enroll_bind: Some("0.0.0.0:9881".to_string()),
            worker_allowlist: Some(PathBuf::from("workers.txt")),
            no_workers: true,
            ..default_serve_args()
        }))
    );
}

#[cfg(feature = "gpu")]
#[test]
fn parses_serve_only_gpu_flag_on_a_gpu_build() {
    let argv = ["serve", "--only-gpu"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            compute_mode: ComputeMode::OnlyGpu,
            render: true,
            ..default_serve_args()
        }))
    );
}

/// Same rule as `render_only_gpu_flag_is_rejected_without_the_gpu_feature`.
#[cfg(not(feature = "gpu"))]
#[test]
fn serve_only_gpu_flag_is_rejected_without_the_gpu_feature() {
    let argv = ["serve", "--only-gpu"].map(String::from);
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("--only-gpu"), "{err}");
    assert!(err.contains("gpu"), "{err}");
}

/// Same requirement as `render_only_gpu_and_only_cpu_together_is_a_parse_error`.
#[test]
fn serve_only_gpu_and_only_cpu_together_is_a_parse_error() {
    let argv = ["serve", "--only-gpu", "--only-cpu"].map(String::from);
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("mutually exclusive"), "{err}");

    let argv = ["serve", "--only-cpu", "--only-gpu"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_serve_db_flag() {
    let argv = ["serve", "--db", "custom.sqlite"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            db: Some(PathBuf::from("custom.sqlite")),
            ..default_serve_args()
        }))
    );
}

#[test]
fn parses_serve_max_connections_flag() {
    let argv = ["serve", "--max-connections", "8"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            max_connections: 8,
            ..default_serve_args()
        }))
    );
}

#[test]
fn serve_max_connections_defaults_to_64_when_not_given() {
    let argv = ["serve".to_string()];
    let Command::Serve(args) = parse(&argv).unwrap() else {
        panic!("expected Serve")
    };
    assert_eq!(args.max_connections, 64);
}

#[test]
fn serve_rejects_a_zero_max_connections() {
    let argv = ["serve", "--max-connections", "0"].map(String::from);
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("--max-connections"), "{err}");
}

#[test]
fn serve_rejects_a_non_numeric_max_connections_value() {
    let argv = ["serve", "--max-connections", "lots"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_serve_max_preauth_per_ip_flag() {
    let argv = ["serve", "--max-preauth-per-ip", "3"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            max_preauth_per_ip: 3,
            ..default_serve_args()
        }))
    );
}

#[test]
fn serve_max_preauth_per_ip_defaults_to_8_when_not_given() {
    let argv = ["serve".to_string()];
    let Command::Serve(args) = parse(&argv).unwrap() else {
        panic!("expected Serve")
    };
    assert_eq!(args.max_preauth_per_ip, 8);
}

#[test]
fn serve_rejects_a_zero_max_preauth_per_ip() {
    let argv = ["serve", "--max-preauth-per-ip", "0"].map(String::from);
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("--max-preauth-per-ip"), "{err}");
}

#[test]
fn serve_rejects_an_unknown_flag() {
    let argv = ["serve", "--bogus"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_serve_enroll_flags() {
    let argv = ["serve", "--enroll-bind", "127.0.0.1:7879", "--no-enroll"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(Box::new(ServeArgs {
            enroll_bind: Some("127.0.0.1:7879".to_string()),
            no_enroll: true,
            ..default_serve_args()
        }))
    );
}
