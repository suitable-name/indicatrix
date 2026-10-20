//! Stage alignment detection and execution for guided tutorial steps.
//!
//! Evaluates whether the user's current view, panel visibility, inspector tab, and
//! accordion sections match the requirements derived by [`indicatrix_editor::guide::stage_for`].
//! When mismatched, informs [`GuideModel`] so the "Show Me Where" button can appear with
//! a contextual hint. When clicked, [`apply_stage`] navigates the application to the
//! expected stage.

use super::runtime;
use crate::{EditorModel, GuideModel, LibraryModel, MainWindow, ParametersModel};
use indicatrix_editor::guide::{LibraryPanel, StageView, stage_for};
use slint::ComponentHandle;

/// Human-readable label for inspector tab `tab`.
const fn inspector_tab_name(tab: i32) -> &'static str {
    match tab {
        0 => "Tier inspector",
        1 => "Preform inspector",
        2 => "Optimize inspector",
        3 => "Schedule inspector",
        4 => "History inspector",
        _ => "inspector",
    }
}

/// Human-readable label for parameters dock tab `tab`.
fn parameters_tab_name(tab: &str) -> &'static str {
    match tab {
        "stone" => "Stone parameters",
        "lighting" => "Lighting parameters",
        "render_output" => "Render parameters",
        _ => "parameters",
    }
}

/// Capitalizes the first character of string `s`.
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// Formulates a human-readable hint string from a list of required staging actions.
fn format_hint(actions: &[String]) -> String {
    match actions.len() {
        0 => String::new(),
        1 => capitalize(&actions[0]),
        2 => format!("{} & {}", capitalize(&actions[0]), actions[1]),
        _ => {
            let (last, head) = actions.split_last().unwrap();
            let mut formatted = Vec::with_capacity(head.len());
            for (i, action) in head.iter().enumerate() {
                if i == 0 {
                    formatted.push(capitalize(action));
                } else {
                    formatted.push(action.clone());
                }
            }
            format!("{} & {last}", formatted.join(", "))
        }
    }
}

/// Checks whether current application state matches the stage required for the current guide step.
///
/// If mismatched, updates `GuideModel.stage_mismatched` to `true` and sets a descriptive `stage_hint`.
/// If aligned, resets `GuideModel.stage_mismatched` to `false`.
pub(super) fn check_stage_alignment(ui: &MainWindow) {
    let guide_model = ui.global::<GuideModel>();
    if !guide_model.get_open() {
        return;
    }

    let guide_id = guide_model.get_guide_id();
    let step_index = guide_model.get_step_index();
    let Ok(step_index) = usize::try_from(step_index) else {
        return;
    };

    let Some(guide) = runtime::guide(guide_id.as_str()) else {
        return;
    };
    let wanted = stage_for(&guide, step_index);

    let active_tab = ui.get_active_tab();
    let render_view_tab = ui.get_render_view_tab();
    let is_edit = active_tab == 0 && render_view_tab == 1;
    let is_live_render = active_tab == 0 && render_view_tab == 0;
    let library_collapsed = ui.global::<LibraryModel>().get_panel_collapsed();

    let editor = ui.global::<EditorModel>();
    let inspector_tab = editor.get_inspector_tab();
    let inspector_collapsed = editor.get_inspector_collapsed();
    let tier_table_collapsed = editor.get_tier_table_collapsed();
    let settings_collapsed = editor.get_settings_collapsed();

    let params = ui.global::<ParametersModel>();
    let params_open = params.get_open();
    let params_tab = params.get_active_tab();

    let mut actions = Vec::new();

    // 1. View alignment
    match wanted.view {
        StageView::Edit => {
            if !is_edit {
                actions.push("switch to Edit view".to_string());
            }
        }
        StageView::LiveRender => {
            if !is_live_render {
                actions.push("switch to Live Render view".to_string());
            }
        }
        StageView::Either => {}
    }

    // 2. Library panel alignment
    match wanted.library {
        LibraryPanel::Hidden => {
            if !library_collapsed {
                actions.push("hide Library panel".to_string());
            }
        }
        LibraryPanel::Shown => {
            if library_collapsed {
                actions.push("open Library panel".to_string());
            }
        }
        LibraryPanel::Keep => {}
    }

    // 3. Inspector tab and collapsed state
    if let Some(t) = wanted.inspector_tab
        && (!is_edit || inspector_collapsed || inspector_tab != t)
    {
        actions.push(format!("open {}", inspector_tab_name(t)));
    }

    // 4. Accordion sections
    if wanted.expand_tier_table && tier_table_collapsed {
        actions.push("expand Tier Table".to_string());
    }

    if wanted.expand_settings && settings_collapsed {
        actions.push("open Design Settings".to_string());
    }

    // 5. Parameters dock
    if let Some(tab) = wanted.parameters_tab {
        let expected_idx = match tab {
            "lighting" => 1,
            "render_output" => 2,
            _ => 0,
        };
        if !params_open || params_tab != expected_idx {
            actions.push(format!("open {}", parameters_tab_name(tab)));
        }
    }

    if actions.is_empty() {
        guide_model.set_stage_mismatched(false);
    } else {
        let hint = format_hint(&actions);
        guide_model.set_stage_mismatched(true);
        guide_model.set_stage_hint(hint.into());
    }
}

/// Navigates the application UI to align with the current step's required stage.
///
/// Switches view tabs, toggles library panel visibility, selects inspector tabs,
/// expands required accordions, opens parameters tabs, and reframes the camera.
pub(super) fn apply_stage(ui: &MainWindow) {
    let guide_model = ui.global::<GuideModel>();
    if !guide_model.get_open() {
        return;
    }

    let guide_id = guide_model.get_guide_id();
    let step_index = guide_model.get_step_index();
    let Ok(step_index) = usize::try_from(step_index) else {
        return;
    };

    let Some(guide) = runtime::guide(guide_id.as_str()) else {
        return;
    };
    let wanted = stage_for(&guide, step_index);

    // 1. View transition
    match wanted.view {
        StageView::Edit => {
            ui.set_active_tab(0);
            ui.invoke_active_tab_changed(0);
            ui.set_render_view_tab(1);
            ui.invoke_render_view_tab_changed(1);
        }
        StageView::LiveRender => {
            if guide_model.invoke_allows_view_tabs() {
                ui.set_active_tab(0);
                ui.invoke_active_tab_changed(0);
                ui.set_render_view_tab(0);
                ui.invoke_render_view_tab_changed(0);
            }
        }
        StageView::Either => {}
    }

    // 2. Library panel
    let library = ui.global::<LibraryModel>();
    match wanted.library {
        LibraryPanel::Hidden => {
            library.set_panel_collapsed(true);
            library.invoke_panel_collapsed_changed(true);
        }
        LibraryPanel::Shown => {
            library.set_panel_collapsed(false);
            library.invoke_panel_collapsed_changed(false);
        }
        LibraryPanel::Keep => {}
    }

    // 3. Inspector
    let editor = ui.global::<EditorModel>();
    editor.set_inspector_collapsed(false);
    if let Some(t) = wanted.inspector_tab {
        editor.set_inspector_tab(t);
    }
    if wanted.expand_tier_table {
        editor.set_tier_table_collapsed(false);
    }
    if wanted.expand_settings {
        editor.set_settings_collapsed(false);
    }

    // 4. Parameters dock
    if let Some(tab) = wanted.parameters_tab {
        let idx = match tab {
            "lighting" => 1,
            "render_output" => 2,
            _ => 0,
        };
        ui.global::<ParametersModel>().invoke_show_tab(idx);
    }

    // 5. Camera reframe
    if let Some((yaw, pitch)) = wanted.focus.pose() {
        let _ = crate::gui::render::camera_handles::set_camera_pose(yaw, pitch, None);
    }

    // 6. Mark aligned
    guide_model.set_stage_mismatched(false);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capitalize_handles_empty_and_casing() {
        assert_eq!(capitalize(""), "");
        assert_eq!(capitalize("open Tier inspector"), "Open Tier inspector");
        assert_eq!(capitalize("Switch to Edit view"), "Switch to Edit view");
    }

    #[test]
    fn format_hint_single_action() {
        let actions = vec!["open Preform inspector".to_string()];
        assert_eq!(format_hint(&actions), "Open Preform inspector");

        let actions = vec!["expand Tier Table".to_string()];
        assert_eq!(format_hint(&actions), "Expand Tier Table");

        let actions = vec!["switch to Live Render view".to_string()];
        assert_eq!(format_hint(&actions), "Switch to Live Render view");
    }

    #[test]
    fn format_hint_two_actions() {
        let actions = vec![
            "switch to Edit view".to_string(),
            "open Tier inspector".to_string(),
        ];
        assert_eq!(
            format_hint(&actions),
            "Switch to Edit view & open Tier inspector"
        );
    }

    #[test]
    fn format_hint_three_actions() {
        let actions = vec![
            "switch to Edit view".to_string(),
            "hide Library panel".to_string(),
            "open Tier inspector".to_string(),
        ];
        assert_eq!(
            format_hint(&actions),
            "Switch to Edit view, hide Library panel & open Tier inspector"
        );
    }

    #[test]
    fn inspector_and_parameter_tab_names() {
        assert_eq!(inspector_tab_name(0), "Tier inspector");
        assert_eq!(inspector_tab_name(1), "Preform inspector");
        assert_eq!(inspector_tab_name(2), "Optimize inspector");
        assert_eq!(inspector_tab_name(3), "Schedule inspector");
        assert_eq!(inspector_tab_name(4), "History inspector");

        assert_eq!(parameters_tab_name("stone"), "Stone parameters");
        assert_eq!(parameters_tab_name("lighting"), "Lighting parameters");
        assert_eq!(parameters_tab_name("render_output"), "Render parameters");
    }
}
