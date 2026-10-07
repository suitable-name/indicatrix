//! The shape of one palette command.
//!
//! The commands themselves are the single table in `table.rs`; adding a command later is
//! one entry there. This file only defines what an entry holds.

use super::{
    rules::{Check, first_failure},
    state::CommandState,
};
use crate::MainWindow;

/// The group a command is listed under in the palette.
///
/// The variants are in menu order; the palette's own listing sorts by the label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Category {
    /// Opening, saving and exporting.
    File,
    /// Undo, redo and preferences.
    Edit,
    /// Tabs, view modes, inspector tabs and viewport tools.
    View,
    /// Adding, moving and removing tiers.
    Tiers,
    /// Solving, optimizing and comparing.
    Solve,
    /// The library, the Rough Planner and the render tools.
    Tools,
    /// The manual, shortcuts and the guide.
    Help,
}

impl Category {
    /// The word shown beside a command, and searched like a keyword.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::View => "View",
            Self::Tiers => "Tiers",
            Self::Solve => "Solve",
            Self::Tools => "Tools",
            Self::Help => "Help",
        }
    }
}

/// One palette command: what it is called, when it is available, and what it does.
pub struct Command {
    /// A stable dotted name such as `file.save`. Unique; used to remember recent commands.
    pub id: &'static str,
    /// The name shown in the palette. Plain English; menu and button names are reused so a
    /// command is found under the name the user already knows.
    pub title: &'static str,
    /// The group it is listed under.
    pub category: Category,
    /// Extra words the search also matches, such as `quit close` for an exit command.
    pub keywords: &'static str,
    /// The key combination shown beside the command, when it has one. It must appear in
    /// the shortcut table (`gui::editor::shortcuts::SHORTCUTS`); a test there checks it.
    pub shortcut: Option<&'static str>,
    /// What must hold for the command to run, checked in order.
    pub requires: &'static [Check],
    /// Runs the command. Invokes the same Slint callbacks and properties the menu and the
    /// buttons use, so there is one behaviour per action.
    pub run: fn(&MainWindow),
}

impl Command {
    /// Whether the command can run in `state`; the error is the plain-English reason.
    ///
    /// # Errors
    ///
    /// Returns the reason of the first requirement that fails.
    pub fn availability(&self, state: &CommandState) -> Result<(), &'static str> {
        first_failure(self.requires, state)
    }
}

/// Builds one table entry. A `const fn` so the whole table can be a `static`.
#[must_use]
pub const fn command(
    id: &'static str,
    title: &'static str,
    category: Category,
    keywords: &'static str,
    shortcut: Option<&'static str>,
    requires: &'static [Check],
    run: fn(&MainWindow),
) -> Command {
    Command {
        id,
        title,
        category,
        keywords,
        shortcut,
        requires,
        run,
    }
}
