//! The pointer and button handlers of the view: orbit, zoom, presets, hover and click.
//!
//! Picking works from what the render thread last drew: the block's edges and corners are
//! projected here (they are twenty points), a facet or a stone is read from the published
//! pick buffer of the frame on screen.
//!
//! A click that would add a cut does not add it at once: the second click of a double-click
//! would otherwise land on a model the first had already changed. The click waits for the
//! double-click interval (see [`ClickAdds`](super::state::ClickAdds)) and the cut is added
//! when a timer finds it still alone, or at once when the gesture turns out to be a
//! double-click. A double-click on empty space resets the view instead.

use super::{
    RESIZE_QUIET, ViewState, WHEEL_QUIET,
    picking::{BoxTarget, PICK_RADIUS_PX, pick_box},
    render::{Hover, camera_for, refit_distance, reset_pose},
    request_render,
    scene::{SCENE_RADIUS, SceneKind, StoneInfo},
    state::DOUBLE_CLICK_INTERVAL,
};
use crate::{
    RoughPlanModel,
    gui::rough_plan::{
        cut_faces::{canonical_normal, default_face},
        editing,
        format::to_i32,
        host::{Host, on_host},
    },
};
use indicatrix_cut_core::rough_plan::RoughCut;
use indicatrix_solid::preview::camera::{orbit_step, standard_view, zoom_step};
use slint::{ComponentHandle, TimerMode};
use std::{rc::Rc, sync::PoisonError, time::Instant};

/// What a click on the image does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ClickAction {
    /// Add the default cut of an edge or corner of the rough block.
    AddBoxCut(BoxTarget),
    /// Cut the model facet flat (face-from-view).
    AddFaceCut(usize),
    /// Select the cut whose face was clicked.
    SelectCut(usize),
    /// Select the design of the stone clicked in a result.
    SelectStone(usize),
    /// Clear the outlined design group of the shown result.
    ClearGroup,
    /// Nothing.
    Nothing,
}

/// What decides the action of a click besides what is under the pointer.
#[derive(Debug, Clone, Copy)]
struct ClickContext {
    /// What the click landed on.
    picked: Picked,
    /// No plan is running (only then the model can be edited).
    editable: bool,
    /// A result scene is being built, so what is on screen is about to change.
    building: bool,
    /// A result is shown rather than the model.
    result_view: bool,
    /// A design group of the shown result is outlined.
    group_selected: bool,
}

/// The action of a click: edge, corner and facet clicks edit the model, so they need an
/// idle planner; the rest only selects. While a result is being built nothing is clicked,
/// because the scene on screen is not the one being asked for.
#[must_use]
const fn click_action(context: &ClickContext) -> ClickAction {
    if context.building {
        return ClickAction::Nothing;
    }
    match context.picked {
        Picked::Box(target) if context.editable => ClickAction::AddBoxCut(target),
        Picked::Facet(facet) if context.editable => ClickAction::AddFaceCut(facet),
        Picked::Cut(cut) => ClickAction::SelectCut(cut),
        Picked::Stone(stone) => ClickAction::SelectStone(stone),
        Picked::Nothing if context.result_view && context.group_selected => ClickAction::ClearGroup,
        _ => ClickAction::Nothing,
    }
}

/// The hint over a facet while the face-from-view mode is armed.
const FACE_HINT: &str = "Click to cut this face flat";

/// The hint over the model while a plan runs and the model cannot be edited.
const LOCKED_HINT: &str = "Editing is locked while planning";

/// What the pointer is over.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Picked {
    /// Nothing that can be clicked.
    Nothing,
    /// An edge or corner of the rough block.
    Box(BoxTarget),
    /// A facet of the model (only looked for in the face-from-view mode).
    Facet(usize),
    /// The face of a cut of the model.
    Cut(usize),
    /// A stone of the selected result.
    Stone(usize),
}

/// Registers the view's callbacks on the planner window.
pub(super) fn register(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.on_view_resized(|width, height| on_host(|host| resized(host, width, height)));
    model.on_view_orbit(|dx, dy| on_host(|host| orbit(host, dx, dy)));
    model.on_view_zoom(|delta| on_host(|host| zoom(host, delta)));
    model.on_view_drag_begin(|| on_host(drag_begin));
    model.on_view_drag_end(|| on_host(drag_end));
    model.on_view_hover(|x, y| on_host(|host| hover(host, x, y)));
    model.on_view_click(|x, y| on_host(|host| click(host, x, y)));
    model.on_view_double_click(|x, y| on_host(|host| double_click(host, x, y)));
    model.on_view_preset(|kind| on_host(|host| preset(host, kind)));
    model.on_view_toggles_changed(|| on_host(request_render));
    model.on_cut_selected(|_row| on_host(super::cut_selected));
}

/// Runs `change` on the view state.
fn edit_view(host: &Rc<Host>, change: impl FnOnce(&mut ViewState)) {
    change(&mut host.session.borrow_mut().view);
}

/// Asks for what is under the pointer to be looked up again once the next frame is on
/// screen (the picture under the pointer changed without the pointer moving).
fn request_rehover(host: &Rc<Host>) {
    edit_view(host, |view| view.rehover = true);
}

/// The image's width over its height.
fn aspect_of(size: (f32, f32)) -> f32 {
    if size.1 > 0.0 { size.0 / size.1 } else { 1.5 }
}

/// The image area was laid out or resized (logical pixels). A size that keeps changing is
/// drawn at half size until it has been still for [`RESIZE_QUIET`], and the camera is
/// pulled back or in so the scene still fits the new shape.
fn resized(host: &Rc<Host>, width: f32, height: f32) {
    if !(width > 0.0 && height > 0.0) {
        return;
    }
    edit_view(host, |view| {
        let old = view.view_size;
        view.view_size = (width, height);
        if old.0 <= 0.0 {
            view.pose = reset_pose(width / height);
        } else if old != (width, height) {
            view.pose.distance = refit_distance(view.pose.distance, aspect_of(old), width / height);
            view.resizing = true;
            view.resize_timer
                .start(TimerMode::SingleShot, RESIZE_QUIET, || {
                    on_host(resize_settled);
                });
        }
    });
    request_render(host);
}

/// The image area has stopped changing: the full-size frame.
fn resize_settled(host: &Rc<Host>) {
    edit_view(host, |view| view.resizing = false);
    request_rehover(host);
    request_render(host);
}

/// Reads the size of the image area off the window and adopts it. The area's own change
/// notifications miss the size it has when it is first laid out (and its mount-time push
/// comes before any callback is registered), so showing the window pulls it.
pub(super) fn sync_size(host: &Rc<Host>) {
    resized(
        host,
        host.window.get_view_area_width(),
        host.window.get_view_area_height(),
    );
}

/// The pointer moved with the button held: the camera follows.
fn orbit(host: &Rc<Host>, dx: f32, dy: f32) {
    // Zoning builds: a drag that started on a zone handle moves the handle, not the camera.
    #[cfg(feature = "zoning")]
    if super::zoning_view::drag_move(host, dx, dy) {
        return;
    }
    edit_view(host, |view| view.pose = orbit_step(view.pose, dx, dy));
    request_render(host);
}

/// A wheel tick: zoom, at half size until the wheel has been still for a moment.
fn zoom(host: &Rc<Host>, delta: f32) {
    if delta == 0.0 {
        return;
    }
    edit_view(host, |view| {
        view.pose.distance = zoom_step(view.pose.distance, delta, SCENE_RADIUS);
        view.wheeling = true;
        view.wheel_timer
            .start(TimerMode::SingleShot, WHEEL_QUIET, || {
                on_host(wheel_settled);
            });
    });
    request_rehover(host);
    request_render(host);
}

/// The wheel has been still: the full-size frame.
fn wheel_settled(host: &Rc<Host>) {
    edit_view(host, |view| view.wheeling = false);
    request_rehover(host);
    request_render(host);
}

/// The pointer button went down on the image: no hover while the view is moved.
fn drag_begin(host: &Rc<Host>) {
    edit_view(host, |view| {
        view.dragging = true;
        view.hover = Hover::None;
    });
    host.window
        .global::<RoughPlanModel>()
        .set_view_hint("".into());
    // Zoning builds: a drag that starts on a zone handle edits the zone.
    #[cfg(feature = "zoning")]
    super::zoning_view::drag_begin(host);
}

/// The pointer button went up: the full-size frame, and the pointer looks at what is
/// under it again.
fn drag_end(host: &Rc<Host>) {
    edit_view(host, |view| view.dragging = false);
    #[cfg(feature = "zoning")]
    super::zoning_view::drag_end(host);
    request_rehover(host);
    request_render(host);
}

/// A double-click on empty space, or Reset: the standard view.
fn reset_view(host: &Rc<Host>) {
    edit_view(host, |view| {
        view.pose = reset_pose(aspect_of(view.view_size));
    });
    request_rehover(host);
    request_render(host);
}

/// A view button: 0 front, 1 top, 2 side, 3 reset.
fn preset(host: &Rc<Host>, kind: i32) {
    if kind == 3 {
        reset_view(host);
        return;
    }
    // `standard_view` numbers its views 0 front, 1 top, 4 right.
    let standard = match kind {
        1 => 1,
        2 => 4,
        _ => 0,
    };
    edit_view(host, |view| {
        view.pose = standard_view(standard, view.pose, SCENE_RADIUS);
    });
    request_rehover(host);
    request_render(host);
}

/// The facet under the point `cursor` (logical pixels) of the frame on screen.
fn facet_under(view: &ViewState, serial: u64, cursor: (f32, f32)) -> Option<usize> {
    let picks = &view.runtime.as_ref()?.picks;
    // Only the reference count is taken under the lock; the buffer is read outside it.
    let snapshot = picks.lock().unwrap_or_else(PoisonError::into_inner).clone();
    if snapshot.serial != serial || view.view_size.0 <= 0.0 || view.view_size.1 <= 0.0 {
        return None;
    }
    snapshot.facet_at(cursor.0 / view.view_size.0, cursor.1 / view.view_size.1)
}

/// What is under `cursor` (logical pixels) in the shown scene. The stones of a result are
/// not offered while another result's scene is being built: the scene on screen is the
/// previous one.
fn pick_at(view: &ViewState, cursor: (f32, f32), armed: bool) -> Picked {
    let Some(scene) = &view.scene else {
        return Picked::Nothing;
    };
    match &scene.kind {
        SceneKind::Model(_) if armed => {
            facet_under(view, scene.serial, cursor).map_or(Picked::Nothing, Picked::Facet)
        }
        SceneKind::Model(model) => {
            let camera = camera_for(view.pose);
            let size = (
                view.view_size.0.round() as u32,
                view.view_size.1.round() as u32,
            );
            if model.block
                && let Some(target) = pick_box(
                    &camera,
                    size,
                    model.half_extents,
                    &model.cut_planes,
                    cursor,
                    PICK_RADIUS_PX,
                )
            {
                return Picked::Box(target);
            }
            facet_under(view, scene.serial, cursor)
                .and_then(|facet| model.cut_of_facet(facet))
                .map_or(Picked::Nothing, Picked::Cut)
        }
        SceneKind::Fit(_) if !view.fit_pickable() => Picked::Nothing,
        SceneKind::Fit(fit) => facet_under(view, scene.serial, cursor)
            .and_then(|facet| fit.stone_at_facet(facet))
            .map_or(Picked::Nothing, Picked::Stone),
    }
}

/// How the frame marks `picked` and what the hint strip says about it.
fn describe(view: &ViewState, picked: Picked) -> (Hover, String) {
    match picked {
        Picked::Nothing => (Hover::None, String::new()),
        Picked::Box(target) => (Hover::Box(target), target.hint()),
        Picked::Facet(facet) => (Hover::Facet(facet), FACE_HINT.to_string()),
        Picked::Cut(cut) => (Hover::None, format!("Click to select cut {}", cut + 1)),
        Picked::Stone(stone) => {
            let hint = match view.scene.as_deref().map(|scene| &scene.kind) {
                Some(SceneKind::Fit(fit)) => fit.info.get(stone).map(StoneInfo::hint),
                _ => None,
            };
            (Hover::None, hint.unwrap_or_default())
        }
    }
}

/// Whether the shown scene is the rough being modelled.
fn model_shown(view: &ViewState) -> bool {
    matches!(
        view.scene.as_deref().map(|scene| &scene.kind),
        Some(SceneKind::Model(_))
    )
}

/// The pointer moved over the image without a button (or left it, at a negative point).
fn hover(host: &Rc<Host>, x: f32, y: f32) {
    // Zoning builds: the pointer lights a zone handle under it and leaves the rest alone.
    #[cfg(feature = "zoning")]
    if super::zoning_view::hover(host, x, y) {
        return;
    }
    let model = host.window.global::<RoughPlanModel>();
    let armed = model.get_face_from_view_armed();
    let editable = !model.get_running();
    let inside = x >= 0.0 && y >= 0.0;
    let (mark, hint) = {
        let mut guard = host.session.borrow_mut();
        let view = &mut guard.view;
        view.pointer = inside.then_some((x, y));
        if !inside || view.dragging {
            (Hover::None, String::new())
        } else if model_shown(view) && !editable {
            (Hover::None, LOCKED_HINT.to_string())
        } else {
            describe(view, pick_at(view, (x, y), armed))
        }
    };
    if model.get_view_hint().as_str() != hint {
        model.set_view_hint(hint.into());
    }
    let changed = {
        let mut session = host.session.borrow_mut();
        let changed = session.view.hover != mark;
        session.view.hover = mark;
        changed
    };
    if changed {
        request_render(host);
    }
}

/// Looks up what is under the pointer again, where it last was (the picture under it
/// changed while it stood still).
pub(super) fn rehover(host: &Rc<Host>) {
    let pointer = host.session.borrow().view.pointer;
    if let Some((x, y)) = pointer {
        hover(host, x, y);
    }
}

/// A click that asks for a cut: an earlier click that this one does not belong to is
/// added now, and this one waits out the double-click interval.
fn queue_add(host: &Rc<Host>, picked: Picked, at: (f32, f32)) {
    let earlier = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        session
            .view
            .adds
            .settle(at, Instant::now(), session.revision)
    };
    if let Some(earlier) = earlier {
        perform_add(host, earlier);
    }
    // The earlier cut may have moved the revision on; this click saw the model as it is now.
    let mut guard = host.session.borrow_mut();
    let session = &mut *guard;
    session
        .view
        .adds
        .click(picked, at, Instant::now(), session.revision);
    session
        .view
        .add_timer
        .start(TimerMode::SingleShot, DOUBLE_CLICK_INTERVAL, || {
            on_host(add_due);
        });
}

/// The double-click interval after a click ran out: the click is added if nothing
/// replaced or cancelled it, the model is the one it saw, and it can still be edited.
fn add_due(host: &Rc<Host>) {
    let running = host.window.global::<RoughPlanModel>().get_running();
    let due = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        let may_edit = !running && model_shown(&session.view) && !session.view.building;
        session.view.adds.due(session.revision, may_edit)
    };
    if let Some(picked) = due {
        perform_add(host, picked);
    }
}

/// Adds the cut a click on `picked` asked for to the model as a committed edit, clears
/// the hover mark and hint, and looks under the pointer again.
fn perform_add(host: &Rc<Host>, picked: Picked) {
    let model = host.window.global::<RoughPlanModel>();
    if model.get_running() {
        return;
    }
    // A size typed but not committed yet is part of the rough being cut.
    if !editing::commit_pending_sizes(host) {
        return;
    }
    let cut = match picked {
        Picked::Box(target) => {
            let extents = host.session.borrow().model.base.bounding_box_extents();
            Some(target.default_cut(extents))
        }
        Picked::Facet(facet) => cut_from_facet(host, facet),
        _ => None,
    };
    let Some(cut) = cut else {
        return;
    };
    if matches!(picked, Picked::Facet(_)) {
        model.set_face_from_view_armed(false);
    }
    edit_view(host, |view| view.hover = Hover::None);
    model.set_view_hint("".into());
    editing::add_committed_cut(host, cut);
    request_rehover(host);
}

/// A double-click on the image at `(x, y)`. The click waiting to add a cut (the first half
/// of the gesture, or the second, which replaced it) is added now rather than after the
/// interval, and the camera stays where it is. Without a waiting click, a double-click on
/// empty space resets the view; one on a cut face or a stone does nothing beyond what its
/// two clicks did.
fn double_click(host: &Rc<Host>, x: f32, y: f32) {
    let model = host.window.global::<RoughPlanModel>();
    let running = model.get_running();
    let armed = model.get_face_from_view_armed();
    let pending = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        session.view.add_timer.stop();
        let may_edit = !running && model_shown(&session.view) && !session.view.building;
        session.view.adds.due(session.revision, may_edit)
    };
    if let Some(picked) = pending {
        perform_add(host, picked);
        return;
    }
    let on_nothing = pick_at(&host.session.borrow().view, (x, y), armed) == Picked::Nothing;
    if on_nothing {
        reset_view(host);
    }
}

/// The Face cut of model facet `facet`: the facet's plane pushed in by the default depth.
fn cut_from_facet(host: &Rc<Host>, facet: usize) -> Option<RoughCut> {
    let session = host.session.borrow();
    let scene = session.view.scene.as_deref()?;
    let SceneKind::Model(model) = &scene.kind else {
        return None;
    };
    let normal = model.facet_normal(facet)?;
    Some(default_face(
        &session.model.base,
        canonical_normal([normal.x, normal.y, normal.z]),
    ))
}

/// A click on the image: in the model view it asks for the cut under the pointer (added
/// after the double-click interval), in a result it selects the design of the stone under
/// the pointer.
fn click(host: &Rc<Host>, x: f32, y: f32) {
    // Zoning builds: a click on a zone handle is not a click on the model, and while the wizard
    // paints polished windows a click on the mesh paints.
    #[cfg(feature = "zoning")]
    if super::zoning_view::click(host, x, y) {
        return;
    }
    let model = host.window.global::<RoughPlanModel>();
    let armed = model.get_face_from_view_armed();
    let (picked, building) = {
        let session = host.session.borrow();
        (pick_at(&session.view, (x, y), armed), session.view.building)
    };
    let context = ClickContext {
        picked,
        editable: !model.get_running(),
        building,
        result_view: model.get_view_mode() == 1,
        group_selected: model.get_selected_group() >= 0,
    };
    match click_action(&context) {
        ClickAction::AddBoxCut(_) | ClickAction::AddFaceCut(_) => queue_add(host, picked, (x, y)),
        ClickAction::SelectCut(cut) => {
            model.set_selected_cut(to_i32(cut));
            super::cut_selected(host);
        }
        ClickAction::SelectStone(stone) => select_stone(host, stone),
        ClickAction::ClearGroup => {
            model.set_selected_group(-1);
            request_render(host);
        }
        ClickAction::Nothing => {}
    }
    request_rehover(host);
}

/// Selects the design group of stone `stone` of the shown result. Nothing is selected
/// while another result is being built: the scene on screen is not that result's.
fn select_stone(host: &Rc<Host>, stone: usize) {
    let group = {
        let session = host.session.borrow();
        let view = &session.view;
        if !view.fit_pickable() {
            return;
        }
        view.scene.as_deref().and_then(|scene| match &scene.kind {
            SceneKind::Fit(fit) => fit.stone_group.get(stone).copied(),
            SceneKind::Model(_) => None,
        })
    };
    if let Some(group) = group {
        host.window
            .global::<RoughPlanModel>()
            .set_selected_group(to_i32(group));
        request_render(host);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gui::rough_plan::view::scene::{FitScene, ModelScene, PiecePosition, Scene};
    use indicatrix::geometry::stone_metrics::SolidMesh;
    use indicatrix_solid::raster::project_point;
    use std::sync::Arc;

    const VIEW: (f32, f32) = (800.0, 600.0);

    fn model_view(block: bool) -> ViewState {
        ViewState {
            view_size: VIEW,
            scene: Some(Arc::new(Scene::new(SceneKind::Model(ModelScene {
                mesh: SolidMesh::default(),
                base_facets: 6,
                cut_count: 1,
                half_extents: [0.6, 0.5, 0.4],
                block,
                cut_planes: Vec::new(),
            })))),
            ..ViewState::default()
        }
    }

    fn fit_view() -> ViewState {
        let fit = FitScene {
            info: vec![StoneInfo {
                title: "Barion".to_string(),
                carat: 0.62,
                position: Some(PiecePosition {
                    slab: 2,
                    bar: 1,
                    piece: 3,
                }),
                note: String::new(),
            }],
            ..FitScene::default()
        };
        ViewState {
            view_size: VIEW,
            scene: Some(Arc::new(Scene::new(SceneKind::Fit(Box::new(fit))))),
            ..ViewState::default()
        }
    }

    /// The pixel 4 px below the middle of the Top-Front edge of the model view's box.
    fn near_top_front_edge(view: &ViewState) -> (f32, f32) {
        let camera = camera_for(view.pose);
        let ends = BoxTarget::Edge(0).points([0.6, 0.5, 0.4]);
        let a = project_point(&camera, ends[0], 800, 600).expect("in front of the camera");
        let b = project_point(&camera, ends[1], 800, 600).expect("in front of the camera");
        (a.0.midpoint(b.0), a.1.midpoint(b.1) + 4.0)
    }

    fn context(picked: Picked) -> ClickContext {
        ClickContext {
            picked,
            editable: true,
            building: false,
            result_view: false,
            group_selected: false,
        }
    }

    #[test]
    fn a_click_on_the_box_asks_for_its_cut_unless_the_planner_runs() {
        let edge = BoxTarget::Edge(3);
        assert_eq!(
            click_action(&context(Picked::Box(edge))),
            ClickAction::AddBoxCut(edge)
        );
        assert_eq!(
            click_action(&context(Picked::Facet(4))),
            ClickAction::AddFaceCut(4)
        );
        for picked in [Picked::Box(edge), Picked::Facet(4)] {
            let running = ClickContext {
                editable: false,
                ..context(picked)
            };
            assert_eq!(click_action(&running), ClickAction::Nothing, "{picked:?}");
        }
    }

    #[test]
    fn selecting_clicks_work_while_a_plan_runs() {
        let busy = ClickContext {
            editable: false,
            ..context(Picked::Cut(2))
        };
        assert_eq!(click_action(&busy), ClickAction::SelectCut(2));
        let busy_stone = ClickContext {
            picked: Picked::Stone(7),
            ..busy
        };
        assert_eq!(click_action(&busy_stone), ClickAction::SelectStone(7));
    }

    #[test]
    fn nothing_is_clicked_while_a_result_is_being_built() {
        for picked in [
            Picked::Box(BoxTarget::Corner(1)),
            Picked::Facet(2),
            Picked::Cut(0),
            Picked::Stone(3),
            Picked::Nothing,
        ] {
            let building = ClickContext {
                building: true,
                result_view: true,
                group_selected: true,
                ..context(picked)
            };
            assert_eq!(click_action(&building), ClickAction::Nothing, "{picked:?}");
        }
    }

    #[test]
    fn a_click_on_nothing_clears_the_outline_only_in_a_result_with_one() {
        let mut nothing = context(Picked::Nothing);
        assert_eq!(click_action(&nothing), ClickAction::Nothing);
        nothing.result_view = true;
        assert_eq!(click_action(&nothing), ClickAction::Nothing);
        nothing.group_selected = true;
        assert_eq!(click_action(&nothing), ClickAction::ClearGroup);
        nothing.result_view = false;
        assert_eq!(click_action(&nothing), ClickAction::Nothing);
    }

    #[test]
    fn nothing_is_picked_without_a_scene() {
        let view = ViewState {
            view_size: VIEW,
            ..ViewState::default()
        };
        assert_eq!(pick_at(&view, (400.0, 300.0), false), Picked::Nothing);
        assert_eq!(pick_at(&view, (400.0, 300.0), true), Picked::Nothing);
    }

    #[test]
    fn the_edge_under_the_pointer_is_picked_on_a_block() {
        let view = model_view(true);
        let cursor = near_top_front_edge(&view);
        assert_eq!(
            pick_at(&view, cursor, false),
            Picked::Box(BoxTarget::Edge(0))
        );
        // Far from the box nothing is picked.
        assert_eq!(pick_at(&view, (3.0, 3.0), false), Picked::Nothing);
    }

    #[test]
    fn a_cut_away_edge_is_not_picked() {
        // A plane y <= 0.3 takes the whole Top-Front edge (at y = 0.5) away.
        let mut view = model_view(true);
        let cursor = near_top_front_edge(&view);
        let Some(scene) = view.scene.take() else {
            panic!("a scene");
        };
        let Ok(mut scene) = Arc::try_unwrap(scene) else {
            panic!("the only owner");
        };
        let SceneKind::Model(model) = &mut scene.kind else {
            panic!("a model scene");
        };
        model.cut_planes = vec![(glam::Vec3::Y, 0.3)];
        view.scene = Some(Arc::new(scene));
        assert_ne!(
            pick_at(&view, cursor, false),
            Picked::Box(BoxTarget::Edge(0)),
            "the edge no longer exists"
        );
    }

    #[test]
    fn other_bases_offer_no_edges_and_the_face_mode_looks_for_facets_only() {
        let cylinder = model_view(false);
        let cursor = near_top_front_edge(&cylinder);
        assert_eq!(pick_at(&cylinder, cursor, false), Picked::Nothing);
        // Armed, the box edges are not offered either (a facet is wanted); without a
        // rendered frame to read a facet from, nothing is under the pointer.
        let block = model_view(true);
        assert_eq!(pick_at(&block, cursor, true), Picked::Nothing);
    }

    #[test]
    fn a_result_scene_picks_stones_from_the_frame_only() {
        let view = fit_view();
        assert_eq!(pick_at(&view, (400.0, 300.0), false), Picked::Nothing);
    }

    #[test]
    fn a_result_scene_offers_no_stones_while_another_result_is_built() {
        let mut view = fit_view();
        assert!(view.fit_pickable());
        view.begin_build(1);
        assert!(!view.fit_pickable());
        assert_eq!(pick_at(&view, (400.0, 300.0), false), Picked::Nothing);
        // The model scene is not affected by a build under way.
        let mut model = model_view(true);
        model.begin_build(1);
        let cursor = near_top_front_edge(&model);
        assert_eq!(
            pick_at(&model, cursor, false),
            Picked::Box(BoxTarget::Edge(0))
        );
    }

    #[test]
    fn the_hover_mark_and_the_hint_follow_what_is_picked() {
        let model = model_view(true);
        assert_eq!(
            describe(&model, Picked::Nothing),
            (Hover::None, String::new())
        );
        let edge = BoxTarget::Edge(0);
        assert_eq!(
            describe(&model, Picked::Box(edge)),
            (Hover::Box(edge), "Click to cut edge Top-Front".to_string())
        );
        assert_eq!(
            describe(&model, Picked::Facet(4)),
            (Hover::Facet(4), FACE_HINT.to_string())
        );
        assert_eq!(
            describe(&model, Picked::Cut(2)),
            (Hover::None, "Click to select cut 3".to_string()),
            "rows are numbered from 1"
        );
    }

    #[test]
    fn a_stone_hint_names_design_weight_and_position_in_the_cut_plan() {
        let view = fit_view();
        assert_eq!(
            describe(&view, Picked::Stone(0)),
            (
                Hover::None,
                "Barion \u{00B7} 0.62 ct \u{00B7} slab 2, bar 1, piece 3".to_string()
            )
        );
        // A stone the scene has no info for gets no hint, and a model scene has none.
        assert_eq!(describe(&view, Picked::Stone(9)).1, "");
        assert_eq!(describe(&model_view(true), Picked::Stone(0)).1, "");
    }
}
