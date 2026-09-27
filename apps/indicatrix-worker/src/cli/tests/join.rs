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
            slots: 1,
            threads: 0,
            compute_mode: ComputeMode::Hybrid,
            token: None,
            enroll_addr: None,
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
