//! The new-version check: asks GitHub (through `indicatrix-update`) whether a newer release
//! exists and tells the user, after the user agreed once.
//!
//! The Slint side is `ui/models/update.slint` (`UpdateModel`), the question in `app.slint`, the
//! notice `ui/components/update_banner.slint` and the Preferences "Updates" section. This module
//! is the Rust half:
//!
//! - [`setup_update_check`] seeds the model from the saved [`UpdatePreferences`], wires every
//!   `UpdateModel` callback and starts the automatic check when one is due;
//! - the network request runs on its own thread ([`start_check`]) and its answer comes back
//!   through `upgrade_in_event_loop`; nothing here blocks the UI thread, and no request is made
//!   before the user said yes;
//! - what an answer means (notice, status line, silence) is the pure [`presentation`].

use crate::{MainWindow, UpdateModel, gui::show_toast, settings::SettingsPersister};
use indicatrix_update::{CheckError, GUI_TIMEOUT, Release, UpdatePreferences};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{info, warn};

/// The name the request announces itself with (`indicatrix-cut/0.7.2` in the `User-Agent`).
const APP_NAME: &str = "indicatrix-cut";
/// This app's own version.
const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");

thread_local! {
    /// The release the notice on screen announces (UI thread only, like every Slint handle).
    static ANNOUNCED: RefCell<Option<Release>> = const { RefCell::new(None) };
}

/// What an answer of the check shows.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Presentation {
    /// Nothing (an automatic check that found nothing to announce, or failed).
    Nothing,
    /// The notice under the header, with its text.
    Notice(String, Release),
    /// A line under "Check Now" only (up to date, or the failure).
    Status(String),
}

/// The notice text of a newer release.
fn notice_text(release: &Release, current: &str) -> String {
    format!(
        "Indicatrix {} is available (you have {current}).",
        release.version
    )
}

/// What `result` shows. `manual` is "Check Now": it reports every outcome and announces a
/// skipped version again; an automatic check stays silent about failures and about the
/// version the user skipped.
fn presentation(
    prefs: &UpdatePreferences,
    manual: bool,
    current: &str,
    result: Result<Option<Release>, CheckError>,
) -> Presentation {
    match result {
        Ok(Some(release)) if manual || prefs.should_notify(&release) => {
            Presentation::Notice(notice_text(&release, current), release)
        }
        Ok(None) if manual => Presentation::Status(format!("Indicatrix {current} is up to date.")),
        Ok(Some(_) | None) => Presentation::Nothing,
        Err(error) if manual => Presentation::Status(capitalized(&error.to_string())),
        Err(error) => {
            warn!("The automatic check for a new version failed: {error}");
            Presentation::Nothing
        }
    }
}

/// `text` with its first letter in capitals (the error texts start in lower case).
fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect::<String>() + "."
    })
}

/// The preferences after the user answered the question.
const fn answered(prefs: &mut UpdatePreferences, check_daily: bool) {
    prefs.enabled = Some(check_daily);
}

/// The preferences after the user skipped `release`.
fn skipped(prefs: &mut UpdatePreferences, release: &Release) {
    prefs.skipped_version = Some(release.version.to_string());
}

/// Seconds since the Unix epoch (0 if the clock is before it).
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The saved update preferences.
fn saved(store: &SettingsPersister) -> UpdatePreferences {
    store.snapshot().settings.update_check
}

/// Seeds the model, wires the callbacks and starts the automatic check when it is due.
/// Called once from `build_main_window`, after the settings are loaded.
pub(super) fn setup_update_check(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let prefs = saved(settings_store);
    let model = ui.global::<UpdateModel>();
    model.set_consent_pending(prefs.needs_consent());
    model.set_check_enabled(prefs.enabled == Some(true));

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_consent_answered(move |check_daily: bool| {
        store.update(|s| answered(&mut s.settings.update_check, check_daily));
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<UpdateModel>();
        model.set_consent_pending(false);
        model.set_check_enabled(check_daily);
        if check_daily {
            start_check(&ui, false);
        }
    });

    let ui_weak = ui.as_weak();
    model.on_consent_dismissed(move || {
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<UpdateModel>().set_consent_pending(false);
        }
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_enabled_changed(move |on: bool| {
        store.update(|s| answered(&mut s.settings.update_check, on));
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        ui.global::<UpdateModel>().set_consent_pending(false);
        if on && saved(&store).is_due(unix_now()) {
            start_check(&ui, false);
        }
    });

    let ui_weak = ui.as_weak();
    model.on_check_now(move || {
        if let Some(ui) = ui_weak.upgrade() {
            start_check(&ui, true);
        }
    });

    let ui_weak = ui.as_weak();
    model.on_view_release(move || {
        let url = ANNOUNCED.with_borrow(|release| release.as_ref().map(|r| r.url.clone()));
        let (Some(url), Some(ui)) = (url, ui_weak.upgrade()) else {
            return;
        };
        if let Err(message) = super::external_links::open_external_url(&url) {
            warn!("{message}");
            show_toast(&ui, &message, "error");
        }
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    model.on_skip_release(move || {
        if let Some(release) = ANNOUNCED.take() {
            store.update(|s| skipped(&mut s.settings.update_check, &release));
        }
        hide_notice(&ui_weak);
    });

    let ui_weak = ui.as_weak();
    model.on_later(move || hide_notice(&ui_weak));

    if prefs.is_due(unix_now()) {
        start_check(ui, false);
    }
}

/// Hides the notice (the release stays remembered only while it is on screen).
fn hide_notice(ui_weak: &slint::Weak<MainWindow>) {
    ANNOUNCED.set(None);
    if let Some(ui) = ui_weak.upgrade() {
        ui.global::<UpdateModel>().set_notice_visible(false);
    }
}

/// Starts a check on a background thread. `manual` is "Check Now". A check that is already
/// running is not doubled.
fn start_check(ui: &MainWindow, manual: bool) {
    let model = ui.global::<UpdateModel>();
    if model.get_checking() {
        return;
    }
    model.set_checking(true);
    if manual {
        model.set_status_text("Checking...".into());
    }
    let ui_weak = ui.as_weak();
    let spawned = std::thread::Builder::new()
        .name("update-check".to_owned())
        .spawn(move || {
            let result = indicatrix_update::check(APP_NAME, CURRENT_VERSION, GUI_TIMEOUT);
            let now = unix_now();
            // The closure's failure to run (the window is gone) needs no handling.
            let _ = ui_weak.upgrade_in_event_loop(move |ui| finish_check(&ui, manual, now, result));
        });
    if let Err(error) = spawned {
        warn!("The check for a new version could not start: {error}");
        model.set_checking(false);
    }
}

/// Shows the answer of a finished check (UI thread) and records that it ran.
fn finish_check(
    ui: &MainWindow,
    manual: bool,
    now: u64,
    result: Result<Option<Release>, CheckError>,
) {
    let model = ui.global::<UpdateModel>();
    model.set_checking(false);
    let prefs = SettingsPersister::installed_for_this_thread().map_or_else(
        UpdatePreferences::default,
        |store| {
            store.update(|s| s.settings.update_check.last_check_unix = now);
            saved(&store)
        },
    );
    match presentation(&prefs, manual, CURRENT_VERSION, result) {
        Presentation::Nothing => {
            if manual {
                model.set_status_text(String::new().into());
            }
        }
        Presentation::Status(text) => model.set_status_text(text.into()),
        Presentation::Notice(text, release) => {
            info!("A newer version is available: {}", release.version);
            if manual {
                model.set_status_text(text.clone().into());
            }
            model.set_notice_text(text.into());
            model.set_notice_visible(true);
            ANNOUNCED.set(Some(release));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_update::{RELEASE_PAGE_PREFIX, parse_version};

    fn release(version: &str) -> Release {
        Release {
            version: parse_version(version).expect("a version"),
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
    fn a_newer_release_is_announced_with_both_versions() {
        let shown = presentation(&enabled(), false, "0.7.2", Ok(Some(release("0.7.3"))));
        assert_eq!(
            shown,
            Presentation::Notice(
                "Indicatrix 0.7.3 is available (you have 0.7.2).".to_owned(),
                release("0.7.3")
            )
        );
    }

    #[test]
    fn the_skipped_version_is_silent_automatically_but_not_for_check_now() {
        let mut prefs = enabled();
        skipped(&mut prefs, &release("0.7.3"));
        assert_eq!(prefs.skipped_version.as_deref(), Some("0.7.3"));
        let found = Ok(Some(release("0.7.3")));
        assert_eq!(
            presentation(&prefs, false, "0.7.2", found.clone()),
            Presentation::Nothing
        );
        assert!(matches!(
            presentation(&prefs, true, "0.7.2", found),
            Presentation::Notice(..)
        ));
        // A still newer version is announced again.
        assert!(matches!(
            presentation(&prefs, false, "0.7.2", Ok(Some(release("0.7.4")))),
            Presentation::Notice(..)
        ));
    }

    #[test]
    fn up_to_date_is_reported_only_by_check_now() {
        assert_eq!(
            presentation(&enabled(), true, "0.7.2", Ok(None)),
            Presentation::Status("Indicatrix 0.7.2 is up to date.".to_owned())
        );
        assert_eq!(
            presentation(&enabled(), false, "0.7.2", Ok(None)),
            Presentation::Nothing
        );
    }

    #[test]
    fn a_failure_is_reported_only_by_check_now() {
        let failed = Err(CheckError::RateLimited);
        assert_eq!(
            presentation(&enabled(), false, "0.7.2", failed.clone()),
            Presentation::Nothing
        );
        assert_eq!(
            presentation(&enabled(), true, "0.7.2", failed),
            Presentation::Status("GitHub's request limit is used up; try again later.".to_owned())
        );
    }

    #[test]
    fn the_answer_to_the_question_is_stored_and_ends_the_asking() {
        let mut prefs = UpdatePreferences::default();
        assert!(prefs.needs_consent());
        answered(&mut prefs, false);
        assert!(!prefs.needs_consent());
        assert!(!prefs.is_due(u64::MAX), "Don't Check never checks");
        answered(&mut prefs, true);
        assert!(prefs.is_due(unix_now()));
    }
}
