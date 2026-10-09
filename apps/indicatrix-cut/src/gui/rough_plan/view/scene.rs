//! The scenes the render worker draws, in world units.
//!
//! A scene is built once (the model scene on the UI thread, the fit scene on the builder
//! thread) and then only read: every frame is a camera and a few switches applied to it.
//! World units make the rough about one unit in radius, like a design, so the same camera
//! and zoom limits serve every scene.

use super::design_mesh::{DesignMesh, MeshMiss, MeshSource, simplified_flags, simplify_ring};
use crate::gui::rough_plan::{
    format::{group_order, piece_positions},
    metrics::PALETTE,
    shape_worker::centred_mesh,
};
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::SolidMesh;
use indicatrix_cut_core::rough_plan::{PlacedStone, RoughBase, RoughLayout, RoughModel, StonePose};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
};

/// The color of a base face of the rough.
pub(super) const BASE_GREY: [u8; 3] = [200, 205, 215];

/// The color of a cut face of the rough while cut faces are tinted.
pub(super) const CUT_TINT: [u8; 3] = [230, 190, 120];

/// The color of a stone whose design is gone.
pub(super) const DELETED_GREY: [u8; 3] = [120, 126, 138];

/// The color of a stone whose design exists but could not be loaded.
pub(super) const UNREADABLE_TINT: [u8; 3] = [150, 110, 118];

/// The tooltip note of a stone whose design is gone from the library.
const NOTE_DELETED: &str = "design deleted";

/// The tooltip note of a stone whose design could not be loaded.
const NOTE_UNREADABLE: &str = "design could not be loaded";

/// The tooltip note of a stone drawn from a design that changed since the plan was made.
const NOTE_CHANGED: &str = "design changed since saved";

/// The tooltip note of a stone drawn from the design its title was matched to.
const NOTE_MATCHED: &str = "matched by title";

/// The tooltip note added when a changed design was scaled to the recorded width.
const NOTE_SCALED: &str = ", drawn at the saved width";

/// The radius of every scene, which the zoom limits are computed for.
pub(super) const SCENE_RADIUS: f64 = 1.0;

/// Serial numbers tell the renderer's frame cache one scene from another.
static NEXT_SERIAL: AtomicU64 = AtomicU64::new(1);

/// A fresh scene serial.
fn next_serial() -> u64 {
    NEXT_SERIAL.fetch_add(1, Ordering::Relaxed)
}

/// The map from the rough's frame in millimetres to world units: the bounding-box centre
/// becomes the origin and half the box diagonal becomes one unit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct WorldFrame {
    /// The bounding-box centre, in the rough frame.
    pub(super) centre: DVec3,
    /// World units per millimetre.
    pub(super) scale: f64,
}

impl WorldFrame {
    /// The frame of `base`.
    #[must_use]
    pub(super) fn for_base(base: &RoughBase) -> Self {
        let half_diagonal = DVec3::from_array(base.bounding_box_extents()).length() * 0.5;
        Self {
            centre: base.bounding_box_centre(),
            scale: if half_diagonal > 0.0 {
                1.0 / half_diagonal
            } else {
                1.0
            },
        }
    }

    /// A point of the rough frame, in world units.
    #[must_use]
    pub(super) fn point(&self, mm: DVec3) -> DVec3 {
        (mm - self.centre) * self.scale
    }
}

/// Adds one facet to a mesh that carries only what the rasterizer reads: a normal and a
/// ring per facet id.
fn push_facet(mesh: &mut SolidMesh, id: usize, normal: DVec3, ring: Vec<DVec3>) {
    mesh.facet_id.push(id);
    mesh.normals.push(normal);
    mesh.rings.push((id, ring));
}

/// Records which edges of the ring `push_facet` just added to `mesh` are drawn. `drawn` is
/// `None` for a ring that draws every edge. `mesh.edge_visible` stays `None` until a
/// ring has something to hide, so a scene of flat designs is exactly what it was; the
/// rings before the first such one are backfilled as fully drawn.
fn set_drawn_edges(mesh: &mut SolidMesh, drawn: Option<&[bool]>) {
    let Some(last) = mesh.rings.len().checked_sub(1) else {
        return;
    };
    if drawn.is_none() && mesh.edge_visible.is_none() {
        return;
    }
    let visible = mesh.edge_visible.get_or_insert_with(Vec::new);
    while visible.len() < last {
        let len = mesh.rings[visible.len()].1.len();
        visible.push(vec![true; len]);
    }
    visible.push(drawn.map_or_else(|| vec![true; mesh.rings[last].1.len()], <[bool]>::to_vec));
}

/// `centred_mm` (a mesh in the centred frame, in mm) scaled to world units, with every
/// ring reduced to its true corners. A mesh with `piece_normals` (a non-convex rough, one
/// normal per ring) keeps them, and its `edge_visible` flags follow the corners kept.
#[must_use]
fn world_mesh(centred_mm: &SolidMesh, scale: f64) -> SolidMesh {
    let mut normals: BTreeMap<usize, DVec3> = BTreeMap::new();
    for (&id, &normal) in centred_mm.facet_id.iter().zip(&centred_mm.normals) {
        normals.entry(id).or_insert(normal);
    }
    let mut mesh = SolidMesh::default();
    for (index, (id, ring)) in centred_mm.rings.iter().enumerate() {
        let normal = centred_mm.piece_normals.as_ref().map_or_else(
            || normals.get(id).copied(),
            |pieces| pieces.get(index).copied(),
        );
        let Some(normal) = normal else {
            continue;
        };
        let scaled: Vec<DVec3> = ring.iter().map(|&p| p * scale).collect();
        let corners = simplify_ring(&scaled);
        if corners.len() < 3 {
            continue;
        }
        let drawn = centred_mm.edge_visible.as_ref().map(|visible| {
            simplified_flags(
                &scaled,
                &corners,
                visible.get(index).map_or(&[], Vec::as_slice),
            )
        });
        push_facet(&mut mesh, *id, normal, corners);
        if centred_mm.piece_normals.is_some() {
            mesh.piece_normals.get_or_insert_with(Vec::new).push(normal);
        }
        set_drawn_edges(&mut mesh, drawn.as_deref());
    }
    mesh
}

/// The six faces of the axis-aligned box `[min, max]`: the outward normal and the four
/// corners in order around the face.
#[must_use]
pub(super) fn box_faces(min: DVec3, max: DVec3) -> [(DVec3, [DVec3; 4]); 6] {
    let corner = |x: bool, y: bool, z: bool| {
        DVec3::new(
            if x { max.x } else { min.x },
            if y { max.y } else { min.y },
            if z { max.z } else { min.z },
        )
    };
    [
        (
            DVec3::X,
            [
                corner(true, false, false),
                corner(true, true, false),
                corner(true, true, true),
                corner(true, false, true),
            ],
        ),
        (
            DVec3::NEG_X,
            [
                corner(false, false, false),
                corner(false, false, true),
                corner(false, true, true),
                corner(false, true, false),
            ],
        ),
        (
            DVec3::Y,
            [
                corner(false, true, false),
                corner(false, true, true),
                corner(true, true, true),
                corner(true, true, false),
            ],
        ),
        (
            DVec3::NEG_Y,
            [
                corner(false, false, false),
                corner(true, false, false),
                corner(true, false, true),
                corner(false, false, true),
            ],
        ),
        (
            DVec3::Z,
            [
                corner(false, false, true),
                corner(true, false, true),
                corner(true, true, true),
                corner(false, true, true),
            ],
        ),
        (
            DVec3::NEG_Z,
            [
                corner(false, false, false),
                corner(false, true, false),
                corner(true, true, false),
                corner(true, false, false),
            ],
        ),
    ]
}

/// The rough as edited: its mesh, and what picking needs to know about it.
#[derive(Debug)]
pub(super) struct ModelScene {
    /// The rough's faces in world units. Facet `i < base_facets` is base plane `i`; facet
    /// `base_facets + k` is cut `k`.
    pub(super) mesh: SolidMesh,
    /// How many facet ids belong to the base shape.
    pub(super) base_facets: usize,
    /// How many cuts the model has.
    pub(super) cut_count: usize,
    /// Half the base box, in world units (edges and corners are picked on this box).
    pub(super) half_extents: [f32; 3],
    /// Whether the base is a block, the only base that offers edge and corner cuts.
    pub(super) block: bool,
    /// The half-space `n . p <= m` of every cut, in world units. A corner or edge of the
    /// uncut box that lies outside one of them no longer exists.
    pub(super) cut_planes: Vec<(Vec3, f32)>,
}

/// The half-spaces of `model`'s cuts in world units: a plane `n . p_mm <= m` of the rough
/// frame becomes `n . p <= (m - n . centre) * scale` for a point `p` of the world frame.
fn cut_planes_world(
    model: &RoughModel,
    base_facets: usize,
    frame: &WorldFrame,
) -> Vec<(Vec3, f32)> {
    let Ok(planes) = model.halfspaces() else {
        return Vec::new();
    };
    planes
        .into_iter()
        .skip(base_facets)
        .map(|(normal, offset)| {
            (
                normal.as_vec3(),
                ((offset - normal.dot(frame.centre)) * frame.scale) as f32,
            )
        })
        .collect()
}

impl ModelScene {
    /// The scene of `model`, whose mesh in the centred frame (mm) is `centred_mm`.
    #[must_use]
    pub(super) fn new(centred_mm: &SolidMesh, model: &RoughModel) -> Self {
        let frame = WorldFrame::for_base(&model.base);
        let extents = model.base.bounding_box_extents();
        let base_facets = model
            .base
            .to_halfspaces(false)
            .map_or(0, |planes| planes.len());
        Self {
            mesh: world_mesh(centred_mm, frame.scale),
            base_facets,
            cut_count: model.cuts.len(),
            half_extents: extents.map(|e| (e * frame.scale * 0.5) as f32),
            block: matches!(model.base, RoughBase::Block { .. }),
            cut_planes: cut_planes_world(model, base_facets, &frame),
        }
    }

    /// The cut that owns facet `facet`, if it is a cut face.
    #[must_use]
    pub(super) fn cut_of_facet(&self, facet: usize) -> Option<usize> {
        let cut = facet.checked_sub(self.base_facets)?;
        (cut < self.cut_count).then_some(cut)
    }

    /// The outward normal of facet `facet`.
    #[must_use]
    pub(super) fn facet_normal(&self, facet: usize) -> Option<DVec3> {
        let at = self.mesh.facet_id.iter().position(|&id| id == facet)?;
        self.mesh.normals.get(at).copied()
    }

    /// The base color of every facet: base faces grey, cut faces tinted when asked.
    #[must_use]
    pub(super) fn facet_colors(&self, tint_cuts: bool) -> Vec<[u8; 3]> {
        (0..self.base_facets + self.cut_count)
            .map(|id| {
                if tint_cuts && id >= self.base_facets {
                    CUT_TINT
                } else {
                    BASE_GREY
                }
            })
            .collect()
    }
}

/// Where a stone's piece sits in the cut plan, each number counted from 1 as the cut plan
/// text counts them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct PiecePosition {
    /// The slab the piece comes from.
    pub(super) slab: usize,
    /// The bar of that slab.
    pub(super) bar: usize,
    /// The piece's number within its bar.
    pub(super) piece: usize,
}

/// What the tooltip says about one stone.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct StoneInfo {
    /// The design's title.
    pub(super) title: String,
    /// The stone's weight in carats.
    pub(super) carat: f64,
    /// The piece the stone is sawn from; `None` when the layout has no saw plan to read it
    /// from (a single exact fit) or the plan does not match the stones.
    pub(super) position: Option<PiecePosition>,
    /// Why the stone is not drawn as planned (a changed, matched, deleted or unreadable
    /// design); empty for a stone drawn as planned.
    pub(super) note: String,
}

impl StoneInfo {
    /// "Barion Oval · 0.62 ct · slab 2, bar 1, piece 3", with the position left out when
    /// unknown and the note (for example "design deleted") added at the end.
    #[must_use]
    pub(super) fn hint(&self) -> String {
        let mut parts = vec![format!("{} \u{00B7} {:.2} ct", self.title, self.carat)];
        if let Some(position) = self.position {
            parts.push(format!(
                "slab {}, bar {}, piece {}",
                position.slab, position.bar, position.piece
            ));
        }
        if !self.note.is_empty() {
            parts.push(self.note.clone());
        }
        parts.join(" \u{00B7} ")
    }
}

/// How a stone of a result is drawn, by its design's standing in the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StoneDraw {
    /// The design as it is, at the plan's scale.
    Design(i64),
    /// The design changed since the plan was made: its mesh is scaled so its caliper
    /// width is the recorded stone's.
    Changed(i64),
    /// The design the stone's title was matched to, scaled like a changed one.
    Matched(i64),
    /// The design is gone: a grey box.
    Deleted,
}

/// The rough of a plan as a mesh in world units. It is built by the first thread that
/// asks (a worker, never the UI thread) and then shared by every scene and thumbnail of
/// the plan.
pub(super) struct RoughMesh {
    model: RoughModel,
    mesh: OnceLock<Arc<SolidMesh>>,
}

impl RoughMesh {
    /// The rough of `model`, not built yet.
    #[must_use]
    pub(super) const fn new(model: RoughModel) -> Self {
        Self {
            model,
            mesh: OnceLock::new(),
        }
    }

    /// The model the plan was made for.
    #[must_use]
    pub(super) const fn model(&self) -> &RoughModel {
        &self.model
    }

    /// The rough's faces in world units (empty when the model has no solid).
    #[must_use]
    pub(super) fn get(&self) -> Arc<SolidMesh> {
        Arc::clone(self.mesh.get_or_init(|| {
            let frame = WorldFrame::for_base(&self.model.base);
            let mesh = centred_mesh(&self.model)
                .map_or_else(|_| SolidMesh::default(), |c| world_mesh(&c, frame.scale));
            Arc::new(mesh)
        }))
    }
}

/// A result of the plan: stones as real designs, the rough as a glass volume, the saw
/// pieces as boxes.
#[derive(Debug, Default)]
pub(super) struct FitScene {
    /// Every stone's faces in one mesh; the faces of stone `s` are the ids from
    /// `stone_starts[s]` up to `stone_starts[s + 1]`.
    pub(super) stones: SolidMesh,
    /// The base color of every stone facet.
    pub(super) facet_colors: Vec<[u8; 3]>,
    /// First facet id of every stone, then the total.
    pub(super) stone_starts: Vec<usize>,
    /// The design group (palette index) of every stone.
    pub(super) stone_group: Vec<usize>,
    /// The tooltip data of every stone.
    pub(super) info: Vec<StoneInfo>,
    /// The rough the plan was made for, shared with the plan's other scenes.
    pub(super) rough: Arc<SolidMesh>,
    /// The sawn pieces, six faces each.
    pub(super) saw: SolidMesh,
}

impl FitScene {
    /// The stone that owns facet `facet` of the merged mesh.
    #[must_use]
    pub(super) fn stone_at_facet(&self, facet: usize) -> Option<usize> {
        let total = *self.stone_starts.last()?;
        if facet >= total {
            return None;
        }
        Some(self.stone_starts.partition_point(|&start| start <= facet) - 1)
    }

    /// Which merged facets belong to a stone of `group`.
    #[must_use]
    pub(super) fn facets_of_group(&self, group: usize) -> Vec<bool> {
        let mut flags = vec![false; self.facet_colors.len()];
        for (stone, span) in self.stone_starts.windows(2).enumerate() {
            if self.stone_group.get(stone) == Some(&group) {
                flags[span[0]..span[1]].fill(true);
            }
        }
        flags
    }

    /// Colours the facets of the stones that have a colour in `colours` (one entry per stone,
    /// `None` keeps the palette colour). A stone drawn as a grey box because its design is gone or
    /// unreadable keeps its grey: that colour is the warning. `zoning` builds only; this is how
    /// the planner shows the colour of a plan that has a rough colour.
    #[cfg(feature = "zoning")]
    pub(super) fn apply_stone_colours(&mut self, colours: &[Option<[u8; 3]>]) {
        for (stone, span) in self.stone_starts.windows(2).enumerate() {
            let Some(Some(colour)) = colours.get(stone) else {
                continue;
            };
            let placeholder = self
                .info
                .get(stone)
                .is_some_and(|info| info.note == NOTE_DELETED || info.note == NOTE_UNREADABLE);
            if placeholder {
                continue;
            }
            if let Some(facets) = self.facet_colors.get_mut(span[0]..span[1]) {
                facets.fill(*colour);
            }
        }
    }

    /// Adds a stone whose design mesh is `design`, placed by `pose`.
    fn push_design(
        &mut self,
        design: &DesignMesh,
        pose: &StonePose,
        frame: &WorldFrame,
        color: [u8; 3],
    ) {
        for facet in &design.facets {
            let ring: Vec<DVec3> = facet
                .ring
                .iter()
                .map(|&v| point_to_world(v, pose, frame))
                .collect();
            let id = self.facet_colors.len();
            push_facet(
                &mut self.stones,
                id,
                normal_to_world(facet.normal, pose),
                ring,
            );
            set_drawn_edges(&mut self.stones, facet.edge_drawn.as_deref());
            self.facet_colors.push(color);
        }
    }

    /// Adds the box that stands in for a stone without a design, in `color`: the stone's
    /// recorded size across, centred on its pose, axis-aligned in the rough.
    fn push_placeholder(&mut self, stone: &PlacedStone, frame: &WorldFrame, color: [u8; 3]) {
        let centre_mm = DVec3::from_array(stone.pose.center_mm);
        let half = DVec3::from_array(stone.stone_size_mm) * 0.5;
        for (normal, corners) in box_faces(centre_mm - half, centre_mm + half) {
            let id = self.facet_colors.len();
            push_facet(
                &mut self.stones,
                id,
                normal,
                corners.map(|c| frame.point(c)).to_vec(),
            );
            set_drawn_edges(&mut self.stones, None);
            self.facet_colors.push(color);
        }
    }

    /// Adds `stone` drawn as `draw` says, and returns the tooltip note that explains any
    /// difference from the plan (empty when there is none).
    fn push_stone(
        &mut self,
        stone: &PlacedStone,
        draw: StoneDraw,
        meshes: &dyn MeshSource,
        frame: &WorldFrame,
        color: [u8; 3],
    ) -> String {
        let (entry_id, rescale, note) = match draw {
            StoneDraw::Design(id) => (id, false, ""),
            StoneDraw::Changed(id) => (id, true, NOTE_CHANGED),
            StoneDraw::Matched(id) => (id, true, NOTE_MATCHED),
            StoneDraw::Deleted => {
                self.push_placeholder(stone, frame, DELETED_GREY);
                return NOTE_DELETED.to_string();
            }
        };
        match meshes.mesh(entry_id) {
            Ok(design) if rescale => {
                let (pose, scaled) = rescaled_pose(stone, design.caliper_width());
                self.push_design(&design, &pose, frame, color);
                if scaled {
                    format!("{note}{NOTE_SCALED}")
                } else {
                    note.to_string()
                }
            }
            Ok(design) => {
                self.push_design(&design, &stone.pose, frame, color);
                note.to_string()
            }
            Err(MeshMiss::Gone) => {
                self.push_placeholder(stone, frame, DELETED_GREY);
                NOTE_DELETED.to_string()
            }
            Err(MeshMiss::Unreadable) => {
                self.push_placeholder(stone, frame, UNREADABLE_TINT);
                NOTE_UNREADABLE.to_string()
            }
        }
    }
}

/// The width in mm the plan recorded for `stone` along its caliper width axis, when that
/// axis is one of the rough's (a box-model stone); `None` for a pose turned off the axes.
fn recorded_width_mm(stone: &PlacedStone) -> Option<f64> {
    let direction = stone.pose.axes[0];
    let index = (0..3).find(|&i| {
        direction[i].abs() > 0.5
            && direction
                .iter()
                .enumerate()
                .all(|(j, component)| j == i || component.abs() < 1e-6)
    })?;
    let width = stone.stone_size_mm[index];
    (width.is_finite() && width > 0.0).then_some(width)
}

/// The pose of `stone` with its scale changed so a design mesh whose caliper width is
/// `width_units` model units is as wide as the width the plan recorded; the second
/// value tells whether the scale was changed. A pose or mesh without a usable width
/// keeps the plan's scale.
fn rescaled_pose(stone: &PlacedStone, width_units: f64) -> (StonePose, bool) {
    let mut pose = stone.pose;
    match recorded_width_mm(stone) {
        Some(width_mm) if width_units.is_finite() && width_units > 0.0 => {
            pose.mm_per_unit = width_mm / width_units;
            (pose, true)
        }
        _ => (pose, false),
    }
}

/// A point of a design (caliper frame, model units) in world units.
#[must_use]
pub(super) fn point_to_world(v: DVec3, pose: &StonePose, frame: &WorldFrame) -> DVec3 {
    let local = normal_to_world(v, pose);
    frame.point(DVec3::from_array(pose.center_mm) + local * pose.mm_per_unit)
}

/// A direction of a design turned into the rough's axes (no scaling, no shift).
#[must_use]
pub(super) fn normal_to_world(n: DVec3, pose: &StonePose) -> DVec3 {
    let [x, y, z] = pose.axes.map(DVec3::from_array);
    x * n.x + y * n.y + z * n.z
}

/// What a fit scene is built from.
pub(super) struct FitInputs<'a> {
    /// The layout to draw.
    pub(super) layout: &'a RoughLayout,
    /// The rough the layout was planned for, with its world mesh.
    pub(super) rough: &'a RoughMesh,
    /// Design titles by entry id.
    pub(super) titles: &'a BTreeMap<i64, String>,
    /// For a stone's entry id, how its design is drawn. A stone missing here draws its
    /// own design as planned.
    pub(super) mesh_ids: &'a BTreeMap<i64, StoneDraw>,
}

/// Adds the six faces of `stone`'s sawn piece to `saw`.
fn push_piece_box(saw: &mut SolidMesh, stone: &PlacedStone, frame: &WorldFrame) {
    let origin = DVec3::from_array(stone.piece_origin_mm);
    let size = DVec3::from_array(stone.piece_size_mm);
    for (normal, corners) in box_faces(origin, origin + size) {
        let id = saw.facet_id.len();
        push_facet(saw, id, normal, corners.map(|c| frame.point(c)).to_vec());
    }
}

/// Builds the fit scene of `inputs`, taking design meshes from `meshes`.
#[must_use]
pub(super) fn build_fit_scene(inputs: &FitInputs<'_>, meshes: &dyn MeshSource) -> FitScene {
    let frame = WorldFrame::for_base(&inputs.rough.model().base);
    let layout = inputs.layout;
    // The order of the design rows of the result card, which is also the palette order.
    let groups: Vec<i64> = group_order(layout).into_iter().map(|(id, _)| id).collect();
    let positions = piece_positions(layout);
    let mut scene = FitScene {
        rough: inputs.rough.get(),
        ..FitScene::default()
    };
    for (index, stone) in layout.stones.iter().enumerate() {
        let group = groups
            .iter()
            .position(|&id| id == stone.entry_id)
            .unwrap_or(0);
        scene.stone_starts.push(scene.facet_colors.len());
        scene.stone_group.push(group);
        let draw = inputs
            .mesh_ids
            .get(&stone.entry_id)
            .copied()
            .unwrap_or(StoneDraw::Design(stone.entry_id));
        let color = PALETTE[group % PALETTE.len()];
        let note = scene.push_stone(stone, draw, meshes, &frame, color);
        scene.info.push(StoneInfo {
            title: inputs
                .titles
                .get(&stone.entry_id)
                .cloned()
                .unwrap_or_else(|| format!("Design #{}", stone.entry_id)),
            carat: stone.carat,
            position: positions
                .get(index)
                .map(|&(slab, bar, piece)| PiecePosition { slab, bar, piece }),
            note,
        });
        push_piece_box(&mut scene.saw, stone, &frame);
    }
    scene.stone_starts.push(scene.facet_colors.len());
    scene
}

/// What the render worker draws.
#[derive(Debug)]
#[expect(
    clippy::large_enum_variant,
    reason = "a scene lives once inside an `Arc<Scene>` and is never moved by value in a loop; boxing `ModelScene` would touch every construction and match site"
)]
pub(super) enum SceneKind {
    /// The rough being modelled.
    Model(ModelScene),
    /// A planned result.
    Fit(Box<FitScene>),
}

/// A scene with the serial that identifies it to the renderer's frame cache.
#[derive(Debug)]
pub(super) struct Scene {
    /// Unique per scene.
    pub(super) serial: u64,
    /// The scene itself.
    pub(super) kind: SceneKind,
}

impl Scene {
    /// Wraps `kind` in a new scene.
    #[must_use]
    pub(super) fn new(kind: SceneKind) -> Self {
        Self {
            serial: next_serial(),
            kind,
        }
    }
}

#[cfg(test)]
mod tests;
