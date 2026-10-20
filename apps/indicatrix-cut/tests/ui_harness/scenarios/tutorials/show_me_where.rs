//! Scenario `tutorial:show_me_where`: interactive verification of the "Show Me Where"
//! stage navigation button on the tutorial guide panel.

use super::{drive::wait_real, start_builtin};
use crate::{
    scenarios::main_window::click,
    support::{Ctx, app::pump, coverage, query::is_clickable_role},
};
use indicatrix_cut::{EditorModel, GuideModel, LibraryModel, harness_support};
use indicatrix_editor::guide::{GuideCatalog, WORKED_EXAMPLE_ID, stage_for};
use slint::ComponentHandle;
use std::time::Duration;

const SCOPE: &str = "tutorial:show_me_where";
const BUTTON_LABEL: &str = "Show Me Where";

fn has_show_me_where_button(ctx: &Ctx<'_>) -> bool {
    ctx.app
        .main_view()
        .by_label(BUTTON_LABEL)
        .into_iter()
        .any(|e| e.accessible_role().is_some_and(is_clickable_role))
}

/// Runs the `tutorial:show_me_where` scenario.
#[expect(
    clippy::too_many_lines,
    reason = "sequential 5-phase UI scenario covering mismatch, click, recovery, and drift"
)]
pub fn run(ctx: &mut Ctx<'_>) {
    ctx.report.set_context(SCOPE.to_owned());

    let catalog = GuideCatalog::new();
    let Some(guide) = catalog.get(WORKED_EXAMPLE_ID) else {
        ctx.report.add_simple(
            "MainWindow",
            "show-me-where-guide-missing",
            format!("{SCOPE}: guide {WORKED_EXAMPLE_ID} not found in catalog"),
        );
        return;
    };

    if !start_builtin(ctx, SCOPE, guide) {
        return;
    }

    // Advance to step 2 (index 1), which stages in Edit view with Tier inspector (tab 0),
    // Tier Table expanded, Library hidden, and Girdle camera focus.
    {
        let ui = ctx.app.ui();
        let guide_model = ui.global::<GuideModel>();
        if guide_model.get_step_index() == 0 {
            guide_model.invoke_next();
            pump(100);
        }
    }

    let wanted = stage_for(guide, 1);

    // 1. Pre-condition setup: intentionally force UI out of stage alignment
    // (Live Render view, Library open, Optimize inspector tab, collapsed Tier Table).
    {
        let ui = ctx.app.ui();
        ui.set_active_tab(0);
        ui.invoke_active_tab_changed(0);
        ui.set_render_view_tab(0);
        ui.invoke_render_view_tab_changed(0);

        let library = ui.global::<LibraryModel>();
        library.set_panel_collapsed(false);
        library.invoke_panel_collapsed_changed(false);

        let editor = ui.global::<EditorModel>();
        editor.set_inspector_tab(2);
        editor.set_tier_table_collapsed(true);

        ui.global::<GuideModel>().invoke_poll();
    }
    pump(100);

    // 2. State & DOM assertion before clicking "Show Me Where".
    {
        let ui = ctx.app.ui();
        let guide_model = ui.global::<GuideModel>();
        if !guide_model.get_stage_mismatched() {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-not-detected",
                format!(
                    "{SCOPE}: GuideModel.stage_mismatched is false despite Live Render + wrong tab"
                ),
            );
            return;
        }
        if guide_model.get_stage_hint().is_empty() {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-empty-hint",
                format!("{SCOPE}: GuideModel.stage_hint is empty when mismatched"),
            );
        }
        if !has_show_me_where_button(ctx) {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-button-missing",
                format!(
                    "{SCOPE}: '{BUTTON_LABEL}' button not found in accessible DOM when mismatched"
                ),
            );
            return;
        }
    }

    // 3. Interactive click via harness.
    if !click(ctx.app, BUTTON_LABEL) {
        ctx.report.add_simple(
            "MainWindow",
            "show-me-where-click-failed",
            format!("{SCOPE}: clicking '{BUTTON_LABEL}' failed"),
        );
        return;
    }
    coverage::record_callback("GuideModel.show_me_where");
    pump(150);

    // 4. Post-transition state verification.
    {
        let ui = ctx.app.ui();
        if ui.get_active_tab() != 0 || ui.get_render_view_tab() != 1 {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-view-not-switched",
                format!(
                    "{SCOPE}: expected Edit view (0, 1), got ({}, {})",
                    ui.get_active_tab(),
                    ui.get_render_view_tab()
                ),
            );
        }
        if !ui.global::<LibraryModel>().get_panel_collapsed() {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-library-not-collapsed",
                format!("{SCOPE}: expected Library panel to be collapsed"),
            );
        }
        let editor = ui.global::<EditorModel>();
        if editor.get_inspector_tab() != 0 {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-inspector-tab",
                format!(
                    "{SCOPE}: expected inspector_tab == 0, got {}",
                    editor.get_inspector_tab()
                ),
            );
        }
        if editor.get_tier_table_collapsed() {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-tier-table-collapsed",
                format!("{SCOPE}: expected tier_table_collapsed == false"),
            );
        }
        if ui.global::<GuideModel>().get_stage_mismatched() {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-still-mismatched",
                format!("{SCOPE}: stage_mismatched remained true after clicking '{BUTTON_LABEL}'"),
            );
        }
        if has_show_me_where_button(ctx) {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-button-still-visible",
                format!("{SCOPE}: '{BUTTON_LABEL}' button remained visible after aligning stage"),
            );
        }
        if let Some((expected_yaw, expected_pitch)) = wanted.focus.pose() {
            let (yaw, pitch, _) = harness_support::camera_pose();
            if (yaw - expected_yaw).abs() > 0.05 || (pitch - expected_pitch).abs() > 0.05 {
                ctx.report.add_simple(
                    "MainWindow",
                    "show-me-where-camera-pose",
                    format!(
                        "{SCOPE}: expected camera pose ({expected_yaw:.2}, {expected_pitch:.2}), got ({yaw:.2}, {pitch:.2})"
                    ),
                );
            }
        }
    }

    // 5. Dynamic drift re-check: user manually switches inspector tab to History (4).
    {
        let ui = ctx.app.ui();
        ui.global::<EditorModel>().set_inspector_tab(4);
        ui.global::<GuideModel>().invoke_poll();
    }
    pump(100);

    let drifted = wait_real(Duration::from_secs(2), || {
        ctx.app.ui().global::<GuideModel>().get_stage_mismatched()
    });
    if !drifted || !has_show_me_where_button(ctx) {
        ctx.report.add_simple(
            "MainWindow",
            "show-me-where-drift-not-detected",
            format!("{SCOPE}: manual tab drift did not re-enable '{BUTTON_LABEL}'"),
        );
    } else if click(ctx.app, BUTTON_LABEL) {
        pump(100);
        let ui = ctx.app.ui();
        if ui.global::<EditorModel>().get_inspector_tab() != 0
            || ui.global::<GuideModel>().get_stage_mismatched()
        {
            ctx.report.add_simple(
                "MainWindow",
                "show-me-where-drift-recovery-failed",
                format!("{SCOPE}: second '{BUTTON_LABEL}' click did not restore stage"),
            );
        }
    }

    ctx.app.ui().global::<GuideModel>().invoke_close();
    pump(50);
    ctx.check_health(SCOPE);
}
