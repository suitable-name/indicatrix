//! The new-version check of the Indicatrix apps.
//!
//! Releases are published on GitHub with `v<version>` tags (`v0.7.3`). [`check`] asks the
//! GitHub API for the latest release (drafts and pre-releases excluded by the endpoint itself),
//! parses its tag and reports it when it is newer than the running app. Nothing is downloaded or
//! installed; the apps only point the user at the release page.
//!
//! The apps own the consent and the bookkeeping: [`UpdatePreferences`] is the serde record they
//! store in their settings (asked or not, enabled, last check, skipped version), and
//! [`UpdatePreferences::is_due`] / [`UpdatePreferences::should_notify`] are the rules. Every rule
//! is a pure function; only [`fetch_latest`] touches the network.

use std::time::Duration;

use semver::Version;
use serde::{Deserialize, Serialize};

/// The GitHub repository the releases are published in (`owner/name`).
pub const REPOSITORY: &str = "suitable-name/indicatrix";
/// The GitHub API endpoint of the latest published release.
pub const LATEST_RELEASE_URL: &str =
    "https://api.github.com/repos/suitable-name/indicatrix/releases/latest";
/// The prefix every accepted release-page link starts with (the link is opened in a browser).
pub const RELEASE_PAGE_PREFIX: &str = "https://github.com/suitable-name/indicatrix/releases/";
/// Seconds between two automatic checks.
pub const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
/// Timeout of an automatic check in a GUI (the check runs on a background thread).
pub const GUI_TIMEOUT: Duration = Duration::from_secs(5);
/// Timeout of the check a command-line tool makes before its first line (it waits for it).
pub const CLI_TIMEOUT: Duration = Duration::from_secs(2);
/// The largest response body read (the release JSON is a few kB without assets).
const MAX_BODY_BYTES: u64 = 512 * 1024;

/// A published release newer than the running app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version the tag names (`0.7.3` for `v0.7.3`).
    pub version: Version,
    /// The tag as published (`v0.7.3`).
    pub tag: String,
    /// The release page, always under [`RELEASE_PAGE_PREFIX`].
    pub url: String,
}

/// Why a check gave no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CheckError {
    /// No connection, a timeout, a TLS or proxy failure.
    Network(String),
    /// GitHub's limit for anonymous requests (60 an hour per address) is used up.
    RateLimited,
    /// No published release, or the repository is not public.
    NoRelease,
    /// The answer was not a release with a `v<version>` tag and a release-page link.
    BadResponse(String),
}

impl std::fmt::Display for CheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Network(reason) => write!(f, "GitHub could not be reached: {reason}"),
            Self::RateLimited => write!(f, "GitHub's request limit is used up; try again later"),
            Self::NoRelease => write!(f, "no published release was found on GitHub"),
            Self::BadResponse(reason) => write!(f, "GitHub's answer was not understood: {reason}"),
        }
    }
}

impl std::error::Error for CheckError {}

/// The update-check record an app keeps in its settings.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdatePreferences {
    /// `None` until the user answered the "check for new versions?" question; then the answer.
    pub enabled: Option<bool>,
    /// Unix seconds of the last completed check (successful or not); 0 = never.
    pub last_check_unix: u64,
    /// A version the user chose to skip (`0.7.3`); it is not announced again.
    pub skipped_version: Option<String>,
}

impl UpdatePreferences {
    /// Whether the app still has to ask the user.
    #[must_use]
    pub const fn needs_consent(&self) -> bool {
        self.enabled.is_none()
    }

    /// Whether an automatic check is due at `now_unix`: enabled and at least
    /// [`CHECK_INTERVAL_SECS`] since the last one (a clock set back counts as due).
    #[must_use]
    pub const fn is_due(&self, now_unix: u64) -> bool {
        matches!(self.enabled, Some(true))
            && (now_unix < self.last_check_unix
                || now_unix - self.last_check_unix >= CHECK_INTERVAL_SECS)
    }

    /// Whether `release` should be announced (it is not the skipped version).
    #[must_use]
    pub fn should_notify(&self, release: &Release) -> bool {
        self.skipped_version
            .as_deref()
            .and_then(parse_version)
            .is_none_or(|skipped| skipped != release.version)
    }
}

/// The version a release tag names: `v0.7.3` / `V0.7.3` / `0.7.3` -> `0.7.3`; `None` for anything
/// that is not a semantic version.
#[must_use]
pub fn parse_version(tag: &str) -> Option<Version> {
    let trimmed = tag.trim();
    let bare = trimmed
        .strip_prefix('v')
        .or_else(|| trimmed.strip_prefix('V'))
        .unwrap_or(trimmed);
    Version::parse(bare).ok()
}

/// `latest` when it is newer than `current` and not a pre-release; a running pre-release is
/// offered the final release of the same version.
#[must_use]
pub fn newer(current: &Version, latest: Release) -> Option<Release> {
    (latest.version.pre.is_empty() && latest.version > *current).then_some(latest)
}

/// The fields of GitHub's release JSON the check reads.
#[derive(Deserialize)]
struct ReleaseJson {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
}

/// Parses GitHub's latest-release JSON into a [`Release`].
///
/// # Errors
///
/// [`CheckError::BadResponse`] for invalid JSON, a draft or pre-release, a tag that is not
/// `v<version>`, or a page link outside [`RELEASE_PAGE_PREFIX`].
pub fn parse_release(json: &str) -> Result<Release, CheckError> {
    let release: ReleaseJson =
        serde_json::from_str(json).map_err(|e| CheckError::BadResponse(e.to_string()))?;
    if release.draft || release.prerelease {
        return Err(CheckError::BadResponse(
            "the latest release is a draft or a pre-release".to_owned(),
        ));
    }
    let version = parse_version(&release.tag_name).ok_or_else(|| {
        CheckError::BadResponse(format!("tag '{}' is not v<version>", release.tag_name))
    })?;
    if !release.html_url.starts_with(RELEASE_PAGE_PREFIX) {
        return Err(CheckError::BadResponse(format!(
            "unexpected release page '{}'",
            release.html_url
        )));
    }
    Ok(Release {
        version,
        tag: release.tag_name,
        url: release.html_url,
    })
}

/// Asks GitHub for the latest published release. `app` names the asking app and its version for
/// the `User-Agent` header GitHub requires (`indicatrix-cut/0.7.2`).
///
/// # Errors
///
/// See [`CheckError`].
pub fn fetch_latest(app: &str, timeout: Duration) -> Result<Release, CheckError> {
    let config = ureq::Agent::config_builder()
        .timeout_global(Some(timeout))
        .user_agent(app)
        .build();
    let agent = ureq::Agent::from(config);
    let mut response = agent
        .get(LATEST_RELEASE_URL)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .call()
        .map_err(|error| match error {
            ureq::Error::StatusCode(404) => CheckError::NoRelease,
            ureq::Error::StatusCode(403 | 429) => CheckError::RateLimited,
            ureq::Error::StatusCode(code) => CheckError::BadResponse(format!("HTTP status {code}")),
            other => CheckError::Network(other.to_string()),
        })?;
    let body = response
        .body_mut()
        .with_config()
        .limit(MAX_BODY_BYTES)
        .read_to_string()
        .map_err(|e| CheckError::Network(e.to_string()))?;
    parse_release(&body)
}

/// The whole check: the latest release when it is newer than `current_version` (the app's
/// `CARGO_PKG_VERSION`), `Ok(None)` when the app is up to date.
///
/// # Errors
///
/// See [`CheckError`]; also [`CheckError::BadResponse`] when `current_version` itself does not
/// parse (a build-configuration error, reported rather than hidden).
pub fn check(
    app_name: &str,
    current_version: &str,
    timeout: Duration,
) -> Result<Option<Release>, CheckError> {
    let current = parse_version(current_version).ok_or_else(|| {
        CheckError::BadResponse(format!("own version '{current_version}' is not semantic"))
    })?;
    let latest = fetch_latest(&format!("{app_name}/{current_version}"), timeout)?;
    Ok(newer(&current, latest))
}

/// The flag that skips a command-line tool's check (accepted anywhere, removed before parsing).
pub const NO_UPDATE_CHECK_FLAG: &str = "--no-update-check";
/// The environment variable that skips a command-line tool's check when set to anything.
pub const NO_UPDATE_CHECK_ENV: &str = "INDICATRIX_NO_UPDATE_CHECK";

/// For a command-line tool: removes every [`NO_UPDATE_CHECK_FLAG`] from `args`.
///
/// Says whether the check should run, i.e. neither the flag was given nor `env_value` (the value of
/// [`NO_UPDATE_CHECK_ENV`], `std::env::var_os`) is set.
pub fn take_update_check(args: &mut Vec<String>, env_value: Option<&std::ffi::OsStr>) -> bool {
    let before = args.len();
    args.retain(|arg| arg != NO_UPDATE_CHECK_FLAG);
    args.len() == before && env_value.is_none()
}

/// For a command-line tool: the notice line when a newer release exists.
///
/// Asked with [`CLI_TIMEOUT`]; `None` when up to date or when GitHub cannot be asked (a tool says nothing
/// then). `app_name` is the tool's name, `current_version` its `CARGO_PKG_VERSION`.
#[must_use]
pub fn cli_notice(app_name: &str, current_version: &str) -> Option<String> {
    check(app_name, current_version, CLI_TIMEOUT)
        .ok()
        .flatten()
        .map(|release| notice_line(&release, current_version))
}

/// The one-line notice of a newer release, the same in every app:
/// `Indicatrix 0.7.3 is available (this is 0.7.2): <url>`.
#[must_use]
pub fn notice_line(release: &Release, current_version: &str) -> String {
    format!(
        "Indicatrix {} is available (this is {current_version}): {}",
        release.version, release.url
    )
}

#[cfg(test)]
mod tests;
