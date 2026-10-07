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
    /// `solid_viewport.slint`'s own `viewport_focus` `FocusScope` -- the Edit tab's
    /// Solid viewport, once it has keyboard focus (click into it first).
    Viewport,
    /// The tier list's inline angle cell and the inspector's Angle field, while they
    /// are being edited.
    AngleField,
    /// Cutting mode (`ui/models/cutting_mode.slint`'s `handle_key`): the full-window
    /// step-by-step screen, while it is open.
    CuttingMode,
}

impl Context {
    /// Every context, in the order the overlay and the manual list them.
    pub const ALL: [Self; 5] = [
        Self::Global,
        Self::TierList,
        Self::Viewport,
        Self::AngleField,
        Self::CuttingMode,
    ];

    /// The heading [`render_markdown_section`] and the overlay both use for
    /// this group.
    #[must_use]
    pub const fn heading(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::TierList => "The tier list",
            Self::Viewport => "The Solid viewport",
            Self::AngleField => "Angle fields",
            Self::CuttingMode => "Cutting mode",
        }
    }

    /// A short hint shown after the heading in the in-app overlay (empty for none).
    #[must_use]
    pub const fn note(self) -> &'static str {
        match self {
            Self::Global => "",
            Self::TierList | Self::Viewport => "click into it first",
            Self::AngleField => "while editing an angle",
            Self::CuttingMode => "while the cutting steps are open",
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
        action: "Open...",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+S",
        action: "Save",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+Shift+S",
        action: "Save As...",
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
        keys: "Ctrl+K / Ctrl+Shift+P",
        action: "Open the command palette: type to search every action in the app, Enter runs the highlighted one. Works even while a text field has focus.",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+E",
        action: "Toggle the 3D Spectral Preview tab's Live Render / Edit pill. **Not** Export -- \"Export Edited .asc\" has no keyboard shortcut of its own.",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+F",
        action: "Focus the catalogue search box, or, while the Edit tab's tier list is showing, the tier list's own filter box instead",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+1 / Ctrl+2 / Ctrl+3",
        action: "Switch to the 3D Spectral Preview tab / Cutting Instructions tab / Files & Downloads tab. There is no Ctrl+4 -- the window only has these three top-level tabs.",
        context: Context::Global,
    },
    Shortcut {
        keys: "1 / 2 / 3 / 4",
        action: "Solid / Path-traced / Both / Diagram view of the Edit tab's viewport. Only while that viewport is on screen and no text field has focus.",
        context: Context::Global,
    },
    Shortcut {
        keys: "Ctrl+, (Ctrl+Comma)",
        action: "Open Preferences -- the Simple/Advanced interface, UI scale, high contrast, larger handles and tutorials. Not while a text field has focus.",
        context: Context::Global,
    },
    Shortcut {
        keys: "F5",
        action: "Solve (Edit tab) -- disabled while a solve is already running, so F5 cannot start a second one on top of it",
        context: Context::Global,
    },
    Shortcut {
        keys: "F1",
        action: "Help for this screen -- opens the manual at the page for what is on screen (the open inspector tab in the Edit tab, Live Render, the Cutting Instructions tab). Works while a text field has focus.",
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
        action: "Move the highlighted row up/down in the table (and so in the cutting order of its side of the stone)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "F2",
        action: "Open the highlighted row's **angle** cell for inline editing (name and other fields have no shortcut of their own)",
        context: Context::TierList,
    },
    Shortcut {
        keys: "Space",
        action: "Add/remove the highlighted row from the multi-select group (the keyboard twin of Ctrl+click -- Chapter 4)",
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
    Shortcut {
        keys: "S",
        action: "Turn the Slice tool on or off (not in the Diagram view, and not while a handle or a slice line is being dragged)",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "F",
        action: "While a slice tier is shown but not yet kept: flip which side of your line is cut away",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "Enter",
        action: "While a slice tier is shown but not yet kept: keep it as one undo step",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "Esc",
        action: "Cancel what is in progress, innermost first: a handle drag, a slice line being drawn, a slice tier not yet kept, Slice mode itself, an enlarged Diagram panel. With none of those, clear the tier selection.",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "Up / Down",
        action: "Select the previous/next tier",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "Page Up / Page Down",
        action: "Select the tier 10 places back/forward",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "Shift (while dragging a handle)",
        action: "Switch the angle and depth handles to their fine snapping step",
        context: Context::Viewport,
    },
    Shortcut {
        keys: "Up / Down",
        action: "Change the angle by 0.1 degrees",
        context: Context::AngleField,
    },
    Shortcut {
        keys: "Shift+Up / Shift+Down",
        action: "Change the angle by 1 degree",
        context: Context::AngleField,
    },
    Shortcut {
        keys: "Ctrl+Up / Ctrl+Down",
        action: "Change the angle by 0.01 degrees",
        context: Context::AngleField,
    },
    Shortcut {
        keys: "Mouse wheel",
        action: "Change the angle by 0.1 degrees. Over the tier list's angle cell this only works while the cell is open for editing, or while you hold Ctrl; otherwise the wheel scrolls the list.",
        context: Context::AngleField,
    },
    Shortcut {
        keys: "Enter",
        action: "Commit the value and close the cell (the tier list's inline cell only)",
        context: Context::AngleField,
    },
    Shortcut {
        keys: "Escape",
        action: "Close the cell without committing (the tier list's inline cell only)",
        context: Context::AngleField,
    },
    Shortcut {
        keys: "Left / Right",
        action: "Go to the previous/next cutting step",
        context: Context::CuttingMode,
    },
    Shortcut {
        keys: "Page Up / Page Down",
        action: "Go to the previous/next cutting step",
        context: Context::CuttingMode,
    },
    Shortcut {
        keys: "Space / D",
        action: "Mark the step done and go on to the next one (a step that is already done is just passed)",
        context: Context::CuttingMode,
    },
    Shortcut {
        keys: "Esc",
        action: "Leave cutting mode (while the Reset progress question is showing, Esc cancels the question instead)",
        context: Context::CuttingMode,
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
    // Grouped by context, in `Context::ALL` order: the overlay starts a new heading
    // whenever the context changes from one row to the next.
    let items: Vec<crate::ShortcutItem> = Context::ALL
        .iter()
        .flat_map(|&context| SHORTCUTS.iter().filter(move |row| row.context == context))
        .map(|row| crate::ShortcutItem {
            keys: row.keys.into(),
            action: row.action.into(),
            context: row.context.heading().into(),
            context_note: row.context.note().into(),
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
            let mut markdown = String::new();
            for (index, context) in Context::ALL.into_iter().enumerate() {
                if index > 0 {
                    markdown.push_str("\n\n");
                }
                let _ = write!(markdown, "## {}\n\n", context.heading());
                markdown.push_str(&render_markdown_section(context));
            }
            crate::gui::library::clipboard::copy_to_clipboard_with_toast(
                &ui,
                markdown,
                "Shortcut table copied to clipboard!".to_owned(),
            );
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
        assert_eq!(
            generated_block("SHORTCUTS_VIEWPORT"),
            render_markdown_section(Context::Viewport),
            "appendix B's Solid viewport shortcuts block is out of date -- regenerate it from SHORTCUTS"
        );
        assert_eq!(
            generated_block("SHORTCUTS_ANGLE_FIELD"),
            render_markdown_section(Context::AngleField),
            "appendix B's angle-field shortcuts block is out of date -- regenerate it from SHORTCUTS"
        );
        assert_eq!(
            generated_block("SHORTCUTS_CUTTING_MODE"),
            render_markdown_section(Context::CuttingMode),
            "appendix B's cutting-mode shortcuts block is out of date -- regenerate it from SHORTCUTS"
        );
    }

    /// Every context has rows, so no overlay heading or manual block is empty.
    #[test]
    fn every_context_lists_at_least_one_shortcut() {
        for context in Context::ALL {
            assert!(
                SHORTCUTS.iter().any(|row| row.context == context),
                "{context:?} has no shortcuts"
            );
            assert_ne!(context.heading(), "");
        }
    }

    /// The key combinations a table row stands for: `"Up / Down"` is two, and a
    /// parenthesised alternative such as `"Ctrl+Y (or Ctrl+Shift+Z)"` or a spelled-out
    /// name such as `"Ctrl+, (Ctrl+Comma)"` is dropped.
    fn key_combinations(keys: &str) -> Vec<String> {
        let mut plain = String::new();
        let mut depth = 0_u32;
        for c in keys.chars() {
            match c {
                '(' => depth += 1,
                ')' => depth = depth.saturating_sub(1),
                _ if depth == 0 => plain.push(c),
                _ => {}
            }
        }
        plain
            .split('/')
            .map(|part| part.trim().to_owned())
            .collect()
    }

    #[test]
    fn key_combinations_split_on_slashes_and_ignore_parentheses() {
        assert_eq!(key_combinations("Up / Down"), ["Up", "Down"]);
        assert_eq!(key_combinations("Ctrl+Y (or Ctrl+Shift+Z)"), ["Ctrl+Y"]);
        assert_eq!(key_combinations("Ctrl+, (Ctrl+Comma)"), ["Ctrl+,"]);
        assert_eq!(key_combinations("1 / 2 / 3 / 4"), ["1", "2", "3", "4"]);
        assert_eq!(key_combinations("?"), ["?"]);
    }

    /// The command palette shows each command's key combination beside it. Each one must
    /// be a real row of this table, so the palette cannot advertise a shortcut that does
    /// not exist (or that was renamed here).
    #[test]
    fn every_shortcut_shown_in_the_command_palette_is_in_the_shortcut_table() {
        let known: Vec<String> = SHORTCUTS
            .iter()
            .flat_map(|row| key_combinations(row.keys))
            .collect();
        let mut shown = 0;
        for command in crate::gui::commands::COMMANDS {
            if let Some(shortcut) = command.shortcut {
                shown += 1;
                assert!(
                    known.iter().any(|keys| keys == shortcut),
                    "command {} shows {shortcut:?}, which is not a row of SHORTCUTS",
                    command.id
                );
            }
        }
        assert!(shown >= 15, "only {shown} palette commands show a shortcut");
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
