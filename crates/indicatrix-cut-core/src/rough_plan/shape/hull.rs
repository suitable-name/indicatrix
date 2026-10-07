//! A rough given as a point cloud (an imported mesh), modelled by its convex hull.
//!
//! The hull is registered once under a content id and a [`RoughBase::Hull`] carries that
//! id, which keeps the base `Copy` like the parametric ones. An id is a hash of the hull's
//! own planes, so importing the same mesh twice gives the same base.
//!
//! A non-convex mesh (see [`import_mesh`]) is registered under the same id scheme, with the
//! mesh stored beside the hull: the hull still bounds the planner's grid and region, so
//! `RoughBase` stays `Copy` and a convex input registers exactly as [`import_hull`] does.
//!
//! # What the registry keeps
//!
//! A base is a plain number, so nothing can tell the registry which ids are still in use
//! (an undo history, a saved plan, a job in flight all hold them). It therefore never
//! drops an id, and instead keeps what it must cheap:
//!
//! - A hull registered by import or load (a ROOT) stays, one per distinct file the session
//!   has read.
//! - A scaled copy (what Fit to weight makes, a new one for every weight tried) is not
//!   stored. The registry keeps its recipe, the root's id and the total factor, a few
//!   bytes, and builds the copy again from the root whenever it is asked for and no one
//!   holds a built one. Only the few most recently built copies are kept
//!   ([`BUILT_SCALED_CAP`]), so repeated fitting on a 50,000-triangle scan costs a bounded
//!   amount of memory, while every id ever handed out stays valid. A least-recently-used
//!   cap on the shapes themselves would free more, but would break the undo step that
//!   still names an evicted id; a weak reference would need a holder the plain base does
//!   not have.
//!
//! The recipe fixes the copy exactly (the root scaled once by the total factor), so a
//! rebuilt copy is bit-for-bit the copy it replaces and the results of a plan never
//! depend on whether a copy happened to be cached.
//!
//! # Smooth scans
//!
//! A convex outline with more than [`MAX_HULL_PLANES`] distinct planes (a smooth scan has
//! about one per hull triangle) is not refused by [`import_mesh`]: its outline is replaced by
//! a simplified OUTER outline (see [`simplify`]) of at most [`OUTLINE_PLANES`] supporting
//! planes, which contains every point, and the mesh is always kept beside it as the authority
//! on where the material is. The outline then only bounds the grid and seeds the LP; pieces
//! inside it but outside the true hull are air by the mesh. An outline made this way is never
//! registered without its mesh. It is built from the mesh's own welded vertices (the mesh is
//! built first), so the hull of a scan is computed once.
//!
//! [`RoughBase::Hull`]: super::RoughBase::Hull

mod design_hull;
mod inclusion;
mod quick;
mod simplify;

pub(in crate::rough_plan) use simplify::simplified_outer_planes;

pub use design_hull::{
    DesignOutline, OutlineExtents, convex_corners, design_outline, outline_extents,
};
pub use inclusion::{
    InclusionInfo, MAX_INCLUSIONS, MeshParts, add_inclusion_mesh, add_inclusion_meshes,
    add_inclusion_points, import_mesh_with_inclusions, inclusion_list, remove_inclusion,
};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt,
    sync::{Arc, Mutex, MutexGuard, PoisonError, Weak},
};

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

use super::{
    RoughBase,
    mesh::{MeshError, RoughMesh},
};

/// The most distinct planes a hull may have.
///
/// The fit works through every plane of the rough for every candidate placement, so a
/// finely tessellated scan must be simplified first (a pebble's fine region has 162).
pub const MAX_HULL_PLANES: usize = 400;

/// The budget of a simplified outer outline: 6 axis planes and at most 58 clusters.
///
/// The outline only bounds the grid and seeds the LP; the mesh kept beside it is the
/// authority on where the material is. Every plane of the outline that a piece box crosses
/// becomes an LP row in every partial piece, and the dense simplex costs about `J^2` per
/// pivot over `~0.4 J` pivots, so a few dozen planes keep a piece solve cheap where 400
/// would cost a thousand times a convex piece. The convex pebble gets by with 162 planes in
/// its fine region and 42 in its coarse one (`shape/base.rs`); 64 is a little richer than
/// the coarse one. [`MAX_HULL_PLANES`] keeps its meaning: the count above which an outline is
/// simplified, and the cap of an exact hull.
pub const OUTLINE_PLANES: usize = 64;

/// The welded vertex count above which an outline is first built with the fast quickhull
/// (see `quick`); every fixture below it takes the exact incremental hull as before.
const QUICK_HULL_POINTS: usize = 4096;

/// Why a point cloud is not a usable rough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HullError {
    /// Fewer than four points, or all of them on one plane.
    Flat,
    /// A coordinate is not finite.
    NotFinite,
    /// The hull has this many planes, more than [`MAX_HULL_PLANES`]. [`import_mesh`] reports
    /// its triangle count here when the points have no faces to keep as a mesh.
    TooComplex(usize),
    /// The outline has more than [`MAX_HULL_PLANES`] planes (this many triangular faces) and
    /// the faces cannot be used as a mesh, so a simplified outline would plan stones into
    /// air. Repair or simplify the scan.
    ScanUnusable {
        /// Triangular faces of the points' convex hull.
        faces: usize,
        /// Why the mesh cannot be used.
        reason: MeshError,
    },
    /// The mesh kept beside the hull could not be moved or scaled with it (it would
    /// enclose no volume).
    Mesh,
    /// An inclusion can only be added to, or removed from, a rough imported from a mesh
    /// file (a [`RoughBase::Hull`]), and this base is not one (or is not registered).
    NotHull,
    /// The inclusion cannot be used: it does not lie inside the material, reaches the
    /// surface, crosses another inclusion, or is not a closed mesh.
    Inclusion(MeshError),
    /// The inclusion fits without its margin but not with it: it is closer to the surface (or
    /// to another inclusion) than the margin.
    InclusionTooClose,
    /// The rough has no inclusion with this number.
    NoInclusion,
    /// A rough may have at most [`MAX_INCLUSIONS`](inclusion::MAX_INCLUSIONS) inclusions.
    TooManyInclusions,
    /// The rough's own mesh, read back with its inclusions, cannot be used.
    OuterUnusable(MeshError),
}

impl fmt::Display for HullError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Flat => write!(f, "the mesh has no volume (its points lie on one plane)"),
            Self::NotFinite => write!(f, "the mesh has a coordinate that is not a number"),
            Self::TooComplex(n) => write!(
                f,
                "the mesh's convex outline has {n} faces; at most {MAX_HULL_PLANES} are \
                 supported, so simplify the mesh first"
            ),
            Self::ScanUnusable { faces, reason } => write!(
                f,
                "the scan's convex outline has {faces} faces and the scan itself cannot be \
                 used ({reason}), so it has no usable outline; repair or simplify the scan"
            ),
            Self::Mesh => write!(f, "the mesh could not be moved with its outline"),
            Self::NotHull => write!(
                f,
                "inclusions can only be added to a rough imported from a mesh file"
            ),
            Self::Inclusion(reason) => write!(f, "{reason}"),
            Self::InclusionTooClose => write!(
                f,
                "the inclusion is closer to the surface (or to another inclusion) than its \
                 margin allows: lower the margin, or cut the inclusion away as a notch in the \
                 rough's own mesh"
            ),
            Self::NoInclusion => write!(f, "the rough has no such inclusion"),
            Self::TooManyInclusions => write!(
                f,
                "a rough may have at most {} inclusions",
                inclusion::MAX_INCLUSIONS
            ),
            Self::OuterUnusable(reason) => {
                write!(f, "the rough's own mesh cannot be used ({reason})")
            }
        }
    }
}

impl std::error::Error for HullError {}

/// How a point in the coordinates of the file a rough was imported from (in millimetres) maps
/// into the rough's own frame (bounding box at the origin): `p * scale + offset`.
///
/// The import moves the rough so that its bounding box starts at the origin, and Fit to weight
/// scales it; a second mesh from the same scene (an inclusion) is in the file's coordinates, so
/// it needs the same move. The frame is part of a rough's registry id (see `shape_id`, and
/// only when it is not the identity): two imports of one shape at different places in their
/// files are two roughs, each keeping its own frame, whichever registered first.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SourceFrame {
    /// The factor the rough was scaled by since the import (1 until Fit to weight).
    pub scale: f64,
    /// The translation after the scaling, in mm.
    pub offset: DVec3,
}

impl SourceFrame {
    /// The frame of a rough whose file coordinates are its own: nothing to move.
    pub const IDENTITY: Self = Self {
        scale: 1.0,
        offset: DVec3::ZERO,
    };

    /// `point` of the file in the rough's frame.
    #[must_use]
    pub fn apply(&self, point: DVec3) -> DVec3 {
        point * self.scale + self.offset
    }

    /// This frame followed by a scaling of the rough by `factor`.
    fn scaled(self, factor: f64) -> Self {
        Self {
            scale: self.scale * factor,
            offset: self.offset * factor,
        }
    }

    /// This frame followed by a translation of the rough by `by`.
    fn shifted(self, by: DVec3) -> Self {
        Self {
            scale: self.scale,
            offset: self.offset + by,
        }
    }
}

/// The planes and corners of a registered hull, in the rough frame (bounding box at the
/// origin), and the mesh inside it when the rough is not convex.
#[derive(Debug)]
struct HullShape {
    /// Outward halfspaces `n . p <= m`.
    planes: Vec<(DVec3, f64)>,
    /// The corners of the polytope.
    vertices: Vec<DVec3>,
    /// The non-convex solid, in the same frame; `None` for a convex rough.
    mesh: Option<Arc<RoughMesh>>,
    /// The triangular faces of the exact convex hull when `planes` is a simplified outer
    /// outline of it (always with a mesh); `None` for an exact outline.
    outline_faces: Option<usize>,
    /// Where the coordinates of the imported file went.
    source: SourceFrame,
}

/// How many built scaled copies the registry keeps alive besides the ones in use.
pub(super) const BUILT_SCALED_CAP: usize = 4;

/// Marks the id of a scaled copy, so it cannot equal the content id of a registered hull.
const SCALED_ID_TAG: u64 = 0x5ca1_ed5c_a1ed_5ca1;

/// Hashed into the id of a mesh that has inclusions, between its triangles and the first
/// triangle of each inclusion (see [`shape_id`]).
const INCLUSION_MARK: f64 = -7.77e300;

/// Hashed into the id of a rough whose source frame is not the identity, before the frame's
/// numbers (see [`shape_id`]).
const FRAME_MARK: f64 = -3.33e300;

/// One id of the registry.
enum Entry {
    /// A hull registered by import or load, kept for the session.
    Root(Arc<HullShape>),
    /// A root scaled once by a factor: a recipe, built again on demand.
    Scaled(Scaled),
}

/// The recipe of a scaled copy and, while someone holds it, the copy.
struct Scaled {
    /// The id of the root hull, an [`Entry::Root`].
    root: u64,
    /// The factor that takes the root to this copy (the product of every scaling since).
    factor: f64,
    /// The bounding-box extents of the copy, so the base needs no rebuild to be made.
    extents: DVec3,
    /// The built copy, while the registry's recent list or a caller holds it.
    built: Weak<HullShape>,
}

/// Every hull registered so far, by id, and the scaled copies built most recently.
struct Registry {
    entries: BTreeMap<u64, Entry>,
    /// The latest built scaled copies, oldest first, which keeps them alive.
    recent: VecDeque<(u64, Arc<HullShape>)>,
}

impl Registry {
    /// Keeps `shape`, the built copy of scaled hull `id`, among the most recent ones.
    fn keep(&mut self, id: u64, shape: &Arc<HullShape>) {
        self.recent.retain(|(kept, _)| *kept != id);
        self.recent.push_back((id, Arc::clone(shape)));
        while self.recent.len() > BUILT_SCALED_CAP {
            self.recent.pop_front();
        }
    }

    /// The built copy of scaled hull `id` that someone already holds, or else `fresh`,
    /// which becomes its built copy. Two threads that built it at once share one copy.
    fn adopt(&mut self, id: u64, fresh: Arc<HullShape>) -> Arc<HullShape> {
        let Some(Entry::Scaled(copy)) = self.entries.get_mut(&id) else {
            return fresh;
        };
        if let Some(live) = copy.built.upgrade() {
            return live;
        }
        copy.built = Arc::downgrade(&fresh);
        fresh
    }
}

static REGISTRY: Mutex<Registry> = Mutex::new(Registry {
    entries: BTreeMap::new(),
    recent: VecDeque::new(),
});

/// The registry, whatever happened to a thread that held it before.
fn registry() -> MutexGuard<'static, Registry> {
    REGISTRY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// How many built scaled copies the registry is holding (at most [`BUILT_SCALED_CAP`]).
#[cfg(test)]
pub(super) fn built_scaled_copies() -> usize {
    registry().recent.len()
}

/// One triangle of the hull under construction, wound counter-clockwise seen from outside.
pub(crate) struct Face {
    /// The point indices.
    v: [usize; 3],
    /// The outward unit normal.
    n: DVec3,
    /// The plane offset: `n . p == d` on the face.
    d: f64,
}

impl Face {
    /// The face through points `a`, `b`, `c` of `pts`.
    fn new(pts: &[DVec3], a: usize, b: usize, c: usize) -> Self {
        let n = (pts[b] - pts[a]).cross(pts[c] - pts[a]).normalize_or_zero();
        Self {
            v: [a, b, c],
            n,
            d: n.dot(pts[a]),
        }
    }

    /// The directed edges, in winding order.
    const fn edges(&self) -> [(usize, usize); 3] {
        [
            (self.v[0], self.v[1]),
            (self.v[1], self.v[2]),
            (self.v[2], self.v[0]),
        ]
    }
}

/// The index in `pts` that maximises `score`, the first on a tie.
fn argmax(pts: &[DVec3], score: impl Fn(DVec3) -> f64) -> usize {
    let mut best = 0;
    let mut best_score = f64::NEG_INFINITY;
    for (i, &p) in pts.iter().enumerate() {
        let s = score(p);
        if s > best_score {
            best_score = s;
            best = i;
        }
    }
    best
}

/// A tetrahedron of well-spread points of `pts`, as outward-wound faces with its point
/// indices, or `None` when the points are flat to within `eps`.
fn first_tetrahedron(pts: &[DVec3], eps: f64) -> Option<([usize; 4], Vec<Face>)> {
    let i0 = argmax(pts, |p| -p.x);
    let p0 = pts[i0];
    let i1 = argmax(pts, |p| p.distance_squared(p0));
    if pts[i1].distance(p0) <= eps {
        return None;
    }
    let dir = (pts[i1] - p0).normalize();
    let i2 = argmax(pts, |p| (p - p0).cross(dir).length_squared());
    let normal = (pts[i1] - p0).cross(pts[i2] - p0).normalize_or_zero();
    if normal == DVec3::ZERO || (pts[i2] - p0).cross(dir).length() <= eps {
        return None;
    }
    let i3 = argmax(pts, |p| (p - p0).dot(normal).abs());
    if (pts[i3] - p0).dot(normal).abs() <= eps {
        return None;
    }
    let ids = [i0, i1, i2, i3];
    let centre = ids.iter().map(|&i| pts[i]).sum::<DVec3>() / 4.0;
    let faces = [[0, 1, 2], [0, 1, 3], [0, 2, 3], [1, 2, 3]]
        .iter()
        .map(|&[a, b, c]| {
            let face = Face::new(pts, ids[a], ids[b], ids[c]);
            if face.n.dot(centre) - face.d > 0.0 {
                Face::new(pts, ids[a], ids[c], ids[b])
            } else {
                face
            }
        })
        .collect();
    Some((ids, faces))
}

/// The triangles of the convex hull of `pts`, outward wound; `None` when `pts` has no
/// volume. Points are added in index order, so the result is deterministic.
fn hull_faces(pts: &[DVec3], eps: f64) -> Option<Vec<Face>> {
    let (seed, mut faces) = first_tetrahedron(pts, eps)?;
    for (index, &p) in pts.iter().enumerate() {
        if seed.contains(&index) {
            continue;
        }
        let visible: Vec<bool> = faces.iter().map(|f| f.n.dot(p) - f.d > eps).collect();
        if !visible.contains(&true) {
            continue;
        }
        let (seen, rest): (Vec<_>, Vec<_>) = std::mem::take(&mut faces)
            .into_iter()
            .zip(visible)
            .partition(|&(_, seen)| seen);
        let lit: BTreeSet<(usize, usize)> = seen.iter().flat_map(|(f, _)| f.edges()).collect();
        faces = rest.into_iter().map(|(f, _)| f).collect();
        faces.extend(
            seen.iter()
                .flat_map(|(f, _)| f.edges())
                .filter(|&(a, b)| !lit.contains(&(b, a)))
                .map(|(a, b)| Face::new(pts, a, b, index)),
        );
    }
    Some(faces)
}

/// The distinct planes of `faces`: coplanar triangles collapse to one plane.
fn distinct_planes(faces: &[Face], eps: f64) -> Vec<(DVec3, f64)> {
    distinct_planes_up_to(faces, eps, usize::MAX).unwrap_or_default()
}

/// [`distinct_planes`], or `None` as soon as there are more than `cap` of them (a smooth
/// scan, whose count would otherwise cost a quadratic scan of every face).
fn distinct_planes_up_to(faces: &[Face], eps: f64, cap: usize) -> Option<Vec<(DVec3, f64)>> {
    let mut planes: Vec<(DVec3, f64)> = Vec::new();
    for face in faces {
        let known = planes
            .iter()
            .any(|&(n, d)| n.dot(face.n) > 1.0 - 1e-9 && (d - face.d).abs() <= 10.0 * eps);
        if !known {
            if planes.len() >= cap {
                return None;
            }
            planes.push((face.n, face.d));
        }
    }
    Some(planes)
}

/// FNV-1a over the little-endian bytes of `words`.
fn fnv(words: impl IntoIterator<Item = u64>) -> u64 {
    words.into_iter().fold(0xcbf2_9ce4_8422_2325, |hash, word| {
        word.to_le_bytes().iter().fold(hash, |h, &byte| {
            (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

/// FNV-1a over the bits of `values`.
fn content_id(values: impl IntoIterator<Item = f64>) -> u64 {
    fnv(values.into_iter().map(f64::to_bits))
}

/// The shape of the polytope `planes` (any frame), translated so its bounding box starts at
/// the origin, and the extents of that box. `mesh`, in the same frame as `planes`, is
/// translated with it. `source` is the frame the planes' own frame is in, which the move
/// is added to.
fn build_shape(
    planes: &[(DVec3, f64)],
    mesh: Option<&RoughMesh>,
    source: SourceFrame,
) -> Result<(HullShape, DVec3), HullError> {
    let (_, corners) = measure_solid_with_vertices(planes).ok_or(HullError::Flat)?;
    let lo = corners
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &v| m.min(v));
    let hi = corners
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &v| m.max(v));
    let planes: Vec<(DVec3, f64)> = planes.iter().map(|&(n, d)| (n, d - n.dot(lo))).collect();
    let vertices: Vec<DVec3> = corners.iter().map(|&v| v - lo).collect();
    let mesh = match mesh {
        Some(m) => Some(Arc::new(m.translated(-lo).ok_or(HullError::Mesh)?)),
        None => None,
    };
    let shape = HullShape {
        planes,
        vertices,
        mesh,
        outline_faces: None,
        source: source.shifted(-lo),
    };
    Ok((shape, hi - lo))
}

/// The content id of `shape`, whose bounding box has `extents`: a hash of its planes, its
/// extents and, when it has one, its mesh, so two meshes with one hull are two roughs and
/// without a mesh the id is the hull's alone.
fn shape_id(shape: &HullShape, extents: DVec3) -> u64 {
    let mesh_values: Vec<f64> = shape.mesh.as_deref().map_or_else(Vec::new, |m| {
        let verts = m.vertices().iter().flat_map(DVec3::to_array);
        let tris = m.triangles().iter().flatten().map(|&i| f64::from(i));
        // Which shells are inclusions is part of the rough: an inclusion and a hollow of the
        // same shape have the same triangles and must not share an id. Without inclusions
        // nothing is added, so every other id is what it always was.
        let marks = (!m.inclusion_shells().is_empty()).then_some(INCLUSION_MARK);
        verts
            .chain(tris)
            .chain(marks)
            .chain(m.inclusion_shells().iter().map(|&i| f64::from(i)))
            .collect()
    });
    // Where the file's coordinates went is part of the rough: the same shape read from two
    // places in two files is two roughs, each with its own frame (the registry keeps the
    // first one registered under an id, so a shared id would hand the second import the
    // first one's frame, whichever happened to register first). A rough whose file
    // coordinates are its own adds nothing, so every such id is what it always was.
    let frame = (shape.source != SourceFrame::IDENTITY).then(|| {
        let SourceFrame { scale, offset } = shape.source;
        // `+ 0.0` turns a negative zero into a zero, which compares equal but hashes apart.
        [FRAME_MARK, scale, offset.x, offset.y, offset.z].map(|v| v + 0.0)
    });
    content_id(
        shape
            .planes
            .iter()
            .flat_map(|&(n, d)| [n.x, n.y, n.z, d])
            .chain(extents.to_array())
            .chain(mesh_values)
            .chain(frame.into_iter().flatten()),
    )
}

/// The base that names hull `id` with bounding box `extents`.
const fn hull_base(id: u64, extents: DVec3) -> RoughBase {
    RoughBase::Hull {
        id,
        x_mm: extents.x,
        y_mm: extents.y,
        z_mm: extents.z,
    }
}

/// Registers the polytope `planes` (any frame) and returns its base, translated so the
/// bounding box starts at the origin. `mesh`, in the same frame as `planes`, is translated
/// with it and joins the id, so two meshes with one hull are two roughs; without a mesh the
/// id is the hull's alone. `outline_faces` is `Some` when `planes` is a simplified outer
/// outline of a hull with that many faces (it needs a mesh).
fn register_planes(
    planes: &[(DVec3, f64)],
    mesh: Option<&RoughMesh>,
    outline_faces: Option<usize>,
) -> Result<RoughBase, HullError> {
    register_planes_from(planes, mesh, outline_faces, SourceFrame::IDENTITY)
}

/// [`register_planes`] for planes whose own frame is `source` away from the file's (a saved
/// rough, whose mesh is already in a moved frame).
fn register_planes_from(
    planes: &[(DVec3, f64)],
    mesh: Option<&RoughMesh>,
    outline_faces: Option<usize>,
    source: SourceFrame,
) -> Result<RoughBase, HullError> {
    let (mut shape, extents) = build_shape(planes, mesh, source)?;
    shape.outline_faces = outline_faces;
    let id = shape_id(&shape, extents);
    registry()
        .entries
        .entry(id)
        .or_insert_with(|| Entry::Root(Arc::new(shape)));
    Ok(hull_base(id, extents))
}

/// The convex hull of `points` (mm) as a rough base.
///
/// # Errors
///
/// Returns [`HullError`] when a coordinate is not finite, the points have no volume, or
/// the hull has more than [`MAX_HULL_PLANES`] planes.
pub fn import_hull(points: &[DVec3]) -> Result<RoughBase, HullError> {
    register_planes(&hull_planes(points)?, None, None)
}

/// The triangles of the convex hull of `points` and the tolerance they were built with.
pub(crate) fn hull_triangles(points: &[DVec3]) -> Result<(Vec<Face>, f64), HullError> {
    if points.iter().any(|p| !p.is_finite()) {
        return Err(HullError::NotFinite);
    }
    let lo = points
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &p| m.min(p));
    let hi = points
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
    let eps = 1e-9 * (hi - lo).max_element().max(1e-12);
    let faces = hull_faces(points, eps).ok_or(HullError::Flat)?;
    Ok((faces, eps))
}

/// The distinct planes of the convex hull of `points`.
fn hull_planes(points: &[DVec3]) -> Result<Vec<(DVec3, f64)>, HullError> {
    let (faces, eps) = hull_triangles(points)?;
    let planes = distinct_planes(&faces, eps);
    if planes.len() > MAX_HULL_PLANES {
        return Err(HullError::TooComplex(planes.len()));
    }
    Ok(planes)
}

/// The outline an imported mesh's points get.
struct Outline {
    /// Outward halfspaces `n . p <= d`: the exact hull's when it has at most
    /// [`MAX_HULL_PLANES`], else a simplified outer outline of at most [`OUTLINE_PLANES`]
    /// supporting planes.
    planes: Vec<(DVec3, f64)>,
    /// The triangular faces of the exact hull when `planes` is the simplified outline.
    simplified_from: Option<usize>,
}

/// The outline of `points`: [`hull_planes`] when the hull is simple enough (bit for bit the
/// same planes), else the simplified outer outline.
///
/// Above [`QUICK_HULL_POINTS`] points the hull of a scan is first built by the fast
/// quickhull; only when its planes are too many (so the outline is simplified anyway, and no
/// bit of an exact outline depends on it) is that hull used. A hull with few planes is always
/// rebuilt by the exact algorithm, so every exact outline is bit for bit the old one.
fn hull_outline(points: &[DVec3]) -> Result<Outline, HullError> {
    if points.len() > QUICK_HULL_POINTS {
        let eps = quick::tolerance(points)?;
        let fast = quick::quick_hull_faces(points, eps).ok_or(HullError::Flat)?;
        if distinct_planes_up_to(&fast, eps, MAX_HULL_PLANES).is_none() {
            return Ok(Outline {
                planes: simplified_outer_planes(points, &fast, OUTLINE_PLANES, eps),
                simplified_from: Some(fast.len()),
            });
        }
    }
    let (faces, eps) = hull_triangles(points)?;
    if let Some(planes) = distinct_planes_up_to(&faces, eps, MAX_HULL_PLANES) {
        return Ok(Outline {
            planes,
            simplified_from: None,
        });
    }
    Ok(Outline {
        planes: simplified_outer_planes(points, &faces, OUTLINE_PLANES, eps),
        simplified_from: Some(faces.len()),
    })
}

/// The distinct outward planes `n . p <= d` of the convex hull of `points`; `None` when
/// they are empty, not finite or flat. For a stone's outline, whose planes a mesh check
/// needs (see the fit's `StoneGuard`).
pub(in crate::rough_plan) fn convex_planes(points: &[DVec3]) -> Option<Vec<(DVec3, f64)>> {
    if points.is_empty() || points.iter().any(|p| !p.is_finite()) {
        return None;
    }
    let lo = points
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &p| m.min(p));
    let hi = points
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &p| m.max(p));
    let eps = 1e-9 * (hi - lo).max_element().max(1e-12);
    let faces = hull_faces(points, eps)?;
    Some(distinct_planes(&faces, eps))
}

/// The rough of a triangle mesh: `points` and triangles `tris` indexing them (mm).
///
/// A closed mesh that is genuinely smaller than its own convex hull (volume under
/// `1 - 1e-6` of it) is KEPT beside that hull, so a notch or hollow is respected by the
/// planner. The hull and the frame (bounding box, origin) are then those of the mesh's own
/// welded, referenced vertices, which is all a saved plan stores: a stray or unreferenced
/// `v` line does not change the rough, before or after a save. A convex mesh is not kept
/// and returns the very base [`import_hull`] returns for ALL of `points` (same id), so
/// convex roughs plan as they always did.
///
/// A mesh that cannot be used (open, inconsistently wound, too many triangles, ...) is not
/// an error: the rough falls back to the hull and the second value is a note saying why,
/// for the app to show. No faces at all is a plain point cloud and gives no note.
///
/// # Errors
///
/// A convex outline with more than [`MAX_HULL_PLANES`] planes (a smooth scan) is replaced by
/// a simplified outer outline of at most [`OUTLINE_PLANES`] planes (built from the mesh's own
/// welded vertices) and the mesh is ALWAYS kept, even a convex one, because the
/// outline then holds air and only the mesh knows where the material is. The note for that
/// is not returned here (a saved plan must reload without one): ask
/// [`outline_note`] for the base. If such a scan's mesh cannot be used, nothing sound
/// stands in for it and the import is refused.
///
/// # Errors
///
/// Returns [`HullError`] when [`import_hull`] does for `points` for any reason but the
/// plane count, and [`HullError::TooComplex`] or [`HullError::ScanUnusable`] when the
/// outline is too complex and there is no usable mesh.
pub fn import_mesh(
    points: &[DVec3],
    tris: &[[u32; 3]],
) -> Result<(RoughBase, Option<String>), HullError> {
    // The same validation the hull of `points` does first, so a bad point is reported before
    // anything else, as `import_hull` does.
    if points.iter().any(|p| !p.is_finite()) {
        return Err(HullError::NotFinite);
    }
    // The mesh comes first: its own welded vertices are what a saved plan stores and what the
    // outline is built from, so the hull is computed once and, for a smooth scan, never for
    // the raw points.
    let mesh = match RoughMesh::new(points, tris) {
        Ok(mesh) => mesh,
        Err(err) => return import_without_mesh(points, err),
    };
    let own = hull_outline(mesh.vertices())?;
    if let Some(faces) = own.simplified_from {
        // A simplified outline holds air, so the mesh stays even if it is convex.
        let base = register_planes(&own.planes, Some(&mesh), Some(faces))?;
        return Ok((base, None));
    }
    import_exact(points, &mesh, own).map(|base| (base, None))
}

/// The base of a usable mesh whose own vertices have an exact outline `own`: the mesh beside
/// it when the mesh is genuinely smaller than that hull, else the plain hull (of ALL of
/// `points`, stray vertices included, as [`import_hull`] gives).
fn import_exact(points: &[DVec3], mesh: &RoughMesh, own: Outline) -> Result<RoughBase, HullError> {
    if measure_solid_with_vertices(&own.planes)
        .is_some_and(|(metrics, _)| mesh.volume() < metrics.volume * (1.0 - 1e-6))
    {
        return register_planes(&own.planes, Some(mesh), None);
    }
    // A convex mesh: the hull of all the points. When they are the mesh's own vertices that
    // hull is `own`; else it is built again (a stray vertex may move it).
    if mesh.vertices() == points {
        return register_planes(&own.planes, None, None);
    }
    let all = hull_outline(points)?;
    // Points whose hull is too complex cannot stand in for the mesh's exact one.
    let planes = if all.simplified_from.is_none() {
        all.planes
    } else {
        own.planes
    };
    register_planes(&planes, None, None)
}

/// The import of a rough whose mesh `err` cannot be used: the hull of all of `points`, with a
/// note saying why, or none for a plain point cloud (no faces at all). A hull that is too
/// complex for a plain outline is refused, since a simplified outline alone would plan stones
/// into air.
fn import_without_mesh(
    points: &[DVec3],
    err: MeshError,
) -> Result<(RoughBase, Option<String>), HullError> {
    let outline = hull_outline(points)?;
    if let Some(faces) = outline.simplified_from {
        return Err(match err {
            MeshError::NoFaces => HullError::TooComplex(faces),
            reason => HullError::ScanUnusable { faces, reason },
        });
    }
    let base = register_planes(&outline.planes, None, None)?;
    let note = match err {
        MeshError::NoFaces => None,
        err => Some(format!(
            "The mesh could not be used as it is ({err}); its convex outline is used instead."
        )),
    };
    Ok((base, note))
}

/// What to tell the user about hull `id` when its outline was simplified.
///
/// That happens because the scan's convex hull has too many faces. `None` for an exact
/// outline or an id that is not a registered root (a scaled copy included).
///
/// It is not part of [`import_mesh`]'s note because reading a saved plan imports the same
/// mesh and must come back without one.
#[must_use]
pub fn outline_note(id: u64) -> Option<String> {
    let shape = root_shape(id)?;
    let faces = shape.outline_faces?;
    Some(format!(
        "The scan's outline had {faces} faces; a simplified outline of {} planes is used for \
         the grid, and the scan itself decides where the material is.",
        shape.planes.len()
    ))
}

/// The shape of the root hull `id`; `None` for any other id.
fn root_shape(id: u64) -> Option<Arc<HullShape>> {
    let reg = registry();
    if let Entry::Root(shape) = reg.entries.get(&id)? {
        Some(Arc::clone(shape))
    } else {
        None
    }
}

/// `root` with every length multiplied by `factor`, and the extents of the result; the
/// copy a [`Scaled`] recipe stands for.
fn scaled_shape(root: &HullShape, factor: f64) -> Result<(HullShape, DVec3), HullError> {
    let planes: Vec<(DVec3, f64)> = root.planes.iter().map(|&(n, d)| (n, d * factor)).collect();
    let mesh = match root.mesh.as_deref() {
        Some(m) => Some(m.scaled(factor).ok_or(HullError::Mesh)?),
        None => None,
    };
    let (mut shape, extents) = build_shape(&planes, mesh.as_ref(), root.source.scaled(factor))?;
    shape.outline_faces = root.outline_faces;
    Ok((shape, extents))
}

/// How the coordinates of the file hull `id` came from map into its frame.
///
/// `None` when `id` is not registered. A rough read back from a saved plan has the frame the
/// plan stored, or the identity when it stored none.
#[must_use]
pub fn source_frame(id: u64) -> Option<SourceFrame> {
    lookup(id).map(|shape| shape.source)
}

/// The shape of hull `id`, built again from its root when it is a scaled copy that no one
/// holds. `None` when `id` is not registered (or its root cannot be scaled any more).
fn lookup(id: u64) -> Option<Arc<HullShape>> {
    let (root, factor) = {
        let mut reg = registry();
        let held = match reg.entries.get(&id)? {
            Entry::Root(shape) => return Some(Arc::clone(shape)),
            Entry::Scaled(copy) => copy.built.upgrade().ok_or((copy.root, copy.factor)),
        };
        match held {
            Ok(shape) => {
                reg.keep(id, &shape);
                drop(reg);
                return Some(shape);
            }
            Err(recipe) => recipe,
        }
    };
    // The build takes a while for a big mesh: not under the lock.
    let root = root_shape(root)?;
    let (shape, _) = scaled_shape(&root, factor).ok()?;
    let mut reg = registry();
    let shape = reg.adopt(id, Arc::new(shape));
    reg.keep(id, &shape);
    drop(reg);
    Some(shape)
}

/// The non-convex mesh of hull `id`: `None` for a convex rough or an id that is not
/// registered.
#[must_use]
pub fn mesh(id: u64) -> Option<Arc<RoughMesh>> {
    lookup(id).and_then(|shape| shape.mesh.clone())
}

/// Whether hull `id` is registered, as a root or as a scaled copy.
///
/// Only the registry's table is read: an evicted scaled copy is NOT built again and nothing
/// is cloned, which is what a validity check wants. Every scaled copy names a root that is
/// registered for good, so a registered id always resolves (see [`lookup`]).
pub(super) fn is_registered(id: u64) -> bool {
    registry().entries.contains_key(&id)
}

/// Whether the registry holds a built copy of scaled hull `id` right now.
#[cfg(test)]
pub(super) fn is_built(id: u64) -> bool {
    registry().recent.iter().any(|(kept, _)| *kept == id)
}

/// The halfspaces of hull `id`, `None` when it is not registered.
pub(super) fn halfspaces(id: u64) -> Option<Vec<(DVec3, f64)>> {
    lookup(id).map(|shape| shape.planes.clone())
}

/// The corners of hull `id`, `None` when it is not registered.
pub(super) fn vertices(id: u64) -> Option<Vec<DVec3>> {
    lookup(id).map(|shape| shape.vertices.clone())
}

/// The root hull that `id` is, or is a scaled copy of, and the factor that takes the root
/// to hull `id` scaled once more by `factor`. `None` when `id` is not registered.
fn lineage(id: u64, factor: f64) -> Option<(u64, f64)> {
    let reg = registry();
    match reg.entries.get(&id)? {
        Entry::Root(_) => Some((id, factor)),
        Entry::Scaled(copy) => Some((copy.root, copy.factor * factor)),
    }
}

/// The extents of the scaled copy `id`, `None` when it is not registered as one.
fn scaled_extents(id: u64) -> Option<DVec3> {
    let reg = registry();
    if let Entry::Scaled(copy) = reg.entries.get(&id)? {
        Some(copy.extents)
    } else {
        None
    }
}

/// Hull `id` with every length multiplied by `factor`; the base itself when `id` is not
/// registered or `factor` is not a usable scale.
///
/// The copy is registered as a recipe (the root and the total factor, see the module
/// documentation), so a hundred scalings of a big mesh do not keep a hundred meshes. Its id
/// follows from the recipe alone: scaling by the same factor again names the same copy.
pub(super) fn scaled(base: RoughBase, id: u64, factor: f64) -> RoughBase {
    if !(factor.is_finite() && factor > 0.0) {
        return base;
    }
    let Some((root, total)) = lineage(id, factor) else {
        return base;
    };
    if !(total.is_finite() && total > 0.0) {
        return base;
    }
    let copy_id = fnv([SCALED_ID_TAG, root, total.to_bits()]);
    if let Some(extents) = scaled_extents(copy_id) {
        return hull_base(copy_id, extents);
    }
    let Some(root_shape) = root_shape(root) else {
        return base;
    };
    let Ok((shape, extents)) = scaled_shape(&root_shape, total) else {
        return base;
    };
    let shape = Arc::new(shape);
    let mut reg = registry();
    reg.entries.entry(copy_id).or_insert_with(|| {
        Entry::Scaled(Scaled {
            root,
            factor: total,
            extents,
            built: Arc::downgrade(&shape),
        })
    });
    reg.keep(copy_id, &shape);
    drop(reg);
    hull_base(copy_id, extents)
}
