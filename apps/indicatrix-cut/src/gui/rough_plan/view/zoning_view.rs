//! The Rough colour wizard's reach into the rough view (`zoning` feature only): coloured result
//! previews, the zone handles drawn on the model view and dragged with the mouse, and the
//! polished-window brush on the mesh.
//!
//! This file holds only glue to the planner's private state. The logic is in
//! `gui::rough_colour` and has its tests there (`preview`, `handles`, `mesh_paint`, `zone_link`,
//! `wizard::state`); nothing in this module is tested, because tests of `gui::rough_plan` are
//! never run.
//!
//! * **Previews.** A result's scene is built from the layout with the cutter's pose choice and
//!   its stones coloured by the plan's rough colour ([`prepare`], [`ColourJob`]); the builder and
//!   the thumbnail threads call [`prepare`] and the scene applies the colours
//!   (`FitScene::apply_stone_colours`).
//! * **Handles.** While the model view is shown and the wizard has a zone selected on its Fit
//!   step, the zone's handles are drawn into every frame ([`draw_overlay`]), the pointer lights
//!   the handle under it ([`hover`]), and a drag that starts on a handle edits the zone instead of
//!   orbiting the camera ([`drag_begin`], [`drag_move`], [`drag_end`]). The edits go to the wizard
//!   (`zone_link`), which applies them through `apply_with_locks` and counts the whole drag as
//!   one undo step.
//! * **Brush.** A click while the wizard paints polished windows becomes a ray in the rough's
//!   frame ([`mesh_ray`]); the wizard finds the triangles under it.

use super::{
    design_mesh::{MeshLibrary, MeshSource},
    render::{Pixels, camera_for},
    request_render,
    scene::{SceneKind, WorldFrame},
};
use crate::{
    RoughPlanModel,
    gui::{
        rough_colour::{
            handles::{
                HIT_RADIUS_PX, Handle, ViewMap, drag_edit, draw_overlay as draw_handles, overlay,
                pick_handle,
            },
            mesh_paint::MeshRay,
            preview::{PreviewInputs, posed_layout, stone_colours},
            zone_link,
        },
        rough_plan::host::Host,
    },
};
use indicatrix_cut_core::rough_plan::{RoughLayout, zoned_plan::DesignPlacement};
use indicatrix_solid::preview::CameraPose;
use slint::ComponentHandle;
use std::{cell::RefCell, collections::BTreeMap, rc::Rc, sync::Arc};

/// What a result's scene needs to show the plan's colour: the plan's inputs and the result's
/// number (the pose choice is stored per result).
#[derive(Debug, Clone)]
pub(in crate::gui::rough_plan) struct ColourJob {
    /// The plan's rough colour, host material and stored pose choices.
    pub(in crate::gui::rough_plan) inputs: Arc<PreviewInputs>,
    /// The result's place in the plan's results, from 0.
    pub(in crate::gui::rough_plan) layout_index: u32,
}

/// The layout a result is drawn from and the colour of each of its stones (`None` keeps the
/// palette colour): the cutter's pose choice applied, the colours from the plan's rough colour.
/// Without a job the layout is unchanged and no stone has a colour. Runs on the builder and
/// thumbnail threads; it reads the design placements from `meshes`.
pub(in crate::gui::rough_plan) fn prepare(
    layout: &RoughLayout,
    job: Option<&ColourJob>,
    meshes: &MeshLibrary,
) -> (RoughLayout, Vec<Option<[u8; 3]>>) {
    let Some(job) = job else {
        return (layout.clone(), Vec::new());
    };
    let posed = posed_layout(layout, job.layout_index, &job.inputs.choices);
    let placements: RefCell<BTreeMap<i64, Option<DesignPlacement>>> = RefCell::default();
    let colours = stone_colours(&job.inputs, &posed, &|entry_id| {
        *placements
            .borrow_mut()
            .entry(entry_id)
            .or_insert_with(|| meshes.placement(entry_id))
    });
    (posed, colours)
}

/// A design's placement in the planner's caliper frame and its caliper width in model units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(in crate::gui::rough_plan) struct DesignInfo {
    /// The caliper turn and centring.
    pub(in crate::gui::rough_plan) placement: DesignPlacement,
    /// The design's width along the caliper, model units.
    pub(in crate::gui::rough_plan) width_units: f64,
}

/// Where the pointer is, which handle is lit and which is dragged.
#[derive(Default)]
struct Link {
    /// The planner's design meshes (for the adopt action, which runs on the UI thread).
    meshes: Option<Arc<MeshLibrary>>,
    /// The handle the pointer is over or drags, as an index into the wizard's handle list.
    hot: Option<usize>,
    /// The handle drag under way.
    drag: Option<ActiveDrag>,
}

/// A handle drag: the handle as it was when the pointer went down (the mapping from pixels to the
/// parameter stays fixed for the whole drag) and how far the pointer has travelled since.
struct ActiveDrag {
    handle: Handle,
    travel: (f32, f32),
}

thread_local! {
    static LINK: RefCell<Link> = RefCell::default();
}

/// Remembers the design meshes of the view (called when the view is set up).
pub(in crate::gui::rough_plan) fn register_meshes(meshes: &Arc<MeshLibrary>) {
    LINK.with(|link| link.borrow_mut().meshes = Some(Arc::clone(meshes)));
}

/// The placement and caliper width of design `entry_id`, read from the library.
pub(in crate::gui::rough_plan) fn design_info(entry_id: i64) -> Option<DesignInfo> {
    let meshes = LINK.with(|link| link.borrow().meshes.clone())?;
    let placement = meshes.placement(entry_id)?;
    let width_units = meshes.mesh(entry_id).ok()?.caliper_width();
    (width_units.is_finite() && width_units > 0.0).then_some(DesignInfo {
        placement,
        width_units,
    })
}

/// What the camera, the frame and the size of the model view are, copied out of the session.
#[derive(Clone, Copy)]
struct ViewData {
    pose: CameraPose,
    size: (f32, f32),
    frame: WorldFrame,
}

impl ViewData {
    const fn pixels(&self) -> (u32, u32) {
        (self.size.0.round() as u32, self.size.1.round() as u32)
    }
}

/// The model view's camera and frame, or `None` while another scene is shown or the view has no
/// size yet. The borrow of the session ends here.
fn view_data(host: &Host) -> Option<ViewData> {
    let session = host.session.borrow();
    let view = &session.view;
    if !matches!(
        view.scene.as_deref().map(|scene| &scene.kind),
        Some(SceneKind::Model(_))
    ) || view.view_size.0 <= 0.0
        || view.view_size.1 <= 0.0
    {
        return None;
    }
    Some(ViewData {
        pose: view.pose,
        size: view.view_size,
        frame: WorldFrame::for_base(&session.model.base),
    })
}

/// The handle under the point `cursor` (logical pixels), with the data it was found with.
fn handle_under(host: &Host, cursor: (f32, f32)) -> Option<(usize, Vec<Handle>)> {
    let data = view_data(host)?;
    let set = zone_link::current_handles()?;
    let camera = camera_for(data.pose);
    let map = ViewMap {
        camera: &camera,
        size: data.pixels(),
        centre: data.frame.centre,
        scale: data.frame.scale,
    };
    let index = pick_handle(&map, &set.handles, cursor, HIT_RADIUS_PX)?;
    Some((index, set.handles))
}

/// The pointer moved over the image. Lights the handle under it and words it in the hint strip.
/// Returns whether the pointer is over a handle (the view then leaves its own hover alone).
pub(in crate::gui::rough_plan) fn hover(host: &Rc<Host>, x: f32, y: f32) -> bool {
    // Where the pointer is stays current even when this returns early: a drag that starts on a
    // handle looks the pointer up here.
    host.session.borrow_mut().view.pointer = (x >= 0.0 && y >= 0.0).then_some((x, y));
    if LINK.with(|link| link.borrow().drag.is_some()) {
        return true;
    }
    let found = (x >= 0.0 && y >= 0.0)
        .then(|| handle_under(host, (x, y)))
        .flatten();
    let hot = found.as_ref().map(|(index, _)| *index);
    let changed = LINK.with(|link| {
        let mut link = link.borrow_mut();
        let changed = link.hot != hot;
        link.hot = hot;
        changed
    });
    if let Some((index, handles)) = &found {
        let hint = handles[*index].kind.hint();
        let model = host.window.global::<RoughPlanModel>();
        if model.get_view_hint().as_str() != hint {
            model.set_view_hint(hint.into());
        }
    }
    if changed {
        request_render(host);
    }
    found.is_some()
}

/// A click at `(x, y)`: used up when it is on a handle, or when the wizard paints polished
/// windows and the click lands on the mesh (or misses it: painting is on, so the click is not a
/// click on the model).
pub(in crate::gui::rough_plan) fn click(host: &Rc<Host>, x: f32, y: f32) -> bool {
    if handle_under(host, (x, y)).is_some() {
        return true;
    }
    mesh_ray(host, x, y).is_some_and(|ray| zone_link::mesh_click(&ray))
}

/// The point `(x, y)` (logical pixels) of the model view as a ray in the rough's frame, mm. The
/// view's world is the rough's frame with its bounding-box centre at the origin, scaled so half the
/// box diagonal is one unit (`WorldFrame`), so the ray is the camera's ray moved and scaled back.
pub(in crate::gui::rough_plan) fn mesh_ray(host: &Host, x: f32, y: f32) -> Option<MeshRay> {
    let data = view_data(host)?;
    let ray = camera_for(data.pose).generate_ray(x, y, data.size.0, data.size.1, 0.0, 0.0);
    Some(MeshRay {
        origin: ray.origin.as_dvec3() / data.frame.scale + data.frame.centre,
        dir: ray.dir.as_dvec3(),
    })
}

/// The pointer went down on the image: a drag that starts on a handle edits the zone.
pub(in crate::gui::rough_plan) fn drag_begin(host: &Rc<Host>) {
    let pointer = host.session.borrow().view.pointer;
    let Some(pointer) = pointer else {
        return;
    };
    let Some((index, handles)) = handle_under(host, pointer) else {
        return;
    };
    zone_link::drag_begin();
    LINK.with(|link| {
        let mut link = link.borrow_mut();
        link.hot = Some(index);
        link.drag = Some(ActiveDrag {
            handle: handles[index],
            travel: (0.0, 0.0),
        });
    });
}

/// The pointer moved by `(dx, dy)` with the button held. Returns whether a handle drag used it (the
/// camera then stays where it is).
pub(in crate::gui::rough_plan) fn drag_move(host: &Rc<Host>, dx: f32, dy: f32) -> bool {
    let step = LINK.with(|link| {
        let mut link = link.borrow_mut();
        link.drag.as_mut().map(|drag| {
            drag.travel.0 += dx;
            drag.travel.1 += dy;
            (drag.handle, drag.travel)
        })
    });
    let Some((handle, travel)) = step else {
        return false;
    };
    let Some(data) = view_data(host) else {
        return true;
    };
    let camera = camera_for(data.pose);
    let map = ViewMap {
        camera: &camera,
        size: data.pixels(),
        centre: data.frame.centre,
        scale: data.frame.scale,
    };
    if let Some(edit) = drag_edit(&map, &handle, (0.0, 0.0), travel)
        && let Err(message) = zone_link::drag_to(&edit)
    {
        host.window
            .global::<RoughPlanModel>()
            .set_view_hint(message.into());
    }
    true
}

/// The button went up: a handle drag ends and counts as one change of the zones.
pub(in crate::gui::rough_plan) fn drag_end(host: &Rc<Host>) {
    let ended = LINK.with(|link| {
        let mut link = link.borrow_mut();
        link.hot = None;
        link.drag.take().is_some()
    });
    if ended {
        // Whether the zones changed does not matter here: the view is redrawn either way.
        let _ = zone_link::drag_end();
        request_render(host);
    }
}

/// Draws the handles of the wizard's selected zone into a finished frame of the model view.
pub(in crate::gui::rough_plan) fn draw_overlay(host: &Host, pixels: &mut Pixels) {
    let Some(data) = view_data(host) else {
        return;
    };
    let Some(set) = zone_link::current_handles() else {
        return;
    };
    let camera = camera_for(data.pose);
    let map = ViewMap {
        camera: &camera,
        size: data.pixels(),
        centre: data.frame.centre,
        scale: data.frame.scale,
    };
    let hot = LINK.with(|link| link.borrow().hot);
    let drawn = overlay(&map, &set.handles, hot);
    let (width, height) = (pixels.width(), pixels.height());
    let scale = width as f32 / data.size.0;
    draw_handles(
        pixels.make_mut_bytes(),
        width,
        height,
        &drawn,
        set.colour,
        scale,
    );
}

/// The frame is drawn again (the zones changed in the wizard).
pub(in crate::gui::rough_plan) fn redraw(host: &Rc<Host>) {
    request_render(host);
}
