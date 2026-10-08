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
// What the guide's Next button does on an unfinished step goes through the same functions the
// buttons and the palette use.
pub(in crate::gui) use run::{add_tier, set_solid_view_mode, show_edit_view, show_inspector_tab};
// Read by the shortcut table's test, which checks every shortcut shown in the palette is a
// real row of that table.
#[cfg(test)]
pub(in crate::gui) use table::COMMANDS;

use crate::MainWindow;
use state::CommandState;

/// Runs the command called `id` for the guide's Next button: the app doing the step, so the
/// guide's own control locks do not apply, while every other requirement (a design is open,
/// nothing else is solving) still does.
///
/// # Errors
///
/// A sentence for the learner: there is no such command, or it cannot run right now.
pub(in crate::gui) fn run_unlocked(ui: &MainWindow, id: &str) -> Result<(), String> {
    let command = table::COMMANDS
        .iter()
        .find(|command| command.id == id)
        .ok_or_else(|| format!("There is no command {id:?}."))?;
    command
        .availability(&CommandState::capture(ui).without_guide_locks())
        .map_err(|reason| palette::blocked_message(command.title, reason))?;
    (command.run)(ui);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::guide::PERFORM_COMMANDS;

    #[test]
    fn every_command_a_guide_step_may_run_is_in_the_table() {
        for id in PERFORM_COMMANDS {
            assert!(
                table::COMMANDS.iter().any(|command| command.id == *id),
                "the guide may run {id}, which the command table does not have"
            );
        }
    }
}
