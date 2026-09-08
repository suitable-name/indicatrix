use super::{
    CertClaimArgs, CertInitArgs, CertIssueClientArgs, CertIssueServerArgs, CertIssueTokenArgs,
    Command, ComputeMode, DEFAULT_BIND, HelpTopic, RenderArgs, ServeArgs, parse,
};
use std::path::PathBuf;

#[test]
fn no_args_is_help() {
    assert_eq!(parse(&[]).unwrap(), Command::Help(HelpTopic::Root));
}

#[test]
fn help_flag_short_circuits_even_with_other_stuff_present() {
    let argv = vec!["render".to_string(), "--help".to_string()];
    assert_eq!(parse(&argv).unwrap(), Command::Help(HelpTopic::Render));

    // A leading `--help` flag stops the scan before "render", so this resolves to Root.
    let argv = vec!["--help".to_string(), "render".to_string()];
    assert_eq!(parse(&argv).unwrap(), Command::Help(HelpTopic::Root));

    let argv = vec!["-h".to_string()];
    assert_eq!(parse(&argv).unwrap(), Command::Help(HelpTopic::Root));
}

/// [`HelpTopic::from_argv`] resolves every subcommand/sub-subcommand's argv prefix to
/// its own page, matching what [`parse`] itself dispatches on.
#[test]
fn help_topic_resolves_from_each_commands_own_argv_path() {
    let cases: &[(&[&str], HelpTopic)] = &[
        (&[], HelpTopic::Root),
        (&["--help"], HelpTopic::Root),
        (&["-h"], HelpTopic::Root),
        (&["render", "--help"], HelpTopic::Render),
        (&["render", "-h"], HelpTopic::Render),
        (&["serve", "--help"], HelpTopic::Serve),
        (&["cert", "--help"], HelpTopic::Cert),
        (&["cert", "init", "--help"], HelpTopic::CertInit),
        (
            &["cert", "issue-server", "--help"],
            HelpTopic::CertIssueServer,
        ),
        (
            &["cert", "issue-client", "--help"],
            HelpTopic::CertIssueClient,
        ),
        (
            &["cert", "issue-token", "--help"],
            HelpTopic::CertIssueToken,
        ),
        (&["cert", "claim", "--help"], HelpTopic::CertClaim),
        // Flags after the leading tokens are skipped, not scanned.
        (
            &["render", "--scene", "s.json", "--help"],
            HelpTopic::Render,
        ),
        // An unrecognized subcommand falls back to the nearest listing page instead of
        // erroring -- --help never itself produces an error.
        (&["bogus", "--help"], HelpTopic::Root),
        (&["cert", "bogus", "--help"], HelpTopic::Cert),
    ];
    for (argv, expected) in cases {
        let argv: Vec<String> = argv.iter().map(std::string::ToString::to_string).collect();
        assert_eq!(HelpTopic::from_argv(&argv), *expected, "argv = {argv:?}");
        assert_eq!(
            parse(&argv).unwrap(),
            Command::Help(*expected),
            "argv = {argv:?}"
        );
    }
}

#[test]
fn unknown_subcommand_is_rejected() {
    assert!(parse(&["frobnicate".to_string()]).is_err());
}

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
        enroll_bind: None,
        no_enroll: false,
        db: None,
        max_connections: super::DEFAULT_MAX_CONNECTIONS,
    }
}

#[test]
fn parses_a_well_formed_serve_command_with_defaults() {
    let argv = ["serve".to_string()];
    assert_eq!(parse(&argv).unwrap(), Command::Serve(default_serve_args()));
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
        Command::Serve(ServeArgs {
            bind: "0.0.0.0:9000".to_string(),
            threads: 4,
            allow_remote: true,
            ..default_serve_args()
        })
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
        Command::Serve(ServeArgs {
            ca: Some(PathBuf::from("ca.pem")),
            cert: Some(PathBuf::from("server.pem")),
            key: Some(PathBuf::from("server.key")),
            allowlist: Some(PathBuf::from("trusted.txt")),
            trust_any_client_cert: true,
            ..default_serve_args()
        })
    );
}

#[test]
fn parses_serve_insecure_no_tls_flag() {
    let argv = ["serve", "--insecure-no-tls"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(ServeArgs {
            insecure_no_tls: true,
            ..default_serve_args()
        })
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

#[test]
fn parses_serve_only_cpu_flag() {
    let argv = ["serve", "--only-cpu"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(ServeArgs {
            compute_mode: ComputeMode::OnlyCpu,
            ..default_serve_args()
        })
    );
}

#[cfg(feature = "gpu")]
#[test]
fn parses_serve_only_gpu_flag_on_a_gpu_build() {
    let argv = ["serve", "--only-gpu"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(ServeArgs {
            compute_mode: ComputeMode::OnlyGpu,
            ..default_serve_args()
        })
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
        Command::Serve(ServeArgs {
            db: Some(PathBuf::from("custom.sqlite")),
            ..default_serve_args()
        })
    );
}

#[test]
fn parses_serve_max_connections_flag() {
    let argv = ["serve", "--max-connections", "8"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(ServeArgs {
            max_connections: 8,
            ..default_serve_args()
        })
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
fn serve_rejects_an_unknown_flag() {
    let argv = ["serve", "--bogus"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_init() {
    let argv = ["cert", "init", "--dir", "pki"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertInit(CertInitArgs {
            dir: PathBuf::from("pki")
        })
    );
}

#[test]
fn cert_init_requires_dir() {
    let argv = ["cert", "init"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_issue_server_with_repeated_host_and_ip() {
    let argv = [
        "cert",
        "issue-server",
        "--dir",
        "pki",
        "--host",
        "worker.lan",
        "--host",
        "worker2.lan",
        "--ip",
        "10.0.0.5",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertIssueServer(CertIssueServerArgs {
            dir: PathBuf::from("pki"),
            hosts: vec!["worker.lan".to_string(), "worker2.lan".to_string()],
            ips: vec!["10.0.0.5".parse().unwrap()],
        })
    );
}

#[test]
fn cert_issue_server_rejects_an_unparseable_ip() {
    let argv = ["cert", "issue-server", "--dir", "pki", "--ip", "not-an-ip"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_issue_client() {
    let argv = [
        "cert",
        "issue-client",
        "--dir",
        "pki",
        "--name",
        "laptop",
        "--out",
        "bundle",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertIssueClient(CertIssueClientArgs {
            dir: PathBuf::from("pki"),
            name: "laptop".to_string(),
            out: PathBuf::from("bundle"),
        })
    );
}

#[test]
fn cert_issue_client_requires_name_and_out() {
    let argv = ["cert", "issue-client", "--dir", "pki"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_serve_enroll_flags() {
    let argv = ["serve", "--enroll-bind", "127.0.0.1:7879", "--no-enroll"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Serve(ServeArgs {
            enroll_bind: Some("127.0.0.1:7879".to_string()),
            no_enroll: true,
            ..default_serve_args()
        })
    );
}

#[test]
fn parses_cert_issue_token() {
    let argv = [
        "cert",
        "issue-token",
        "--ca",
        "pki/ca.pem",
        "--admin-addr",
        "127.0.0.1:7879",
        "--name",
        "laptop",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertIssueToken(CertIssueTokenArgs {
            ca: PathBuf::from("pki/ca.pem"),
            admin_addr: "127.0.0.1:7879".to_string(),
            name: "laptop".to_string(),
        })
    );
}

#[test]
fn cert_issue_token_requires_ca_admin_addr_and_name() {
    let argv = ["cert", "issue-token", "--ca", "pki/ca.pem"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_claim() {
    let argv = [
        "cert",
        "claim",
        "--token",
        "GW1-XXXXX",
        "--addr",
        "127.0.0.1:7879",
        "--out",
        "bundle",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertClaim(CertClaimArgs {
            token: "GW1-XXXXX".to_string(),
            addr: "127.0.0.1:7879".to_string(),
            out: PathBuf::from("bundle"),
        })
    );
}

#[test]
fn cert_claim_requires_token_addr_and_out() {
    let argv = ["cert", "claim", "--token", "GW1-XXXXX"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn unknown_cert_sub_subcommand_is_rejected() {
    let argv = ["cert", "frobnicate"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn bare_cert_with_no_sub_subcommand_is_rejected() {
    let argv = ["cert".to_string()];
    assert!(parse(&argv).is_err());
}
