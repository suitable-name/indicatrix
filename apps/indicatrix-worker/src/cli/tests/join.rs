//! The `join` subcommand (`worker` builds; the refusals parse on every build).

use crate::cli::parse;
#[cfg(feature = "worker")]
use crate::cli::{Command, ComputeMode, DEFAULT_JOIN_CERT_DIR, JoinArgs};
#[cfg(feature = "worker")]
use std::path::PathBuf;

#[cfg(feature = "worker")]
#[test]
fn parses_join_with_a_positional_coordinator_and_defaults() {
    let argv = ["join", "coord.lan:7880"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Join(JoinArgs {
            coordinator: "coord.lan:7880".to_string(),
            cert_dir: PathBuf::from(DEFAULT_JOIN_CERT_DIR),
            slots: 2,
            threads: 0,
            compute_mode: ComputeMode::Hybrid,
            token: None,
            enroll_addr: None,
            payload_encoding: indicatrix_net::messages::adaptive::PayloadChoice::Auto,
        })
    );
}

#[cfg(feature = "worker")]
#[test]
fn parses_join_with_every_flag() {
    let argv = [
        "join",
        "--coordinator",
        "10.0.0.2:7880",
        "--cert-dir",
        "certs",
        "--slots",
        "3",
        "--threads",
        "8",
        "--only-cpu",
        "--token",
        "GW1-XXXX",
        "--enroll-addr",
        "10.0.0.2:7881",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::Join(JoinArgs {
            coordinator: "10.0.0.2:7880".to_string(),
            cert_dir: PathBuf::from("certs"),
            slots: 3,
            threads: 8,
            compute_mode: ComputeMode::OnlyCpu,
            token: Some("GW1-XXXX".to_string()),
            enroll_addr: Some("10.0.0.2:7881".to_string()),
            payload_encoding: indicatrix_net::messages::adaptive::PayloadChoice::Auto,
        })
    );
}

#[test]
fn join_rejects_a_missing_or_doubled_coordinator_and_zero_slots() {
    assert!(parse(&["join".to_string()]).is_err());
    let doubled = ["join", "a:1", "--coordinator", "b:2"].map(String::from);
    assert!(parse(&doubled).unwrap_err().contains("exactly one"));
    let zero = ["join", "a:1", "--slots", "0"].map(String::from);
    assert!(parse(&zero).unwrap_err().contains("--slots"));
}

/// `join --payload-encoding` takes the same spellings as `serve`'s and defaults to `auto`.
#[cfg(feature = "worker")]
#[test]
fn join_payload_encoding_defaults_to_auto_and_pins_a_fixed_policy_otherwise() {
    use indicatrix_net::messages::{
        PayloadEncoding,
        adaptive::PayloadChoice::{self, Auto, Fixed},
    };
    let choice = |extra: &[&str]| -> Result<PayloadChoice, String> {
        let mut argv = vec!["join".to_string(), "coord.lan:7880".to_string()];
        argv.extend(extra.iter().map(ToString::to_string));
        match parse(&argv)? {
            Command::Join(join) => Ok(join.payload_encoding),
            other => panic!("expected Join, got {other:?}"),
        }
    };
    let accepts = PayloadEncoding::default_accept_list();

    assert_eq!(choice(&[]), Ok(Auto));
    for (text, want) in [
        ("raw", Fixed(PayloadEncoding::Raw)),
        ("lz4", Fixed(PayloadEncoding::ShuffleLz4)),
        ("zstd:5", Fixed(PayloadEncoding::ShuffleZstd { level: 5 })),
    ] {
        let parsed = choice(&["--payload-encoding", text]).unwrap();
        assert_eq!(parsed, want, "{text}");
        assert!(
            !parsed.policy(&accepts, false).is_adaptive(),
            "{text} must force the fixed policy"
        );
    }
    assert!(choice(&["--payload-encoding", "zstd:23"]).is_err());
}
