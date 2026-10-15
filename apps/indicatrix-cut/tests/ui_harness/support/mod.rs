//! Indicatrix glue around the published `slint-harness` crate: launch and shutdown of the real
//! `MainWindow`, the throw-away profile's settings seeding, fixtures, the secondary-window
//! operations and the click-everything hosts. The engine itself (element queries, the
//! invariant sweep, the smoke pass, coverage, key chords, the report) lives in `slint-harness`
//! and is re-exported here under the paths the scenarios already use.
//!
//! Scenario lanes mostly need [`Ctx`] (what a scenario receives), [`query`] (find a control
//! by label) and [`app::pump`] / [`app::wait_until`].
//!
//! `dead_code` and `unused_imports` are allowed module-wide: this is the helper API that scenario files written
//! later are expected to use, so not every item has a caller in the built-in scenarios.
#![allow(dead_code, unused_imports)]

pub mod app;
pub mod ctx;
pub mod fixtures;
pub mod library_source;
pub mod part;
pub mod profile;
pub mod secondary;
pub mod smoke;

/// The coverage recorder entry points and the gate configuration of the crate.
pub mod coverage {
    pub use slint_harness::coverage::*;
}
/// Element queries and key chords of the crate, under the old path.
pub use slint_harness::keys;
pub use slint_harness::{report, view as query};

pub use app::{HarnessApp, pump, wait_until};
pub use ctx::Ctx;
pub use fixtures::{FixtureDesign, sample_designs, seed_library};
pub use indicatrix_cut::harness_support::SecondaryWindow;
pub use profile::{Profile, ProfileExt, SettingsSeed};
pub use slint_harness::{
    ElementHandle,
    checks::{SweepOptions, sweep},
    report::{Finding, Report},
    view::{Control, WindowView},
};

/// The sweep thresholds of every Indicatrix sweep (the main window, the secondary windows and
/// every scenario sweep): the crate defaults plus the floating layers the UI draws above the
/// content on purpose. A control inside a layer never overlaps a control outside it or in
/// another layer; overlaps inside one layer are still reported. The docked `ParametersPanel`
/// mount shares the type name, so docked controls are exempt against outside controls too
/// (accepted, as before the migration).
#[must_use]
pub fn sweep_options() -> SweepOptions {
    SweepOptions {
        floating_layers: [
            "Toast",
            "PopupWindow",
            "GuideOverlayLayer",
            "ParametersPanel",
        ]
        .map(String::from)
        .to_vec(),
        ..SweepOptions::default()
    }
}

/// `find`-style queries return a `Result<ElementHandle, Finding>` in the crate; the old support
/// code panicked, so a missing control ended the scenario as a `scenario-panic` finding with the
/// near-label hint. `must` keeps that behaviour (never `?`: that would make the finding
/// `not-found` and let the scenario go on).
pub trait Must {
    /// The element, or a panic with the finding's detail.
    fn must(self) -> ElementHandle;
}

impl Must for Result<ElementHandle, Finding> {
    fn must(self) -> ElementHandle {
        match self {
            Ok(element) => element,
            Err(finding) => panic!("{}", finding.detail),
        }
    }
}
