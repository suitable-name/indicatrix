//! The raw-text editing route and the palette route of the scripted lessons.
use crate::{
    director::Film,
    pacing,
    targets::{self, Spot},
};
use indicatrix_cut::{CommandPaletteModel, RawTextModel, SolidPreviewModel};
use slint::{ComponentHandle, Model, SharedString, platform::WindowEvent};
use std::time::Duration;

impl Film<'_> {
    pub(super) fn dialog_button(&mut self, label: &str) -> Result<(), String> {
        let dialog = self
            .view()
            .root
            .query_descendants()
            .match_inherits("RawTextDialog")
            .find_first()
            .ok_or("Edit as Text dialog is missing")?;
        let matches: Vec<_> = dialog
            .query_descendants()
            .find_all()
            .into_iter()
            .filter(|e| {
                e.accessible_role() == Some(uiprobe::AccessibleRole::Button)
                    && e.accessible_label().is_some_and(|s| s == label)
            })
            .map(|e| Spot::of(&e))
            .filter(|s| s.usable(self.view().size))
            .collect();
        if matches.len() != 1 {
            return Err(format!(
                "Edit as Text {label:?}: expected one dialog control, found {}",
                matches.len()
            ));
        }
        self.click_and_invoke(&matches[0])?;
        Ok(())
    }

    pub(super) fn open_raw_text(&mut self) -> Result<(), String> {
        self.key("Ctrl+K")?;
        let query = targets::text_input(&self.view(), "Search Commands")
            .ok_or("palette search is missing")?;
        self.click_and_type(&query, "text")?;
        self.press_label("Edit Instructions as Text...")?;
        let raw = self.ui.global::<RawTextModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            raw.get_is_open() && !raw.get_busy() && !raw.get_text().is_empty()
        }) {
            return Err(format!(
                "Edit as Text did not load: {}",
                raw.get_load_error()
            ));
        }
        Ok(())
    }

    /// The lesson's own route: Ctrl+K, type `typed`, check that the highlighted (first) row reads
    /// `row_title`, press Enter, and require the palette closed with Solid view mode `mode`.
    pub(super) fn palette_choose_view(
        &mut self,
        typed: &str,
        row_title: &str,
        mode: i32,
    ) -> Result<(), String> {
        let palette = self.ui.global::<CommandPaletteModel>();
        self.key("Ctrl+K")?;
        if !pacing::wait_real(pacing::START_TIMEOUT, || palette.get_open()) {
            return Err("Ctrl+K did not open the command palette".into());
        }
        let search = targets::text_input(&self.view(), "Search Commands")
            .ok_or("command palette search input is missing")?;
        self.click_and_type(&search, typed)?;
        let first_row_reads = || {
            palette.get_query() == typed
                && palette
                    .get_rows()
                    .row_data(0)
                    .is_some_and(|row| row.title == row_title)
        };
        if !pacing::wait_real(pacing::START_TIMEOUT, first_row_reads) {
            return Err(format!(
                "after typing {typed:?} the highlighted palette row does not read {row_title:?}"
            ));
        }
        self.hold(pacing::MENU_HOLD)?;
        self.key("Return")?;
        let solid = self.ui.global::<SolidPreviewModel>();
        if !pacing::wait_real(pacing::START_TIMEOUT, || {
            !palette.get_open() && solid.get_view_mode() == mode
        }) {
            return Err(format!(
                "Enter on {row_title:?} left the palette open={} with view mode {}",
                palette.get_open(),
                solid.get_view_mode()
            ));
        }
        Ok(())
    }

    pub(super) fn edit_raw_text(&mut self) -> Result<(), String> {
        self.open_raw_text()?;
        let raw = self.ui.global::<RawTextModel>();
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            raw.get_is_open() && !raw.get_busy() && !raw.get_text().is_empty()
        }) {
            return Err(format!(
                "Edit as Text did not load: {}",
                raw.get_load_error()
            ));
        }
        let original = raw.get_text().to_string();
        // Where the token is: its byte offset (for the readback), its line and its column. The
        // caret walks lines and columns, not bytes or characters: a line break is crossed with
        // one key whatever it is made of (LF or CR LF).
        let mut offset = 0;
        let mut token = None;
        for (line_no, line) in original.split_inclusive('\n').enumerate() {
            if line.contains("Crown_Main") {
                let start = line
                    .find("34.5")
                    .ok_or("Crown_Main line has no original 34.5 angle")?;
                let prefix = line[..start].trim();
                if prefix != "a" {
                    return Err("Crown_Main 34.5 token is not the angle after a".into());
                }
                token = Some((offset + start, line_no, line[..start].chars().count()));
                break;
            }
            offset += line.len();
        }
        let (offset, line_no, column) = token.ok_or("Crown_Main line is missing")?;
        let field = targets::text_input(&self.view(), "Cutting instructions as .asc text")
            .ok_or("raw text input is missing")?;
        self.click_and_invoke(&field)?;
        self.key("Ctrl+Home")?;
        self.key_times("Down", line_no)?;
        self.key("Home")?;
        self.key_times("Right", column)?;
        self.key_times("Shift+Right", 4)?;
        self.hold(pacing::RESULT_HOLD)?;
        for ch in "36".chars() {
            let text: SharedString = ch.to_string().into();
            self.ui
                .window()
                .dispatch_event(WindowEvent::KeyPressed { text: text.clone() });
            self.ui
                .window()
                .dispatch_event(WindowEvent::KeyReleased { text });
            self.hold(Duration::from_millis(100))?;
        }
        let expected = format!("{}36{}", &original[..offset], &original[offset + 4..]);
        if raw.get_text().as_str() != expected {
            return Err("raw text edit changed more than the Crown_Main angle token".into());
        }
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || raw.get_can_apply()) {
            return Err(format!("raw text validation: {}", raw.get_status()));
        }
        self.hold(pacing::RESULT_HOLD)?;
        self.dialog_button("Compare With...")?;
        self.dialog_button("Snapshot")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || {
            raw.get_compare_open() && !raw.get_compare_busy()
        }) {
            return Err("raw text Snapshot diff did not open".into());
        }
        let rows = raw.get_diff_rows();
        if !rows
            .iter()
            .any(|r| r.kind == 1 && r.text.contains("Crown_Main"))
            || !rows
                .iter()
                .any(|r| r.kind == 2 && r.text.contains("Crown_Main"))
        {
            return Err("raw text diff is missing the added/removed Crown_Main lines".into());
        }
        // The diff's actual production callback raises raw_text_diff_shown. The full
        // tutorial goal intentionally remains false until Apply updates the design.
        self.hold(pacing::RESULT_HOLD)?;
        self.dialog_button("Back to the Text")?;
        self.dialog_button("Apply")?;
        if !pacing::wait_real(pacing::STEP_TIMEOUT, || !raw.get_is_open()) {
            return Err("raw text Apply did not close the dialog".into());
        }
        Ok(())
    }
}
