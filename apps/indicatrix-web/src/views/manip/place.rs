//! Placing the three handles on the selected facet, hit-testing them and lighting the
//! one under the pointer.

use super::{CachedFacetMap, VIEWS, cutoff, drag, set_hint, slice, with_views};
use crate::{
    AppModel, AppWindow, ManipulateModel,
    app::{Ctx, state::WebApp},
    views::state::{ViewsState, view_mode_for_tab},
};
use indicatrix_cut_core::Design;
use indicatrix_editor::manipulate::{
    HANDLE_HIT_RADIUS_PX, HandleKind, HandleTarget, ScreenPoint,
    gesture::{kind_to_int, snap_mode},
    provisional::PROVISIONAL_GENERATION,
    target::{masts_aligned, target_for},
    text, tiers_meeting,
};
use indicatrix_solid::{
    facet_map::FacetMap,
    preview::view::{ContainFit, contain_fit},
};
use slint::ComponentHandle;
use std::{rc::Rc, sync::Arc};

/// Where the last frame's raster sits in the view (`image-fit: contain`).
pub(super) fn fit_of(views: &ViewsState) -> Option<ContainFit> {
    let geometry = views.geometry.as_ref()?;
    contain_fit(
        views.view_size.0,
        views.view_size.1,
        geometry.size.0,
        geometry.size.1,
    )
}

/// The pick-frame position (fractional, not clamped) under the logical pointer `(x, y)`
/// of the last frame.
pub(super) fn pointer_to_pick(views: &ViewsState, x: f32, y: f32) -> Option<ScreenPoint> {
    let (px, py) = fit_of(views)?.to_image(x, y);
    Some(ScreenPoint::new(px, py))
}

/// The committed design and facet map the last planned frame came from, the map rebuilt
/// only when the solve cache moved. `None` when the masts do not describe the design
/// one to one.
fn committed_map(views: &mut ViewsState) -> Option<(Arc<Design>, Rc<FacetMap>)> {
    let rev = views.cache_rev;
    let (design, masts) = views.cache.as_ref()?;
    if !masts_aligned(design.tiers.len(), masts.len()) {
        return None;
    }
    let design = Arc::clone(design);
    if let Some(cached) = views.manip.facet_map.as_ref().filter(|c| c.rev == rev) {
        return Some((design, Rc::clone(&cached.map)));
    }
    let masts = views.cache.as_ref().map(|(_, masts)| masts.as_slice())?;
    let map = Rc::new(FacetMap::from_design(&design, masts));
    views.manip.facet_map = Some(CachedFacetMap {
        rev,
        map: Rc::clone(&map),
    });
    Some((design, map))
}

/// Works out where the handles go for the current selection and frame, or `None` when
/// they should be hidden: no selection, Diagram view, no geometry, an unsolved or
/// misaligned design, a tier with no facet on screen, or a facet behind the camera.
/// In Slice mode the only handles are the provisional tier's.
fn place_target(
    app: &WebApp,
    views: &mut ViewsState,
    view_mode: u8,
    slice_mode: bool,
) -> Option<HandleTarget> {
    if view_mode == 3 {
        return None;
    }
    let geometry = views.geometry.clone()?;
    if slice_mode {
        if views.frame_generation != PROVISIONAL_GENERATION {
            return None;
        }
        return views.manip.provisional.as_mut()?.place(&geometry);
    }
    // A provisional frame (or one behind the design) describes another facet-id space
    // than the committed cache: hide rather than land on the wrong facet.
    if views.frame_generation != views.cache_generation {
        return None;
    }
    let tier = app.selected_tier?;
    let (design, map) = committed_map(views)?;
    target_for(
        &geometry,
        &map,
        &design,
        tier,
        views.manip.remembered_facet,
        false,
    )
}

/// Re-places the handles for the frame that just landed, or hides them -- see
/// [`place_target`].
pub(super) fn refresh_handles(ui: &AppWindow, app: &WebApp, views: &mut ViewsState, view_mode: u8) {
    let slice_mode = ui.global::<ManipulateModel>().get_slice_mode();
    match place_target(app, views, view_mode, slice_mode) {
        Some(target) => show(ui, views, target),
        None => hide(ui, views),
    }
}

/// [`refresh_handles`] for a caller outside `refresh` (a tool that just changed what the
/// handles should be on).
pub(super) fn refresh_now(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let view_mode = view_mode_for_tab(ui.global::<AppModel>().get_view_tab()).unwrap_or(0);
    let app = ctx.state.borrow();
    with_views(|views| refresh_handles(&ui, &app, views, view_mode));
}

/// Stores `target` and pushes its logical positions to the model.
fn show(ui: &AppWindow, views: &mut ViewsState, target: HandleTarget) {
    let Some(fit) = fit_of(views) else {
        hide(ui, views);
        return;
    };
    let layout = &target.layout;
    let at = |p: ScreenPoint| fit.to_view(p.x, p.y);
    let (anchor, angle, depth, index) = (
        at(layout.anchor),
        at(layout.angle_tip),
        at(layout.depth_tip),
        at(layout.index_tip),
    );
    let model = ui.global::<ManipulateModel>();
    model.set_anchor_x(anchor.0);
    model.set_anchor_y(anchor.1);
    model.set_angle_tip_x(angle.0);
    model.set_angle_tip_y(angle.1);
    model.set_depth_tip_x(depth.0);
    model.set_depth_tip_y(depth.1);
    model.set_index_tip_x(index.0);
    model.set_index_tip_y(index.1);
    model.set_index_visible(!target.frame.is_indexless());
    model.set_handles_visible(true);
    views.manip.target = Some(target);
}

/// Hides the handles and drops the hover hint if it is ours.
pub(super) fn hide(ui: &AppWindow, views: &mut ViewsState) {
    views.manip.target = None;
    views.manip.hovered = None;
    let had_hint = std::mem::take(&mut views.manip.hover_hint_active);
    let model = ui.global::<ManipulateModel>();
    model.set_handles_visible(false);
    model.set_hovered_handle(-1);
    if had_hint {
        slice::resting_hint(ui, views);
    }
}

/// The handle under the logical pointer `(x, y)`, per the last placed layout. The grab
/// radius keeps its size on screen however the raster is scaled.
fn hit_kind_at(views: &ViewsState, x: f32, y: f32) -> Option<HandleKind> {
    let target = views.manip.target.as_ref()?;
    let fit = fit_of(views)?;
    let pick = pointer_to_pick(views, x, y)?;
    target.hit(pick, fit.length_to_image(HANDLE_HIT_RADIUS_PX))
}

/// `ManipulateModel.handle-hit-test`: the handle under the pointer as an int, `-1` none.
fn hit_test_int(x: f32, y: f32) -> i32 {
    VIEWS
        .with(|cell| hit_kind_at(&cell.borrow(), x, y))
        .map_or(-1, kind_to_int)
}

/// The hover hint for `kind` on the current target: what dragging does, how it snaps and
/// how many tiers meet this one by name.
fn hover_hint(ctx: &Ctx, ui: &AppWindow, kind: HandleKind) -> Option<String> {
    let (tier, label, provisional) = VIEWS.with(|cell| {
        cell.borrow()
            .manip
            .target
            .as_ref()
            .map(|t| (t.tier, t.label.clone(), t.provisional))
    })?;
    // A provisional tier is pinned by a scale reference: nothing meets it by name.
    let meeting = if provisional {
        0
    } else {
        ctx.state.try_borrow().map_or(0, |app| {
            app.design
                .as_ref()
                .map_or(0, |d| tiers_meeting(&d.session.design, tier).len())
        })
    };
    let snap = snap_mode(false, ui.global::<ManipulateModel>().get_snap_off());
    Some(text::handle_hover_hint(kind, &label, meeting, snap))
}

/// Lights `kind`'s handle and words the hint (or clears both for `None`).
fn apply_hover(ctx: &Ctx, ui: &AppWindow, kind: Option<HandleKind>) {
    let model = ui.global::<ManipulateModel>();
    model.set_hovered_handle(kind.map_or(-1, kind_to_int));
    let Some(hint) = kind.and_then(|kind| hover_hint(ctx, ui, kind)) else {
        // Off every handle: drop the hint, but only if it is still ours.
        with_views(|views| {
            if std::mem::take(&mut views.manip.hover_hint_active) {
                slice::resting_hint(ui, views);
            }
        });
        return;
    };
    with_views(|views| views.manip.hover_hint_active = true);
    set_hint(ui, &hint);
}

/// `ManipulateModel.handle-hover`: the pointer moved with no drag in progress.
fn hover(ctx: &Ctx, x: f32, y: f32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if ui.global::<ManipulateModel>().get_dragging() {
        return;
    }
    let (kind, changed) = with_views(|views| {
        let kind = hit_kind_at(views, x, y);
        let changed = views.manip.hovered != kind;
        views.manip.hovered = kind;
        (kind, changed)
    });
    if changed {
        apply_hover(ctx, &ui, kind);
    }
}

/// Re-words the hover hint for the handle still under the pointer (the Snap pill just
/// changed what dragging it would snap to).
fn refresh_hover_hint(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    if ui.global::<ManipulateModel>().get_dragging() {
        return;
    }
    let hovered = VIEWS.with(|cell| cell.borrow().manip.hovered);
    if hovered.is_some() {
        apply_hover(ctx, &ui, hovered);
    }
}

/// Runs after every shown frame (from `refresh`): re-places the handles on the new
/// frame, or, mid-drag, refreshes the "follows your drag" feedback; syncs the
/// provisional outline and, when something new landed, the resting hint.
pub fn frame_landed(ui: &AppWindow, app: &WebApp, views: &mut ViewsState, view_mode: u8) {
    let outline_new = slice::sync_outline(views);
    let cut = cutoff(ui);
    let cutoff_new = views.manip.last_cutoff.replace(cut) != Some(cut);
    let dragging = ui.global::<ManipulateModel>().get_dragging();
    if dragging {
        drag::update_feedback(ui, views);
    } else {
        refresh_handles(ui, app, views, view_mode);
    }
    if (outline_new || cutoff_new) && !dragging && !views.manip.hover_hint_active {
        slice::resting_hint(ui, views);
    }
}

/// Wires the pointer callbacks: hit-testing, hover and the Snap pill.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<ManipulateModel>();
    model.on_handle_hit_test(hit_test_int);
    let c = ctx.clone();
    model.on_handle_hover(move |x, y| hover(&c, x, y));
    let c = ctx.clone();
    model.on_snap_toggled(move || refresh_hover_hint(&c));
}
