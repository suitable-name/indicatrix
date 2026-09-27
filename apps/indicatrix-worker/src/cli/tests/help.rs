//! `--help` resolution and unknown subcommands.

use crate::cli::{Command, HelpTopic, parse};

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
        (&["join", "--help"], HelpTopic::Join),
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
