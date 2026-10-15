//! Tests of the version rules and the release parsing (no network).

use super::*;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn release(version: &str) -> Release {
    Release {
        version: v(version),
        tag: format!("v{version}"),
        url: format!("{RELEASE_PAGE_PREFIX}tag/v{version}"),
    }
}

fn json(tag: &str, url: &str, draft: bool, prerelease: bool) -> String {
    format!(
        r#"{{"tag_name":"{tag}","html_url":"{url}","draft":{draft},"prerelease":{prerelease},
        "name":"Indicatrix","assets":[]}}"#
    )
}

#[test]
fn tags_with_and_without_the_v_prefix_parse() {
    assert_eq!(parse_version("v0.7.3"), Some(v("0.7.3")));
    assert_eq!(parse_version("V1.0.0"), Some(v("1.0.0")));
    assert_eq!(parse_version(" 0.7.2 "), Some(v("0.7.2")));
    assert_eq!(parse_version("v0.8.0-rc.1"), Some(v("0.8.0-rc.1")));
    for bad in ["", "v", "latest", "v0.7", "release-0.7.3", "vv0.7.3"] {
        assert_eq!(parse_version(bad), None, "{bad}");
    }
}

#[test]
fn only_a_newer_final_release_is_offered() {
    let current = v("0.7.2");
    assert_eq!(newer(&current, release("0.7.3")), Some(release("0.7.3")));
    assert_eq!(newer(&current, release("1.0.0")), Some(release("1.0.0")));
    assert_eq!(newer(&current, release("0.7.2")), None);
    assert_eq!(newer(&current, release("0.7.1")), None);
    // 0.10 is newer than 0.9 (numeric, not text, comparison).
    assert!(newer(&v("0.9.0"), release("0.10.0")).is_some());
    // A pre-release tag is never offered, a running pre-release gets the final release.
    assert_eq!(newer(&current, release("0.8.0-rc.1")), None);
    assert!(newer(&v("0.8.0-rc.1"), release("0.8.0")).is_some());
}

#[test]
fn the_release_json_is_read_and_checked() {
    let url = format!("{RELEASE_PAGE_PREFIX}tag/v0.7.3");
    let parsed = parse_release(&json("v0.7.3", &url, false, false)).unwrap();
    assert_eq!(parsed, release("0.7.3"));

    assert!(matches!(
        parse_release(&json("v0.7.3", &url, true, false)),
        Err(CheckError::BadResponse(_))
    ));
    assert!(matches!(
        parse_release(&json("v0.7.3", &url, false, true)),
        Err(CheckError::BadResponse(_))
    ));
    assert!(matches!(
        parse_release(&json("nightly", &url, false, false)),
        Err(CheckError::BadResponse(_))
    ));
    // The link is opened in a browser, so it must point at this repository's releases.
    assert!(matches!(
        parse_release(&json("v0.7.3", "https://example.com/x", false, false)),
        Err(CheckError::BadResponse(_))
    ));
    assert!(matches!(
        parse_release("not json"),
        Err(CheckError::BadResponse(_))
    ));
}

#[test]
fn consent_and_the_daily_interval_gate_the_automatic_check() {
    let now = 1_800_000_000;
    let mut prefs = UpdatePreferences::default();
    assert!(prefs.needs_consent());
    assert!(!prefs.is_due(now), "never before the user said yes");

    prefs.enabled = Some(false);
    assert!(!prefs.needs_consent());
    assert!(!prefs.is_due(now));

    prefs.enabled = Some(true);
    assert!(prefs.is_due(now), "never checked");
    prefs.last_check_unix = now - CHECK_INTERVAL_SECS + 1;
    assert!(!prefs.is_due(now));
    prefs.last_check_unix = now - CHECK_INTERVAL_SECS;
    assert!(prefs.is_due(now));
    prefs.last_check_unix = now + 3600;
    assert!(prefs.is_due(now), "a clock set back counts as due");
}

#[test]
fn a_skipped_version_is_not_announced_again() {
    let mut prefs = UpdatePreferences {
        enabled: Some(true),
        ..UpdatePreferences::default()
    };
    assert!(prefs.should_notify(&release("0.7.3")));
    prefs.skipped_version = Some("0.7.3".to_owned());
    assert!(!prefs.should_notify(&release("0.7.3")));
    assert!(prefs.should_notify(&release("0.7.4")));
    prefs.skipped_version = Some("garbage".to_owned());
    assert!(prefs.should_notify(&release("0.7.3")));
}

#[test]
fn preferences_load_tolerantly_from_older_settings() {
    let prefs: UpdatePreferences = serde_json::from_str("{}").unwrap();
    assert_eq!(prefs, UpdatePreferences::default());
    let prefs: UpdatePreferences = serde_json::from_str(r#"{"enabled":true}"#).unwrap();
    assert_eq!(prefs.enabled, Some(true));
}

#[test]
fn the_notice_line_names_both_versions_and_the_page() {
    assert_eq!(
        notice_line(&release("0.7.3"), "0.7.2"),
        format!("Indicatrix 0.7.3 is available (this is 0.7.2): {RELEASE_PAGE_PREFIX}tag/v0.7.3")
    );
}

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| (*w).to_owned()).collect()
}

#[test]
fn a_command_line_tool_checks_by_default() {
    let mut args = words(&["info", "a.isdd", "--json"]);
    assert!(take_update_check(&mut args, None));
    assert_eq!(args, words(&["info", "a.isdd", "--json"]));
}

#[test]
fn the_flag_anywhere_skips_the_check_and_is_removed_before_parsing() {
    for list in [
        &["--no-update-check", "info", "a.isdd"][..],
        &["info", "--no-update-check", "a.isdd"][..],
        &["info", "a.isdd", "--no-update-check", "--no-update-check"][..],
    ] {
        let mut args = words(list);
        assert!(!take_update_check(&mut args, None), "{list:?}");
        assert_eq!(args, words(&["info", "a.isdd"]), "{list:?}");
    }
}

#[test]
fn the_environment_variable_skips_the_check_whatever_its_value() {
    for value in ["1", "0", ""] {
        let mut args = words(&["info", "a.isdd"]);
        assert!(
            !take_update_check(&mut args, Some(std::ffi::OsStr::new(value))),
            "{value:?}"
        );
        assert_eq!(args, words(&["info", "a.isdd"]));
    }
}

#[test]
fn the_constants_agree() {
    assert!(LATEST_RELEASE_URL.contains(REPOSITORY));
    assert!(RELEASE_PAGE_PREFIX.contains(REPOSITORY));
    assert!(CLI_TIMEOUT < GUI_TIMEOUT);
}
