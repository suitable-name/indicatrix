//! The scenario registry.
//!
//! A scenario is a named function that drives the app through one user journey and reports
//! what it finds. The runner relaunches the app before each scenario (fresh library, settings
//! seeded as the scenario's `seed` says) and checks panics, unexpected file dialogs and error
//! toasts after it.
//!
//! To add a scenario file: create `scenarios/<name>.rs` with a `pub fn scenarios() ->
//! Vec<Scenario>`, add `mod <name>;` below and one `all.extend(<name>::scenarios());` line in
//! [`all`]. Nothing else (in particular not `main.rs`) needs to change. The scenario lanes'
//! files are pre-registered as empty stubs, so a lane only edits its own files.

use crate::support::{Ctx, SettingsSeed};

mod builtin;
mod coverage;
mod dialogs;
mod editor;
mod files;
mod library;
mod main_window;
mod rough_planner;
mod secondary_windows;
mod shortcuts;
mod tutorials;

/// One named user journey.
pub struct Scenario {
    /// Name shown in the report and usable as a command-line filter.
    pub name: &'static str,
    /// The `settings.toml` the app starts with.
    pub seed: fn() -> SettingsSeed,
    /// The journey itself.
    pub run: fn(&mut Ctx<'_>),
}

/// The boxed journey of a [`Piece`].
pub type RunFn = Box<dyn Fn(&mut Ctx<'_>)>;

/// One unit of scheduling: what the runner registers with the crate. A [`Scenario`] converts into
/// one; the split scenarios (one per overlay state, per coverage-invoke chunk, per click-everything
/// part) build their own with a closure, a computed name and the optional flags below. A worker
/// takes a whole piece, so the pieces are sized to run for a minute or two at most.
pub struct Piece {
    /// Name shown in the report and usable as a command-line filter.
    pub name: String,
    /// The `settings.toml` the app starts with.
    pub seed: fn() -> SettingsSeed,
    /// The journey itself.
    pub run: RunFn,
    /// Runs after every ordinary piece and, in a parallel run, sees the coverage every worker
    /// recorded (`Harness::scenario_last`).
    pub last: bool,
    /// Click-everything passes of this piece press only the controls of part `index` of `of`
    /// (see `support::part`).
    pub part: Option<(usize, usize)>,
    /// Asked before the app is relaunched; `false` skips the piece without starting the app.
    pub needed: Option<Box<dyn Fn() -> bool>>,
}

impl Piece {
    /// A piece with a computed name and a closure.
    pub fn new(
        name: impl Into<String>,
        seed: fn() -> SettingsSeed,
        run: impl Fn(&mut Ctx<'_>) + 'static,
    ) -> Self {
        Self {
            name: name.into(),
            seed,
            run: Box::new(run),
            last: false,
            part: None,
            needed: None,
        }
    }

    /// Skips the piece (no relaunch) when `needed` answers `false`.
    #[must_use]
    pub fn needed_if(mut self, needed: impl Fn() -> bool + 'static) -> Self {
        self.needed = Some(Box::new(needed));
        self
    }

    /// Marks the piece as one that runs after all ordinary ones.
    #[must_use]
    pub const fn last(mut self) -> Self {
        self.last = true;
        self
    }

    /// Limits the click-everything passes of the piece to part `index` of `of`.
    #[must_use]
    pub const fn part(mut self, index: usize, of: usize) -> Self {
        self.part = Some((index, of));
        self
    }
}

impl From<Scenario> for Piece {
    fn from(scenario: Scenario) -> Self {
        Self::new(scenario.name, scenario.seed, scenario.run)
    }
}

/// An existing install: Advanced UI, tour already done.
#[must_use]
pub const fn existing_install() -> SettingsSeed {
    SettingsSeed::ExistingInstall
}

/// A first run: no settings file, so Simple UI and the Welcome dialog.
#[must_use]
pub const fn first_run() -> SettingsSeed {
    SettingsSeed::FirstRun
}

/// Every registered piece, in run order.
#[must_use]
pub fn all() -> Vec<Piece> {
    let mut all: Vec<Piece> = Vec::new();
    all.extend(builtin::scenarios().into_iter().map(Piece::from));
    all.extend(main_window::pieces());
    all.extend(library::pieces());
    all.extend(editor::scenarios().into_iter().map(Piece::from));
    all.extend(files::scenarios().into_iter().map(Piece::from));
    all.extend(dialogs::scenarios().into_iter().map(Piece::from));
    all.extend(shortcuts::scenarios().into_iter().map(Piece::from));
    all.extend(secondary_windows::scenarios().into_iter().map(Piece::from));
    all.extend(rough_planner::pieces());
    all.extend(tutorials::scenarios().into_iter().map(Piece::from));
    // The click-through passes of the overlays and secondary states, one piece each, then the
    // direct-invoke chunks, which are marked `last`: they invoke only the callbacks no other
    // piece (in any worker) reached.
    all.extend(coverage::pieces());
    all
}
