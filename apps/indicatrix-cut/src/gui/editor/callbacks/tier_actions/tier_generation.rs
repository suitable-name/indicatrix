//! Generated tier series ("Generate steps") and mirroring a tier to the other block.
//!
//! Both edits live on `indicatrix_editor::EditorSession`
//! (`generate_step_series`, `mirror_tier_to_other_block`), shared with the web app's tier
//! table; the callbacks here only refresh and announce. The name-generation helpers they
//! (and Duplicate/Save Tier) share moved to `indicatrix_editor::loading`.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::{ComponentHandle, SharedString};

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

// The Duplicate naming helpers (`unique_duplicate_name`, `split_duplicate_suffix`) and the
// "Generate steps" form parser (`parse_step_series_form`) moved to
// `indicatrix_editor::loading`, where `EditorSession::duplicate_tier` and
// `generate_step_series` (shared with the web app's tier table) use them.

/// "Generate steps": builds `count` tiers via `ConstraintTier::step_series` and
/// applies them as one `Edit::Batch` of `Edit::AddTier`s, appended after the design's
/// current last tier -- see `indicatrix_editor::EditorSession::generate_step_series`
/// for the form parsing and the edit. Mirrors `editor_tier_table.slint`'s own call
/// site's argument order: name prefix, start-angle text, angle-step text, tier count, a
/// comma-separated index list shared by every generated tier, and an optional anchor.
///
/// `pub(super)` since [`super::tier_crud::setup_toggle_detach_callback`] is the one
/// call site that registers it.
pub(super) fn setup_generate_step_series_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_generate_step_series(
        move |name_prefix: SharedString,
              start_angle_text: SharedString,
              angle_step_text: SharedString,
              count: i32,
              indices_text: SharedString,
              anchor_text: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            match st.generate_step_series(
                &name_prefix,
                &start_angle_text,
                &angle_step_text,
                count,
                &indices_text,
                &anchor_text,
            ) {
                Ok(series) => {
                    let dirty: BTreeSet<usize> =
                        (series.start_index..series.start_index + series.added).collect();
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &dirty);
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        dirty,
                        false,
                    );
                    drop(st);
                    show_toast(&ui, &format!("Generated {} tier(s).", series.added), "info");
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// "Mirror tier to other block": duplicates the tier at `tier_index` to the opposite
/// block (angle negated, same indices/constraint/detached set, name suffixed by
/// `name_suffix`) and applies it as one `Edit::AddTier`, appended after the design's
/// current last tier -- see `indicatrix_editor::EditorSession::
/// mirror_tier_to_other_block`. A silent no-op for an out-of-range `tier_index`.
///
/// `pub(super)` for the same reason as [`setup_generate_step_series_callback`] above.
pub(super) fn setup_mirror_tier_to_other_block_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_mirror_tier_to_other_block(
        move |tier_index: i32, name_suffix: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let Ok(tier_index) = usize::try_from(tier_index) else {
                return;
            };
            match st.mirror_tier_to_other_block(tier_index, &name_suffix) {
                Ok(None) => {}
                Ok(Some(mirrored)) => {
                    let new_index = mirrored.new_index;
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::from([new_index]));
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::from([new_index]),
                        false,
                    );
                    drop(st);
                    ui.global::<EditorModel>()
                        .set_selected_tier_index(new_index as i32);
                    show_toast(&ui, &format!("Mirrored to {}", mirrored.label), "info");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );
}

// `setup_save_tier_callback`'s auto-name for a brand-new tier saved with a blank Name
// field (`unique_duplicate_name` does not cover this case -- that one only ever runs
// against an already-named source). The function now lives in
// `indicatrix_editor::loading` (the mouse-driven slice tool names its provisional tier
// the same way); this re-export keeps the old path for `super::tier_form`.
pub(super) use indicatrix_editor::loading::next_free_block_name;
