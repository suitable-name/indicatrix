//! Keeping the provisional slice: the one `Edit::AddTier` that turns the session into a
//! committed tier, and the cleanup after it.

use super::{
    NO_DEPTH_HINT, Shared, expire_if_stale, handles, provisional::keep_allowed, resting_hint,
    set_hint, tear_down, tier_label, with_provisional_ref,
};
use crate::{
    EditorModel, MainWindow, ManipulateModel,
    gui::{
        editor::view::{refresh_editor_panel_stale, submit_preview_replan},
        show_toast,
    },
};
use indicatrix_cut_core::{ConstraintTier, Edit};
use indicatrix_editor::manipulate::text;
use slint::ComponentHandle as _;
use std::collections::BTreeSet;

/// `ManipulateModel.slice_keep`: commits the provisional tier as ONE `Edit::AddTier`.
pub(in super::super) fn keep(ui: &MainWindow, ctx: &Shared) {
    if ui.global::<ManipulateModel>().get_dragging() || expire_if_stale(ui, ctx) {
        return;
    }
    let Some((tier, facet_count, surviving)) = with_provisional_ref(|p| {
        p.tier()
            .map(|tier| (tier.clone(), p.facet_count(), p.surviving))
    })
    .flatten() else {
        return;
    };
    // A freshly sliced tier sits at the tangency mast: its facet has no area, so
    // keeping it would add a tier that cuts nothing. Enter and the button both land here.
    if !keep_allowed(surviving) {
        set_hint(ui, NO_DEPTH_HINT);
        show_toast(ui, NO_DEPTH_HINT, "info");
        return;
    }
    let Ok(mut st) = ctx.state.try_borrow_mut() else {
        return;
    };
    let index = st.design.tiers.len();
    st.history.end_coalesce_run();
    match st.apply(Edit::AddTier {
        index,
        tier: tier.clone(),
    }) {
        Ok(()) => {
            let dirty = BTreeSet::from([index]);
            refresh_editor_panel_stale(ui, &ctx.render_ctx, &st, &dirty);
            submit_preview_replan(
                ui,
                &ctx.render_ctx,
                &ctx.preview_state,
                &ctx.solid_last_solved,
                &st,
                dirty,
                false,
            );
            drop(st);
            finish_keep(ui, ctx, index, &tier, facet_count);
        }
        Err(error) => {
            drop(st);
            show_toast(ui, &error.to_string(), "error");
        }
    }
}

/// After a successful Keep: drops the session, selects the new row, leaves Slice mode
/// and says what happened (and that Undo removes it).
fn finish_keep(ui: &MainWindow, ctx: &Shared, index: usize, tier: &ConstraintTier, count: usize) {
    drop(tear_down(ui, ctx));
    let model = ui.global::<ManipulateModel>();
    model.set_slice_mode(false);
    ui.global::<EditorModel>()
        .set_selected_tier_index(i32::try_from(index).unwrap_or(-1));
    handles::refresh_handles(ui, ctx);
    resting_hint(ui);
    let toast = text::slice_kept_toast(&tier_label(tier, index), count, tier.angle_deg);
    show_toast(ui, &toast, "info");
}
