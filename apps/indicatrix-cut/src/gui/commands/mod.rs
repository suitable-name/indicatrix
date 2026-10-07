//! The command palette: every action in the app, reachable from the keyboard.
//!
//! Ctrl+K (or Ctrl+Shift+P, or Edit > Command Palette...) opens a search box over one table
//! of commands. Typing narrows the list with a forgiving fuzzy search, Up and Down move the
//! highlight, Enter runs the highlighted command and Escape closes the palette.
//!
//! The pieces, each small and testable on its own:
//!
//! - [`table`] -- the one table of commands. Adding a command is one entry there.
//! - [`registry`] -- what an entry holds.
//! - [`rules`] and [`state`] -- when a command is available, as pure checks over a plain
//!   snapshot of the window. A command the worked-example guide has locked is disabled with
//!   the reason "Locked by the guide".
//! - [`search`] and [`recents`] -- the fuzzy ranking and the session's recent commands.
//! - [`run`] -- what the commands do beyond calling one Slint callback.
//! - [`palette`] -- the wiring to `ui/models/command_palette.slint`.
//!
//! A command never reimplements an action: it invokes the same Slint callback or sets the
//! same property its menu item or button does.

mod palette;
mod recents;
mod registry;
mod rules;
mod run;
mod search;
mod state;
mod table;

pub(in crate::gui) use palette::setup_command_palette;
// Read by the shortcut table's test, which checks every shortcut shown in the palette is a
// real row of that table.
#[cfg(test)]
pub(in crate::gui) use table::COMMANDS;
