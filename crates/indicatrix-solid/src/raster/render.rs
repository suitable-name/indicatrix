//! The triangle/edge rasterizer itself: the two-pass (fill, then edges) render
//! pipeline, the affine-depth convex-polygon span fill, and the Bresenham edge
//! stroke -- see the parent module's doc comment for the camera/depth model.

use super::{
    CachedMesh, EDGE_DEPTH_BIAS, FillMode, HATCH_PERIOD, NEAR_EPS, SolidRasterizer, SolidStyle,
    shading::{blend_toward, shade},
    simplify_ring,
};
use glam::{DVec3, Vec3};
use indicatrix::{geometry::stone_metrics::SolidMesh, optics::raytracer::Camera};

impl SolidRasterizer {
    /// Renders `mesh` as seen by `camera`, styled by `style`, clearing every
    /// buffer first.
    ///
    /// Two passes over `mesh.rings`: back-face-culled, depth-tested, flat-shaded
    /// convex-polygon span fill of every facet (each ring through [`simplify_ring`]
    /// first), then a 1px edge pass along the same rings, run only after every
    /// facet is filled so an edge is depth-tested against the complete surface.
    ///
    /// No near-plane clipping: a facet with any ring point behind (or edge-on to)
    /// the camera is dropped whole. Invisible for the "whole stone in frame"
    /// viewing this preview targets, and keeps the hot loop free of a clip stage.
    pub fn render(&mut self, mesh: &SolidMesh, camera: &Camera, style: &SolidStyle) {
        self.clear(style.background);
        self.edge_points.clear();
        self.edge_ranges.clear();
        if self.width == 0 || self.height == 0 {
            return;
        }
        let (w, h) = (self.width as f32, self.height as f32);

        // One flat normal per facet (`normals[i]` belongs to facet `facet_id[i]`).
        let facet_count = mesh.rings.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
        let mut facet_normal: Vec<Option<DVec3>> = vec![None; facet_count];
        for (&id, &normal) in mesh.facet_id.iter().zip(&mesh.normals) {
            if let Some(slot) = facet_normal.get_mut(id)
                && slot.is_none()
            {
                *slot = Some(normal);
            }
        }

        let mut ring_scratch = std::mem::take(&mut self.ring_scratch);
        let mut dedup_scratch = std::mem::take(&mut self.dedup_scratch);
        let mut screen: Vec<(f32, f32, f32)> = Vec::new();
        let mut label_spots: Vec<(usize, f32, f32, f32, f32)> = Vec::new();
        for (facet_id, ring) in &mesh.rings {
            let Some(Some(normal)) = facet_normal.get(*facet_id).copied() else {
                continue;
            };
            let Some(&first) = ring.first() else {
                continue;
            };
            let normal = to_vec3(normal);

            // Back-face cull: a facet whose normal points away from the camera can
            // never be the nearest surface (the solid is watertight).
            let to_camera = camera.origin - to_vec3(first);
            if normal.dot(to_camera) <= 0.0 {
                continue;
            }

            simplify_ring(ring, &mut dedup_scratch, &mut ring_scratch);
            if ring_scratch.len() < 3 {
                continue;
            }
            screen.clear();
            let mut all_in_front = true;
            for &p in &ring_scratch {
                if let Some(sp) = project(camera, p, w, h) {
                    screen.push(sp);
                } else {
                    all_in_front = false;
                    break;
                }
            }
            if !all_in_front {
                continue;
            }

            // A preform (rough-bounding) facet either tints distinctly or,
            // when hidden, is culled here exactly like a back-face.
            let is_preform = *facet_id < style.preform_plane_count;
            if is_preform && !style.show_preform {
                continue;
            }
            let base_color = if *facet_id < style.facet_base_colors.len() {
                style.facet_base_colors[*facet_id]
            } else {
                style.base_color
            };
            let mut color = shade(normal, to_vec3(first), camera, style, base_color);
            if is_preform {
                color = blend_toward(color, style.preform_tint_color, 0.5);
            }
            self.fill_convex_polygon(&screen, *facet_id, color, style);
            self.edge_ranges
                .push((*facet_id, self.edge_points.len(), screen.len()));
            self.edge_points.extend_from_slice(&screen);
            let (min_x, max_x, min_y, max_y) = screen_bounds(&screen);
            label_spots.push((
                *facet_id,
                min_x.midpoint(max_x),
                min_y.midpoint(max_y),
                max_x - min_x,
                max_y - min_y,
            ));
        }
        self.ring_scratch = ring_scratch;
        self.dedup_scratch = dedup_scratch;

        let edge_ranges = std::mem::take(&mut self.edge_ranges);
        let edge_points = std::mem::take(&mut self.edge_points);
        self.draw_facet_edges(&edge_ranges, &edge_points, style);
        self.edge_ranges = edge_ranges;
        self.edge_points = edge_points;
        self.draw_facet_labels(&label_spots, style);
        if style.show_orientation_marker {
            self.draw_orientation_marker(mesh, camera);
        }
    }

    /// Same output as [`Self::render`], but sources already-[`simplify_ring`]d
    /// rings from a [`CachedMesh`] instead of re-simplifying every facet from
    /// scratch (see `mesh_cache`'s doc comment for why that cost mattered). Kept
    /// as its own method so `render`'s own tests stay untouched.
    pub fn render_prepared(&mut self, prepared: &CachedMesh, camera: &Camera, style: &SolidStyle) {
        self.clear(style.background);
        self.edge_points.clear();
        self.edge_ranges.clear();
        if self.width == 0 || self.height == 0 {
            return;
        }
        let (w, h) = (self.width as f32, self.height as f32);
        let mesh = &prepared.mesh;

        let facet_count = mesh.rings.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
        let mut facet_normal: Vec<Option<DVec3>> = vec![None; facet_count];
        for (&id, &normal) in mesh.facet_id.iter().zip(&mesh.normals) {
            if let Some(slot) = facet_normal.get_mut(id)
                && slot.is_none()
            {
                *slot = Some(normal);
            }
        }

        let mut screen: Vec<(f32, f32, f32)> = Vec::new();
        let mut label_spots: Vec<(usize, f32, f32, f32, f32)> = Vec::new();
        for (facet_id, ring) in &prepared.rings {
            let Some(Some(normal)) = facet_normal.get(*facet_id).copied() else {
                continue;
            };
            let Some(&first) = ring.first() else {
                continue;
            };
            let normal = to_vec3(normal);

            let to_camera = camera.origin - to_vec3(first);
            if normal.dot(to_camera) <= 0.0 {
                continue;
            }

            screen.clear();
            let mut all_in_front = true;
            for &p in ring {
                if let Some(sp) = project(camera, p, w, h) {
                    screen.push(sp);
                } else {
                    all_in_front = false;
                    break;
                }
            }
            if !all_in_front {
                continue;
            }

            let is_preform = *facet_id < style.preform_plane_count;
            if is_preform && !style.show_preform {
                continue;
            }
            let base_color = if *facet_id < style.facet_base_colors.len() {
                style.facet_base_colors[*facet_id]
            } else {
                style.base_color
            };
            let mut color = shade(normal, to_vec3(first), camera, style, base_color);
            if is_preform {
                color = blend_toward(color, style.preform_tint_color, 0.5);
            }
            self.fill_convex_polygon(&screen, *facet_id, color, style);
            self.edge_ranges
                .push((*facet_id, self.edge_points.len(), screen.len()));
            self.edge_points.extend_from_slice(&screen);
            let (min_x, max_x, min_y, max_y) = screen_bounds(&screen);
            label_spots.push((
                *facet_id,
                min_x.midpoint(max_x),
                min_y.midpoint(max_y),
                max_x - min_x,
                max_y - min_y,
            ));
        }

        let edge_ranges = std::mem::take(&mut self.edge_ranges);
        let edge_points = std::mem::take(&mut self.edge_points);
        self.draw_facet_edges(&edge_ranges, &edge_points, style);
        self.edge_ranges = edge_ranges;
        self.edge_points = edge_points;
        self.draw_facet_labels(&label_spots, style);
        if style.show_orientation_marker {
            self.draw_orientation_marker(mesh, camera);
        }
    }

    /// Depth-tested scanline span fill of one screen-space convex polygon (see the
    /// module doc comment for why `1 / a` is exact perspective-correct depth).
    /// `verts` are the `(screen_x, screen_y, a)` triples [`project`] produced for
    /// the facet's simplified ring, in order.
    ///
    /// The affine `1 / a` plane is fitted through the vertex triple with the
    /// largest screen area (best-conditioned for a slightly noisy corner). Each
    /// scanline's span is tested half-open on `y` so a vertex on a pixel row is
    /// counted once. Winding-agnostic (back faces already culled in world space).
    pub(super) fn fill_convex_polygon(
        &mut self,
        verts: &[(f32, f32, f32)],
        facet_id: usize,
        color: [u8; 3],
        style: &SolidStyle,
    ) {
        let n = verts.len();
        if n < 3 {
            return;
        }
        let (x0, y0, a0) = verts[0];
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
            return; // Degenerate (zero-area) polygon: nothing to fill.
        }
        let (x1, y1, a1) = verts[best.0];
        let (x2, y2, a2) = verts[best.1];
        let inv_area = 1.0 / best_area;
        let (inv_a0, inv_a1, inv_a2) = (1.0 / a0, 1.0 / a1, 1.0 / a2);
        // Barycentric weights are affine in screen position; gradients follow from
        // `edge_fn`'s definition.
        let grad_x =
            (y1 - y2).mul_add(inv_a0, (y2 - y0).mul_add(inv_a1, (y0 - y1) * inv_a2)) * inv_area;
        let grad_y =
            (x2 - x1).mul_add(inv_a0, (x0 - x2).mul_add(inv_a1, (x1 - x0) * inv_a2)) * inv_area;
        let plane_c = inv_a0 - grad_x.mul_add(x0, grad_y * y0);

        let (mut min_y, mut max_y) = (f32::INFINITY, f32::NEG_INFINITY);
        for &(_, y, _) in verts {
            min_y = min_y.min(y);
            max_y = max_y.max(y);
        }
        let row_start = min_y.floor().max(0.0) as i32;
        let row_end = max_y.ceil().min((self.height - 1) as f32) as i32;
        if row_start > row_end {
            return; // Polygon's screen bounds miss the frame entirely.
        }

        let flagged = style.flagged.get(facet_id).copied().unwrap_or(false);
        let selected = style.selected.get(facet_id).copied().unwrap_or(false);
        // A 58% blend toward `selected_color`, strong enough to read clearly
        // against an unselected facet even on a light-grey solid turned away
        // from the key light. Still leaves the facet's own lighting visible
        // underneath (unlike the flagged hatch, which fully replaces the
        // pixel), while reading clearly next to a hatched facet instead of
        // losing to it for attention.
        let selected_fill = selected.then(|| blend_toward(color, style.selected_color, 0.58));
        let alpha: u8 = if style.fill_mode == FillMode::Transparent {
            0
        } else {
            255
        };
        let max_px = (self.width - 1) as f32;

        for py in row_start..=row_end {
            let sy = py as f32 + 0.5;
            let (mut x_left, mut x_right) = (f32::INFINITY, f32::NEG_INFINITY);
            let mut crossings = 0u32;
            for (i, &(px_a, py_a, _)) in verts.iter().enumerate() {
                let (px_b, py_b, _) = verts[(i + 1) % n];
                if (py_a <= sy) == (py_b <= sy) {
                    continue; // Edge does not straddle this row (half-open).
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
            let col_start = (x_left - 0.5).ceil().max(0.0) as i32;
            let col_end = (x_right - 0.5).floor().min(max_px) as i32;
            if col_start > col_end {
                continue;
            }
            let row_base = (py as u32 * self.width) as usize;
            for px in col_start..=col_end {
                let sx = px as f32 + 0.5;
                let inv_a = grad_x.mul_add(sx, grad_y.mul_add(sy, plane_c));
                if inv_a <= 0.0 {
                    continue; // Behind the camera at this pixel -- skip.
                }
                let depth = 1.0 / inv_a;
                let idx = row_base + px as usize;
                if depth < self.depth[idx] {
                    self.depth[idx] = depth;
                    self.pick[idx] = facet_id as u32 + 1;
                    let stripe = (px + py).rem_euclid(HATCH_PERIOD * 2) < HATCH_PERIOD;
                    let painted = if flagged && stripe {
                        style.hatch_color
                    } else {
                        selected_fill.unwrap_or(color)
                    };
                    let o = idx * 4;
                    self.color[o] = painted[0];
                    self.color[o + 1] = painted[1];
                    self.color[o + 2] = painted[2];
                    self.color[o + 3] = alpha;
                }
            }
        }
    }

    /// Draws one edge segment from `a` to `b` (each a `(screen_x, screen_y,
    /// view_depth)` triple from [`project`]), `width` pixels wide (`1` for an
    /// ordinary facet boundary, `2` for a `selected`/`pending` highlight -- see
    /// [`Self::draw_facet_edges`]), depth-tested against the fill pass so an edge
    /// behind an already-painted facet never draws -- see [`EDGE_DEPTH_BIAS`] for
    /// the slack needed against the edge's own facet.
    pub(super) fn draw_edge(
        &mut self,
        from: (f32, f32, f32),
        to: (f32, f32, f32),
        color: [u8; 3],
        width: i32,
    ) {
        let (x0, y0, a0) = (from.0.round() as i32, from.1.round() as i32, from.2);
        let (x1, y1, a1) = (to.0.round() as i32, to.1.round() as i32, to.2);
        let dx = (x1 - x0).abs();
        let dy = -(y1 - y0).abs();
        let sx: i32 = if x0 < x1 { 1 } else { -1 };
        let sy: i32 = if y0 < y1 { 1 } else { -1 };
        let steps_total = dx.max(-dy).max(1);
        let (inv_a0, inv_a1) = (1.0 / a0, 1.0 / a1);

        let (mut cx, mut cy) = (x0, y0);
        let mut err = dx + dy;
        let mut step = 0;
        loop {
            let frac = step as f32 / steps_total as f32;
            let inv_a = (1.0 - frac).mul_add(inv_a0, frac * inv_a1);
            if inv_a > 0.0 {
                let depth = 1.0 / inv_a;
                self.try_paint_edge_pixel(cx, cy, depth, color);
                // A broadened brush for the priority passes: also paint the pixel
                // to the right and below, each depth-tested independently against
                // this same (1px-away-approximate) depth -- cheap and good enough
                // for a preview stroke, unlike a true perpendicular-offset line.
                if width >= 2 {
                    self.try_paint_edge_pixel(cx + 1, cy, depth, color);
                    self.try_paint_edge_pixel(cx, cy + 1, depth, color);
                }
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

    /// Paints one edge pixel at `(x, y)` with `color` when in bounds and `depth`
    /// passes [`EDGE_DEPTH_BIAS`]'s slack against the fill pass -- the single-pixel
    /// primitive [`Self::draw_edge`]'s Bresenham walk (and its width-2 broadening)
    /// both go through.
    pub(super) fn try_paint_edge_pixel(&mut self, x: i32, y: i32, depth: f32, color: [u8; 3]) {
        if x < 0 || y < 0 || (x as u32) >= self.width || (y as u32) >= self.height {
            return;
        }
        let idx = (y as u32 * self.width + x as u32) as usize;
        if depth <= self.depth[idx] + EDGE_DEPTH_BIAS {
            let offset = idx * 4;
            self.color[offset] = color[0];
            self.color[offset + 1] = color[1];
            self.color[offset + 2] = color[2];
            self.color[offset + 3] = 255;
        }
    }
}

/// Screen-space `(min_x, max_x, min_y, max_y)` bounds of a facet's projected
/// ring -- shared by [`SolidRasterizer::render`]'s and [`SolidRasterizer::
/// render_prepared`]'s label-spot collection.
fn screen_bounds(pts: &[(f32, f32, f32)]) -> (f32, f32, f32, f32) {
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

/// Narrows a mesh vertex's `f64` position/normal to the `f32` the camera works in.
pub(super) const fn to_vec3(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// The algebraic inverse of `Camera::generate_ray` -- see the module doc comment
/// for the derivation. Returns `None` when `p` is behind (or within
/// [`NEAR_EPS`] of) the camera plane.
pub(super) fn project(
    camera: &Camera,
    point: DVec3,
    width: f32,
    height: f32,
) -> Option<(f32, f32, f32)> {
    let delta = to_vec3(point) - camera.origin;
    let depth = delta.dot(camera.forward);
    if depth <= NEAR_EPS {
        return None;
    }
    let right_comp = delta.dot(camera.right);
    let up_comp = delta.dot(camera.up);
    let u_ratio = right_comp / depth;
    let v_ratio = up_comp / depth;
    let aspect = width / height;
    let screen_x = width * (0.5 + u_ratio / (2.0 * aspect * camera.fov_tan));
    let screen_y = height * (0.5 - v_ratio / (2.0 * camera.fov_tan));
    Some((screen_x, screen_y, depth))
}

/// Projects a 3D world-space point onto the screen using `camera`.
///
/// Returns `(screen_x, screen_y, depth)` where `depth` is view-space depth
/// (distance along the camera's view axis), or `None` when `point` is behind
/// (or within [`NEAR_EPS`] of) the camera plane, or if `width == 0` or `height == 0`.
#[must_use]
pub fn project_point(
    camera: &Camera,
    point: glam::Vec3,
    width: u32,
    height: u32,
) -> Option<(f32, f32, f32)> {
    if width == 0 || height == 0 {
        return None;
    }
    project(camera, point.as_dvec3(), width as f32, height as f32)
}

/// Twice the signed area of triangle `(ax,ay)-(bx,by)-(px,py)`: positive when `p`
/// is left of the directed edge `a -> b`. Used to pick the best-conditioned vertex
/// triple for the depth plane and derive its screen-space gradients.
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (by - ay).mul_add(-(px - ax), (bx - ax) * (py - ay))
}
