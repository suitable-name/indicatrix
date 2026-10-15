//! Harness operations on the secondary windows (compare, planner, locate, rig, rough color,
//! help) and on key chords sent to the main window: more `impl Ctx` blocks, kept apart from
//! `ctx.rs` for size.

use super::{
    Control, SweepOptions,
    app::{HarnessApp, pump},
    ctx::Ctx,
    profile::{Profile, SettingsSeed},
    query::WindowView,
    smoke::{self, DenyMatch, DenyRule, SmokeHost, SmokeStats, indicatrix_smoke},
    sweep_options,
};
use indicatrix_cut::harness_support::{self, SecondaryWindow};
use slint::ComponentHandle;
use slint_harness::report::Report;

/// The window's declared `preferred-width/height` (what a user sees first), in logical pixels.
const fn default_size(which: SecondaryWindow) -> (u32, u32) {
    match which {
        SecondaryWindow::Compare => (1100, 780),
        SecondaryWindow::RoughPlanner => (1280, 820),
        SecondaryWindow::Locate => (1260, 820),
        SecondaryWindow::Rig => (1200, 780),
        SecondaryWindow::RoughColor => (1320, 860),
        SecondaryWindow::Help => (1000, 680),
    }
}

/// The window's declared `min-width/min-height`.
const fn minimum_size(which: SecondaryWindow) -> (u32, u32) {
    match which {
        SecondaryWindow::Compare => (640, 480),
        SecondaryWindow::RoughPlanner => (1040, 560),
        SecondaryWindow::Locate => (960, 560),
        SecondaryWindow::Rig => (900, 540),
        SecondaryWindow::RoughColor => (1000, 600),
        SecondaryWindow::Help => (640, 420),
    }
}

/// The name used in findings.
const fn view_name(which: SecondaryWindow) -> &'static str {
    match which {
        SecondaryWindow::Compare => "CompareWindow",
        SecondaryWindow::RoughPlanner => "RoughPlannerWindow",
        SecondaryWindow::Locate => "LocateWindow",
        SecondaryWindow::Rig => "RigWindow",
        SecondaryWindow::RoughColor => "RoughColourWindow",
        SecondaryWindow::Help => "HelpWindow",
    }
}

/// The view of `which` when it is open.
fn open_view(which: SecondaryWindow) -> Option<WindowView> {
    let name = view_name(which);
    match which {
        SecondaryWindow::Compare => {
            harness_support::compare_window().map(|w| WindowView::of(name, &w))
        }
        SecondaryWindow::RoughPlanner => {
            harness_support::rough_planner_window().map(|w| WindowView::of(name, &w))
        }
        SecondaryWindow::Locate => {
            harness_support::locate_window().map(|w| WindowView::of(name, &w))
        }
        SecondaryWindow::Rig => harness_support::rig_window().map(|w| WindowView::of(name, &w)),
        SecondaryWindow::RoughColor => {
            harness_support::rough_color_window().map(|w| WindowView::of(name, &w))
        }
        SecondaryWindow::Help => harness_support::help_window().map(|w| WindowView::of(name, &w)),
    }
}

/// Resizes `which` (logical pixels) and lets its layout settle; `false` when it is not open.
fn resize(which: SecondaryWindow, (width, height): (u32, u32)) -> bool {
    let size = slint::LogicalSize::new(width as f32, height as f32);
    macro_rules! set {
        ($window:expr) => {
            match $window {
                Some(w) => {
                    w.window().set_size(size);
                    true
                }
                None => false,
            }
        };
    }
    let done = match which {
        SecondaryWindow::Compare => set!(harness_support::compare_window()),
        SecondaryWindow::RoughPlanner => set!(harness_support::rough_planner_window()),
        SecondaryWindow::Locate => set!(harness_support::locate_window()),
        SecondaryWindow::Rig => set!(harness_support::rig_window()),
        SecondaryWindow::RoughColor => set!(harness_support::rough_color_window()),
        SecondaryWindow::Help => set!(harness_support::help_window()),
    };
    pump(50);
    done
}

impl Ctx<'_> {
    /// Sends a key chord to the main window, e.g. "Ctrl+S", "F5", "Escape", "?", "Ctrl+Shift+P",
    /// "Ctrl+Alt+2" (Slint `KeyPressed`/`KeyReleased` events, modifiers pressed first and released
    /// last), then pumps 50 ms. An unparsable chord is a `bad-chord` finding and sends nothing.
    pub fn press(&mut self, chord: &str) {
        let window = self.app.ui().window();
        if let Err(finding) = slint_harness::view::press(window, chord) {
            self.report.add(finding);
        }
    }

    /// The view of an open secondary window, or `None` (no finding) when it is not open.
    #[must_use]
    #[expect(
        clippy::unused_self,
        reason = "method form keeps scenario call sites uniform with the other Ctx queries"
    )]
    pub fn secondary_view(&self, which: SecondaryWindow) -> Option<WindowView> {
        open_view(which)
    }

    /// Sweeps an open secondary window at its default size and its minimum size; a closed window
    /// is a finding. `label` names the UI state.
    pub fn sweep_secondary(&mut self, which: SecondaryWindow, label: &str) {
        if !harness_support::is_window_open(which) {
            self.report.add_simple(
                view_name(which),
                "window-not-open",
                format!("{label}: the window is not open, so it cannot be swept"),
            );
            return;
        }
        for size in [default_size(which), minimum_size(which)] {
            resize(which, size);
            if let Some(view) = open_view(which) {
                self.sweep_view(&view, label);
            }
        }
        resize(which, default_size(which));
    }

    /// Click-everything over a secondary window: `prepare` runs after every relaunch and must
    /// open the window (and reach the state); restore = relaunch + `prepare`, as `smoke_main`
    /// does.
    pub fn smoke_secondary(
        &mut self,
        which: SecondaryWindow,
        label: &str,
        prepare: &mut dyn FnMut(&mut HarnessApp),
    ) -> SmokeStats {
        self.smoke_secondary_inner(which, label, true, prepare)
    }

    /// [`Self::smoke_secondary`] for a `prepare` that shuts the app down and launches it again
    /// itself: restore skips its own relaunch, which would launch the app twice per restore.
    pub fn smoke_secondary_launching(
        &mut self,
        which: SecondaryWindow,
        label: &str,
        prepare: &mut dyn FnMut(&mut HarnessApp),
    ) -> SmokeStats {
        self.smoke_secondary_inner(which, label, false, prepare)
    }

    fn smoke_secondary_inner(
        &mut self,
        which: SecondaryWindow,
        label: &str,
        relaunch: bool,
        prepare: &mut dyn FnMut(&mut HarnessApp),
    ) -> SmokeStats {
        let mut config = indicatrix_smoke(format!("{} {label}", view_name(which)));
        config.extra_deny = SECONDARY_DENY.to_vec();
        let mut host = SecondaryHost {
            which,
            relaunch,
            app: &mut *self.app,
            profile: self.profile,
            seed: &self.seed,
            report: &mut *self.report,
            prepare,
        };
        let stats = smoke::run(&mut host, &config);
        println!(
            "slint-harness:   smoke {}: {} control(s), {} activated, {} denied, {} gone, {} restore(s)",
            config.state,
            stats.enumerated,
            stats.activated,
            stats.denied,
            stats.gone,
            stats.restores
        );
        stats
    }
}

/// Extra deny rules of the secondary-window smoke: the window is closed by its own scenarios,
/// so a click on its close button would only make the engine see a vanished window.
const SECONDARY_DENY: &[DenyRule] = &[
    DenyRule::label(DenyMatch::Exact, "Close", "closes the window under test"),
    DenyRule::label(
        DenyMatch::Exact,
        "Close Window",
        "closes the window under test",
    ),
];

/// [`SmokeHost`] for one secondary window: restore = relaunch + `prepare`; the window must be
/// open afterwards.
struct SecondaryHost<'a> {
    which: SecondaryWindow,
    relaunch: bool,
    app: &'a mut HarnessApp,
    profile: &'a Profile,
    seed: &'a SettingsSeed,
    report: &'a mut Report,
    prepare: &'a mut dyn FnMut(&mut HarnessApp),
}

impl SmokeHost for SecondaryHost<'_> {
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        if self.relaunch {
            self.app.relaunch(self.profile, self.seed)?;
        }
        (self.prepare)(self.app);
        pump(50);
        match open_view(self.which) {
            Some(view) => Ok(vec![view]),
            None => Err(format!(
                "{} is not open after the scenario's prepare step",
                view_name(self.which)
            )),
        }
    }

    fn views(&self) -> Vec<WindowView> {
        open_view(self.which).into_iter().collect()
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
