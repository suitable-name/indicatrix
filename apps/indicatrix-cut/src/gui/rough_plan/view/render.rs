//! Drawing a scene: the frame renderer that lives on the render thread, the request it
//! answers, and the pick buffer it publishes.
//!
//! The renderer keeps the composed frame it drew last. A request that differs from the last
//! one only in the hover marker reuses that frame and draws just the marker, so moving the
//! pointer over the rough costs a copy, not a raster. Two output buffers take turns, so a
//! hover step allocates nothing while the window has let go of the frame before last.

use super::{
    compose::{self, HOVER_MARK, Layers, ROUGH_EDGE, RoughLayer, SAW_EDGE},
    picking::{BoxTarget, surviving_points},
    scene::{BASE_GREY, FitScene, ModelScene, Scene, SceneKind},
};
use glam::Vec3;
use indicatrix::{geometry::stone_metrics::SolidMesh, optics::raytracer::Camera};
use indicatrix_solid::{
    preview::{
        CameraPose,
        camera::{fit_distance_for_radius, orbit_distance_bounds},
    },
    raster::{FillMode, SolidRasterizer, SolidStyle, project_point},
};
use slint::{Rgba8Pixel, SharedPixelBuffer};
use std::sync::{Arc, Mutex};

/// Finished pixels; unlike a `slint::Image` they can cross threads.
pub(super) type Pixels = SharedPixelBuffer<Rgba8Pixel>;

/// The vertical field of view every viewport of the app uses.
pub(super) const FIELD_OF_VIEW: f32 = 42.0;

/// The frame background, the theme's input colour (`#12141c`).
const BACKGROUND: [u8; 4] = [0x12, 0x14, 0x1c, 0xff];

/// The yaw of the reset view.
const RESET_YAW: f32 = 0.60;

/// The pitch of the reset view.
const RESET_PITCH: f32 = 0.45;

/// The camera every frame is drawn with, the call every viewport of the app makes.
#[must_use]
pub(super) fn camera_for(pose: CameraPose) -> Camera {
    Camera::new(pose.yaw, pose.pitch, pose.distance, FIELD_OF_VIEW)
}

/// How much further out the camera must be for an image of `aspect` (width over height)
/// than for a square or wide one: the field of view is the vertical one, so a tall image
/// needs `1 / aspect` times the distance.
fn widen(aspect: f32) -> f32 {
    if aspect.is_finite() && aspect > 0.0 {
        1.0 / aspect.min(1.0)
    } else {
        1.0
    }
}

/// The reset view for an image of `aspect` (width over height): the standard angle at a
/// distance that frames the unit sphere of a scene, further out for a tall image (the field
/// of view is the vertical one).
#[must_use]
pub(super) fn reset_pose(aspect: f32) -> CameraPose {
    let (near, far) = orbit_distance_bounds(super::scene::SCENE_RADIUS);
    CameraPose {
        yaw: RESET_YAW,
        pitch: RESET_PITCH,
        distance: (fit_distance_for_radius(super::scene::SCENE_RADIUS) * widen(aspect))
            .clamp(near, far),
    }
}

/// The camera `distance` after the image changed from `old_aspect` to `new_aspect`: the
/// zoom the user chose relative to the framing of the scene is kept, so the scene still
/// fits the narrower side.
#[must_use]
pub(super) fn refit_distance(distance: f32, old_aspect: f32, new_aspect: f32) -> f32 {
    let (near, far) = orbit_distance_bounds(super::scene::SCENE_RADIUS);
    (distance * widen(new_aspect) / widen(old_aspect)).clamp(near, far)
}

/// What the pointer is over, as far as the drawing is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Hover {
    /// Nothing worth marking.
    #[default]
    None,
    /// An edge or corner of the rough block, marked on top of the frame.
    Box(BoxTarget),
    /// A facet of the model, outlined (the face-from-view mode).
    Facet(usize),
}

/// The switches of a frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct RenderOptions {
    /// Model: tint the faces of the cuts.
    pub(super) show_cut_faces: bool,
    /// Fit: draw the rough as a glass volume.
    pub(super) show_rough: bool,
    /// Fit: draw the saw pieces.
    pub(super) show_saw: bool,
    /// Fit: draw the stones.
    pub(super) show_stones: bool,
    /// Model: the cut whose face is outlined.
    pub(super) selected_cut: Option<usize>,
    /// Fit: the design group whose stones are outlined.
    pub(super) highlight_group: Option<usize>,
    /// What the pointer is over.
    pub(super) hover: Hover,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            show_cut_faces: true,
            show_rough: true,
            show_saw: true,
            show_stones: true,
            selected_cut: None,
            highlight_group: None,
            hover: Hover::None,
        }
    }
}

/// One frame to draw.
pub(super) struct RenderRequest {
    /// Increases with every request; the UI drops a frame older than the one it shows.
    pub(super) generation: u64,
    /// The scene to draw.
    pub(super) scene: Arc<Scene>,
    /// The camera.
    pub(super) pose: CameraPose,
    /// The frame size in pixels.
    pub(super) size: (u32, u32),
    /// The switches.
    pub(super) options: RenderOptions,
}

/// The pick buffer of the last frame drawn from a scene, for the UI thread to read. The
/// buffer is shared, so taking a copy of the snapshot out of the lock costs a reference
/// count and the render thread never waits for a reader to finish scanning it.
#[derive(Debug, Default, Clone)]
pub(super) struct PickSnapshot {
    /// The scene the buffer belongs to.
    pub(super) serial: u64,
    /// The buffer's width in pixels.
    pub(super) width: u32,
    /// The buffer's height in pixels.
    pub(super) height: u32,
    /// `facet id + 1` per pixel, `0` for background.
    pub(super) pick: Arc<Vec<u32>>,
}

impl PickSnapshot {
    /// The facet under the point `(fx, fy)` of the image, both in `0..1` from the top
    /// left.
    #[must_use]
    pub(super) fn facet_at(&self, fx: f32, fy: f32) -> Option<usize> {
        if self.width == 0
            || self.height == 0
            || !(0.0..1.0).contains(&fx)
            || !(0.0..1.0).contains(&fy)
        {
            return None;
        }
        let x = (fx * self.width as f32) as usize;
        let y = (fy * self.height as f32) as usize;
        let value = *self.pick.get(y * self.width as usize + x)?;
        value.checked_sub(1).map(|facet| facet as usize)
    }
}

/// A pick snapshot shared between the render thread (writes) and the UI thread (reads).
pub(super) type SharedPick = Arc<Mutex<PickSnapshot>>;

/// Everything that decides the composed frame except the hover marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct FrameKey {
    serial: u64,
    pose: [u32; 3],
    size: (u32, u32),
    options: RenderOptions,
}

impl FrameKey {
    /// The key of `request`, with the options reduced to what changes the raster of its
    /// scene: a model scene ignores the layer switches and the outlined design group, a
    /// fit scene ignores the cut tint, the selected cut and the facet hover, and a box
    /// hover is only a marker drawn afterwards.
    fn of(request: &RenderRequest) -> Self {
        let options = request.options;
        let options = match request.scene.kind {
            SceneKind::Model(_) => RenderOptions {
                show_rough: false,
                show_saw: false,
                show_stones: false,
                highlight_group: None,
                hover: match options.hover {
                    Hover::Facet(id) => Hover::Facet(id),
                    Hover::None | Hover::Box(_) => Hover::None,
                },
                ..options
            },
            SceneKind::Fit(_) => RenderOptions {
                show_cut_faces: false,
                selected_cut: None,
                hover: Hover::None,
                ..options
            },
        };
        Self {
            serial: request.scene.serial,
            pose: [
                request.pose.yaw.to_bits(),
                request.pose.pitch.to_bits(),
                request.pose.distance.to_bits(),
            ],
            size: request.size,
            options,
        }
    }
}

/// A style for a pass that only draws edges: no fill shows, the edges are opaque.
fn edge_pass_style(edge: [u8; 3]) -> SolidStyle {
    SolidStyle {
        background: [0, 0, 0, 0],
        fill_mode: FillMode::Transparent,
        edge_color: edge,
        show_orientation_marker: false,
        ..SolidStyle::default()
    }
}

/// The style of the model view: base faces grey, cut faces tinted, the selected cut's face
/// outlined, the hovered facet marked.
fn model_style(scene: &ModelScene, options: &RenderOptions) -> SolidStyle {
    let mut selected = vec![false; scene.base_facets + scene.cut_count];
    if let Some(cut) = options.selected_cut
        && let Some(flag) = selected.get_mut(scene.base_facets + cut)
    {
        *flag = true;
    }
    SolidStyle {
        background: BACKGROUND,
        base_color: BASE_GREY,
        facet_base_colors: scene.facet_colors(options.show_cut_faces),
        selected,
        hovered: match options.hover {
            Hover::Facet(id) => u32::try_from(id).ok(),
            Hover::None | Hover::Box(_) => None,
        },
        show_orientation_marker: false,
        ..SolidStyle::default()
    }
}

/// Draws the marker of a hovered edge or corner of the rough block on `out`: `world` are
/// the points of the target that the cuts left (see [`surviving_points`]).
fn draw_box_hover(
    out: &mut [u8],
    size: (u32, u32),
    camera: &Camera,
    target: BoxTarget,
    world: &[Vec3],
) {
    let points: Vec<(f32, f32)> = world
        .iter()
        .filter_map(|&p| project_point(camera, p, size.0, size.1).map(|s| (s.0, s.1)))
        .collect();
    match (target, points.as_slice()) {
        (BoxTarget::Edge(_), &[a, b]) => compose::draw_line(out, size, a, b, 3, HOVER_MARK),
        (BoxTarget::Corner(_), &[c]) => compose::draw_dot(out, size, c, 6, HOVER_MARK),
        _ => {}
    }
}

/// The render thread's state: three rasterizers (the stones or the model, the rough, the
/// saw pieces), the composed frame and what it was drawn for.
pub(super) struct ViewRenderer {
    main: SolidRasterizer,
    rough: SolidRasterizer,
    saw: SolidRasterizer,
    /// The composed frame without the hover marker.
    base: Vec<u8>,
    key: Option<FrameKey>,
    /// The style of the fit scene drawn last, which never changes for a scene.
    fit_style: Option<(u64, SolidStyle)>,
    /// The two output buffers that take turns; the window may still show the last one.
    frames: [Option<Pixels>; 2],
    /// Which of `frames` the next render fills.
    turn: usize,
    /// The pick buffer the window has let go of, to be filled again instead of
    /// allocating a new one.
    spare_pick: Option<Arc<Vec<u32>>>,
}

impl Default for ViewRenderer {
    fn default() -> Self {
        Self {
            main: SolidRasterizer::new(1, 1),
            rough: SolidRasterizer::new(1, 1),
            saw: SolidRasterizer::new(1, 1),
            base: Vec::new(),
            key: None,
            fit_style: None,
            frames: [None, None],
            turn: 0,
            spare_pick: None,
        }
    }
}

impl ViewRenderer {
    /// Draws `request` and returns its pixels. When `picks` is given and the raster was
    /// redrawn, the pick buffer is published there.
    pub(super) fn render(&mut self, request: &RenderRequest, picks: Option<&SharedPick>) -> Pixels {
        let size = (request.size.0.max(1), request.size.1.max(1));
        let key = FrameKey::of(request);
        if self.key.as_ref() != Some(&key) {
            self.draw_base(request, size);
            self.key = Some(key);
            if let Some(shared) = picks {
                self.publish(shared, request.scene.serial);
            }
        }
        let turn = self.turn;
        self.turn = 1 - turn;
        let slot = &mut self.frames[turn];
        if slot
            .as_ref()
            .is_none_or(|frame| (frame.width(), frame.height()) != size)
        {
            *slot = Some(Pixels::new(size.0, size.1));
        }
        let frame = slot.get_or_insert_with(|| Pixels::new(size.0, size.1));
        frame.make_mut_bytes().copy_from_slice(&self.base);
        if let (Hover::Box(target), SceneKind::Model(model)) =
            (request.options.hover, &request.scene.kind)
            && let Some(world) = surviving_points(target, model.half_extents, &model.cut_planes)
        {
            let camera = camera_for(request.pose);
            draw_box_hover(frame.make_mut_bytes(), size, &camera, target, &world);
        }
        frame.clone()
    }

    /// Publishes the main pick buffer to `shared`: the copy is made outside the lock, and
    /// the lock is held only to swap the shared buffer in.
    fn publish(&mut self, shared: &SharedPick, serial: u64) {
        let mut buffer = self
            .spare_pick
            .take()
            .and_then(|spare| Arc::try_unwrap(spare).ok())
            .unwrap_or_default();
        buffer.clear();
        buffer.extend_from_slice(&self.main.pick);
        let fresh = Arc::new(buffer);
        let (width, height) = (self.main.width, self.main.height);
        let old = {
            let mut snapshot = shared
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            snapshot.serial = serial;
            snapshot.width = width;
            snapshot.height = height;
            std::mem::replace(&mut snapshot.pick, fresh)
        };
        self.spare_pick = Some(old);
    }

    /// Draws the raster and the composed passes of `request` into `self.base`.
    fn draw_base(&mut self, request: &RenderRequest, size: (u32, u32)) {
        let camera = camera_for(request.pose);
        self.main.resize(size.0, size.1);
        match &request.scene.kind {
            SceneKind::Model(model) => {
                let style = model_style(model, &request.options);
                self.main.render(&model.mesh, &camera, &style);
                self.base.clear();
                self.base.extend_from_slice(&self.main.color);
            }
            SceneKind::Fit(fit) => {
                self.draw_fit(request.scene.serial, fit, &camera, size, &request.options);
            }
        }
    }

    /// The fit scene: stones, then the rough and the saw pieces composed over them, then
    /// the outline of the highlighted design.
    fn draw_fit(
        &mut self,
        serial: u64,
        fit: &FitScene,
        camera: &Camera,
        size: (u32, u32),
        options: &RenderOptions,
    ) {
        if self
            .fit_style
            .as_ref()
            .is_none_or(|(known, _)| *known != serial)
        {
            let style = SolidStyle {
                background: BACKGROUND,
                facet_base_colors: fit.facet_colors.clone(),
                show_orientation_marker: false,
                ..SolidStyle::default()
            };
            self.fit_style = Some((serial, style));
        }
        let Some((_, style)) = &self.fit_style else {
            return;
        };
        let nothing = SolidMesh::default();
        let stones = if options.show_stones {
            &fit.stones
        } else {
            &nothing
        };
        self.main.render(stones, camera, style);
        self.base.clear();
        self.base.extend_from_slice(&self.main.color);

        let show_rough = options.show_rough && !fit.rough.rings.is_empty();
        let show_saw = options.show_saw && !fit.saw.rings.is_empty();
        if show_rough {
            self.rough.resize(size.0, size.1);
            self.rough
                .render(&fit.rough, camera, &edge_pass_style(ROUGH_EDGE));
        }
        if show_saw {
            self.saw.resize(size.0, size.1);
            self.saw
                .render(&fit.saw, camera, &edge_pass_style(SAW_EDGE));
        }
        compose::compose(
            &mut self.base,
            &Layers {
                rough: show_rough.then_some(RoughLayer {
                    edges: &self.rough.color,
                    cover: &self.rough.pick,
                }),
                saw: show_saw.then_some(self.saw.color.as_slice()),
            },
        );

        if let Some(group) = options.highlight_group
            && options.show_stones
        {
            let flags = fit.facets_of_group(group);
            let selected = |pick: u32| {
                (pick as usize)
                    .checked_sub(1)
                    .and_then(|facet| flags.get(facet))
                    .copied()
                    .unwrap_or(false)
            };
            let thickness = if size.0 >= 600 { 2 } else { 1 };
            compose::outline_region(&mut self.base, &self.main.pick, size, &selected, thickness);
        }
    }
}

#[cfg(test)]
mod tests;
