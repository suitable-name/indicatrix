//! CPU software rasterizer for a design's [`SolidMesh`].
//!
//! Pure Rust, no GUI toolkit types anywhere in this crate -- the desktop's
//! `gui::solid_preview::to_pixel_buffer` (`apps/indicatrix-cut`, a different crate)
//! is the only place that touches a `slint::SharedPixelBuffer`, so
//! [`SolidRasterizer`] stays fully unit-testable and reusable if a `wgpu` backend
//! ever replaces it.
//!
//! # Camera derivation
//!
//! The solid view must line up pixel-for-pixel with the path tracer, so this
//! module reuses [`indicatrix::optics::raytracer::Camera`] verbatim (the type every
//! viewport builds via `Camera::new(yaw, pitch, distance, 42.0)`) rather than a
//! locally redefined lookalike, so the two views can never drift apart.
//!
//! [`render::project`] is the exact algebraic inverse of `Camera::generate_ray`'s pixel ->
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
//! Solving for `screen_x`/`screen_y` gives [`render::project`]'s formula.
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

mod overlays;
mod render;
mod shading;
#[cfg(test)]
mod tests;

pub use render::project_point;

/// A world point projects to no pixel when its view-space depth (`a`) is at
/// or below this: at exactly zero the inverse projection divides by zero,
/// and a small positive slack keeps near-edge-on points from ever being
/// rasterized to a wild screen position.
const NEAR_EPS: f32 = 1e-4;

/// Screen-space period, in pixels, of the diagonal hatch stripes drawn over a
/// [`SolidStyle::flagged`] facet.
const HATCH_PERIOD: i32 = 6;

/// Minimum on-screen span (either axis), in pixels, before [`SolidRasterizer::
/// draw_facet_labels`] bothers stamping a facet's own label on it -- mirrors
/// `diagram2d::MIN_LABEL_SPAN`'s own threshold and reasoning.
const MIN_LABEL_SPAN: f32 = 26.0;

/// Depth slack (view-space units, same as [`render::project`]'s `a`) an edge pixel may be
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
    /// Optional per-facet base unlit color, indexed by `facet_id`. An empty vector
    /// or an index past the end falls back to [`Self::base_color`].
    pub facet_base_colors: Vec<[u8; 3]>,
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
    /// Outline color for the single facet named in `hovered` -- a light
    /// neutral, distinct from every other overlay color so it never gets mistaken
    /// for a selection.
    pub hover_color: [u8; 3],
    /// Outline color for the single facet named in `selected_facet` --
    /// `ui/theme.slint`'s `accent-purple`, deliberately different from
    /// `selected_color` (a whole tier) so "this exact facet" and "this facet's
    /// tier" read as two different kinds of highlight.
    pub selected_facet_color: [u8; 3],
    /// Outline color for every facet id listed in `multi_selected` --
    /// `ui/theme.slint`'s `accent-cyan`. NOT the same color
    /// `editor_tier_table.slint` uses for its own multi-selected row outline,
    /// which is `Theme.accent-amber` (see that file's `row.multi_selected`
    /// border-color binding) -- the 3D viewport and the tier table deliberately
    /// disagree on this color today, this is not a shared convention.
    pub multi_selected_color: [u8; 3],
    /// Outline color for every facet id listed in `provisional` (the slice tier not
    /// yet committed to the design) -- a green distinct from every other overlay,
    /// drawn last so it wins every shared edge.
    pub provisional_color: [u8; 3],
    /// Outline color for every facet id listed in `moved` (a tier whose mast moved
    /// because of the drag in progress) -- an orange distinct from `pending_color`'s
    /// amber, so "this tier follows" and "this re-solve is late" never read alike.
    pub moved_color: [u8; 3],
    /// Buffer-clear color (RGBA8) before each `render` call.
    pub background: [u8; 4],
    /// How facet interiors are filled.
    pub fill_mode: FillMode,
    /// Per-facet flag, indexed by facet id: facet is flagged by the critical-angle overlay.
    pub flagged: Vec<bool>,
    /// Per-facet flag, indexed by facet id: facet awaits a re-solve.
    pub pending: Vec<bool>,
    /// Per-facet flag, indexed by facet id: facet belongs to a selected tier.
    pub selected: Vec<bool>,
    /// The one facet under the cursor, or `None` -- set by
    /// `preview_state::SolidPreviewState::request_facet_overlay`, never by a
    /// fresh replan (a new plan always starts with no hover; the next mouse-move
    /// request re-establishes it).
    pub hovered: Option<u32>,
    /// The one facet a click identified within its (possibly multi-facet) tier
    /// selection, or `None` -- same update path as `hovered`.
    pub selected_facet: Option<u32>,
    /// Every facet id belonging to a multi-selected tier (table checkbox/ctrl-click
    /// selection) -- same update path as `hovered`. A `Vec` rather than a
    /// per-facet `bool` table since the caller already has the small set of ids
    /// straight from `facet_map::FacetMap::facets_of_tier`, and multi-select sizes
    /// are small enough that a linear scan per facet is not worth a second buffer.
    pub multi_selected: Vec<u32>,
    /// Every facet id of the provisional slice tier -- same update path as
    /// `hovered`, and like it cleared by a fresh replan (a new plan starts with
    /// none; the caller re-sends the overlay).
    pub provisional: Vec<u32>,
    /// Every facet id of a tier whose mast moved during the drag in progress -- same
    /// update path and lifetime as `provisional`.
    pub moved: Vec<u32>,
    /// Facet id -> short on-facet label (`facet_map::FacetMap::facet_label`),
    /// shown directly on the facet in the 3D view. Empty string (or a facet id
    /// past the end) draws nothing; a facet also needs a large enough on-screen
    /// span (see [`MIN_LABEL_SPAN`]) and must still win the depth test at its own
    /// centroid before its label is drawn (never labels an occluded facet).
    pub facet_labels: Vec<String>,
    /// Draws an orientation cue after the ordinary render pass: a tick
    /// toward world `+X` (the same "index 0" direction `diagram2d`'s index wheel
    /// uses) at the girdle plane, labeled "0", plus a CROWN/PAVILION caption --
    /// so a cutter who free-orbited the stone can tell orientation without
    /// switching to Diagram mode. Defaults to `true`.
    pub show_orientation_marker: bool,
    /// Facet ids below this are the rough's own bounding (preform)
    /// planes, in `Design::planes_from_solved`'s order (`facet_map::FacetMap::
    /// preform_plane_count`). `0` (the default) means "no preform to
    /// distinguish", matching every caller that never sets it.
    pub preform_plane_count: usize,
    /// When `false`, a preform-plane facet is skipped entirely (culled,
    /// as if back-face-culled) instead of rendered, so a cutter can see through
    /// the rough's own bounding box to the actual cut. `true` (the default)
    /// keeps every preform facet visible, tinted by [`Self::preform_tint_color`]
    /// rather than drawn as an ordinary cut facet.
    pub show_preform: bool,
    /// Tint blended into a preform-plane facet's own shaded color so the
    /// rough's bounding planes read as visually distinct from an ordinary cut
    /// facet -- otherwise a preform plane looks and behaves (under hover/click)
    /// exactly like a real facet with nothing marking it as the uncut rough.
    pub preform_tint_color: [u8; 3],
}

impl Default for SolidStyle {
    fn default() -> Self {
        Self {
            base_color: [200, 205, 215],
            facet_base_colors: Vec::new(),
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
            provisional_color: [74, 222, 128],
            moved_color: [251, 146, 60],
            background: [0, 0, 0, 0],
            fill_mode: FillMode::Opaque,
            flagged: Vec::new(),
            pending: Vec::new(),
            selected: Vec::new(),
            hovered: None,
            selected_facet: None,
            multi_selected: Vec::new(),
            provisional: Vec::new(),
            moved: Vec::new(),
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
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
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

    /// Resets every buffer to background/unpainted.
    fn clear(&mut self, background: [u8; 4]) {
        self.depth.fill(f32::INFINITY);
        self.pick.fill(0);
        let (pixels, _remainder) = self.color.as_chunks_mut::<4>();
        for pixel in pixels {
            pixel.copy_from_slice(&background);
        }
    }
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
