//! What a scenario receives: the running app, the profile and the report, plus the common
//! operations (sweep, click-everything, health check).
//!
//! This is Indicatrix's own context (the crate's `Ctx` knows nothing of `HarnessApp`); `main.rs`
//! builds it per scenario from the crate context's report and profile.

use super::{
    Control, SweepOptions,
    app::{DEFAULT_SIZE, HarnessApp, MINIMUM_SIZE, pump, wait_until},
    fixtures::{self, FixtureDesign},
    profile::{Profile, ProfileExt, SettingsSeed},
    query::WindowView,
    smoke::{self, SmokeHost, SmokeStats, indicatrix_smoke},
    sweep, sweep_options,
};
use indicatrix_cut::{LibraryModel, harness_support};
use slint::{ComponentHandle, Model};
use std::{path::PathBuf, time::Duration};
use uiprobe::{backend::wait_until_labeled, report::Report, smoke::Smoke, timing};

/// The window sizes every state is swept at: a common laptop and the declared minimum.
pub const SWEEP_SIZES: [(u32, u32); 2] = [DEFAULT_SIZE, MINIMUM_SIZE];

thread_local! {
    /// The error or warning toast `check_health` reported last: its generation and message.
    static REPORTED_TOAST: std::cell::RefCell<Option<(i32, String)>> =
        const { std::cell::RefCell::new(None) };
}

/// Everything a scenario may touch.
pub struct Ctx<'a> {
    /// The running app.
    pub app: &'a mut HarnessApp,
    /// The throw-away profile (scratch directory, settings seeding).
    pub profile: &'a Profile,
    /// Where findings go.
    pub report: &'a mut Report,
    /// The settings the scenario started with; used when the click-everything engine relaunches.
    pub seed: SettingsSeed,
}

impl Ctx<'_> {
    /// Sweeps one window view and records the findings, tagged with `label` (the UI state).
    pub fn sweep_view(&mut self, view: &WindowView, label: &str) {
        let findings = sweep(view, &sweep_options());
        let size = format!(
            "{} {:.0}x{:.0} {label}",
            view.name, view.size.0, view.size.1
        );
        self.report.add_all(findings, &size);
    }

    /// Sweeps the main window in its current state at every size in [`SWEEP_SIZES`], then
    /// restores the default size.
    pub fn sweep_main_at_sizes(&mut self, label: &str) {
        for (width, height) in SWEEP_SIZES {
            self.app.set_size(width, height);
            pump(50);
            let view = self.app.main_view();
            self.sweep_view(&view, label);
        }
        self.app.set_size(DEFAULT_SIZE.0, DEFAULT_SIZE.1);
    }

    /// Records panics and unexpected file dialogs that happened since the last check, and any
    /// error toast showing. `label` names the step.
    pub fn check_health(&mut self, label: &str) {
        for text in smoke::take_panics() {
            self.report
                .add_simple("-", "panic", format!("{label}: {text}"));
        }
        for text in harness_support::take_picker_failures() {
            self.report
                .add_simple("-", "unexpected-file-dialog", format!("{label}: {text}"));
        }
        let ui = self.app.ui();
        if ui.get_toast_visible() {
            let kind = ui.get_toast_type().to_string();
            if kind == "error" || kind == "warning" {
                let message = ui.get_toast_message().to_string();
                // Error and warning toasts stay until dismissed: one toast is one finding, not
                // one per later check. `show_toast` bumps the generation on every toast.
                let shown = (ui.get_toast_generation(), message.clone());
                if REPORTED_TOAST.with_borrow(|last| last.as_ref() == Some(&shown)) {
                    return;
                }
                REPORTED_TOAST.set(Some(shown));
                self.report.add_simple(
                    "MainWindow",
                    "error-toast",
                    format!("{label}: {kind}: {message}"),
                );
            }
        }
    }

    /// Records a finding `check`/`detail` when `condition` is false; returns `condition`.
    pub fn expect(&mut self, condition: bool, check: &str, detail: impl Into<String>) -> bool {
        if !condition {
            self.report.add_simple("-", check, detail);
        }
        condition
    }

    /// Pumps until `condition` holds or `timeout` (real time) passes; a timeout is a finding named
    /// `label`.
    pub fn wait_for(
        &mut self,
        label: &str,
        timeout: Duration,
        condition: impl FnMut() -> bool,
    ) -> bool {
        let held = wait_until(timeout, condition);
        self.expect(
            held,
            "timeout",
            format!("{label}: not reached within {} ms", timeout.as_millis()),
        )
    }

    /// A path inside the profile's scratch directory for a file the scenario writes.
    #[must_use]
    pub fn scratch_file(&self, name: &str) -> PathBuf {
        self.profile.scratch_dir().join(name)
    }

    /// Seeds the library with `designs`, relaunches the app with the scenario's settings seed,
    /// and waits (real time, up to 10 s) until the library list shows them; a timeout is a
    /// finding. Returns whether the app is up with the designs listed.
    pub fn relaunch_with_library(&mut self, designs: &[FixtureDesign]) -> bool {
        timing::time("relaunch", || self.relaunch_with_library_inner(designs))
    }

    fn relaunch_with_library_inner(&mut self, designs: &[FixtureDesign]) -> bool {
        self.app.shutdown();
        self.profile.wipe_app_data();
        self.profile.seed_settings(&self.seed);
        harness_support::clear_picker_answers();
        if let Err(e) = fixtures::seed_library(self.profile, designs) {
            self.report
                .add_simple("-", "launch", format!("cannot seed the library: {e}"));
            return false;
        }
        match HarnessApp::launch() {
            Ok(app) => *self.app = app,
            Err(e) => {
                self.report
                    .add_simple("-", "launch", format!("cannot start with a library: {e}"));
                return false;
            }
        }
        let wanted = designs.len();
        let rows = |app: &HarnessApp| {
            app.ui()
                .global::<LibraryModel>()
                .get_diagram_list()
                .row_count()
        };
        let mut listed = {
            let app = &*self.app;
            wait_until_labeled("library-load", Duration::from_secs(10), || {
                rows(app) >= wanted
            })
        };
        if !listed {
            // The start-up load did not deliver (its result is dropped when another search
            // bumped the sequence meanwhile). Say so, then ask for the list the way a filter
            // change does, so the scenario that needs the designs can still run.
            let library = self.app.ui().global::<LibraryModel>();
            let detail = format!(
                "status {:?}, total_count {}, matched_count {}, search {:?}, my designs only {}",
                library.get_status_message().to_string(),
                library.get_total_count(),
                library.get_matched_count(),
                library.get_search_text().to_string(),
                library.get_local_only_filter()
            );
            self.report.add_simple(
                "-",
                "library-initial-load",
                format!(
                    "the start-up library load listed {} of {wanted} seeded design(s) within 10 s \
                     ({detail}); retrying with a filter refresh",
                    rows(&*self.app)
                ),
            );
            library.invoke_filters_changed();
            let app = &*self.app;
            listed = wait_until_labeled("library-load", Duration::from_secs(10), || {
                rows(app) >= wanted
            });
        }
        // The list rows are instantiated by the next pump after the model changed; the
        // queries of the scenarios (`ListItem` elements) need them.
        crate::support::app::pump(100);
        let shown = rows(&*self.app);
        self.expect(
            listed,
            "library-not-listed",
            format!("the library list shows {shown} of {wanted} seeded design(s) after 20 s"),
        )
    }

    /// Runs the click-everything pass over the state `prepare` reaches. `prepare` runs after every
    /// relaunch (the app is fresh, with this scenario's settings seed) and must navigate to the
    /// state; for the startup state it does nothing.
    pub fn smoke_main(
        &mut self,
        label: &str,
        prepare: &mut dyn FnMut(&mut HarnessApp),
    ) -> SmokeStats {
        let smoke = indicatrix_smoke(label);
        self.smoke_main_with(&smoke, prepare)
    }

    /// Like [`Ctx::smoke_main`] with explicit settings (extra deny rules, allowed toasts ...).
    pub fn smoke_main_with(
        &mut self,
        smoke: &Smoke,
        prepare: &mut dyn FnMut(&mut HarnessApp),
    ) -> SmokeStats {
        let mut host = MainHost {
            app: &mut *self.app,
            profile: self.profile,
            seed: &self.seed,
            report: &mut *self.report,
            prepare,
        };
        let stats = smoke::run(&mut host, smoke);
        println!(
            "uiprobe:   smoke {}: {} control(s), {} activated, {} denied, {} gone, {} restore(s)",
            smoke.state,
            stats.enumerated,
            stats.activated,
            stats.denied,
            stats.gone,
            stats.restores
        );
        stats
    }
}

/// [`SmokeHost`] for the main window: restore = relaunch + `prepare`.
struct MainHost<'a> {
    app: &'a mut HarnessApp,
    profile: &'a Profile,
    seed: &'a SettingsSeed,
    report: &'a mut Report,
    prepare: &'a mut dyn FnMut(&mut HarnessApp),
}

impl SmokeHost for MainHost<'_> {
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        self.app.relaunch(self.profile, self.seed)?;
        (self.prepare)(self.app);
        Ok(self.views())
    }

    fn views(&self) -> Vec<WindowView> {
        vec![self.app.main_view()]
    }

    fn notice(&mut self) -> Option<(String, String)> {
        let ui = self.app.ui();
        ui.get_toast_visible().then(|| {
            (
                ui.get_toast_type().to_string(),
                ui.get_toast_message().to_string(),
            )
        })
    }

    fn clear_notice(&mut self) {
        self.app.ui().set_toast_visible(false);
    }

    fn main_alive(&self) -> bool {
        self.app.is_open()
    }

    fn report(&mut self) -> &mut Report {
        &mut *self.report
    }

    fn sweep_options(&self) -> SweepOptions {
        sweep_options()
    }

    fn admits(&self, control: &Control) -> bool {
        super::part::admits(control)
    }
}
