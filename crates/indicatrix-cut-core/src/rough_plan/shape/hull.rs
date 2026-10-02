//! A rough given as a point cloud (an imported mesh), modelled by its convex hull.
//!
//! The hull is registered once under a content id and a [`RoughBase::Hull`] carries that
//! id, which keeps the base `Copy` like the parametric ones. The registry only grows
//! (one entry per distinct hull imported or loaded in the session), and an id is a hash
//! of the hull's own planes, so importing the same mesh twice gives the same base.
//!
//! [`RoughBase::Hull`]: super::RoughBase::Hull

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    sync::{Arc, Mutex, PoisonError},
};

use glam::DVec3;
use indicatrix::geometry::stone_metrics::measure_solid_with_vertices;

use super::RoughBase;

/// The most distinct planes a hull may have.
///
/// The fit works through every plane of the rough for every candidate placement, so a
/// finely tessellated scan must be simplified first (a pebble's fine region has 162).
pub const MAX_HULL_PLANES: usize = 400;

/// Why a point cloud is not a usable rough.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HullError {
    /// Fewer than four points, or all of them on one plane.
    Flat,
    /// A coordinate is not finite.
    NotFinite,
    /// The hull has this many planes, more than [`MAX_HULL_PLANES`].
    TooComplex(usize),
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
        }
    }
}

impl std::error::Error for HullError {}

/// The planes and corners of a registered hull, in the rough frame (bounding box at the
/// origin).
#[derive(Debug)]
struct HullShape {
    /// Outward halfspaces `n . p <= m`.
    planes: Vec<(DVec3, f64)>,
    /// The corners of the polytope.
    vertices: Vec<DVec3>,
}

/// Every hull registered so far, by id.
static REGISTRY: Mutex<BTreeMap<u64, Arc<HullShape>>> = Mutex::new(BTreeMap::new());

/// One triangle of the hull under construction, wound counter-clockwise seen from outside.
struct Face {
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
    let mut planes: Vec<(DVec3, f64)> = Vec::new();
    for face in faces {
        let known = planes
            .iter()
            .any(|&(n, d)| n.dot(face.n) > 1.0 - 1e-9 && (d - face.d).abs() <= 10.0 * eps);
        if !known {
            planes.push((face.n, face.d));
        }
    }
    planes
}

/// FNV-1a over the bits of `values`.
fn content_id(values: impl IntoIterator<Item = f64>) -> u64 {
    values.into_iter().fold(0xcbf2_9ce4_8422_2325, |hash, v| {
        v.to_bits().to_le_bytes().iter().fold(hash, |h, &byte| {
            (h ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

/// Registers the polytope `planes` (any frame) and returns its base, translated so the
/// bounding box starts at the origin.
fn register_planes(planes: &[(DVec3, f64)]) -> Result<RoughBase, HullError> {
    let (_, corners) = measure_solid_with_vertices(planes).ok_or(HullError::Flat)?;
    let lo = corners
        .iter()
        .fold(DVec3::splat(f64::INFINITY), |m, &v| m.min(v));
    let hi = corners
        .iter()
        .fold(DVec3::splat(f64::NEG_INFINITY), |m, &v| m.max(v));
    let extents = hi - lo;
    let planes: Vec<(DVec3, f64)> = planes.iter().map(|&(n, d)| (n, d - n.dot(lo))).collect();
    let vertices: Vec<DVec3> = corners.iter().map(|&v| v - lo).collect();
    let id = content_id(
        planes
            .iter()
            .flat_map(|&(n, d)| [n.x, n.y, n.z, d])
            .chain(extents.to_array()),
    );
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .entry(id)
        .or_insert_with(|| Arc::new(HullShape { planes, vertices }));
    Ok(RoughBase::Hull {
        id,
        x_mm: extents.x,
        y_mm: extents.y,
        z_mm: extents.z,
    })
}

/// The convex hull of `points` (mm) as a rough base.
///
/// # Errors
///
/// Returns [`HullError`] when a coordinate is not finite, the points have no volume, or
/// the hull has more than [`MAX_HULL_PLANES`] planes.
pub fn import_hull(points: &[DVec3]) -> Result<RoughBase, HullError> {
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
    let planes = distinct_planes(&faces, eps);
    if planes.len() > MAX_HULL_PLANES {
        return Err(HullError::TooComplex(planes.len()));
    }
    register_planes(&planes)
}

/// The registered shape of hull `id`.
fn lookup(id: u64) -> Option<Arc<HullShape>> {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&id)
        .cloned()
}

/// The halfspaces of hull `id`, `None` when it is not registered.
pub(super) fn halfspaces(id: u64) -> Option<Vec<(DVec3, f64)>> {
    lookup(id).map(|shape| shape.planes.clone())
}

/// The corners of hull `id`, `None` when it is not registered.
pub(super) fn vertices(id: u64) -> Option<Vec<DVec3>> {
    lookup(id).map(|shape| shape.vertices.clone())
}

/// Hull `id` with every length multiplied by `factor`; the base itself when `id` is not
/// registered or `factor` is not a usable scale.
pub(super) fn scaled(base: RoughBase, id: u64, factor: f64) -> RoughBase {
    let Some(shape) = lookup(id) else {
        return base;
    };
    if !(factor.is_finite() && factor > 0.0) {
        return base;
    }
    let planes: Vec<(DVec3, f64)> = shape.planes.iter().map(|&(n, d)| (n, d * factor)).collect();
    register_planes(&planes).unwrap_or(base)
}
