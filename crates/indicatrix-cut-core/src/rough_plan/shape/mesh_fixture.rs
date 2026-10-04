//! Test fixtures for the mesh roughs: two closed OBJ solids and the little parser the
//! core's tests use (the core itself does not read OBJ text).

use glam::DVec3;

use super::{RoughBase, hull::import_mesh};
use crate::rough_plan::{DesignHull, PlacedStone, RoughLayout};

/// The notch of [`C_SHAPE_OBJ`] as a box `(min, max)`, extended past the cube in `z` and
/// `+x`.
pub const NOTCH: ([f64; 3], [f64; 3]) = ([10.0, 5.0, -1.0], [21.0, 15.0, 21.0]);

/// A 20 x 20 x 20 mm cube with a 10 x 10 x 20 mm notch cut from the middle of its `+x`
/// face (`x > 10`, `5 < y < 15`, all of `z`): a C seen along `z`. Its volume is 6000 mm^3
/// and its convex hull is the whole cube (8000), so a planner that works from the hull
/// alone fills the notch. The `z` caps are triangulated by hand.
pub const C_SHAPE_OBJ: &str = "\
# C-shaped rough
v 0 0 0
v 20 0 0
v 20 5 0
v 10 5 0
v 10 15 0
v 20 15 0
v 20 20 0
v 0 20 0
v 0 0 20
v 20 0 20
v 20 5 20
v 10 5 20
v 10 15 20
v 20 15 20
v 20 20 20
v 0 20 20
# top
f 9 10 11
f 9 11 12
f 9 12 13
f 9 13 16
f 13 14 15
f 13 15 16
# bottom
f 1 3 2
f 1 4 3
f 1 5 4
f 1 8 5
f 5 7 6
f 5 8 7
# sides
f 1 2 10 9
f 2 3 11 10
f 3 4 12 11
f 4 5 13 12
f 5 6 14 13
f 6 7 15 14
f 7 8 16 15
f 8 1 9 16
";

/// A convex 20 mm cube, as quads.
pub const CUBE_OBJ: &str = "\
# cube
v 0 0 0
v 20 0 0
v 20 20 0
v 0 20 0
v 0 0 20
v 20 0 20
v 20 20 20
v 0 20 20
f 1 4 3 2
f 5 6 7 8
f 1 2 6 5
f 2 3 7 6
f 3 4 8 7
f 4 1 5 8
";

/// The vertices and triangles of OBJ `text`: `v` lines and `f` lines of 1-based plain
/// indices, polygons fanned.
pub fn parse_obj(text: &str) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut points = Vec::new();
    let mut tris = Vec::new();
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match words.next() {
            Some("v") => {
                let c: Vec<f64> = words.map(|w| w.parse().expect("a coordinate")).collect();
                points.push(DVec3::new(c[0], c[1], c[2]));
            }
            Some("f") => {
                let face: Vec<u32> = words
                    .map(|w| w.parse::<u32>().expect("an index") - 1)
                    .collect();
                for k in 1..face.len() - 1 {
                    tris.push([face[0], face[k], face[k + 1]]);
                }
            }
            _ => {}
        }
    }
    (points, tris)
}

/// [`C_SHAPE_OBJ`] as a noisy scan: every triangle split into four `levels` times, and the
/// vertices inside the three walls of the notch moved along the wall's normal by up to
/// `jitter` mm in a fixed pseudo-random pattern. The surface stays closed, and the notch
/// walls become a thicket of slightly different planes, as a scanned surface has.
pub fn noisy_c_shape(levels: u32, jitter: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, mut tris) = parse_obj(C_SHAPE_OBJ);
    for _ in 0..levels {
        let mut mids: std::collections::BTreeMap<(u32, u32), u32> =
            std::collections::BTreeMap::new();
        let mut mid = |points: &mut Vec<DVec3>, a: u32, b: u32| {
            *mids.entry((a.min(b), a.max(b))).or_insert_with(|| {
                points.push((points[a as usize] + points[b as usize]) * 0.5);
                (points.len() - 1) as u32
            })
        };
        let mut split = Vec::with_capacity(tris.len() * 4);
        for &[a, b, c] in &tris {
            let (ab, bc, ca) = (
                mid(&mut points, a, b),
                mid(&mut points, b, c),
                mid(&mut points, c, a),
            );
            split.extend([[a, ab, ca], [ab, b, bc], [ca, bc, c], [ab, bc, ca]]);
        }
        tris = split;
    }
    let near = |x: f64, to: f64| (x - to).abs() < 1e-9;
    let between = |x: f64, lo: f64, hi: f64| x > lo + 1e-9 && x < hi - 1e-9;
    for (i, p) in points.iter_mut().enumerate() {
        let noise = ((i as f64 + 1.0) * 12.9898)
            .sin()
            .mul_add(43758.5453, 0.0)
            .fract()
            * jitter;
        if !between(p.z, 0.0, 20.0) {
            continue;
        }
        if near(p.x, 10.0) && between(p.y, 5.0, 15.0) {
            p.x += noise;
        } else if (near(p.y, 5.0) || near(p.y, 15.0)) && between(p.x, 10.0, 20.0) {
            p.y += noise;
        }
    }
    (points, tris)
}

/// The base of OBJ `text`, imported as a mesh; panics on a note or an error.
pub fn import_obj(text: &str) -> RoughBase {
    let (points, tris) = parse_obj(text);
    let (base, note) = import_mesh(&points, &tris).expect("imports");
    assert_eq!(note, None, "the fixture imports without a note");
    base
}

/// The base of a mesh given as points and triangles, imported; panics on a note or an
/// error.
pub fn import_parts(points: &[DVec3], tris: &[[u32; 3]]) -> RoughBase {
    let (base, note) = import_mesh(points, tris).expect("imports");
    assert_eq!(note, None, "the mesh imports without a note");
    base
}

/// A box design `width` (x) by `height` (y) by `length` (z) centred on the origin, as the
/// exact fit's outline.
pub fn box_hull(entry_id: i64, width: f64, height: f64, length: f64) -> DesignHull {
    let mut vertices = Vec::new();
    for sx in [-0.5, 0.5] {
        for sy in [-0.5, 0.5] {
            for sz in [-0.5, 0.5] {
                vertices.push([sx * width, sy * height, sz * length]);
            }
        }
    }
    DesignHull {
        entry_id,
        vertices,
        volume: width * height * length,
        width,
    }
}

/// Whether the box with `centre`, unit `axes` and half extents `half` along them reaches
/// into the interior of the axis-aligned box `[lo, hi]` (separating axis test; touching
/// is not entering). Independent of the mesh code under test.
pub fn box_enters(centre: DVec3, axes: [DVec3; 3], half: DVec3, lo: DVec3, hi: DVec3) -> bool {
    let (mid, reach) = ((lo + hi) * 0.5, (hi - lo) * 0.5);
    let basis = [DVec3::X, DVec3::Y, DVec3::Z];
    let mut candidates: Vec<DVec3> = axes.to_vec();
    candidates.extend(basis);
    for a in axes {
        for b in basis {
            candidates.push(a.cross(b));
        }
    }
    for axis in candidates {
        if axis.length() < 1e-9 {
            continue;
        }
        let axis = axis.normalize();
        let on_a: f64 = (0..3).map(|i| half[i] * axes[i].dot(axis).abs()).sum();
        let on_b: f64 = (0..3).map(|i| reach[i] * basis[i].dot(axis).abs()).sum();
        if (mid - centre).dot(axis).abs() >= on_a + on_b - 1e-7 {
            return false;
        }
    }
    true
}

/// Whether `stone` of `layout` reaches into the notch: a box-model stone is its axis-aligned
/// box; an exact fit is its design's box (from `hulls`) under its pose.
pub fn stone_enters_notch(layout: &RoughLayout, stone: &PlacedStone, hulls: &[DesignHull]) -> bool {
    let (lo, hi) = (DVec3::from(NOTCH.0), DVec3::from(NOTCH.1));
    let centre = DVec3::from(stone.pose.center_mm);
    if layout.exact_fit {
        let hull = hulls
            .iter()
            .find(|h| h.entry_id == stone.entry_id)
            .expect("the design's outline");
        let mut half = DVec3::ZERO;
        for v in &hull.vertices {
            half = half.max(DVec3::from(*v).abs());
        }
        let axes = stone.pose.axes.map(DVec3::from);
        box_enters(centre, axes, half * stone.pose.mm_per_unit, lo, hi)
    } else {
        let half = DVec3::from(stone.stone_size_mm) * 0.5;
        box_enters(centre, [DVec3::X, DVec3::Y, DVec3::Z], half, lo, hi)
    }
}
