//! The single source of truth for every keyboard shortcut this app's editor
//! implements. One Rust table feeds BOTH the in-app overlay
//! (`ui/components/shortcut_overlay.slint`, opened from Help > Keyboard
//! Shortcuts and from `?`) and a generated section of
//! `docs/manual/appendix-b-keyboard-shortcuts.md`, via [`render_markdown_section`]
//! and this module's own test comparing that output against the file's generated
//! block -- so the two cannot drift.
//!
//! [`SHORTCUTS`] was built by grepping `Key.`/`key-pressed` across `ui/`
//! (`ui/app.slint`'s `global_shortcuts` `FocusScope`, `editor_tier_table.slint`'s
//! `table_focus` `FocusScope`, and `TierAngleCell`'s own inline `LineEdit`) --
//! every row names a key binding this app's Slint actually reads, not an
//! aspirational one.

/// One keyboard shortcut: the literal key combination, what it does, and which
/// context it applies in (groups the in-app overlay into sections and picks
/// which context's rows the generated Markdown table covers).
#[derive(Debug, Clone, Copy)]
pub struct Shortcut {
    /// The key combination as shown to the user, e.g. `"Ctrl+Shift+S"`.
    pub keys: &'static str,
    /// What the shortcut does, in the same wording the manual already used
    /// (`docs/manual/appendix-b-keyboard-shortcuts.md`) before this table
    /// replaced its two hand-authored tables.
    pub action: &'static str,
    /// Which context this shortcut is active in -- also the grouping key for
    /// both the overlay's sections and [`render_markdown_section`]'s per-
    /// context table.
    pub context: Context,
}

/// Grouping for [`Shortcut::context`]. `Global` matches `ui/app.slint`'s
/// `global_shortcuts` `FocusScope` (active anywhere in the window, unless a text
/// field has focus); `TierList` matches `editor_tier_table.slint`'s own
/// `table_focus` `FocusScope` (active only once the tier list has keyboard
/// focus).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Context {
    Global,
    TierList,
}

impl Context {
    /// The heading [`render_markdown_section`] and the overlay both use for
    /// this group.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::TierList => "The tier list",
        }
    }
}

/// Every shortcut this app's editor implements, in display order within each
/// [`Context`] -- see this module's own top doc comment for how this list was
/// built and what it feeds.
pub const SHORTCUTS: &[Shortcut] = &[
    Shortcut {
        keys: "Ctrl+N",
        action: "New Design...",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+O",
        action: "Open Native...",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+S",
        action: "Save Native",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+Shift+S",
        action: "Save Native As...",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+Z",
        action: "Undo (Edit tab)",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+Y (or Ctrl+Shift+Z)",
        action: "Redo (Edit tab)",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+D",
        action: "Duplicate the selected tier (tier list must have keyboard focus -- see \"The tier list\" below)",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+E",
        action: "Toggle the 3D Gem tab's Live Render / Edit pill. **Not** Export -- \"Export Edited .asc\" has no keyboard shortcut of its own.",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+F",
        action: "Focus the catalogue search box, or, while the Edit tab's tier list is showing, the tier list's own filter box instead",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+1 / Ctrl+2 / Ctrl+3",
        action: "Switch to the 3D Gem tab / Cutting Table tab / Attachments tab. There is no Ctrl+4 -- the window only has these three top-level tabs.",
        context: Context::Global,
    },
    Shortcut {
        keys: "F5",
        action: "Solve (Edit tab) -- disabled while a solve is already running, so F5 cannot start a second one on top of it",
        context: Context::Global,
    },
    Shortcut {
        keys: "Esc",
        action: "Clear the current tier selection and dismiss whatever toast is showing",
        context: Context::Global,
    },
    Shortcut {
        keys: "?",
        action: "Open this Keyboard Shortcuts overlay -- only while no text field has focus",
        context: Context::Global,
    },
    Shortcut {
        keys: "Up / Down",
        action: "Select the previous/next row (a real selection, not just a cursor -- it re-seeds the inspector immediately, same as clicking)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Home / End",
        action: "Select the first/last row",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Page Up / Page Down",
        action: "Select 10 rows back/forward",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Enter",
        action: "Select the highlighted row (redundant with Up/Down, kept for habit)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Delete",
        action: "Remove the highlighted row (**not** Backspace -- that key is left alone here, since it is the universal \"delete the previous character\" key in every text field elsewhere in the app)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Ctrl+D",
        action: "Duplicate the highlighted row",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Alt+Up / Alt+Down",
        action: "Move the highlighted row up/down in cutting order",
        context: Context::TierList,
    },
    Shortcut {
        keys: "F2",
        action: "Open the highlighted row's **angle** cell for inline editing (name and other fields have no shortcut of their own)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Ctrl+click a row",
        action: "Add/remove that row from the multi-select group (batched angle nudging, or batch delete -- Chapter 4)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Shift+click a row",
        action: "Select every row between it and whichever row you selected last, replacing the current selection (Chapter 4)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Escape (list focused, nothing else open)",
        action: "Clear the current tier selection",
        context: Context::TierList,
    },
];

/// Renders every [`SHORTCUTS`] row whose [`Shortcut::context`] is `context` as
/// a Markdown `| Shortcut | Action |` table, byte-for-byte the same shape
/// `docs/manual/appendix-b-keyboard-shortcuts.md`'s own two tables already
/// used before this table replaced their hand-authored rows -- no trailing
/// newline, so a caller composing multiple sections controls its own spacing.
#[must_use]
pub fn render_markdown_section(context: Context) -> String {
    let mut out = String::from("| Shortcut | Action |\n| --- | --- |");
    for row in SHORTCUTS.iter().filter(|s| s.context == context) {
        out.push('\n');
        out.push_str("| ");
        out.push_str(row.keys);
        out.push_str(" | ");
        out.push_str(row.action);
        out.push_str(" |");
    }
    out
}

/// Pushes [`SHORTCUTS`] into `ShortcutsModel.shortcuts` once, at startup --
/// called from `gui::editor::setup_editor_callbacks`. The overlay's own
/// open/close state (`ShortcutsModel.open`/`toggle`/`close`) needs no Rust
/// callback registration (implemented entirely in
/// `ui/models/shortcuts.slint`, the same "small enough to live entirely in
/// Slint" idiom `ui/models/guide.slint`'s `GuideModel` uses).
pub(in crate::gui::editor) fn setup_shortcuts_overlay(ui: &crate::MainWindow) {
    use slint::{ComponentHandle as _, ModelRc, VecModel};
    let items: Vec<crate::ShortcutItem> = SHORTCUTS
        .iter()
        .map(|row| crate::ShortcutItem {
            keys: row.keys.into(),
            action: row.action.into(),
            context: row.context.heading().into(),
        })
        .collect();
    ui.global::<crate::ShortcutsModel>()
        .set_shortcuts(ModelRc::new(VecModel::from(items)));
    setup_shortcuts_copy_markdown(ui);
}

/// "Copy as Markdown" (`shortcut_overlay.slint`'s header button): the ONE real,
/// non-test caller of [`render_markdown_section`] -- the manual's own generated
/// blocks are checked against that same function's output by this module's
/// `appendix_b_generated_blocks_match_the_shortcut_table` test, so between the
/// two, [`render_markdown_section`] backs both the shipped manual text and a
/// live, in-app production path, not only a test assertion.
fn setup_shortcuts_copy_markdown(ui: &crate::MainWindow) {
    use slint::ComponentHandle as _;
    use std::fmt::Write as _;
    let ui_weak = ui.as_weak();
    ui.global::<crate::ShortcutsModel>()
        .on_copy_markdown(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut markdown = format!("## {}\n\n", Context::Global.heading());
            markdown.push_str(&render_markdown_section(Context::Global));
            let _ = write!(markdown, "\n\n## {}\n\n", Context::TierList.heading());
            markdown.push_str(&render_markdown_section(Context::TierList));
            crate::gui::library::clipboard::copy_to_clipboard(&markdown);
            crate::gui::show_toast(&ui, "Shortcut table copied to clipboard!", "success");
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MANUAL: &str = include_str!("../../../docs/manual/appendix-b-keyboard-shortcuts.md");

    /// Extracts the text strictly between a `<!-- NAME:BEGIN -->` and
    /// `<!-- NAME:END -->` marker pair, panicking with a clear message if
    /// either marker (or their expected order) is missing -- a missing marker
    /// means the manual's own generated block was edited by hand and needs
    /// restoring, not that this test should silently pass.
    fn generated_block(marker: &str) -> &'static str {
        let begin = format!("<!-- {marker}:BEGIN -->\n");
        let end = format!("<!-- {marker}:END -->");
        let start = MANUAL
            .find(&begin)
            .unwrap_or_else(|| panic!("appendix B is missing the {marker}:BEGIN marker"))
            + begin.len();
        let stop = MANUAL[start..]
            .find(&end)
            .unwrap_or_else(|| panic!("appendix B is missing the {marker}:END marker"));
        MANUAL[start..start + stop].trim_end_matches('\n')
    }

    /// The whole point of this module: `docs/manual/appendix-b-keyboard-shortcuts.md`'s
    /// generated blocks must be exactly [`render_markdown_section`]'s output
    /// for their own context, so the manual and the in-app overlay cannot
    /// silently drift apart. Run this after editing [`SHORTCUTS`] and paste
    /// the new output back between the matching markers.
    #[test]
    fn appendix_b_generated_blocks_match_the_shortcut_table() {
        assert_eq!(
            generated_block("SHORTCUTS_GLOBAL"),
            render_markdown_section(Context::Global),
            "appendix B's global shortcuts block is out of date -- regenerate it from SHORTCUTS"
        );
        assert_eq!(
            generated_block("SHORTCUTS_TIER_LIST"),
            render_markdown_section(Context::TierList),
            "appendix B's tier-list shortcuts block is out of date -- regenerate it from SHORTCUTS"
        );
    }

    /// Every shortcut needs a non-empty key label and action text, and no two
    /// rows in the same context should share a key combination (the overlay
    /// and the manual would both show two conflicting rows for the same key).
    #[test]
    fn every_shortcut_is_well_formed_and_unique_within_its_context() {
        // `SHORTCUTS` is a non-empty `const` array literal a few lines above --
        // `.is_empty()` on it is compile-time-decidable (clippy::const_is_empty),
        // so the real check below (every row's own fields) is what actually
        // earns its keep.
        for row in SHORTCUTS {
            assert_ne!(row.keys, "");
            assert_ne!(row.action, "");
        }
        for (i, a) in SHORTCUTS.iter().enumerate() {
            for b in &SHORTCUTS[i + 1..] {
                if a.context == b.context {
                    assert_ne!(
                        a.keys, b.keys,
                        "duplicate key binding {:?} in context {:?}",
                        a.keys, a.context
                    );
                }
            }
        }
    }
}
