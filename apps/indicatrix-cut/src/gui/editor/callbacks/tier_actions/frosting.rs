//! "Frosted" on a tier and on a single facet: [`Edit::SetTierFrosting`], one undo step each.
//!
//! Frosting changes no geometry, so -- like the cheater offset and the tier note -- it needs
//! no solid-preview replan. What it does need is the path tracer: the frosted planes live in
//! `RenderContext::active_frosted`, so each edit here recomputes them for the stone the
//! viewport shows and claims them under the same lock, so the render restarts at once instead of
//! waiting for the next background solve (which claims them too).

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex, PoisonError},
};

use indicatrix_cut_core::{Edit, TierFrosting};
use slint::ComponentHandle;

use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::{PlanesOwner, RenderContext},
    gui::{
        editor::{
            state::EditorState,
            view::{SolidLastSolved, refresh_editor_panel_stale},
        },
        show_toast,
        solid_preview::cut_slider,
    },
};

/// The tier position `position` names among `tier_index`'s facets: the tier's own value whose
/// `f32` is `position` (the chips carry positions as `f32`), so the value is the one the
/// design stores.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the chips carry f32 positions; the stored f64 is matched through the same rounding"
)]
fn facet_position(state: &EditorState, tier_index: usize, position: f32) -> Option<f64> {
    state
        .design
        .tiers
        .get(tier_index)?
        .indices
        .iter()
        .copied()
        .find(|&stored| (stored as f32).to_bits() == position.to_bits())
}

/// Hands the path tracer the frosted planes of the stone it shows after a frosting edit, when
/// the editor owns the render context's planes and the last solve fits the design (the planes,
/// and so which of them are frosted, only depend on the tiers and their masts, which the edit
/// did not touch). Otherwise does nothing: the background solve the edit schedules claims them.
fn push_frosted_planes(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid_last_solved: &SolidLastSolved,
    state: &EditorState,
) {
    let masts = solid_last_solved
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(_, masts)| masts.clone())
        .filter(|masts| masts.len() == state.design.tiers.len());
    let Some(masts) = masts else {
        return;
    };
    let steps = cut_slider::current_steps(ui, &state.design);
    let frosted = cut_slider::cut_geometry_no_solve(&state.design, Some(&masts), steps).frosted;
    let mut ctx = render_ctx.lock().unwrap_or_else(PoisonError::into_inner);
    if matches!(ctx.planes_owner, PlanesOwner::Editor { .. }) {
        ctx.claim_active_frosted(frosted);
        ctx.dirty = true;
    }
}

/// Applies `frosting` to the tier at `index` as one undoable edit and refreshes the panel and
/// the render.
fn apply_frosting(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid_last_solved: &SolidLastSolved,
    index: usize,
    frosting: Option<TierFrosting>,
) {
    let mut st = state.borrow_mut();
    if let Err(e) = st.apply(Edit::SetTierFrosting { index, frosting }) {
        show_toast(ui, &e.to_string(), "error");
        return;
    }
    refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::new());
    push_frosted_planes(ui, render_ctx, solid_last_solved, &st);
}

/// The inspector's "Frosted" tick box (`EditorModel.set_tier_frosted(int, bool)`): frosts the
/// whole tier, or makes it polished again. A tier frosted facet by facet that is ticked becomes
/// frosted as a whole; unticked it is polished (the facets' own buttons are the finer control).
pub(super) fn setup_set_tier_frosted_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_set_tier_frosted(move |tier_index: i32, frosted: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(tier_index) else {
                return;
            };
            apply_frosting(
                &ui,
                &state,
                &render_ctx,
                &solid_last_solved,
                index,
                frosted.then_some(TierFrosting::Tier),
            );
        });
}

/// A facet chip's frost button (`EditorModel.toggle_facet_frosted(int, float)`): flips that one
/// facet of the tier between frosted and polished
/// ([`indicatrix_cut_core::Design::frosting_with_facet_toggled`]).
pub(super) fn setup_toggle_facet_frosted_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_toggle_facet_frosted(move |tier_index: i32, position: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Ok(index) = usize::try_from(tier_index) else {
                return;
            };
            let frosting = {
                let st = state.borrow();
                let Some(stored) = facet_position(&st, index, position) else {
                    return;
                };
                st.design.frosting_with_facet_toggled(index, stored)
            };
            apply_frosting(
                &ui,
                &state,
                &render_ctx,
                &solid_last_solved,
                index,
                frosting,
            );
        });
}
