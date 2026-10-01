//! Placing the three handles on the selected facet, hit-testing them and lighting the
//! one under the pointer.

use super::{
    CachedFacetMap, SESSION, Shared, Target, kind_to_int, set_hint, slice, snap_mode, tier_label,
};
use crate::{
    EditorModel, MainWindow, ManipulateModel, SolidPreviewModel,
    gui::{
        editor::callbacks::{
            map_to_pick_coordinates, pick_margin, pick_pixels_to_logical, selected_facet_id,
        },
        solid_preview::{facet_map::FacetMap, preview_state::FrameGeometry},
    },
};
use glam::Vec3;
use indicatrix::{geometry::meet_solver::SolvedTier, optics::raytracer::Camera};
use indicatrix_editor::manipulate::{
    FacetFrame, HANDLE_HIT_RADIUS_PX, HandleKind, HandleLayout, ScreenPoint, ScreenSize,
    handle_layout, hit_test, text, tiers_meeting,
};
use slint::ComponentHandle as _;
use std::{rc::Rc, sync::PoisonError};

/// The field of view the solid raster and the path tracer share.
pub(super) const CAMERA_FOV_DEG: f32 = indicatrix::optics::raytracer::DEFAULT_FOV_DEG;

/// How long each handle is, as a fraction of the shown stone's bounding radius.
const HANDLE_LENGTH_FRACTION: f64 = 0.35;

/// The handles' anchor and tips in the viewport's LOGICAL coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct LogicalHandles {
    pub(super) anchor: (f32, f32),
    pub(super) angle_tip: (f32, f32),
    pub(super) depth_tip: (f32, f32),
    pub(super) index_tip: (f32, f32),
}

/// `layout` (pick-frame pixels) converted to logical viewport coordinates with the
/// inverse of `map_to_pick_coordinates`'s arithmetic: `(pick + margin) / scale`.
pub(super) fn layout_to_logical(
    layout: &HandleLayout,
    scale: f32,
    margin: (f32, f32),
) -> LogicalHandles {
    let convert = |p: ScreenPoint| pick_pixels_to_logical((p.x, p.y), scale, margin);
    LogicalHandles {
        anchor: convert(layout.anchor),
        angle_tip: convert(layout.angle_tip),
        depth_tip: convert(layout.depth_tip),
        index_tip: convert(layout.index_tip),
    }
}

/// The facet the handles anchor on: the last clicked facet when it belongs to `tier`
/// and has a centroid, else the tier's first facet that has one (a selection made from
/// the tier table names no facet).
pub(super) fn pick_facet(
    remembered: Option<u32>,
    tier: usize,
    facets_of_tier: &[u32],
    tier_of: impl Fn(u32) -> Option<usize>,
    has_centroid: impl Fn(u32) -> bool,
) -> Option<u32> {
    remembered
        .filter(|&id| tier_of(id) == Some(tier) && has_centroid(id))
        .or_else(|| facets_of_tier.iter().copied().find(|&id| has_centroid(id)))
}

/// Whether the solved masts describe the design's tiers one to one -- a stale cache
/// (a tier was just added or removed) would put every facet id on the wrong tier.
pub(super) const fn masts_aligned(tier_count: usize, masts_len: usize) -> bool {
    tier_count == masts_len
}

/// Whether the facet map and the frame's geometry share one facet-id space. They differ
/// while a frame is behind the design, and under the Cut slider (the mesh then holds
/// only the first tiers' planes); the handles hide rather than land on the wrong facet.
pub(super) const fn ids_aligned(map_facets: usize, centroid_count: usize) -> bool {
    map_facets == centroid_count
}

/// The handle whose tip lies within `radius` of `p`. A tier with no index-wheel
/// positions has no index handle to grab.
pub(super) fn hit_kind(
    layout: &HandleLayout,
    indexless: bool,
    p: ScreenPoint,
    radius: f32,
) -> Option<HandleKind> {
    let mut layout = *layout;
    if indexless {
        // A NaN tip is never within any radius.
        layout.index_tip = ScreenPoint::new(f32::NAN, f32::NAN);
    }
    hit_test(&layout, p, radius)
}

/// The solid-preview pick-frame pixel the logical pointer `(x, y)` lands on.
pub(super) fn pointer_to_pick(ui: &MainWindow, x: f32, y: f32) -> ScreenPoint {
    let (px, py) = map_to_pick_coordinates(ui, x, y);
    ScreenPoint::new(px, py)
}

/// `(generation, length)` of the cached solved masts, `None` before the first solve.
fn solved_summary(ctx: &Shared) -> Option<(u64, usize)> {
    ctx.solid_last_solved
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(generation, masts)| (*generation, masts.len()))
}

/// A copy of the cached solved masts when they describe exactly `tier_count` tiers.
pub(super) fn aligned_masts(ctx: &Shared, tier_count: usize) -> Option<Vec<SolvedTier>> {
    ctx.solid_last_solved
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .filter(|(_, masts)| masts_aligned(tier_count, masts.len()))
        .map(|(_, masts)| masts.clone())
}

/// The facet map of `design` against the cached masts, rebuilt only when the design
/// generation or the solved-masts generation moved since the last call. `None` when the
/// masts do not describe the design.
pub(super) fn facet_map_for(
    ctx: &Shared,
    design_generation: u64,
    design: &indicatrix_cut_core::Design,
) -> Option<Rc<FacetMap>> {
    let (solved_generation, masts_len) = solved_summary(ctx)?;
    if !masts_aligned(design.tiers.len(), masts_len) {
        return None;
    }
    let cached = SESSION.with(|cell| {
        cell.borrow()
            .facet_map
            .as_ref()
            .filter(|c| {
                c.design_generation == design_generation && c.solved_generation == solved_generation
            })
            .map(|c| Rc::clone(&c.map))
    });
    if cached.is_some() {
        return cached;
    }
    let masts = aligned_masts(ctx, design.tiers.len())?;
    let map = Rc::new(FacetMap::from_design(design, &masts));
    SESSION.with(|cell| {
        cell.borrow_mut().facet_map = Some(CachedFacetMap {
            design_generation,
            solved_generation,
            map: Rc::clone(&map),
        });
    });
    Some(map)
}

/// The centroid of `facet_id` in `centroids`, if it has one.
fn centroid_of(centroids: &[Option<Vec3>], facet_id: u32) -> Option<Vec3> {
    centroids.get(facet_id as usize).copied().flatten()
}

/// The handle target on `tier` of `design`, whose facets `map` describes and `geometry`
/// (the frame on screen) has centroids for: the `remembered` facet when it belongs to
/// the tier, else the tier's first facet with a centroid. `None` when the map and the
/// frame hold different facet-id spaces, no facet of the tier is on screen, or a
/// handle would land behind the camera.
pub(super) fn target_for(
    geometry: &FrameGeometry,
    map: &FacetMap,
    design: &indicatrix_cut_core::Design,
    tier: usize,
    remembered: Option<u32>,
    provisional: bool,
) -> Option<Target> {
    let tier_data = design.tiers.get(tier)?;
    let centroids = geometry.facet_centroids.as_slice();
    if !ids_aligned(map.facet_count(), centroids.len()) {
        return None;
    }
    let facet_id = pick_facet(
        remembered,
        tier,
        map.facets_of_tier(tier),
        |id| map.tier_of(id as usize),
        |id| centroid_of(centroids, id).is_some(),
    )?;
    let centroid = centroid_of(centroids, facet_id)?;
    let frame = FacetFrame::from_tier(
        tier_data,
        f64::from(map.index_on_gear(facet_id as usize)),
        design.meta.gear_teeth_abs(),
        centroid,
    );
    let pose = geometry.camera;
    let camera = Camera::new(pose.yaw, pose.pitch, pose.distance, CAMERA_FOV_DEG);
    let size = ScreenSize::new(geometry.size.0 as f32, geometry.size.1 as f32);
    let length = (HANDLE_LENGTH_FRACTION * geometry.bounding_radius) as f32;
    let layout = handle_layout(&frame, &camera, size, length)?;
    Some(Target {
        tier,
        frame,
        layout,
        label: tier_label(tier_data, tier),
        provisional,
    })
}

/// Works out where the handles go for the current selection and frame, or `None` when
/// they should be hidden: no selection, Diagram view, no geometry, an unsolved or
/// misaligned design, a tier with no facet on screen, or a facet behind the camera.
/// In Slice mode the only handles are the provisional tier's.
fn place(ui: &MainWindow, ctx: &Shared) -> Option<Target> {
    if ui.global::<SolidPreviewModel>().get_view_mode() == 3 {
        return None;
    }
    let geometry = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    if ui.global::<ManipulateModel>().get_slice_mode() {
        return slice::place_provisional(&geometry);
    }
    let tier = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()).ok()?;
    let st = ctx.state.try_borrow().ok()?;
    let map = facet_map_for(ctx, st.current_generation(), &st.design)?;
    target_for(
        &geometry,
        &map,
        &st.design,
        tier,
        selected_facet_id(),
        false,
    )
}

/// Re-places the handles for the frame that just landed, or hides them -- see [`place`].
pub(super) fn refresh_handles(ui: &MainWindow, ctx: &Shared) {
    match place(ui, ctx) {
        Some(target) => show(ui, target),
        None => hide(ui),
    }
}

/// Stores `target` in the session and pushes its logical positions to the model.
fn show(ui: &MainWindow, target: Target) {
    let scale = ui.window().scale_factor();
    let logical = layout_to_logical(&target.layout, scale, pick_margin(ui));
    let indexless = target.frame.is_indexless();
    let model = ui.global::<ManipulateModel>();
    model.set_anchor_x(logical.anchor.0);
    model.set_anchor_y(logical.anchor.1);
    model.set_angle_tip_x(logical.angle_tip.0);
    model.set_angle_tip_y(logical.angle_tip.1);
    model.set_depth_tip_x(logical.depth_tip.0);
    model.set_depth_tip_y(logical.depth_tip.1);
    model.set_index_tip_x(logical.index_tip.0);
    model.set_index_tip_y(logical.index_tip.1);
    model.set_index_visible(!indexless);
    model.set_handles_visible(true);
    SESSION.with(|cell| cell.borrow_mut().target = Some(target));
}

/// Hides the handles and drops the hover hint if it is ours.
fn hide(ui: &MainWindow) {
    let had_hint = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        session.target = None;
        session.hovered = None;
        std::mem::take(&mut session.hover_hint_active)
    });
    let model = ui.global::<ManipulateModel>();
    model.set_handles_visible(false);
    model.set_hovered_handle(-1);
    if had_hint {
        slice::resting_hint(ui);
    }
}

/// The handle under the logical pointer `(x, y)`, per the last placed layout.
fn hit_kind_at(ui: &MainWindow, x: f32, y: f32) -> Option<HandleKind> {
    let (layout, indexless) = SESSION.with(|cell| {
        cell.borrow()
            .target
            .as_ref()
            .map(|t| (t.layout, t.frame.is_indexless()))
    })?;
    let radius = HANDLE_HIT_RADIUS_PX * ui.window().scale_factor();
    hit_kind(&layout, indexless, pointer_to_pick(ui, x, y), radius)
}

/// `ManipulateModel.handle_hit_test`: the handle under the pointer as an int, `-1` none.
pub(super) fn hit_test_int(ui: &MainWindow, x: f32, y: f32) -> i32 {
    hit_kind_at(ui, x, y).map_or(-1, kind_to_int)
}

/// The hover hint for `kind` on the current target: what dragging does, how it snaps
/// and how many tiers meet this one by name.
fn hover_hint(ui: &MainWindow, ctx: &Shared, kind: HandleKind) -> Option<String> {
    let (tier, label, provisional) = SESSION.with(|cell| {
        cell.borrow()
            .target
            .as_ref()
            .map(|t| (t.tier, t.label.clone(), t.provisional))
    })?;
    // A provisional tier is pinned by a scale reference: nothing meets it by name.
    let meeting = if provisional {
        0
    } else {
        ctx.state
            .try_borrow()
            .map_or(0, |st| tiers_meeting(&st.design, tier).len())
    };
    let snap = snap_mode(false, ui.global::<ManipulateModel>().get_snap_off());
    Some(text::handle_hover_hint(kind, &label, meeting, snap))
}

/// Lights `kind`'s handle and words the hint (or clears both for `None`).
fn apply_hover(ui: &MainWindow, ctx: &Shared, kind: Option<HandleKind>) {
    let model = ui.global::<ManipulateModel>();
    model.set_hovered_handle(kind.map_or(-1, kind_to_int));
    let Some(hint) = kind.and_then(|kind| hover_hint(ui, ctx, kind)) else {
        // Off every handle: drop the hint, but only if it is still ours.
        let had_hint =
            SESSION.with(|cell| std::mem::take(&mut cell.borrow_mut().hover_hint_active));
        if had_hint {
            slice::resting_hint(ui);
        }
        return;
    };
    SESSION.with(|cell| cell.borrow_mut().hover_hint_active = true);
    set_hint(ui, &hint);
}

/// `ManipulateModel.handle_hover`: the pointer moved with no drag in progress.
pub(super) fn hover(ui: &MainWindow, ctx: &Shared, x: f32, y: f32) {
    if ui.global::<ManipulateModel>().get_dragging() {
        return;
    }
    let kind = hit_kind_at(ui, x, y);
    let changed = SESSION.with(|cell| {
        let mut session = cell.borrow_mut();
        let changed = session.hovered != kind;
        session.hovered = kind;
        changed
    });
    if changed {
        apply_hover(ui, ctx, kind);
    }
}

/// Re-words the hover hint for the handle still under the pointer (the Snap pill just
/// changed what dragging it would snap to).
pub(super) fn refresh_hover_hint(ui: &MainWindow, ctx: &Shared) {
    if ui.global::<ManipulateModel>().get_dragging() {
        return;
    }
    let hovered = SESSION.with(|cell| cell.borrow().hovered);
    if hovered.is_some() {
        apply_hover(ui, ctx, hovered);
    }
}
