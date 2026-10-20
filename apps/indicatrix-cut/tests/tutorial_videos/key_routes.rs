//! Filmed key presses of the Keyboard Shortcuts lesson: the digit keys that switch the Solid
//! viewport's view mode, Ctrl+E between Live Render and Edit, and the ? key that lists every
//! shortcut. The lesson teaches the keys, so no pill, menu or palette stands in for them, and each
//! press is followed by the visible change it must cause.

use crate::{director::Film, pacing, support::app::pump};
use indicatrix_cut::{ShortcutsModel, SolidPreviewModel};
use slint::ComponentHandle;

impl Film<'_> {
    /// Presses the digit `key` (1 to 4) and requires the Solid viewport to show view mode `mode`.
    pub(super) fn press_view_key(&mut self, key: &str, mode: i32) -> Result<(), String> {
        self.key(key)?;
        pump(80);
        let shown = self.ui.global::<SolidPreviewModel>().get_view_mode();
        if shown != mode {
            return Err(format!(
                "pressing {key} left the view mode at {shown}, not {mode}; the Edit tab must be \
                 open and no text box may have the keyboard"
            ));
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Presses Ctrl+E to leave the Edit tab for Live Render and once more to come back, and
    /// requires the tab to change both times.
    pub(super) fn swap_tabs_with_keys(&mut self) -> Result<(), String> {
        let edit = self.ui.get_render_view_tab();
        self.key("Ctrl+E")?;
        pump(80);
        if self.ui.get_render_view_tab() == edit {
            return Err("Ctrl+E did not leave the Edit tab".to_owned());
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Ctrl+E")?;
        pump(80);
        if self.ui.get_render_view_tab() != edit {
            return Err("the second Ctrl+E did not bring the Edit tab back".to_owned());
        }
        self.hold(pacing::RESULT_HOLD)
    }

    /// Presses ? to open the list of shortcuts and Esc to close it, and requires it to open and to
    /// close.
    pub(super) fn open_shortcuts_with_key(&mut self) -> Result<(), String> {
        self.key("?")?;
        pump(80);
        let shortcuts = self.ui.global::<ShortcutsModel>();
        if !shortcuts.get_open() {
            return Err(
                "pressing ? did not open the shortcuts list; no text box may have the keyboard"
                    .to_owned(),
            );
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.key("Escape")?;
        pump(80);
        if shortcuts.get_open() {
            return Err("Esc did not close the shortcuts list".to_owned());
        }
        Ok(())
    }
}
