//! The new-version check of the GUI.
//!
//! The user is asked once ("Check Daily" / "Don't Check", shown while the window is idle); the
//! answer lives in the settings. With the check enabled the GUI asks GitHub for the latest release
//! at startup when a day has passed, and "Check Now" asks at once. The request runs on its own
//! thread and reports through the background-message channel the timer drains, so the UI never
//! waits for the network. Nothing is downloaded: a newer release is announced in a banner whose
//! "View Release" button opens the release page in the browser.
//!
//! What a result means for the screen is a pure function ([`evaluate`]); the `Gui` methods only
//! apply it.

use std::{
    process::Command,
    time::{SystemTime, UNIX_EPOCH},
};

use indicatrix_update::{
    CheckError, GUI_TIMEOUT, RELEASE_PAGE_PREFIX, Release, UpdatePreferences, check,
};

use super::{Gui, UiMessage};

/// The name the check sends in its `User-Agent` header.
const APP_NAME: &str = "indicatrix-trainer";
/// The running version.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
/// The status line while a check runs.
const CHECKING: &str = "Checking…";

/// What a finished check changes on screen.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Evaluation {
    /// The release to announce in the banner.
    pub notice: Option<Release>,
    /// Hide the banner (the app is up to date).
    pub hide_notice: bool,
    /// The new text of the status line under "Check Now" (`None` keeps it).
    pub status: Option<String>,
    /// A console line.
    pub log: Option<String>,
}

/// The banner text of a newer release.
pub fn notice_text(release: &Release, current: &str) -> String {
    format!(
        "Indicatrix {} is available (you have {current}).",
        release.version
    )
}

/// Turns the outcome of a check into screen changes. An automatic check announces a release only
/// when it was not skipped and says nothing else on screen; "Check Now" reports every outcome,
/// the skipped version included (the user asked).
pub fn evaluate(
    prefs: &UpdatePreferences,
    manual: bool,
    result: Result<Option<Release>, CheckError>,
    current: &str,
) -> Evaluation {
    match result {
        Ok(Some(release)) => {
            let announce = manual || prefs.should_notify(&release);
            let text = notice_text(&release, current);
            Evaluation {
                status: manual.then(|| text.clone()),
                log: Some(if announce {
                    text
                } else {
                    format!("{text} (skipped by choice)")
                }),
                notice: announce.then_some(release),
                hide_notice: false,
            }
        }
        Ok(None) => Evaluation {
            hide_notice: true,
            status: manual.then(|| format!("Indicatrix {current} is up to date.")),
            ..Evaluation::default()
        },
        Err(error) => Evaluation {
            status: manual.then(|| format!("The check failed: {error}.")),
            log: Some(format!("Update check failed: {error}")),
            ..Evaluation::default()
        },
    }
}

/// The program and arguments that open `url` in the default browser.
fn open_command(windows: bool, macos: bool, url: &str) -> (&'static str, Vec<String>) {
    if windows {
        (
            "cmd",
            vec!["/C".into(), "start".into(), String::new(), url.into()],
        )
    } else if macos {
        ("open", vec![url.into()])
    } else {
        ("xdg-open", vec![url.into()])
    }
}

/// Opens a release page in the browser. The process is started and never waited on. Only links
/// under the project's release pages are opened (the crate validated this one already).
fn open_url(url: &str) -> Result<(), String> {
    if !url.starts_with(RELEASE_PAGE_PREFIX) {
        return Err(format!("refusing to open '{url}'"));
    }
    let (program, args) = open_command(cfg!(windows), cfg!(target_os = "macos"), url);
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW: no console flashes up for the `cmd` helper.
        command.creation_flags(0x0800_0000);
    }
    command.spawn().map(drop).map_err(|e| e.to_string())
}

/// Seconds since the Unix epoch (0 when the clock is before it).
fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

impl Gui {
    /// At startup: shows the switch's state, asks for consent when it was never given, and runs
    /// the daily check when it is due. No request is made before the user said yes.
    pub(super) fn init_update_check(&self) {
        let prefs = self.settings.borrow().update.clone();
        self.with_c(|c| {
            c.set_update_enabled(prefs.enabled == Some(true));
            c.set_update_prompt(prefs.needs_consent());
        });
        if prefs.is_due(now_unix()) {
            self.start_update_check(false);
        }
    }

    /// The answer to the consent question.
    pub(super) fn answer_update_prompt(&self, enabled: bool) {
        self.with_c(|c| c.set_update_prompt(false));
        self.set_update_enabled(enabled);
    }

    /// The settings switch (and the consent answer): remembers the choice; turning it on checks
    /// at once when a check is due.
    pub(super) fn set_update_enabled(&self, enabled: bool) {
        self.update_settings(|s| s.update.enabled = Some(enabled));
        self.with_c(|c| c.set_update_enabled(enabled));
        if enabled && self.settings.borrow().update.is_due(now_unix()) {
            self.start_update_check(false);
        }
    }

    /// "Check Now": asks whatever the daily rule says and reports the outcome.
    pub(super) fn check_updates_now(&self) {
        self.with_c(|c| c.set_update_status(CHECKING.into()));
        if self.update_in_flight.get() {
            // The running check reports as a manual one when it finishes.
            self.update_manual_wanted.set(true);
        } else {
            self.start_update_check(true);
        }
    }

    /// Starts the request on its own thread; one runs at a time.
    fn start_update_check(&self, manual: bool) {
        if self.update_in_flight.replace(true) {
            return;
        }
        let tx = self.msg_tx.clone();
        let spawned = std::thread::Builder::new()
            .name("update-check".into())
            .spawn(move || {
                let result = check(APP_NAME, CURRENT_VERSION, GUI_TIMEOUT);
                let _ = tx.send(UiMessage::UpdateChecked { manual, result });
            });
        if let Err(error) = spawned {
            self.finish_update_check(manual, Err(CheckError::Network(error.to_string())));
        }
    }

    /// A check finished on the UI thread: stores the time, then shows what [`evaluate`] says.
    pub(super) fn finish_update_check(
        &self,
        manual: bool,
        result: Result<Option<Release>, CheckError>,
    ) {
        self.update_in_flight.set(false);
        let manual = manual || self.update_manual_wanted.replace(false);
        let now = now_unix();
        self.update_settings(|s| s.update.last_check_unix = now);
        let prefs = self.settings.borrow().update.clone();
        let evaluation = evaluate(&prefs, manual, result, CURRENT_VERSION);
        if let Some(line) = evaluation.log {
            self.log("INFO", line);
        }
        if let Some(release) = evaluation.notice {
            let text = notice_text(&release, CURRENT_VERSION);
            self.with_c(|c| c.set_update_notice(text.into()));
            *self.update_release.borrow_mut() = Some(release);
        } else if evaluation.hide_notice {
            self.hide_update_notice();
        }
        // Only "Check Now" shows the status line, and it always has a text to show.
        if let Some(status) = evaluation.status {
            self.with_c(|c| c.set_update_status(status.into()));
        }
    }

    /// "View Release": opens the release page.
    pub(super) fn open_release(&self) {
        let url = self.update_release.borrow().as_ref().map(|r| r.url.clone());
        if let Some(url) = url
            && let Err(error) = open_url(&url)
        {
            self.log("WARN", format!("Could not open the release page: {error}"));
        }
    }

    /// "Skip This Version": the release is not announced again.
    pub(super) fn skip_release(&self) {
        let version = self
            .update_release
            .borrow()
            .as_ref()
            .map(|r| r.version.to_string());
        if let Some(version) = version {
            self.update_settings(|s| s.update.skipped_version = Some(version));
        }
        self.hide_update_notice();
    }

    /// "Later": hides the banner until the next check finds the release again.
    pub(super) fn dismiss_release(&self) {
        self.hide_update_notice();
    }

    fn hide_update_notice(&self) {
        *self.update_release.borrow_mut() = None;
        self.with_c(|c| c.set_update_notice("".into()));
    }
}

#[cfg(test)]
mod tests {
    use indicatrix_update::parse_version;

    use super::*;

    fn release(version: &str) -> Release {
        Release {
            version: parse_version(version).expect("version"),
            tag: format!("v{version}"),
            url: format!("{RELEASE_PAGE_PREFIX}tag/v{version}"),
        }
    }

    fn enabled() -> UpdatePreferences {
        UpdatePreferences {
            enabled: Some(true),
            ..UpdatePreferences::default()
        }
    }

    #[test]
    fn the_notice_names_both_versions() {
        assert_eq!(
            notice_text(&release("0.7.3"), "0.7.2"),
            "Indicatrix 0.7.3 is available (you have 0.7.2)."
        );
    }

    #[test]
    fn an_automatic_check_announces_a_newer_release_and_says_nothing_else() {
        let eval = evaluate(&enabled(), false, Ok(Some(release("0.7.3"))), "0.7.2");
        assert_eq!(eval.notice, Some(release("0.7.3")));
        assert_eq!(eval.status, None);
        assert!(!eval.hide_notice);
    }

    #[test]
    fn a_skipped_version_is_not_announced_automatically_but_is_by_check_now() {
        let prefs = UpdatePreferences {
            skipped_version: Some("0.7.3".into()),
            ..enabled()
        };
        let auto = evaluate(&prefs, false, Ok(Some(release("0.7.3"))), "0.7.2");
        assert_eq!(auto.notice, None);
        assert!(auto.log.is_some_and(|l| l.contains("skipped")));
        let manual = evaluate(&prefs, true, Ok(Some(release("0.7.3"))), "0.7.2");
        assert_eq!(manual.notice, Some(release("0.7.3")));
        // A still newer release is announced even though 0.7.3 was skipped.
        let newer = evaluate(&prefs, false, Ok(Some(release("0.7.4"))), "0.7.2");
        assert_eq!(newer.notice, Some(release("0.7.4")));
    }

    #[test]
    fn up_to_date_is_silent_automatically_and_reported_by_check_now() {
        let auto = evaluate(&enabled(), false, Ok(None), "0.7.2");
        assert_eq!(auto.status, None);
        assert_eq!(auto.log, None);
        assert!(auto.hide_notice);
        let manual = evaluate(&enabled(), true, Ok(None), "0.7.2");
        assert_eq!(
            manual.status.as_deref(),
            Some("Indicatrix 0.7.2 is up to date.")
        );
    }

    #[test]
    fn a_failure_is_a_log_line_automatically_and_the_error_text_for_check_now() {
        let error = CheckError::RateLimited;
        let auto = evaluate(&enabled(), false, Err(error.clone()), "0.7.2");
        assert_eq!(auto.status, None);
        assert_eq!(auto.notice, None);
        assert!(auto.log.is_some_and(|l| l.contains("request limit")));
        let manual = evaluate(&enabled(), true, Err(error), "0.7.2");
        assert!(manual.status.is_some_and(|s| s.contains("request limit")));
        assert!(!manual.hide_notice);
    }

    #[test]
    fn the_browser_command_follows_the_platform() {
        let url = "https://github.com/suitable-name/indicatrix/releases/tag/v0.7.3";
        let (program, args) = open_command(true, false, url);
        assert_eq!(program, "cmd");
        assert_eq!(args, ["/C", "start", "", url]);
        assert_eq!(
            open_command(false, true, url),
            ("open", vec![url.to_owned()])
        );
        assert_eq!(
            open_command(false, false, url),
            ("xdg-open", vec![url.to_owned()])
        );
    }

    #[test]
    fn only_release_pages_are_opened() {
        assert!(open_url("https://example.com/").is_err());
        assert!(open_url("file:///C:/Windows/System32/calc.exe").is_err());
    }
}
