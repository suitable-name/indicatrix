//! CPU software rasterizer for a design's [`SolidMesh`].
//!
//! Pure Rust, no Slint types anywhere in this file -- [`super::to_pixel_buffer`]
//! is the only place that touches a Slint type, so [`SolidRasterizer`] stays
//! fully unit-testable and reusable if a `wgpu` backend ever replaces it.
//!
//! # Camera derivation
//!
//! The solid view must line up pixel-for-pixel with the path tracer, so this
//! module reuses [`indicatrix::optics::raytracer::Camera`] verbatim (the type every
//! viewport builds via `Camera::new(yaw, pitch, distance, 42.0)`) rather than a
//! locally redefined lookalike, so the two views can never drift apart.
//!
//! [`project`] is the exact algebraic inverse of `Camera::generate_ray`'s pixel ->
//! ray mapping. `generate_ray` builds, for pixel `(screen_x, screen_y)` in image
//! `(width, height)`:
//!
//! ```text
//! aspect = width / height
//! u = (screen_x / width - 0.5) * 2 * aspect * fov_tan
//! v = (0.5 - screen_y / height) * 2 * fov_tan
//! dir = forward + right * u + up * v        (before normalize; jitter = 0 here)
//! ```
//!
//! `forward`, `right`, `up` are an orthonormal basis, so with `d = P - origin`:
//!
//! ```text
//! a = d . forward     (view-space depth: distance along the view axis)
//! b = d . right
//! c = d . up
//! ```
//!
//! `P` lies on the ray through `(screen_x, screen_y)` exactly when `b / a = u` and
//! `c / a = v` (for `a > 0`; `a <= 0` means `P` is behind the camera, no pixel).
//! Solving for `screen_x`/`screen_y` gives [`project`]'s formula.
//!
//! `a` doubles as the depth-test value: monotonic along the view axis, and,
//! because `u = b / a`, `1 / a` is affine in screen space for a perspective
//! projection -- the quantity real pipelines interpolate for correct hyperbolic
//! depth. [`SolidRasterizer::fill_convex_polygon`] evaluates `1 / a` as an affine
//! function of screen position then inverts, exact for this pinhole model.
//!
//! # Why facet polygons, not fan triangles
//!
//! `SolidMesh::indices` triangulates every facet as a centroid fan, and a
//! bounding-box triangle fill tests roughly the whole facet's box once per
//! fan triangle -- an `n`-fold overhead for an `n`-gon. Worse, the mesh
//! builder does not merge near-coincident vertices from nearly-coplanar plane
//! triples: the 103-tier benchmark design yields 202 facets but 210 553 ring
//! points, and filling one fan triangle per ring point measured 59 ms/frame
//! on it. [`simplify_ring`] collapses each ring to its true corners first, so
//! the span fill costs one depth test per covered pixel regardless of ring size.

use super::{diagram2d, mesh_cache::CachedMesh};
use glam::{DVec3, Vec3};
use indicatrix::{geometry::stone_metrics::SolidMesh, optics::raytracer::Camera};

/// A world point projects to no pixel when its view-space depth (`a`) is at
/// or below this: at exactly zero the inverse projection divides by zero,
/// and a small positive slack keeps near-edge-on points from ever being
/// rasterized to a wild screen position.
const NEAR_EPS: f32 = 1e-4;

/// Screen-space period, in pixels, of the diagonal hatch stripes drawn over a
/// [`SolidStyle::flagged`] facet.
const HATCH_PERIOD: i32 = 6;

/// Minimum on-screen span (either axis), in pixels, before [`SolidRasterizer::
/// draw_facet_labels`] (#122) bothers stamping a facet's own label on it --
/// mirrors `diagram2d::MIN_LABEL_SPAN`'s own threshold and reasoning.
const MIN_LABEL_SPAN: f32 = 26.0;

/// Which of [`SolidRasterizer::draw_facet_edges`]'s ordered passes an edge segment
/// belongs to -- see that method's doc comment for why ordinary facets must be
/// drawn before any highlighted one, and later passes before earlier ones win a
/// shared edge. Ordered least to most "important to still be visible":
/// hover is the most transient overlay, `pending`/`selected_facet` the ones a
/// cutter most needs to keep seeing regardless of what else a facet is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgePass {
    /// None of the below: the plain `edge_color`, drawn first.
    Ordinary,
    /// The one facet named in `style.hovered` (#20).
    Hovered,
    /// A facet listed in `style.multi_selected` (#19), not otherwise highlighted.
    MultiSelected,
    /// `selected` (a whole tier) and not otherwise highlighted.
    Selected,
    /// The one facet named in `style.selected_facet` (#18) -- a stronger, more
    /// specific identification than the tier-level `Selected` pass.
    SelectedFacet,
    /// `pending`: drawn last, so it wins over every other pass too.
    Pending,
}

/// Depth slack (view-space units, same as [`project`]'s `a`) an edge pixel may be
/// behind the fill depth buffer and still draw. Needed because an edge point lies
/// exactly ON its own facet's surface, so without slack, rounding between the
/// fill and edge passes' depth would randomly fail an edge's own depth-test.
const EDGE_DEPTH_BIAS: f32 = 5e-2;

/// How [`SolidRasterizer::render`] writes a triangle's fill color.
///
/// `Opaque` for the ordinary Solid view, `Transparent` for the "Both" view (solid
/// edges over the path-traced image), where the fill must let the traced
/// frame underneath show through while the edge pass stays fully opaque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillMode {
    #[default]
    Opaque,
    Transparent,
}

/// Everything [`SolidRasterizer::render`] needs beyond the mesh and camera.
///
/// `flagged`/`pending`/`selected` are indexed by `facet_id`; a facet past the end
/// of a vector is treated as unset, so a caller may pass a shorter (or empty) one.
#[derive(Debug, Clone)]
pub struct SolidStyle {
    /// Unlit base color a facet is painted before Lambert/rim shading.
    pub base_color: [u8; 3],
    /// Ambient term so a facet turned fully away from the key light stays faintly visible.
    pub ambient: f32,
    /// Lambert diffuse coefficient (`ambient + diffuse * N.L`, clamped).
    pub diffuse: f32,
    /// Fresnel-style rim strength keeping near-edge-on facets from going unreadably dark.
    pub rim_strength: f32,
    /// World-space direction the key light shines FROM.
    pub key_light_dir: Vec3,
    /// 1px facet-boundary edge color (drawn over the fill).
    pub edge_color: [u8; 3],
    /// Edge color for a facet whose id is set in `pending` (re-solve missed budget).
    /// Matches `ui/theme.slint`'s `accent-amber` (`#f59e0b`) so the viewport agrees
    /// with any other amber "not yet resolved" indicator in the app.
    pub pending_color: [u8; 3],
    /// Hatch stripe color for a facet whose id is set in `flagged`
    /// (critical-angle-risk overlay).
    pub hatch_color: [u8; 3],
    /// Edge color for a facet whose id is set in `selected`
    /// (`facet_map::OverlayFlags::selected`); lower precedence than `pending_color`.
    /// Matches `ui/theme.slint`'s `primary` (`#3b82f6`), the same color the tier
    /// table paints its own selected row with (`editor_tier_table.slint`'s
    /// `primary-glow` background), so the two views read as one selection.
    pub selected_color: [u8; 3],
    /// Outline color for the single facet named in `hovered` (#20) -- a light
    /// neutral, distinct from every other overlay color so it never gets mistaken
    /// for a selection.
    pub hover_color: [u8; 3],
    /// Outline color for the single facet named in `selected_facet` (#18) --
    /// `ui/theme.slint`'s `accent-purple`, deliberately different from
    /// `selected_color` (a whole tier) so "this exact facet" and "this facet's
    /// tier" read as two different kinds of highlight.
    pub selected_facet_color: [u8; 3],
    /// Outline color for every facet id listed in `multi_selected` (#19) --
    /// `ui/theme.slint`'s `accent-cyan`, the same color `editor_tier_table.slint`
    /// already outlines a multi-selected row with.
    pub multi_selected_color: [u8; 3],
    /// Buffer-clear color (RGBA8) before each `render` call.
    pub background: [u8; 4],
    pub fill_mode: FillMode,
    pub flagged: Vec<bool>,
    pub pending: Vec<bool>,
    pub selected: Vec<bool>,
    /// The one facet under the cursor, or `None` -- set by
    /// `preview_state::SolidPreviewState::request_facet_overlay`, never by a
    /// fresh replan (a new plan always starts with no hover; the next mouse-move
    /// request re-establishes it). See #20.
    pub hovered: Option<u32>,
    /// The one facet a click identified within its (possibly multi-facet) tier
    /// selection, or `None` -- same update path as `hovered`. See #18.
    pub selected_facet: Option<u32>,
    /// Every facet id belonging to a multi-selected tier (table checkbox/ctrl-click
    /// selection) -- same update path as `hovered`. See #19. A `Vec` rather than a
    /// per-facet `bool` table since the caller already has the small set of ids
    /// straight from `facet_map::FacetMap::facets_of_tier`, and multi-select sizes
    /// are small enough that a linear scan per facet is not worth a second buffer.
    pub multi_selected: Vec<u32>,
    /// Facet id -> short on-facet label (`facet_map::FacetMap::facet_label`) --
    /// #122's "no facet labels in the 3D view" fix. Empty string (or a facet id
    /// past the end) draws nothing; a facet also needs a large enough on-screen
    /// span (see [`MIN_LABEL_SPAN`]) and must still win the depth test at its own
    /// centroid before its label is drawn (never labels an occluded facet).
    pub facet_labels: Vec<String>,
    /// Draws #119's orientation cue after the ordinary render pass: a tick
    /// toward world `+X` (the same "index 0" direction `diagram2d`'s index wheel
    /// uses) at the girdle plane, labeled "0", plus a CROWN/PAVILION caption --
    /// so a cutter who free-orbited the stone can tell orientation without
    /// switching to Diagram mode. Defaults to `true`.
    pub show_orientation_marker: bool,
    /// #122: facet ids below this are the rough's own bounding (preform)
    /// planes, in `Design::planes_from_solved`'s order (`facet_map::FacetMap::
    /// preform_plane_count`). `0` (the default) means "no preform to
    /// distinguish", matching every caller that never sets it.
    pub preform_plane_count: usize,
    /// #122: when `false`, a preform-plane facet is skipped entirely (culled,
    /// as if back-face-culled) instead of rendered, so a cutter can see through
    /// the rough's own bounding box to the actual cut. `true` (the default)
    /// keeps every preform facet visible, tinted by [`Self::preform_tint_color`]
    /// rather than drawn as an ordinary cut facet.
    pub show_preform: bool,
    /// Tint blended into a preform-plane facet's own shaded color (#122) so the
    /// rough's bounding planes read as visually distinct from an ordinary cut
    /// facet -- otherwise a preform plane looks and behaves (under hover/click)
    /// exactly like a real facet with nothing marking it as the uncut rough.
    pub preform_tint_color: [u8; 3],
}

impl Default for SolidStyle {
    fn default() -> Self {
        Self {
            base_color: [200, 205, 215],
            ambient: 0.25,
            diffuse: 0.75,
            rim_strength: 0.18,
            key_light_dir: Vec3::new(0.4, 0.8, 0.5).normalize(),
            edge_color: [30, 32, 38],
            pending_color: [245, 158, 11],
            hatch_color: [90, 40, 40],
            selected_color: [59, 130, 246],
            hover_color: [226, 232, 240],
            selected_facet_color: [168, 85, 247],
            multi_selected_color: [56, 189, 248],
            background: [0, 0, 0, 0],
            fill_mode: FillMode::Opaque,
            flagged: Vec::new(),
            pending: Vec::new(),
            selected: Vec::new(),
            hovered: None,
            selected_facet: None,
            multi_selected: Vec::new(),
            facet_labels: Vec::new(),
            show_orientation_marker: true,
            preform_plane_count: 0,
            show_preform: true,
            preform_tint_color: [80, 90, 140],
        }
    }
}

/// A flat-shaded, edge-outlined software render of a [`SolidMesh`], plus a
/// per-pixel facet-picking buffer. See the module doc comment for the camera model.
#[derive(Debug, Clone)]
pub struct SolidRasterizer {
    pub width: u32,
    pub height: u32,
    /// View-space depth (`a`) of the closest surface written to each pixel so far
    /// this frame; `f32::INFINITY` where unpainted. Row-major, `width * height`.
    pub depth: Vec<f32>,
    /// RGBA8 color buffer, row-major, four `u8`s per pixel.
    pub color: Vec<u8>,
    /// `facet_id + 1` per pixel, `0` meaning background/unpainted. See [`Self::pick_at`].
    pub pick: Vec<u32>,
    /// Scratch buffers reused per facet/frame so the hot loop never allocates.
    ring_scratch: Vec<DVec3>,
    dedup_scratch: Vec<DVec3>,
    /// Every visible facet's projected ring, concatenated, kept from the fill
    /// pass for the edge pass (see [`Self::render`]).
    edge_points: Vec<(f32, f32, f32)>,
    /// `(facet_id, first index into edge_points, point count)` per visible facet.
    edge_ranges: Vec<(usize, usize, usize)>,
}

impl SolidRasterizer {
    /// Allocates a rasterizer for a `width x height` frame. [`Self::render`] clears
    /// every buffer at the start of every call, so it can be reused frame to frame.
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        let n = (width as usize) * (height as usize);
        Self {
            width,
            height,
            depth: vec![f32::INFINITY; n],
            color: vec![0u8; n * 4],
            pick: vec![0u32; n],
            ring_scratch: Vec::new(),
            dedup_scratch: Vec::new(),
            edge_points: Vec::new(),
            edge_ranges: Vec::new(),
        }
    }

    /// Re-allocates every buffer for a new `width x height`, discarding the previous
    /// frame's contents. A no-op when the size is unchanged.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == self.width && height == self.height {
            return;
        }
        *self = Self::new(width, height);
    }

    /// Returns the `facet_id` painted at pixel `(x, y)`, or `None` for
    /// background/out-of-bounds.
    #[must_use]
    pub fn pick_at(&self, x: u32, y: u32) -> Option<u32> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let v = self.pick[(y * self.width + x) as usize];
        (v != 0).then(|| v - 1)
    }

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

            // #122: a preform (rough-bounding) facet either tints distinctly or,
            // when hidden, is culled here exactly like a back-face.
            let is_preform = *facet_id < style.preform_plane_count;
            if is_preform && !style.show_preform {
                continue;
            }
            let mut color = shade(normal, to_vec3(first), camera, style);
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
            let mut color = shade(normal, to_vec3(first), camera, style);
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

    /// Draws every visible facet's boundary in [`EdgePass`]'s ordered passes --
    /// ordinary facets first, then hover/multi-select/tier-select/single-facet-
    /// select/pending, each pass only overpainting a facet already drawn by an
    /// earlier one -- so a shared edge between an ordinary facet and a highlighted
    /// one always ends up in the highlighted color, never the other way around.
    /// Before this, edges were drawn once per facet in mesh-ring order, so
    /// whichever of a shared edge's two owning facets happened to be visited LAST
    /// silently overpainted the other's color (`draw_edge`'s depth test admits a
    /// coincident edge from either side) -- on roughly half of a highlighted
    /// facet's boundary the neighbour's plain dark edge would win, making the
    /// highlight look like a rendering glitch rather than a selection. Every
    /// highlighted pass also draws with a wider stroke (see [`EdgePass`]'s match
    /// arms below) so it reads clearly even where it doesn't win the fight.
    fn draw_facet_edges(
        &mut self,
        edge_ranges: &[(usize, usize, usize)],
        edge_points: &[(f32, f32, f32)],
        style: &SolidStyle,
    ) {
        let is_selected = |facet_id: usize| style.selected.get(facet_id).copied().unwrap_or(false);
        let is_pending = |facet_id: usize| style.pending.get(facet_id).copied().unwrap_or(false);
        let is_hovered = |facet_id: usize| style.hovered == Some(facet_id as u32);
        let is_selected_facet = |facet_id: usize| style.selected_facet == Some(facet_id as u32);
        let is_multi_selected = |facet_id: usize| style.multi_selected.contains(&(facet_id as u32));

        for pass in [
            EdgePass::Ordinary,
            EdgePass::Hovered,
            EdgePass::MultiSelected,
            EdgePass::Selected,
            EdgePass::SelectedFacet,
            EdgePass::Pending,
        ] {
            for &(facet_id, start, count) in edge_ranges {
                let selected = is_selected(facet_id);
                let pending = is_pending(facet_id);
                let hovered = is_hovered(facet_id);
                let selected_facet = is_selected_facet(facet_id);
                let multi_selected = is_multi_selected(facet_id);
                let highlighted = selected || pending || hovered || selected_facet;
                let (draw_this_pass, color, width) = match pass {
                    EdgePass::Ordinary => (!highlighted && !multi_selected, style.edge_color, 1),
                    EdgePass::Hovered => {
                        (hovered && !pending && !selected_facet, style.hover_color, 2)
                    }
                    EdgePass::MultiSelected => (
                        multi_selected && !selected && !pending && !selected_facet,
                        style.multi_selected_color,
                        2,
                    ),
                    EdgePass::Selected => (
                        selected && !pending && !selected_facet,
                        style.selected_color,
                        2,
                    ),
                    EdgePass::SelectedFacet => {
                        (selected_facet && !pending, style.selected_facet_color, 3)
                    }
                    EdgePass::Pending => (pending, style.pending_color, 2),
                };
                if !draw_this_pass {
                    continue;
                }
                let pts = &edge_points[start..start + count];
                for (i, &a) in pts.iter().enumerate() {
                    let b = pts[(i + 1) % count];
                    self.draw_edge(a, b, color, width);
                }
            }
        }
    }

    /// Resets every buffer to background/unpainted.
    fn clear(&mut self, background: [u8; 4]) {
        self.depth.fill(f32::INFINITY);
        self.pick.fill(0);
        let (pixels, _remainder) = self.color.as_chunks_mut::<4>();
        for pixel in pixels {
            pixel.copy_from_slice(&background);
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
    fn fill_convex_polygon(
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
        // A 58% blend toward `selected_color` -- raised from an earlier 35%, which
        // read as barely distinguishable from an unselected facet on a light-grey
        // solid, especially one turned away from the key light. Still leaves the
        // facet's own lighting visible underneath (unlike the flagged hatch, which
        // fully replaces the pixel), while now reading clearly next to a hatched
        // facet instead of losing to it for attention.
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
    fn draw_edge(
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
    fn try_paint_edge_pixel(&mut self, x: i32, y: i32, depth: f32, color: [u8; 3]) {
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

    /// #122: stamps each visible facet's own label (`style.facet_labels`,
    /// `facet_map::FacetMap::facet_label`'s output) at its screen centroid, but
    /// only where the facet is big enough to read ([`MIN_LABEL_SPAN`]) and still
    /// wins the depth test at its own centroid -- the same two guards
    /// `diagram2d::draw_panel_labels` uses, so a facet occluded by a nearer one
    /// never gets a floating label drawn on top of whatever DID win that pixel.
    fn draw_facet_labels(
        &mut self,
        label_spots: &[(usize, f32, f32, f32, f32)],
        style: &SolidStyle,
    ) {
        for &(facet_id, cx, cy, w, h) in label_spots {
            if w < MIN_LABEL_SPAN || h < MIN_LABEL_SPAN {
                continue;
            }
            let Some(label) = style.facet_labels.get(facet_id) else {
                continue;
            };
            if label.is_empty() {
                continue;
            }
            let (px, py) = (cx.round() as i32, cy.round() as i32);
            if px < 0 || py < 0 || px as u32 >= self.width || py as u32 >= self.height {
                continue;
            }
            let idx = (py as u32 * self.width + px as u32) as usize;
            if self.pick[idx] != facet_id as u32 + 1 {
                continue; // Occluded at its own centroid -- another facet won here.
            }
            let (label_w, label_h) = diagram2d::text_size(label, 1);
            let (width, height) = (self.width, self.height);
            diagram2d::draw_text_into_buffer(
                diagram2d::TextCanvas {
                    color_buf: &mut self.color,
                    width,
                    height,
                },
                cx - label_w as f32 / 2.0,
                cy - label_h as f32 / 2.0,
                label,
                1,
                style.edge_color,
                None,
            );
        }
    }

    /// Draws #119's orientation cue directly into the finished color buffer,
    /// after every facet/edge -- an overlay, not a lit surface, so it is never
    /// depth-tested against the mesh (it would otherwise vanish behind whatever
    /// facet happens to be nearest at that screen point). See
    /// [`SolidStyle::show_orientation_marker`]'s own doc comment for what it
    /// draws and why.
    fn draw_orientation_marker(&mut self, mesh: &SolidMesh, camera: &Camera) {
        if self.width == 0 || self.height == 0 {
            return;
        }
        let (w, h) = (self.width as f32, self.height as f32);
        // The same world radius `diagram2d::mesh_radii` would give this mesh's
        // horizontal extent -- a plain re-derivation here rather than a shared
        // helper, since this module intentionally carries no dependency on
        // `diagram2d`'s panel-layout types (only its free-standing text helpers).
        let radius = mesh
            .positions
            .iter()
            .fold(1e-3_f64, |acc, p| acc.max(p.x.hypot(p.z)));
        let marker_color = [240u8, 240, 245];
        let origin = DVec3::new(0.0, 0.0, 0.0);
        let tip = DVec3::new(radius * 1.25, 0.0, 0.0);
        let (width, height) = (self.width, self.height);
        if let (Some(from), Some(to)) = (project(camera, origin, w, h), project(camera, tip, w, h))
        {
            self.draw_edge(from, to, marker_color, 2);
            let (label_w, _) = diagram2d::text_size("0", 1);
            diagram2d::draw_text_into_buffer(
                diagram2d::TextCanvas {
                    color_buf: &mut self.color,
                    width,
                    height,
                },
                to.0 - label_w as f32 / 2.0,
                to.1 - 10.0,
                "0",
                1,
                marker_color,
                None,
            );
        }
        // The camera looks FROM `camera.origin` at the stone centered on the
        // world origin -- a positive `origin.y` means the camera sits above the
        // girdle plane (`y = 0`, the same crown/pavilion split every facet
        // normal already uses, see `facet_map.rs`'s candidate-normal
        // construction), i.e. the crown is the side more toward the camera.
        let side_label = if camera.origin.y >= 0.0 {
            "CROWN"
        } else {
            "PAVILION"
        };
        diagram2d::draw_text_into_buffer(
            diagram2d::TextCanvas {
                color_buf: &mut self.color,
                width,
                height,
            },
            8.0,
            8.0,
            side_label,
            1,
            marker_color,
            None,
        );
    }
}

/// Screen-space `(min_x, max_x, min_y, max_y)` bounds of a facet's projected
/// ring -- shared by [`SolidRasterizer::render`]'s and [`SolidRasterizer::
/// render_prepared`]'s label-spot collection (#122).
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

/// Collapses one facet ring to its true corners: consecutive points closer than
/// `1e-7` of the ring's extent are merged, then every point collinear (within a
/// `1e-6` sine tolerance) with its kept predecessor and successor is dropped. One
/// pass suffices since `SolidMesh` rings are ordered around the facet boundary;
/// result lands in `out` (`dedup` is the intermediate buffer, both reused across
/// facets). A convex facet polygon stays convex under this.
pub(super) fn simplify_ring(ring: &[DVec3], dedup: &mut Vec<DVec3>, out: &mut Vec<DVec3>) {
    const SIN_TOL: f64 = 1e-6;
    dedup.clear();
    out.clear();
    let Some(&first) = ring.first() else {
        return;
    };
    let (mut lo, mut hi) = (first, first);
    for &p in ring {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    let merge_eps = (hi - lo).length().max(1e-12) * 1e-7;

    for &p in ring {
        if dedup
            .last()
            .is_some_and(|&last| (p - last).length() <= merge_eps)
        {
            continue;
        }
        dedup.push(p);
    }
    while dedup.len() > 1 && (dedup[0] - dedup[dedup.len() - 1]).length() <= merge_eps {
        dedup.pop();
    }
    if dedup.len() < 3 {
        out.extend_from_slice(dedup);
        return;
    }

    let n = dedup.len();
    let mut prev = dedup[n - 1];
    for i in 0..n {
        let cur = dedup[i];
        let next = dedup[(i + 1) % n];
        let e1 = cur - prev;
        let e2 = next - cur;
        let collinear = e1.cross(e2).length() <= SIN_TOL * e1.length() * e2.length();
        if !collinear {
            out.push(cur);
            prev = cur;
        }
    }
}

/// Narrows a mesh vertex's `f64` position/normal to the `f32` the camera works in.
const fn to_vec3(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}

/// The algebraic inverse of `Camera::generate_ray` -- see the module doc comment
/// for the derivation. Returns `None` when `p` is behind (or within
/// [`NEAR_EPS`] of) the camera plane.
fn project(camera: &Camera, point: DVec3, width: f32, height: f32) -> Option<(f32, f32, f32)> {
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

/// Twice the signed area of triangle `(ax,ay)-(bx,by)-(px,py)`: positive when `p`
/// is left of the directed edge `a -> b`. Used to pick the best-conditioned vertex
/// triple for the depth plane and derive its screen-space gradients.
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (by - ay).mul_add(-(px - ax), (bx - ax) * (py - ay))
}

/// Linearly blends `base` toward `target` by `amount` (`0.0` = `base` unchanged,
/// `1.0` = `target` exactly) -- the selected-facet tint.
fn blend_toward(base: [u8; 3], target: [u8; 3], amount: f32) -> [u8; 3] {
    std::array::from_fn(|i| {
        let b = f32::from(base[i]);
        let t = f32::from(target[i]);
        amount.mul_add(t - b, b).round().clamp(0.0, 255.0) as u8
    })
}

/// Flat Lambert shading from a fixed key light, plus a small Fresnel-style rim
/// term so near-edge-on facets (`N . V` close to zero) stay distinguishable from
/// background instead of going nearly black. Computed once per triangle since
/// facet normals are flat.
fn shade(normal: Vec3, world_point: Vec3, camera: &Camera, style: &SolidStyle) -> [u8; 3] {
    let n = normal.normalize();
    let light_dir = style.key_light_dir.normalize();
    let view_dir = (camera.origin - world_point).normalize();
    let n_dot_l = n.dot(light_dir).max(0.0);
    let n_dot_v = n.dot(view_dir).max(0.0);
    let rim = (1.0 - n_dot_v).powi(2) * style.rim_strength;
    let intensity = style
        .diffuse
        .mul_add(n_dot_l, style.ambient + rim)
        .clamp(0.0, 1.0);
    [
        (f32::from(style.base_color[0]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8,
        (f32::from(style.base_color[1]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8,
        (f32::from(style.base_color[2]) * intensity)
            .round()
            .clamp(0.0, 255.0) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::{
        cuts::StandardGemCuts,
        meet_solver,
        plane::GpuFacetPlane,
        stone_metrics::{SolidStatus, build_solid_mesh},
    };

    /// The `stone_metrics.rs` "plain box" fixture: `x,z in [-1,1]`, `y in [-0.6,0.6]`,
    /// six axis-aligned facets, indices `0..=5` (`+X, -X, +Y, -Y, +Z, -Z`).
    fn unit_box_mesh() -> SolidMesh {
        let planes = vec![
            (DVec3::X, 1.0),
            (DVec3::NEG_X, 1.0),
            (DVec3::Y, 0.6),
            (DVec3::NEG_Y, 0.6),
            (DVec3::Z, 1.0),
            (DVec3::NEG_Z, 1.0),
        ];
        match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("box fixture must close: {other:?}"),
        }
    }

    #[test]
    fn unit_cube_renders_expected_coverage_and_depth_ordering() {
        // Camera on +Z looking down -Z: only the +Z facet (index 4) can be nearest.
        let mesh = unit_box_mesh();
        let camera = Camera::new(0.0, 0.0, 5.0, 42.0);
        let style = SolidStyle::default();
        let mut rasterizer = SolidRasterizer::new(64, 64);
        rasterizer.render(&mesh, &camera, &style);

        assert_eq!(
            rasterizer.pick_at(32, 32),
            Some(4),
            "screen center must show the +Z facet"
        );
        assert_eq!(
            rasterizer.pick_at(0, 0),
            None,
            "image corner must be outside the box's silhouette"
        );

        // The box is convex, so back-face culling + depth test only ever leave the
        // facet directly facing the camera -- never the opposite (-Z) facet.
        let mut painted = 0;
        for &p in &rasterizer.pick {
            if p != 0 {
                painted += 1;
                assert_eq!(p, 5, "every painted pixel must be the +Z facet (id 4)");
            }
        }
        assert!(
            painted > 100,
            "expected a real silhouette, got {painted} px"
        );
    }

    /// Two axis-facing quads at different depths (0 nearer/smaller, 1 farther/bigger),
    /// built directly so the exact expected pixels are known.
    fn two_quads_mesh() -> SolidMesh {
        let mut mesh = SolidMesh::default();
        for (facet_id, z, half_extent) in [(0usize, 0.0f64, 0.5f64), (1usize, -2.0f64, 1.0f64)] {
            let normal = DVec3::new(0.0, 0.0, 1.0);
            let corners = [
                DVec3::new(-half_extent, -half_extent, z),
                DVec3::new(half_extent, -half_extent, z),
                DVec3::new(half_extent, half_extent, z),
                DVec3::new(-half_extent, half_extent, z),
            ];
            let base = mesh.positions.len() as u32;
            for c in corners {
                mesh.positions.push(c);
                mesh.normals.push(normal);
                mesh.facet_id.push(facet_id);
            }
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
            mesh.rings.push((facet_id, corners.to_vec()));
        }
        mesh
    }

    /// Camera at `(0,0,5)` looking down `-Z`. Near quad's right edge projects to
    /// `screen_x ~= 40.3`, far quad's to `~= 43.9`: `32` sees both (near wins),
    /// `42` sees only the far quad, `60` sees neither.
    fn two_quads_camera() -> Camera {
        Camera::new(0.0, 0.0, 5.0, 42.0)
    }

    #[test]
    fn a_facet_hidden_behind_another_never_paints() {
        let mesh = two_quads_mesh();
        let camera = two_quads_camera();
        let style = SolidStyle::default();
        let mut rasterizer = SolidRasterizer::new(64, 64);
        rasterizer.render(&mesh, &camera, &style);

        // Where both quads cover the same pixel, the nearer one (facet 0) must win.
        assert_eq!(rasterizer.pick_at(32, 32), Some(0));
    }

    #[test]
    fn pick_buffer_returns_the_right_facet_id_at_known_pixels() {
        let mesh = two_quads_mesh();
        let camera = two_quads_camera();
        let style = SolidStyle::default();
        let mut rasterizer = SolidRasterizer::new(64, 64);
        rasterizer.render(&mesh, &camera, &style);

        assert_eq!(
            rasterizer.pick_at(32, 32),
            Some(0),
            "center: covered by both, near facet (0) wins"
        );
        assert_eq!(
            rasterizer.pick_at(42, 32),
            Some(1),
            "just outside the near quad's footprint, inside the far quad's"
        );
        assert_eq!(
            rasterizer.pick_at(60, 32),
            None,
            "outside both quads' footprints"
        );
    }

    #[test]
    fn flagged_facets_are_visibly_hatched() {
        let mesh = unit_box_mesh();
        let mut flagged = vec![false; 6];
        flagged[4] = true; // +Z facet, the only one visible here
        let hatched_style = SolidStyle {
            flagged,
            ..SolidStyle::default()
        };

        let mut plain = SolidRasterizer::new(64, 64);
        plain.render(
            &mesh,
            &Camera::new(0.0, 0.0, 5.0, 42.0),
            &SolidStyle::default(),
        );
        let mut hatched = SolidRasterizer::new(64, 64);
        hatched.render(&mesh, &Camera::new(0.0, 0.0, 5.0, 42.0), &hatched_style);

        assert_ne!(
            plain.color, hatched.color,
            "hatching a visible facet must change at least one pixel"
        );
    }

    #[test]
    fn selected_facets_get_a_visible_tint_and_the_selected_edge_color() {
        let mesh = unit_box_mesh();
        let mut selected = vec![false; 6];
        selected[4] = true; // +Z facet, the only one visible here
        let style = SolidStyle {
            selected,
            selected_color: [0, 0, 255],
            edge_color: [10, 10, 10],
            ..SolidStyle::default()
        };

        let mut plain = SolidRasterizer::new(64, 64);
        plain.render(
            &mesh,
            &Camera::new(0.0, 0.0, 5.0, 42.0),
            &SolidStyle::default(),
        );
        let mut tinted = SolidRasterizer::new(64, 64);
        tinted.render(&mesh, &Camera::new(0.0, 0.0, 5.0, 42.0), &style);

        assert_ne!(
            plain.color, tinted.color,
            "tinting a selected facet must change at least one pixel"
        );
        let found_selected_edge = tinted
            .color
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[0] == 0 && px[1] == 0 && px[2] == 255);
        assert!(
            found_selected_edge,
            "expected the selected edge color somewhere on screen"
        );
    }

    #[test]
    fn pending_facets_get_the_pending_edge_color() {
        let mesh = unit_box_mesh();
        let mut pending = vec![false; 6];
        pending[4] = true;
        let style = SolidStyle {
            pending,
            pending_color: [255, 0, 0],
            edge_color: [10, 10, 10],
            ..SolidStyle::default()
        };
        let mut rasterizer = SolidRasterizer::new(64, 64);
        rasterizer.render(&mesh, &Camera::new(0.0, 0.0, 5.0, 42.0), &style);

        // The box's silhouette edge must be drawn in the pending color somewhere.
        let found_pending_edge = rasterizer
            .color
            .as_chunks::<4>()
            .0
            .iter()
            .any(|px| px[0] == 255 && px[1] == 0 && px[2] == 0);
        assert!(
            found_pending_edge,
            "expected the pending edge color somewhere on screen"
        );
    }

    #[test]
    fn preform_facets_are_tinted_and_can_be_hidden() {
        // #122: facet 4 (+Z, the only one visible from this camera) counts as a
        // preform plane under this style (`preform_plane_count: 5`).
        let mesh = unit_box_mesh();
        let camera = Camera::new(0.0, 0.0, 5.0, 42.0);

        let mut plain = SolidRasterizer::new(64, 64);
        plain.render(
            &mesh,
            &camera,
            &SolidStyle {
                show_orientation_marker: false,
                ..SolidStyle::default()
            },
        );

        let mut tinted = SolidRasterizer::new(64, 64);
        tinted.render(
            &mesh,
            &camera,
            &SolidStyle {
                preform_plane_count: 5,
                show_orientation_marker: false,
                ..SolidStyle::default()
            },
        );
        assert_eq!(
            tinted.pick_at(32, 32),
            Some(4),
            "a shown preform facet must still render and still be pickable"
        );
        assert_ne!(
            plain.color, tinted.color,
            "a tinted preform facet must look different from an ordinary one"
        );

        let mut hidden = SolidRasterizer::new(64, 64);
        hidden.render(
            &mesh,
            &camera,
            &SolidStyle {
                preform_plane_count: 5,
                show_preform: false,
                show_orientation_marker: false,
                ..SolidStyle::default()
            },
        );
        assert_eq!(
            hidden.pick_at(32, 32),
            None,
            "a hidden preform facet must be culled entirely, like a back-face"
        );
    }

    #[test]
    fn a_big_enough_facet_label_changes_the_rendered_pixels() {
        let mesh = unit_box_mesh();
        // A 128 px frame and a closer camera on purpose: `draw_facet_labels` skips
        // any facet whose projected span is under `MIN_LABEL_SPAN` (26 px), and in a
        // 64x64 frame at distance 5 this box lands under it, so the label would be
        // skipped by design and the assertion below would prove nothing.
        let camera = Camera::new(0.0, 0.0, 4.0, 42.0);
        let mut plain = SolidRasterizer::new(128, 128);
        plain.render(
            &mesh,
            &camera,
            &SolidStyle {
                show_orientation_marker: false,
                ..SolidStyle::default()
            },
        );
        // Label whichever facet actually faces the camera at the frame centre. A
        // hard-coded id is a trap here: on a box at this pose the top face is
        // edge-on, its projected span falls below `MIN_LABEL_SPAN`, and
        // `draw_facet_labels` skips it by design -- so the test would fail for a
        // reason that has nothing to do with labelling.
        let visible = plain
            .pick_at(64, 64)
            .expect("the box must paint the frame centre") as usize;
        let mut labels = vec![String::new(); 6];
        labels[visible] = "T".to_string();

        let mut labeled = SolidRasterizer::new(128, 128);
        labeled.render(
            &mesh,
            &camera,
            &SolidStyle {
                facet_labels: labels,
                show_orientation_marker: false,
                ..SolidStyle::default()
            },
        );

        assert_ne!(
            plain.color, labeled.color,
            "a facet label on a facet this large (the box fills most of a 64x64 \
             frame) must change at least one pixel"
        );
    }

    #[test]
    fn orientation_marker_draws_a_crown_pavilion_label_and_can_be_turned_off() {
        let mesh = unit_box_mesh();
        // Pitch above the pole: the camera sits above the girdle plane, looking
        // down onto the crown side.
        let camera = Camera::new(0.0, 0.6, 5.0, 42.0);

        let mut with_marker = SolidRasterizer::new(64, 64);
        with_marker.render(&mesh, &camera, &SolidStyle::default());
        let mut without_marker = SolidRasterizer::new(64, 64);
        without_marker.render(
            &mesh,
            &camera,
            &SolidStyle {
                show_orientation_marker: false,
                ..SolidStyle::default()
            },
        );

        assert_ne!(
            with_marker.color, without_marker.color,
            "the orientation marker must actually draw something when enabled"
        );
    }

    /// Mirrors `stone_metrics.rs`'s `planes_from_asc_schedule` helper: converts
    /// `GpuFacetPlane`'s `n . x + d <= 0` convention to the `n . x <= m` pairs
    /// `build_solid_mesh` takes.
    fn rbc_planes() -> Vec<(DVec3, f64)> {
        StandardGemCuts::standard_round_brilliant()
            .into_iter()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect()
    }

    /// The 103-tier "CrackOtto-Step" design (`indicatrix-cut-core`'s benchmark
    /// fixture, PC 05.115), embedded via `include_str!` so there is exactly one
    /// copy in the workspace. Every tier is genuinely meet-derived, so one
    /// `ScaleReference` bootstrap per block (crown/pavilion/girdle) is added here,
    /// reimplemented over `indicatrix::geometry::meet_solver` so this crate's test
    /// does not need to depend on `indicatrix-cut-core`.
    fn crackotto_step_planes() -> Vec<(DVec3, f64)> {
        const TEXT: &str = include_str!(
            "../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
        );
        let schedule = indicatrix_formats::asc::parse_asc(TEXT).expect("fixture must parse");
        let mut inputs = meet_solver::meet_tier_inputs_from_asc(&schedule);
        let blocks = meet_solver::classify_blocks(&inputs);
        for block in [
            meet_solver::Block::Crown,
            meet_solver::Block::Pavilion,
            meet_solver::Block::Girdle,
        ] {
            let anchored = inputs.iter().zip(&blocks).any(|(t, &b)| {
                b == block && matches!(t.constraint, meet_solver::MeetConstraint::ScaleReference(_))
            });
            if anchored {
                continue;
            }
            if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
                inputs[i].constraint =
                    meet_solver::MeetConstraint::ScaleReference(schedule.tiers[i].mast);
            }
        }
        let normals = meet_solver::tier_instance_normals(schedule.gear_teeth_abs(), &inputs);
        let solved = meet_solver::solve_meet_points(schedule.gear_teeth_abs(), &inputs);
        normals
            .iter()
            .zip(solved.iter().map(|s| s.mast))
            .flat_map(|(ns, m)| ns.iter().map(move |&n| (n, m)))
            .collect()
    }

    #[test]
    #[ignore = "writes a PNG to the system temp dir -- a visual smoke test, not a \
                correctness check; run explicitly with --ignored"]
    fn renders_the_standard_round_brilliant_without_panicking() {
        let planes = rbc_planes();
        let mesh = match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("RBC-445 must close: {other:?}"),
        };
        let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
        let style = SolidStyle::default();
        let mut rasterizer = SolidRasterizer::new(800, 600);
        rasterizer.render(&mesh, &camera, &style);

        let visible = rasterizer.pick.iter().filter(|&&p| p != 0).count();
        assert!(
            visible > 1000,
            "expected a substantial visible silhouette, got {visible} px"
        );

        let image = image::RgbaImage::from_raw(800, 600, rasterizer.color.clone())
            .expect("color buffer length must match 800x600 RGBA8");
        let path = std::env::temp_dir().join("indicatrix_cut_solid_preview_rbc.png");
        image
            .save(&path)
            .expect("PNG write to the temp dir must succeed");
        println!("wrote {}", path.display());
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture"]
    fn timing_rbc_445_800x600() {
        let planes = rbc_planes();
        let mesh = match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("RBC-445 must close: {other:?}"),
        };
        let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
        let style = SolidStyle::default();
        let mut rasterizer = SolidRasterizer::new(800, 600);
        rasterizer.render(&mesh, &camera, &style); // warm-up

        let iters = 200u32;
        let start = std::time::Instant::now();
        for _ in 0..iters {
            rasterizer.render(&mesh, &camera, &style);
        }
        let per_frame = start.elapsed() / iters;
        println!(
            "RBC-445 800x600: {per_frame:?} per frame over {iters} iterations (target < 1 ms)"
        );
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture"]
    fn timing_crackotto_step_103_tier_800x600() {
        let planes = crackotto_step_planes();
        let mesh = match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("CrackOtto-Step must close: {other:?}"),
        };
        let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
        let style = SolidStyle::default();
        let mut rasterizer = SolidRasterizer::new(800, 600);
        rasterizer.render(&mesh, &camera, &style); // warm-up

        let iters = 100u32;
        let start = std::time::Instant::now();
        for _ in 0..iters {
            rasterizer.render(&mesh, &camera, &style);
        }
        let per_frame = start.elapsed() / iters;
        println!(
            "CrackOtto-Step (103 tiers) 800x600: {per_frame:?} per frame over {iters} \
             iterations (target < 5 ms)"
        );
    }
}
