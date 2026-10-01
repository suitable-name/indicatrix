//! Low-level scanline polygon fill and depth-tested edge/marker drawing
//! primitives shared by [`super::render`] and [`super::legend`].

use super::{DiagramFrame, DiagramStyle, HATCH_PERIOD};
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::SolidMesh;

/// Axis-aligned 2D bounds `(min_x, max_x, min_y, max_y)` of the projected points.
pub fn bounds(pts: &[(f32, f32, f32)]) -> (f32, f32, f32, f32) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::INFINITY,
        f32::NEG_INFINITY,
    );
    for &(x, y, _) in pts {
        min_x = min_x.min(x);
        max_x = max_x.max(x);
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    (min_x, max_x, min_y, max_y)
}

/// Flat, simple lighting for the diagram: no camera-relative rim term (there is
/// no single camera in an orthographic multi-panel diagram), just a fixed key
/// light so facets at different elevations read as visually distinct.
pub fn shade(normal: Vec3, style: &DiagramStyle) -> [u8; 3] {
    const KEY_LIGHT: Vec3 = Vec3::new(0.35, 0.8, 0.48);
    let light = KEY_LIGHT.normalize();
    let n_dot_l = normal.normalize().dot(light).max(0.0);
    let intensity = 0.45f32.mul_add(n_dot_l, 0.55).clamp(0.0, 1.0);
    std::array::from_fn(|i| {
        (f32::from(style.base_color[i]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8
    })
}

fn blend_toward(base: [u8; 3], target: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        let b = f32::from(base[i]);
        let t = f32::from(target[i]);
        amount.mul_add(t - b, b).round().clamp(0.0, 255.0) as u8
    })
}

/// Twice the signed area of triangle `(ax,ay)-(bx,by)-(px,py)` -- same helper as
/// `raster::edge_fn`, reimplemented here so this module stays free-standing.
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (by - ay).mul_add(-(px - ax), (bx - ax) * (py - ay))
}

/// Depth-tested scanline span fill of one screen-space convex polygon, clipped to
/// `clip` (inclusive `x0,y0,x1,y1`). Unlike `raster::SolidRasterizer::
/// fill_convex_polygon`, `depth` is already affine in screen position (an
/// orthographic projection of a flat facet), so the fitted plane is used
/// directly with no `1/depth` step.
pub fn fill_polygon(
    frame: &mut DiagramFrame,
    verts: &[(f32, f32, f32)],
    facet_id: usize,
    color: [u8; 3],
    style: &DiagramStyle,
    clip: (i32, i32, i32, i32),
    panel_tag: u8,
) {
    let n = verts.len();
    if n < 3 {
        return;
    }
    let (x0, y0, d0) = verts[0];
    let mut best = (1usize, 2usize);
    let mut best_area = 0.0f32;
    for i in 1..n - 1 {
        let (xi, yi, _) = verts[i];
        let (xj, yj, _) = verts[i + 1];
        let area = edge_fn(x0, y0, xi, yi, xj, yj);
        if area.abs() > best_area.abs() {
            best_area = area;
            best = (i, i + 1);
        }
    }
    if best_area.abs() < 1e-8 {
        return;
    }
    let (x1, y1, d1) = verts[best.0];
    let (x2, y2, d2) = verts[best.1];
    let inv_area = 1.0 / best_area;
    let grad_x = (y1 - y2).mul_add(d0, (y2 - y0).mul_add(d1, (y0 - y1) * d2)) * inv_area;
    let grad_y = (x2 - x1).mul_add(d0, (x0 - x2).mul_add(d1, (x1 - x0) * d2)) * inv_area;
    let plane_c = d0 - grad_x.mul_add(x0, grad_y * y0);

    let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
    for &(_, y, _) in verts {
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let (clip_x0, clip_y0, clip_x1, clip_y1) = clip;
    let row_start = (min_y.floor() as i32).max(clip_y0);
    let row_end = (max_y.ceil() as i32).min(clip_y1);
    if row_start > row_end {
        return;
    }

    let flagged = style.flagged.get(facet_id).copied().unwrap_or(false);
    let selected = style.selected.get(facet_id).copied().unwrap_or(false);
    // Blended well toward the highlight color (58%) so a selected facet reads
    // clearly against its neighbors -- see `raster::SolidRasterizer::
    // fill_convex_polygon`'s matching comment.
    let selected_fill = selected.then(|| blend_toward(color, style.selected_color, 0.58));

    for py in row_start..=row_end {
        let sy = py as f32 + 0.5;
        let (mut x_left, mut x_right) = (f32::INFINITY, f32::NEG_INFINITY);
        let mut crossings = 0u32;
        for (i, &(px_a, py_a, _)) in verts.iter().enumerate() {
            let (px_b, py_b, _) = verts[(i + 1) % n];
            if (py_a <= sy) == (py_b <= sy) {
                continue;
            }
            let t = (sy - py_a) / (py_b - py_a);
            let x = t.mul_add(px_b - px_a, px_a);
            x_left = x_left.min(x);
            x_right = x_right.max(x);
            crossings += 1;
        }
        if crossings < 2 {
            continue;
        }
        let col_start = ((x_left - 0.5).ceil() as i32).max(clip_x0);
        let col_end = ((x_right - 0.5).floor() as i32).min(clip_x1);
        if col_start > col_end {
            continue;
        }
        for px in col_start..=col_end {
            let sx = px as f32 + 0.5;
            let depth = grad_x.mul_add(sx, grad_y.mul_add(sy, plane_c));
            let Some(idx) = frame.idx(px, py) else {
                continue;
            };
            let stripe = (px + py).rem_euclid(HATCH_PERIOD * 2) < HATCH_PERIOD;
            if depth < frame.depth[idx] {
                frame.depth[idx] = depth;
                frame.pick[idx] = facet_id as u32 + 1;
                frame.panel[idx] = panel_tag;
                let painted = if flagged && stripe {
                    style.hatch_color
                } else {
                    selected_fill.unwrap_or(color)
                };
                let o = idx * 4;
                frame.color[o] = painted[0];
                frame.color[o + 1] = painted[1];
                frame.color[o + 2] = painted[2];
                frame.color[o + 3] = 255;
            }
        }
    }
}

/// Draws one edge segment `width` pixels wide (`1` for an ordinary facet
/// boundary, `2` for a `selected`/`pending` highlight -- see `super::render::
/// draw_panel_edges`), depth-tested against the fill pass -- see
/// `raster::SolidRasterizer::draw_edge`'s doc comment for [`EDGE_DEPTH_BIAS`]'s
/// purpose, reused here at the same value.
const EDGE_DEPTH_BIAS: f32 = 5e-2;

/// Draws one depth-tested edge line into the diagram frame.
pub fn draw_edge(
    frame: &mut DiagramFrame,
    from: (f32, f32, f32),
    to: (f32, f32, f32),
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
    width: i32,
) {
    let (x0, y0, d0) = (from.0.round() as i32, from.1.round() as i32, from.2);
    let (x1, y1, d1) = (to.0.round() as i32, to.1.round() as i32, to.2);
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let steps_total = dx.max(-dy).max(1);

    let (mut cx, mut cy) = (x0, y0);
    let mut err = dx + dy;
    let mut step = 0;
    loop {
        let frac = step as f32 / steps_total as f32;
        let depth = (1.0 - frac).mul_add(d0, frac * d1);
        try_paint_edge_pixel(frame, cx, cy, depth, color, clip);
        // A broadened brush for the priority passes -- see `raster::
        // SolidRasterizer::draw_edge`'s matching comment.
        if width >= 2 {
            try_paint_edge_pixel(frame, cx + 1, cy, depth, color, clip);
            try_paint_edge_pixel(frame, cx, cy + 1, depth, color, clip);
        }
        if cx == x1 && cy == y1 {
            break;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
        step += 1;
    }
}

/// Paints one edge pixel at `(x, y)` with `color` when inside `clip` and `depth`
/// passes [`EDGE_DEPTH_BIAS`]'s slack against the fill pass -- the single-pixel
/// primitive [`draw_edge`]'s Bresenham walk (and its width-2 broadening) both go
/// through.
fn try_paint_edge_pixel(
    frame: &mut DiagramFrame,
    x: i32,
    y: i32,
    depth: f32,
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
) {
    if x < clip.0 || y < clip.1 || x > clip.2 || y > clip.3 {
        return;
    }
    let Some(idx) = frame.idx(x, y) else {
        return;
    };
    if depth <= frame.depth[idx] + EDGE_DEPTH_BIAS {
        let offset = idx * 4;
        frame.color[offset] = color[0];
        frame.color[offset + 1] = color[1];
        frame.color[offset + 2] = color[2];
        frame.color[offset + 3] = 255;
    }
}

/// Resolves [`DiagramStyle::meet_marker_pairs`] (facet-id pairs `Design::
/// facet_meets`' tier-level resolution names) to actual world-space points: the
/// vertex the two facets' mesh rings genuinely share, found by nearest-point
/// matching rather than trusting index alignment -- `facet_meets` only says
/// WHICH tiers meet, never where. A pair with no ring vertex closer than
/// `tolerance` contributes nothing (a generously over-listed candidate pair,
/// see `FacetMap::meeting_facet_pairs`'s own doc comment, not a real shared
/// vertex).
///
/// `tolerance` should scale with the mesh's own size -- `render_diagram` uses
/// a fraction of its bounding-box diagonal, mirroring `raster::simplify_ring`'s
/// own scale-relative merge tolerance.
pub fn meet_marker_points(mesh: &SolidMesh, pairs: &[(u32, u32)], tolerance: f64) -> Vec<DVec3> {
    if pairs.is_empty() {
        return Vec::new();
    }
    let ring_of = |facet_id: u32| -> Option<&Vec<DVec3>> {
        mesh.rings
            .iter()
            .find(|(id, _)| *id == facet_id as usize)
            .map(|(_, ring)| ring)
    };
    let mut points: Vec<DVec3> = Vec::new();
    for &(facet_a, facet_b) in pairs {
        let (Some(ring_a), Some(ring_b)) = (ring_of(facet_a), ring_of(facet_b)) else {
            continue;
        };
        for &pa in ring_a {
            for &pb in ring_b {
                if (pa - pb).length() <= tolerance {
                    // Dedup against points already found for an earlier pair --
                    // a shared vertex between more than two facets (a table/star
                    // apex, say) would otherwise get one marker per pair.
                    if !points.iter().any(|&p| (p - pa).length() <= tolerance) {
                        points.push(pa);
                    }
                }
            }
        }
    }
    points
}

/// Draws one meet-point marker: a small filled diamond, depth-tested against
/// `frame`'s fill pass with the same slack [`draw_edge`] uses, so a marker on
/// the far side of the stone from this panel's view stays hidden rather than
/// drawing through the solid.
pub fn draw_meet_marker(
    frame: &mut DiagramFrame,
    point: (f32, f32, f32),
    color: [u8; 3],
    clip: (i32, i32, i32, i32),
) {
    let (cx, cy, depth) = point;
    let (cx, cy) = (cx.round() as i32, cy.round() as i32);
    for dy in -2..=2i32 {
        for dx in -2..=2i32 {
            if dx.abs() + dy.abs() > 2 {
                continue; // Diamond, not a square.
            }
            try_paint_edge_pixel(frame, cx + dx, cy + dy, depth, color, clip);
        }
    }
}
