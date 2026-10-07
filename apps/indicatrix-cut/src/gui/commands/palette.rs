//! Wires the command palette's Slint model (`ui/models/command_palette.slint`) to the
//! command table, the search and the recent-command list.
//!
//! The palette's UI owns the open state, the typed query and the highlighted row. Rust owns
//! the list: when the palette opens or the query changes it ranks the table, works out which
//! commands are available right now, and pushes the rows. Choosing a row closes the palette
//! first and runs the command a moment later, on a later pass of the event loop, so a command
//! that opens a dialog or a file picker never runs inside the palette's own handlers.

use super::{
    recents::Recents,
    registry::Command,
    search::{self, Entry},
    state::CommandState,
    table::COMMANDS,
};
use crate::{CommandPaletteModel, GuideModel, MainWindow, PaletteRow, gui::show_toast};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{cell::RefCell, rc::Rc, time::Duration};

/// How long after a command is chosen it runs. Long enough for the palette to close and for
/// the keyboard focus to settle on the main window first, so a dialog the command opens is
/// the one that keeps the focus.
const RUN_DELAY: Duration = Duration::from_millis(20);

/// One palette row as plain data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RowData {
    /// The command's title.
    pub title: &'static str,
    /// The category label.
    pub category: &'static str,
    /// The key combination, or an empty string.
    pub shortcut: &'static str,
    /// Why the command cannot run right now, or an empty string when it can.
    pub reason: &'static str,
}

/// What the palette remembers between keystrokes.
#[derive(Default)]
struct Session {
    /// Indices into the command table for the rows on screen, top first.
    shown: Vec<usize>,
    /// The commands used this session.
    recents: Recents,
}

/// The search view of every command in `commands`.
fn entries(commands: &[Command]) -> Vec<Entry<'_>> {
    commands
        .iter()
        .map(|command| Entry {
            id: command.id,
            title: command.title,
            keywords: command.keywords,
            category: command.category.label(),
        })
        .collect()
}

/// The rows to show for `order` (indices into `commands`), with each command's availability
/// worked out against `state`.
#[must_use]
pub fn rows_for(order: &[usize], commands: &[Command], state: &CommandState) -> Vec<RowData> {
    order
        .iter()
        .filter_map(|&index| commands.get(index))
        .map(|command| RowData {
            title: command.title,
            category: command.category.label(),
            shortcut: command.shortcut.unwrap_or(""),
            reason: command.availability(state).err().unwrap_or(""),
        })
        .collect()
}

/// The toast shown when the user picks a command that cannot run.
#[must_use]
pub fn blocked_message(title: &str, reason: &str) -> String {
    format!("{title} is not available right now. {reason}.")
}

fn to_slint_row(row: &RowData) -> PaletteRow {
    PaletteRow {
        title: row.title.into(),
        category: row.category.into(),
        shortcut: row.shortcut.into(),
        reason: row.reason.into(),
    }
}

/// Re-ranks the table for the palette's current query and pushes the rows.
fn refresh_rows(ui: &MainWindow, model: &VecModel<PaletteRow>, session: &RefCell<Session>) {
    let palette = ui.global::<CommandPaletteModel>();
    let query = palette.get_query();
    let state = CommandState::capture(ui);
    let order = {
        let session = session.borrow();
        search::rank(&query, &entries(COMMANDS), session.recents.ids())
    };
    let rows: Vec<PaletteRow> = rows_for(&order, COMMANDS, &state)
        .iter()
        .map(to_slint_row)
        .collect();
    session.borrow_mut().shown = order;
    model.set_vec(rows);
    palette.set_selected(0);
    palette.set_generation(palette.get_generation().wrapping_add(1));
}

/// Runs `command` if it is still available, else says why not.
fn run_command(ui_weak: &slint::Weak<MainWindow>, command: &Command) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    match command.availability(&CommandState::capture(&ui)) {
        Ok(()) => (command.run)(&ui),
        Err(reason) => show_toast(&ui, &blocked_message(command.title, reason), "info"),
    }
}

/// Connects the palette: fills its rows when it opens or the query changes, and runs the
/// chosen command. Called once while the main window is built.
pub(in crate::gui) fn setup_command_palette(ui: &MainWindow) {
    let model = Rc::new(VecModel::<PaletteRow>::default());
    let session = Rc::new(RefCell::new(Session::default()));
    let palette = ui.global::<CommandPaletteModel>();
    palette.set_rows(ModelRc::from(Rc::clone(&model)));

    let refresh: Rc<dyn Fn()> = {
        let ui_weak = ui.as_weak();
        let model = Rc::clone(&model);
        let session = Rc::clone(&session);
        Rc::new(move || {
            if let Some(ui) = ui_weak.upgrade() {
                refresh_rows(&ui, &model, &session);
            }
        })
    };
    let on_open = Rc::clone(&refresh);
    let ui_weak = ui.as_weak();
    palette.on_opened(move || {
        on_open();
        // A tutorial step may wait for the palette to open.
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<GuideModel>()
                .invoke_event("palette_opened".into());
        }
    });
    palette.on_query_changed(move |_query| refresh());

    let ui_weak = ui.as_weak();
    palette.on_activate(move |row| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let chosen = usize::try_from(row)
            .ok()
            .and_then(|row| session.borrow().shown.get(row).copied())
            .and_then(|index| COMMANDS.get(index));
        let Some(command) = chosen else {
            return;
        };
        // A command that cannot run keeps the palette open, so the reason shown under it and
        // in the toast can be read and another command picked.
        if let Err(reason) = command.availability(&CommandState::capture(&ui)) {
            show_toast(&ui, &blocked_message(command.title, reason), "info");
            return;
        }
        session.borrow_mut().recents.record(command.id);
        ui.global::<CommandPaletteModel>().invoke_close_palette();
        let ui_weak = ui.as_weak();
        slint::Timer::single_shot(RUN_DELAY, move || run_command(&ui_weak, command));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::commands::state::{Fact, GuideGroup};

    fn position(id: &str) -> usize {
        COMMANDS
            .iter()
            .position(|command| command.id == id)
            .unwrap_or_else(|| panic!("no command {id}"))
    }

    #[test]
    fn the_search_sees_every_command_with_its_category_label() {
        let all = entries(COMMANDS);
        assert_eq!(all.len(), COMMANDS.len());
        let save = &all[position("file.save")];
        assert_eq!((save.title, save.category), ("Save", "File"));
    }

    #[test]
    fn a_row_carries_its_category_and_shortcut() {
        let rows = rows_for(&[position("file.save")], COMMANDS, &CommandState::ready());
        assert_eq!(
            rows,
            vec![RowData {
                title: "Save",
                category: "File",
                shortcut: "Ctrl+S",
                reason: "",
            }]
        );
    }

    #[test]
    fn a_command_without_a_shortcut_has_an_empty_one() {
        let rows = rows_for(&[position("tiers.add")], COMMANDS, &CommandState::ready());
        assert_eq!(rows[0].shortcut, "");
    }

    #[test]
    fn a_guide_locked_command_shows_the_lock_as_its_reason() {
        let locked = CommandState::ready().with(Fact::Allows(GuideGroup::FileOps), false);
        let rows = rows_for(
            &[position("file.save"), position("tiers.add")],
            COMMANDS,
            &locked,
        );
        assert_eq!(rows[0].reason, "Locked by the guide");
        // Add Tier belongs to another group, which the guide left open.
        assert_eq!(rows[1].reason, "");
    }

    #[test]
    fn a_command_missing_a_design_shows_that_as_its_reason() {
        let none = CommandState::ready().with(Fact::Design, false);
        let rows = rows_for(&[position("solve.run")], COMMANDS, &none);
        assert_eq!(rows[0].reason, "Open or create a design first");
    }

    #[test]
    fn rows_follow_the_order_they_are_given() {
        let order = [position("help.manual"), position("edit.undo")];
        let rows = rows_for(&order, COMMANDS, &CommandState::ready());
        let titles: Vec<&str> = rows.iter().map(|row| row.title).collect();
        assert_eq!(titles, vec!["User Manual", "Undo"]);
    }

    #[test]
    fn an_index_outside_the_table_is_skipped() {
        let rows = rows_for(
            &[usize::MAX, position("help.manual")],
            COMMANDS,
            &CommandState::ready(),
        );
        assert_eq!(rows.len(), 1);
    }

    #[test]
    fn searching_the_real_table_finds_a_command_by_a_typo_free_fragment() {
        let all = entries(COMMANDS);
        let order = search::rank("deep", &all, &[]);
        assert_eq!(COMMANDS[order[0]].id, "solve.deep");
        let order = search::rank("prefs", &all, &[]);
        assert!(order.iter().any(|&i| COMMANDS[i].id == "edit.preferences"));
        let order = search::rank("export png", &all, &[]);
        assert_eq!(COMMANDS[order[0]].id, "file.export_diagram");
    }

    #[test]
    fn the_empty_search_lists_what_was_used_last_first() {
        let mut session = Session::default();
        session.recents.record("solve.run");
        session.recents.record("file.save");
        let all = entries(COMMANDS);
        let order = search::rank("", &all, session.recents.ids());
        assert_eq!(COMMANDS[order[0]].id, "file.save");
        assert_eq!(COMMANDS[order[1]].id, "solve.run");
        assert_eq!(order.len(), COMMANDS.len());
    }

    #[test]
    fn the_blocked_message_names_the_command_and_the_reason() {
        assert_eq!(
            blocked_message("Solve", "Open or create a design first"),
            "Solve is not available right now. Open or create a design first."
        );
    }
}
