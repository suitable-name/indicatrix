//! "Help for this screen": F1 and the palette's "Help for This Screen" open the manual at the
//! page for what the main window shows.
//!
//! The window has no single "focused panel" the program could ask, and a key press inside a
//! text field must not depend on which widget happens to hold the focus. So the page follows
//! what is on screen: the library's tab, the Live Render / Edit pill and, in the editor, the
//! open inspector tab. An open Parameters panel wins on the 3D Spectral Preview tab, because it is
//! what the person is looking at. [`context_topic`] is the whole rule and a pure function of those
//! facts, so a test can pin it. Anything it does not know opens the contents page.

use slint::ComponentHandle;

use super::{open_topic, topics};
use crate::{EditorModel, HelpModel, MainWindow, ParametersModel, gui::show_toast};

/// What the main window shows, as far as help is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Screen {
    /// The library's tab: 0 the 3D Spectral Preview, 1 Cutting Instructions, 2 Files &
    /// Downloads (`MainWindow.active_tab`).
    pub library_tab: i32,
    /// The 3D Spectral Preview's pill: 0 Live Render, 1 Edit (`MainWindow.render_view_tab`).
    pub view_tab: i32,
    /// Whether the editor exists in this build and for this session (`EditorModel.enabled`).
    pub editor_enabled: bool,
    /// The editor's inspector tab: 0 Tier, 1 Preform, 2 Optimize, 3 Schedule, 4 History
    /// (`EditorModel.inspector_tab`).
    pub inspector_tab: i32,
    /// The Parameters panel's tab while the panel is open (0 Stone, 1 Lighting & Environment,
    /// 2 Render & Output; `ParametersModel.active_tab`), `None` while it is folded or closed.
    pub parameters_tab: Option<i32>,
}

/// The manual page for `screen`.
#[must_use]
pub const fn context_topic(screen: Screen) -> &'static str {
    match screen.library_tab {
        0 if screen.parameters_tab.is_some() => match screen.parameters_tab {
            Some(0) => topics::STONE_TAB,
            Some(1) => topics::LIGHTING_TAB,
            Some(2) => topics::RENDER_OUTPUT_TAB,
            _ => topics::LIVE_RENDER,
        },
        0 if screen.view_tab == 1 && screen.editor_enabled => match screen.inspector_tab {
            0 => topics::TIER_FORM,
            1 => topics::PREFORM_TAB,
            2 => topics::OPTIMIZE_TAB,
            3 => topics::SCHEDULE_TAB,
            4 => topics::HISTORY,
            _ => topics::TIER_TABLE,
        },
        0 => topics::LIVE_RENDER,
        1 => topics::CUTTING_TABLE,
        2 => topics::LIBRARY_DETAILS,
        _ => topics::CONTENTS,
    }
}

/// What the window shows right now.
fn current_screen(ui: &MainWindow) -> Screen {
    let editor = ui.global::<EditorModel>();
    let parameters = ui.global::<ParametersModel>();
    Screen {
        library_tab: ui.get_active_tab(),
        view_tab: ui.get_render_view_tab(),
        editor_enabled: editor.get_enabled(),
        inspector_tab: editor.get_inspector_tab(),
        parameters_tab: parameters.get_open().then(|| parameters.get_active_tab()),
    }
}

/// Opens the help window at the page for what is on screen.
pub fn open_context_help(ui: &MainWindow) {
    let topic = context_topic(current_screen(ui));
    if let Err(message) = open_topic(ui, topic) {
        show_toast(ui, &message, "error");
    }
}

/// Registers `HelpModel.open_context_help` (the F1 key of `app.slint`).
pub fn setup_context_help(ui: &MainWindow) {
    let weak = ui.as_weak();
    ui.global::<HelpModel>().on_open_context_help(move || {
        if let Some(ui) = weak.upgrade() {
            open_context_help(&ui);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(library_tab: i32, view_tab: i32, inspector_tab: i32) -> Screen {
        Screen {
            library_tab,
            view_tab,
            editor_enabled: true,
            inspector_tab,
            parameters_tab: None,
        }
    }

    #[test]
    fn the_editor_follows_its_open_inspector_tab() {
        assert_eq!(context_topic(screen(0, 1, 0)), topics::TIER_FORM);
        assert_eq!(context_topic(screen(0, 1, 1)), topics::PREFORM_TAB);
        assert_eq!(context_topic(screen(0, 1, 2)), topics::OPTIMIZE_TAB);
        assert_eq!(context_topic(screen(0, 1, 3)), topics::SCHEDULE_TAB);
        assert_eq!(context_topic(screen(0, 1, 4)), topics::HISTORY);
        assert_eq!(context_topic(screen(0, 1, 9)), topics::TIER_TABLE);
    }

    #[test]
    fn live_render_and_the_other_library_tabs_have_their_own_pages() {
        assert_eq!(context_topic(screen(0, 0, 0)), topics::LIVE_RENDER);
        assert_eq!(context_topic(screen(1, 1, 0)), topics::CUTTING_TABLE);
        assert_eq!(context_topic(screen(2, 0, 0)), topics::LIBRARY_DETAILS);
    }

    #[test]
    fn an_open_parameters_panel_opens_its_own_tab_page() {
        let with_panel = |tab| Screen {
            parameters_tab: Some(tab),
            ..screen(0, 0, 0)
        };
        assert_eq!(context_topic(with_panel(0)), topics::STONE_TAB);
        assert_eq!(context_topic(with_panel(1)), topics::LIGHTING_TAB);
        assert_eq!(context_topic(with_panel(2)), topics::RENDER_OUTPUT_TAB);
        assert_eq!(context_topic(with_panel(9)), topics::LIVE_RENDER);
        let edit = Screen {
            parameters_tab: Some(1),
            ..screen(0, 1, 2)
        };
        assert_eq!(context_topic(edit), topics::LIGHTING_TAB);
        let other_library_tab = Screen {
            parameters_tab: Some(1),
            ..screen(1, 0, 0)
        };
        assert_eq!(context_topic(other_library_tab), topics::CUTTING_TABLE);
    }

    #[test]
    fn a_build_without_the_editor_never_opens_the_editor_pages() {
        let no_editor = Screen {
            editor_enabled: false,
            ..screen(0, 1, 2)
        };
        assert_eq!(context_topic(no_editor), topics::LIVE_RENDER);
    }

    #[test]
    fn an_unknown_tab_opens_the_contents() {
        assert_eq!(context_topic(screen(7, 0, 0)), topics::CONTENTS);
        assert_eq!(context_topic(screen(-1, 0, 0)), topics::CONTENTS);
    }

    #[test]
    fn every_page_it_can_pick_resolves_in_the_manual() {
        for library_tab in 0..=3 {
            for view_tab in 0..=1 {
                for inspector_tab in 0..=5 {
                    let topic = context_topic(screen(library_tab, view_tab, inspector_tab));
                    let target = topics::resolve(topic).expect("a chapter of the manual");
                    assert!(target.exact, "{topic} names no heading");
                }
            }
        }
    }
}
