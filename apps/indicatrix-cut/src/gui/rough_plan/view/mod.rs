//! The planner window's 3D view: the rough being modelled, or a selected result.
//!
//! # Threads
//!
//! The UI thread keeps the camera, the drag and hover state and the current scene, and only
//! ever sends requests and applies answers:
//!
//! - the **render thread** draws frames (latest request wins) and publishes the pick buffer
//!   of the last raster for hover and click lookups;
//! - the **scene thread** builds the fit scene of a result (design meshes are read from the
//!   catalogue there, and cached until the design's library row changes);
//! - the **thumbnail thread** draws every result once, in rank order, and is never
//!   latest-wins.
//!
//! An answer carries the generation, ticket or batch it belongs to and is dropped when a
//! newer one was issued in the meantime. A worker that panics on a request reports it, so
//! the UI stops waiting for the answer.
//!
//! # Drag rule
//!
//! While the pointer button is held, while wheel ticks keep coming and while the window is
//! being resized, frames are drawn at half size; the full size follows on release and after
//! 120 ms of wheel quiet (150 ms after the last resize).
//!
//! # State
//!
//! [`ViewState`] and the transitions that need no window (which result's scene is shown or
//! kept, which click is waiting out its double-click) live in `state`.

mod compose;
mod design_mesh;
mod interaction;
mod picking;
mod render;
mod scene;
mod state;
mod thumbnails;
mod workers;
#[cfg(feature = "zoning")]
mod zoning_view;

pub(super) use self::state::ViewState;
#[cfg(feature = "zoning")]
pub(super) use self::zoning_view::{ColourJob, design_info, redraw};
use self::{
    design_mesh::MeshLibrary,
    render::{Hover, RenderOptions, RenderRequest, SharedPick},
    scene::{ModelScene, RoughMesh, Scene, SceneKind, StoneDraw},
    state::{Runtime, SelectStep},
    thumbnails::{ThumbnailBatch, ThumbnailItem, ThumbnailWorker},
    workers::BuildRequest,
};
use super::{
    host::{Host, on_host},
    run::DesignStatus,
    shape_worker,
};
use crate::{RoughPlanModel, gui::show_toast};
use indicatrix_cut_core::rough_plan::{RoughLayout, RoughModel};
use slint::{ComponentHandle, Image, Model};
use std::{
    collections::{BTreeMap, BTreeSet},
    rc::Rc,
    sync::Arc,
    time::Duration,
};

/// How long the wheel must be still before the full-size frame is drawn.
const WHEEL_QUIET: Duration = Duration::from_millis(120);

/// How long the image area must keep its size before the full-size frame is drawn.
const RESIZE_QUIET: Duration = Duration::from_millis(150);

/// The longest edge of a frame in pixels, whatever the display scale.
const MAX_FRAME_EDGE: f32 = 1600.0;

/// The size in pixels of the next frame: the image area at the display scale (at most
/// [`MAX_FRAME_EDGE`] on its longer side), halved while the view is being moved. `(0, 0)`
/// until the area is laid out.
#[must_use]
fn frame_size(view_size: (f32, f32), scale_factor: f32, interactive: bool) -> (u32, u32) {
    let (width, height) = view_size;
    if !(width > 0.0 && height > 0.0) {
        return (0, 0);
    }
    let mut scale = scale_factor.max(0.5);
    let longest = width.max(height) * scale;
    if longest > MAX_FRAME_EDGE {
        scale *= MAX_FRAME_EDGE / longest;
    }
    if interactive {
        scale *= 0.5;
    }
    (
        (width * scale).round().max(1.0) as u32,
        (height * scale).round().max(1.0) as u32,
    )
}

/// Registers the `view_*` and selection callbacks on the planner window and starts the
/// view's threads.
pub(super) fn setup_view_callbacks(host: &Rc<Host>) {
    let meshes = Arc::new(MeshLibrary::new(Arc::clone(&host.db)));
    #[cfg(feature = "zoning")]
    zoning_view::register_meshes(&meshes);
    let picks: SharedPick = Arc::default();
    let window = host.window.as_weak();
    let runtime = Runtime {
        renderer: workers::spawn_render_worker(window.clone(), Arc::clone(&picks)),
        builder: workers::spawn_build_worker(window.clone(), Arc::clone(&meshes)),
        thumbnails: ThumbnailWorker::spawn(window, meshes),
        picks,
    };
    host.session.borrow_mut().view.runtime = Some(runtime);
    interaction::register(host);
    let model = host.window.global::<RoughPlanModel>();
    model.on_select_result(|index| on_host(|host| select_result(host, index)));
    model.on_select_group(|result, group| on_host(|host| select_group(host, result, group)));
}

/// Adopts the image area's current size (see [`interaction::sync_size`]): called when the
/// planner window is shown.
pub(super) fn sync_view_size(host: &Rc<Host>) {
    interaction::sync_size(host);
}

/// Sends the current scene to the render thread with the current camera and switches. The
/// display scale is read again each time, so a window dragged to another display draws at
/// that display's resolution.
fn request_render(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    let armed = model.get_face_from_view_armed();
    let options = RenderOptions {
        show_cut_faces: model.get_show_cut_faces(),
        show_rough: model.get_show_rough(),
        show_saw: model.get_show_saw(),
        show_stones: model.get_show_stones(),
        selected_cut: usize::try_from(model.get_selected_cut()).ok(),
        highlight_group: usize::try_from(model.get_selected_group()).ok(),
        hover: Hover::None,
    };
    let scale = host.window.window().scale_factor();
    let mut guard = host.session.borrow_mut();
    let view = &mut guard.view;
    view.scale_factor = scale;
    let size = frame_size(view.view_size, view.scale_factor, view.interactive());
    let (Some(scene), Some(runtime)) = (view.scene.clone(), view.runtime.as_ref()) else {
        return;
    };
    if size.0 == 0 || size.1 == 0 {
        return;
    }
    if matches!(view.hover, Hover::Facet(_)) && !armed {
        view.hover = Hover::None;
    }
    view.generation += 1;
    runtime.renderer.submit(RenderRequest {
        generation: view.generation,
        scene,
        pose: view.pose,
        size,
        options: RenderOptions {
            hover: view.hover,
            ..options
        },
    });
}

/// Makes `scene` the one shown and draws it; without a scene the image is emptied.
fn show_scene(host: &Rc<Host>, scene: Option<Arc<Scene>>) {
    let empty = scene.is_none();
    {
        let mut session = host.session.borrow_mut();
        let view = &mut session.view;
        view.scene = scene;
        if empty {
            view.generation += 1;
            view.applied = view.generation;
        }
    }
    if empty {
        host.window
            .global::<RoughPlanModel>()
            .set_view_image(Image::default());
    } else {
        request_render(host);
    }
}

/// A finished frame arrives from the render thread: shown unless a newer one already is.
/// When the picture under the pointer changed since the pointer last moved, what is under
/// the pointer is looked up again in it.
fn frame_ready(host: &Rc<Host>, generation: u64, pixels: render::Pixels) {
    let (newer, rehover) = {
        let mut session = host.session.borrow_mut();
        let view = &mut session.view;
        let newer = generation > view.applied;
        if newer {
            view.applied = generation;
        }
        let rehover = newer && view.rehover && !view.dragging;
        if rehover {
            view.rehover = false;
        }
        (newer, rehover)
    };
    if newer {
        // Zoning builds: the handles of the wizard's selected zone are drawn into the frame.
        #[cfg(feature = "zoning")]
        let pixels = {
            let mut pixels = pixels;
            zoning_view::draw_overlay(host, &mut pixels);
            pixels
        };
        host.window
            .global::<RoughPlanModel>()
            .set_view_image(Image::from_rgba8(pixels));
    }
    if rehover {
        interaction::rehover(host);
    }
}

/// Mirrors whether a result scene is being built into the window (`view_busy`), which shows
/// it in the hint strip.
fn push_busy(host: &Rc<Host>) {
    let building = host.session.borrow().view.building;
    let model = host.window.global::<RoughPlanModel>();
    if model.get_view_busy() != building {
        model.set_view_busy(building);
    }
}

/// A finished fit scene arrives from the scene thread: shown unless a newer build was asked
/// for since.
fn scene_ready(host: &Rc<Host>, ticket: u64, scene: Arc<Scene>) {
    let current = host.session.borrow_mut().view.finish_build(ticket, scene);
    if current {
        push_busy(host);
        // The result view starts with its scene, not while the model is still on screen.
        host.window.global::<RoughPlanModel>().set_view_mode(1);
        request_render(host);
    }
}

/// The drawing or the building of a result failed (a worker panicked, or is gone): the
/// view stops waiting for it, forgets which result it was for, and says so. Selecting the
/// result again tries again.
fn draw_failed(host: &Rc<Host>) {
    host.session.borrow_mut().view.fail_build();
    push_busy(host);
    if let Some(main) = host.main.upgrade() {
        show_toast(&main, "Could not draw this result", "error");
    }
}

/// The thumbnails of batch `generation` could not be drawn: the skeleton rows stop
/// waiting, unless a newer batch has taken over.
fn thumbnails_failed(host: &Rc<Host>, generation: u64) {
    if host.session.borrow().view.thumbnail_generation == generation {
        host.window
            .global::<RoughPlanModel>()
            .set_thumbnails_pending(false);
    }
}

/// The result index `index` names when it is one of `count` results.
#[must_use]
fn result_index(index: i32, count: usize) -> Option<usize> {
    usize::try_from(index).ok().filter(|&i| i < count)
}

/// The design group selected after result `result` is picked while `previous_result` was
/// selected: the group stays only within the same result.
#[must_use]
const fn group_after_select(previous_result: i32, result: i32, group: i32) -> i32 {
    if previous_result == result { group } else { -1 }
}

/// The group selected after a click on group row `clicked`: a second click on the same row
/// clears the outline.
#[must_use]
const fn toggled_group(current: i32, clicked: i32) -> i32 {
    if current == clicked { -1 } else { clicked }
}

/// Whether the model differs from the one the results were planned for.
#[must_use]
fn edited_since_plan(planned: Option<&RoughModel>, model: &RoughModel) -> bool {
    planned.is_some_and(|planned| planned != model)
}

/// A thumbnail arrives from its thread: put into the row of its rank, unless it belongs to
/// an older batch of results.
fn thumbnail_ready(
    host: &Rc<Host>,
    generation: u64,
    index: usize,
    pixels: render::Pixels,
    last: bool,
) {
    if host.session.borrow().view.thumbnail_generation != generation {
        return;
    }
    let model = host.window.global::<RoughPlanModel>();
    let results = model.get_results();
    if let Some(mut row) = results.row_data(index) {
        row.thumbnail = Image::from_rgba8(pixels);
        row.has_thumbnail = true;
        results.set_row_data(index, row);
    }
    if last {
        model.set_thumbnails_pending(false);
    }
}

/// How each stone of `layout` is drawn, by the stone's entry id: its own design as planned,
/// a changed design or the design it was matched to by title (both scaled to the recorded
/// width), or a grey box for a deleted design.
fn mesh_ids_for(
    layout: &RoughLayout,
    statuses: &BTreeMap<i64, DesignStatus>,
) -> BTreeMap<i64, StoneDraw> {
    let ids: BTreeSet<i64> = layout.stones.iter().map(|stone| stone.entry_id).collect();
    ids.into_iter()
        .map(|id| {
            let draw = match statuses.get(&id) {
                Some(DesignStatus::Deleted) => StoneDraw::Deleted,
                Some(DesignStatus::Changed) => StoneDraw::Changed(id),
                Some(DesignStatus::MatchedByTitle { resolved_entry_id }) => {
                    StoneDraw::Matched(*resolved_entry_id)
                }
                Some(DesignStatus::Unchanged) | None => StoneDraw::Design(id),
            };
            (id, draw)
        })
        .collect()
}

/// Shows the model scene. A fit scene still being built is no longer wanted: its ticket is
/// retired so that it is dropped when it arrives.
fn show_model(host: &Rc<Host>) {
    let scene = host.session.borrow_mut().view.show_model();
    host.window.global::<RoughPlanModel>().set_view_mode(0);
    push_busy(host);
    show_scene(host, scene);
}

/// Starts building the fit scene of result `index`; the model view (and its mode) stays
/// until the scene is ready. A worker that is gone counts as a failed draw.
fn show_fit(host: &Rc<Host>, index: usize) {
    let submitted = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        let (Some(layout), Some(rough)) = (
            session.run.layouts.get(index),
            session.view.rough_mesh.clone(),
        ) else {
            return;
        };
        let ticket = session.view.begin_build(index);
        let request = BuildRequest {
            ticket,
            epoch: session.view.plan_epoch,
            layout: layout.clone(),
            rough,
            titles: session.run.titles.clone(),
            mesh_ids: mesh_ids_for(layout, &session.run.statuses),
            #[cfg(feature = "zoning")]
            colour: super::zoning_hooks::colour_job(index),
        };
        session
            .view
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.builder.submit(request))
    };
    push_busy(host);
    if !submitted {
        draw_failed(host);
    }
}

/// A result is selected in the window: its scene is shown (built, or kept from before),
/// and its highlight cleared unless it was the selected one already. A negative index is
/// the "Rough" side of the view switch: no result is selected and the model is shown.
fn select_result(host: &Rc<Host>, index: i32) {
    if index < 0 {
        deselect_result(host);
        return;
    }
    let count = host.session.borrow().run.layouts.len();
    let Some(result) = result_index(index, count) else {
        return;
    };
    let model = host.window.global::<RoughPlanModel>();
    model.set_selected_group(group_after_select(
        model.get_selected_result(),
        index,
        model.get_selected_group(),
    ));
    model.set_selected_result(index);
    let step = host.session.borrow_mut().view.select(result);
    match step {
        SelectStep::Idle => {}
        SelectStep::Redraw | SelectStep::ShowKept => {
            push_busy(host);
            model.set_view_mode(1);
            request_render(host);
        }
        SelectStep::Build => show_fit(host, result),
    }
}

/// No result is selected any more: the model is shown.
fn deselect_result(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    model.set_selected_result(-1);
    model.set_selected_group(-1);
    show_model(host);
}

/// A design row of a result is clicked: its stones are outlined, and a second click on the
/// same row clears the outline.
fn select_group(host: &Rc<Host>, result: i32, group: i32) {
    select_result(host, result);
    let model = host.window.global::<RoughPlanModel>();
    model.set_selected_group(toggled_group(model.get_selected_group(), group));
    if !host.session.borrow().view.building {
        request_render(host);
    }
}

/// Leaves the result view for the model view when the model was edited away from the one
/// the shown results were planned for.
fn leave_result_if_edited(host: &Rc<Host>) {
    let edited = {
        let session = host.session.borrow();
        edited_since_plan(session.run.plan_model.as_ref(), &session.model)
    };
    if edited {
        let model = host.window.global::<RoughPlanModel>();
        if model.get_selected_result() >= 0 {
            model.set_selected_result(-1);
            model.set_selected_group(-1);
        }
    }
}

/// Whether a result of the plan is selected.
fn showing_result(host: &Rc<Host>) -> bool {
    let count = host.session.borrow().run.layouts.len();
    result_index(
        host.window.global::<RoughPlanModel>().get_selected_result(),
        count,
    )
    .is_some()
}

/// The shape worker stored a new model mesh in the session: the model view redraws it. An
/// edit away from the planned model also leaves the selected result; a refresh of the same
/// model (the selected cut, the material) does not, and keeps the scene it has: the
/// outline of the selected cut is a render option, not part of the scene.
pub(super) fn model_changed(host: &Rc<Host>) {
    let (edited, reuse) = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        let edited = session.view.last_model.as_ref() != Some(&session.model);
        if edited {
            session.view.last_model = Some(session.model.clone());
        }
        let reuse = !edited && session.view.model_scene.is_some() && session.model_mesh.is_some();
        (edited, reuse)
    };
    if edited {
        leave_result_if_edited(host);
    }
    if !reuse {
        let scene = {
            let session = host.session.borrow();
            session.model_mesh.as_ref().map(|mesh| {
                Arc::new(Scene::new(SceneKind::Model(ModelScene::new(
                    mesh,
                    &session.model,
                ))))
            })
        };
        host.session.borrow_mut().view.model_scene = scene;
    }
    if !showing_result(host) {
        show_model(host);
    }
}

/// Starts the thumbnails of the shown results, or stops the running batch when there are
/// none.
fn start_thumbnails(host: &Rc<Host>) {
    let pending = {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        session.view.thumbnail_generation += 1;
        let generation = session.view.thumbnail_generation;
        let run = &session.run;
        let Some(runtime) = &session.view.runtime else {
            return;
        };
        match &session.view.rough_mesh {
            Some(rough) if !run.layouts.is_empty() => {
                // A dead thumbnail thread must not leave the skeleton rows waiting.
                runtime.thumbnails.start(ThumbnailBatch {
                    generation,
                    epoch: session.view.plan_epoch,
                    rough: Arc::clone(rough),
                    titles: run.titles.clone(),
                    // The zoning build needs each layout's index for its colour job; the
                    // default build keeps the plain map it always had.
                    #[cfg(feature = "zoning")]
                    items: run
                        .layouts
                        .iter()
                        .enumerate()
                        .map(|(index, layout)| ThumbnailItem {
                            layout: layout.clone(),
                            mesh_ids: mesh_ids_for(layout, &run.statuses),
                            colour: super::zoning_hooks::colour_job(index),
                        })
                        .collect(),
                    #[cfg(not(feature = "zoning"))]
                    items: run
                        .layouts
                        .iter()
                        .map(|layout| ThumbnailItem {
                            layout: layout.clone(),
                            mesh_ids: mesh_ids_for(layout, &run.statuses),
                        })
                        .collect(),
                })
            }
            _ => {
                runtime.thumbnails.cancel(generation);
                false
            }
        }
    };
    host.window
        .global::<RoughPlanModel>()
        .set_thumbnails_pending(pending);
}

/// The results changed (a plan finished, a saved plan was loaded, or the list was
/// cleared): the scenes built for the old list are forgotten and the new plan's rough gets
/// a world mesh that all its scenes and thumbnails share (built by the first worker that
/// needs it), the selection is checked, the view shows it, and the thumbnails restart. The
/// design meshes are kept: a worker checks them against the library once for the new plan
/// and drops those whose design was edited.
pub(super) fn results_changed(host: &Rc<Host>) {
    // Zoning builds: the plan's rough colour (if it has one) is read before the scenes and
    // thumbnails of the new results are asked for.
    #[cfg(feature = "zoning")]
    super::zoning_hooks::refresh(host);
    {
        let mut guard = host.session.borrow_mut();
        let session = &mut *guard;
        session.view.results_replaced();
        session.view.rough_mesh = session
            .run
            .plan_model
            .clone()
            .map(|model| Arc::new(RoughMesh::new(model)));
    }
    let count = host.session.borrow().run.layouts.len();
    let model = host.window.global::<RoughPlanModel>();
    let selected = result_index(model.get_selected_result(), count);
    if selected.is_none() {
        model.set_selected_result(-1);
    }
    model.set_selected_group(-1);
    match selected {
        Some(index) => show_fit(host, index),
        None => show_model(host),
    }
    start_thumbnails(host);
}

/// The selected cut changed: its figures are asked for again and its face is outlined.
fn cut_selected(host: &Rc<Host>) {
    shape_worker::selected_cut_changed(host);
    request_render(host);
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{
        Axis, CutOrder, CutPlan, PlacedStone, RoughBase, RoughCut, StonePose,
    };

    fn layout_of(ids: &[i64]) -> RoughLayout {
        let stones = ids
            .iter()
            .map(|&entry_id| PlacedStone {
                entry_id,
                piece_origin_mm: [0.0; 3],
                piece_size_mm: [1.0; 3],
                stone_size_mm: [1.0; 3],
                table_axis: Axis::Y,
                carat: 0.1,
                volume_mm3: 1.0,
                pose: StonePose {
                    center_mm: [0.5; 3],
                    axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                    mm_per_unit: 1.0,
                },
            })
            .collect();
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones,
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 0.1,
            total_volume_mm3: 1.0,
            yield_fraction: 0.1,
            exact_fit: false,
        }
    }

    #[test]
    fn frames_follow_the_view_size_the_display_scale_and_the_drag_rule() {
        assert_eq!(frame_size((800.0, 560.0), 1.0, false), (800, 560));
        assert_eq!(frame_size((800.0, 560.0), 1.0, true), (400, 280));
        assert_eq!(frame_size((800.0, 560.0), 1.5, false), (1200, 840));
        // A large display is capped on its longer side.
        assert_eq!(frame_size((1000.0, 700.0), 2.0, false), (1600, 1120));
        assert_eq!(frame_size((1000.0, 700.0), 2.0, true), (800, 560));
        // Not laid out yet, or a degenerate area.
        assert_eq!(frame_size((0.0, 0.0), 1.0, false), (0, 0));
        assert_eq!(frame_size((100.0, -3.0), 1.0, false), (0, 0));
        assert_eq!(frame_size((f32::NAN, 100.0), 1.0, false), (0, 0));
    }

    #[test]
    fn a_stone_is_drawn_by_the_standing_of_its_design() {
        let layout = layout_of(&[4, 4, 9, 12, 15]);
        let statuses = BTreeMap::from([
            (9, DesignStatus::Deleted),
            (
                12,
                DesignStatus::MatchedByTitle {
                    resolved_entry_id: 77,
                },
            ),
            (4, DesignStatus::Changed),
            (15, DesignStatus::Unchanged),
        ]);
        let ids = mesh_ids_for(&layout, &statuses);
        assert_eq!(
            ids,
            BTreeMap::from([
                (4, StoneDraw::Changed(4)),
                (9, StoneDraw::Deleted),
                (12, StoneDraw::Matched(77)),
                (15, StoneDraw::Design(15)),
            ])
        );
        assert_eq!(
            mesh_ids_for(&layout, &BTreeMap::new()),
            BTreeMap::from([
                (4, StoneDraw::Design(4)),
                (9, StoneDraw::Design(9)),
                (12, StoneDraw::Design(12)),
                (15, StoneDraw::Design(15)),
            ])
        );
    }

    #[test]
    fn a_result_index_must_name_one_of_the_results() {
        assert_eq!(result_index(0, 3), Some(0));
        assert_eq!(result_index(2, 3), Some(2));
        assert_eq!(result_index(3, 3), None, "one past the last result");
        assert_eq!(result_index(-1, 3), None, "nothing selected");
        assert_eq!(result_index(0, 0), None, "no results at all");
        assert_eq!(result_index(i32::MAX, 10), None);
    }

    #[test]
    fn a_design_outline_belongs_to_the_result_it_was_picked_in() {
        // Picking the selected result again keeps the outline; another result drops it.
        assert_eq!(group_after_select(4, 4, 2), 2);
        assert_eq!(group_after_select(4, 5, 2), -1);
        assert_eq!(group_after_select(-1, 0, 1), -1, "from no selection");
        assert_eq!(group_after_select(4, 4, -1), -1, "no outline stays none");
    }

    #[test]
    fn a_second_click_on_a_group_row_clears_its_outline() {
        assert_eq!(toggled_group(-1, 2), 2);
        assert_eq!(toggled_group(2, 2), -1);
        assert_eq!(toggled_group(1, 2), 2, "another row moves the outline");
    }

    #[test]
    fn only_a_changed_model_leaves_the_planned_results() {
        let planned = RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            Vec::new(),
        );
        let mut edited = planned.clone();
        edited.cuts.push(RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 1.0,
        });
        assert!(!edited_since_plan(Some(&planned), &planned.clone()));
        assert!(edited_since_plan(Some(&planned), &edited));
        assert!(
            !edited_since_plan(None, &edited),
            "no plan, nothing to leave"
        );
    }

    #[test]
    fn a_new_view_state_starts_at_the_reset_pose_with_nothing_shown() {
        let view = ViewState::default();
        assert_eq!(view.pose, render::reset_pose(1.5));
        assert!(view.scene.is_none() && view.runtime.is_none());
    }
}
