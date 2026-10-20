//! Tests for the stage model and derivation rules (Lane S1).

use super::{
    CameraFocus, GuideCatalog, HIGHLIGHT_TARGETS, LibraryPanel, StageView, opening_stage,
    stage::OVERRIDES, stage_for,
};

#[test]
fn every_highlight_target_is_mapped_in_stage_derivation() {
    let dummy_guide = GuideCatalog::new()
        .all()
        .iter()
        .find(|g| g.id == "new-design-walkthrough")
        .expect("new-design-walkthrough exists")
        .clone();

    // Verify all targets in HIGHLIGHT_TARGETS produce consistent stages without panic.
    for &target in HIGHLIGHT_TARGETS {
        let mut guide = dummy_guide.clone();
        guide.steps[0].highlight_target = target.to_owned();
        let stage = stage_for(&guide, 0);

        match target {
            "inspector_tier" => {
                assert_eq!(stage.view, StageView::Edit);
                assert_eq!(stage.inspector_tab, Some(0));
            }
            "optimize_tab" => {
                assert_eq!(stage.view, StageView::Edit);
                assert_eq!(stage.inspector_tab, Some(2));
            }
            "history_tab" => {
                assert_eq!(stage.view, StageView::Edit);
                assert_eq!(stage.inspector_tab, Some(4));
            }
            "tier_table" => {
                assert_eq!(stage.view, StageView::Edit);
                assert!(stage.expand_tier_table);
            }
            "design_settings" => {
                assert_eq!(stage.view, StageView::Edit);
                assert!(stage.expand_settings);
            }
            "preform_tab"
            | "solve_button"
            | "auto_solve"
            | "deep_solve_button"
            | "retarget_button"
            | "cutting_mode_button"
            | "snapshot_button"
            | "compare_button"
            | "export_asc_button"
            | "export_menu"
            | "save_button"
            | "open_button"
            | "load_button"
            | "verdict_badge"
            | "solid_viewport"
            | "view_modes"
            | "cut_slider"
            | "snap_pill"
            | "slice_pill"
            | "new_design_dialog" => {
                assert_eq!(stage.view, StageView::Edit);
            }
            "lighting_combo" => {
                assert_eq!(stage.view, StageView::LiveRender);
            }
            "stone_tab" => {
                assert_eq!(stage.parameters_tab, Some("stone"));
            }
            "lighting_tab" => {
                assert_eq!(stage.parameters_tab, Some("lighting"));
            }
            "render_output_tab" => {
                assert_eq!(stage.parameters_tab, Some("render_output"));
            }
            "library_search" | "library_filters" | "library_import" => {
                assert_eq!(stage.library, LibraryPanel::Shown);
            }
            _ => {}
        }
    }
}

#[test]
fn every_builtin_guide_opening_stage_has_concrete_view() {
    let catalog = GuideCatalog::new();
    for guide in catalog.all() {
        let opening = opening_stage(guide);
        assert_ne!(
            opening.view,
            StageView::Either,
            "Guide {:?} has Either for opening_stage view",
            guide.id
        );
    }
}

#[test]
fn spot_check_new_design_walkthrough_step_2() {
    let catalog = GuideCatalog::new();
    let guide = catalog
        .get("new-design-walkthrough")
        .expect("new-design-walkthrough exists");

    // Step 2 is index 1.
    let stage = stage_for(guide, 1);
    assert_eq!(stage.view, StageView::Edit);
    assert_eq!(stage.inspector_tab, Some(0));
}

#[test]
fn spot_check_viewing_live_render_step_3() {
    let catalog = GuideCatalog::new();
    let guide = catalog
        .get("viewing-live-render")
        .expect("viewing-live-render exists");

    // Step 3 is index 2 ("Choose a lighting rig").
    let stage = stage_for(guide, 2);
    assert_eq!(stage.view, StageView::LiveRender);
}

#[test]
fn spot_check_library_search_steps_show_library() {
    let catalog = GuideCatalog::new();
    let guide = catalog
        .get("library-search")
        .expect("library-search exists");

    for (i, _) in guide.steps.iter().enumerate() {
        let stage = stage_for(guide, i);
        assert_eq!(
            stage.library,
            LibraryPanel::Shown,
            "library-search step {i} should have LibraryPanel::Shown"
        );
    }
}

#[test]
fn spot_check_solving_preform_step_1() {
    let catalog = GuideCatalog::new();
    let guide = catalog
        .get("solving-preform")
        .expect("solving-preform exists");

    // Step 1 is index 0 ("Open the Preform tab").
    let stage = stage_for(guide, 0);
    assert_eq!(stage.view, StageView::Edit);
    assert_ne!(
        stage.inspector_tab,
        Some(1),
        "solving-preform step 1 tab must not yet be Preform (tab 1)"
    );
}

#[test]
fn spot_check_tiers_step_series_focus() {
    let catalog = GuideCatalog::new();
    let guide = catalog
        .get("tiers-step-series")
        .expect("tiers-step-series exists");

    // Step 2 is index 1 ("Generate a crown ladder").
    let stage = stage_for(guide, 1);
    assert_eq!(stage.focus, CameraFocus::Crown);
}

#[test]
fn overrides_refer_only_to_existing_guides_and_steps() {
    let catalog = GuideCatalog::new();
    let all_guides = catalog.all();

    for &(g_id, s_title, _) in OVERRIDES {
        let guide = all_guides
            .iter()
            .find(|g| g.id == g_id)
            .unwrap_or_else(|| panic!("override specifies unknown guide id {g_id:?}"));

        if let Some(title) = s_title {
            let step_exists = guide.steps.iter().any(|s| s.title == title);
            assert!(
                step_exists,
                "override specifies unknown step title {title:?} for guide {g_id:?}"
            );
        }
    }
}
