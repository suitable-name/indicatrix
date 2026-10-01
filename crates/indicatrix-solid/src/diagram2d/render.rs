//! The top-level diagram render entry points and one panel's whole draw pass:
//! caption, index wheel, facet fill/edges, labels -- see the parent module's doc
//! comment for the panel conventions this implements.

use super::{
    DiagramConfig, DiagramFrame, DiagramStyle, PanelKind,
    fill::{bounds, draw_edge, draw_meet_marker, fill_polygon, meet_marker_points, shade},
    labels::{draw_panel_labels, draw_text, text_size},
    layout::{
        PanelLayout, compute_layout, compute_single_panel_layout, crown_project, crown_visible,
        facet_normals, pavilion_project, pavilion_visible, profile_project, profile_visible,
    },
    legend::{draw_index_wheel, draw_selected_index_radials, draw_symmetry_and_mirror_lines},
    simplify_ring,
};
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::SolidMesh;

/// Renders `mesh` into a three-panel [`DiagramFrame`] per the parent module's doc
/// comment.
///
/// Never panics on an empty mesh (draws background only, per-panel captions and
/// index wheels still appear).
#[must_use]
pub fn render_diagram(
    mesh: &SolidMesh,
    config: &DiagramConfig,
    style: &DiagramStyle,
) -> DiagramFrame {
    let mut frame = DiagramFrame::blank(config.width, config.height, style.background);
    if config.width == 0 || config.height == 0 {
        return frame;
    }
    let layouts = compute_layout(mesh, config.width, config.height);
    let facet_normal = facet_normals(mesh);
    let wheel = WheelConfig {
        gear_teeth: config.gear_teeth.max(1),
        gear_reference_angle: config.gear_reference_angle,
        symmetry_order: config.symmetry_order,
        mirror: config.mirror,
    };
    // Resolved once, panel-agnostic -- `render_panel` below only needs to
    // filter by which side of the girdle a point falls on, not recompute it.
    let marker_points =
        meet_marker_points(mesh, &style.meet_marker_pairs, mesh_diagonal(mesh) * 1e-4);

    let mut dedup_scratch: Vec<DVec3> = Vec::new();
    let mut ring_scratch: Vec<DVec3> = Vec::new();

    for layout in &layouts {
        render_panel(
            &mut frame,
            mesh,
            layout,
            &facet_normal,
            wheel,
            style,
            &mut dedup_scratch,
            &mut ring_scratch,
            &marker_points,
        );
    }

    frame
}

/// The "enlarge this panel" mode: renders `mesh` as a SINGLE panel filling the
/// whole frame.
///
/// Everything else matches [`render_diagram`]'s fixed three-column layout: the same
/// style, the same meet-marker and index-radial passes, and the same pick, panel and
/// tooth buffer contract. `gui::solid_preview::diagram_wiring`'s hover and click
/// callbacks therefore work against a frame from either function identically, since
/// both go through the same [`render_panel`] into the same [`DiagramFrame`].
#[must_use]
pub fn render_diagram_single_panel(
    mesh: &SolidMesh,
    config: &DiagramConfig,
    style: &DiagramStyle,
    panel: PanelKind,
) -> DiagramFrame {
    let mut frame = DiagramFrame::blank(config.width, config.height, style.background);
    if config.width == 0 || config.height == 0 {
        return frame;
    }
    let layout = compute_single_panel_layout(mesh, config.width, config.height, panel);
    let facet_normal = facet_normals(mesh);
    let wheel = WheelConfig {
        gear_teeth: config.gear_teeth.max(1),
        gear_reference_angle: config.gear_reference_angle,
        symmetry_order: config.symmetry_order,
        mirror: config.mirror,
    };
    let marker_points =
        meet_marker_points(mesh, &style.meet_marker_pairs, mesh_diagonal(mesh) * 1e-4);

    let mut dedup_scratch: Vec<DVec3> = Vec::new();
    let mut ring_scratch: Vec<DVec3> = Vec::new();
    render_panel(
        &mut frame,
        mesh,
        &layout,
        &facet_normal,
        wheel,
        style,
        &mut dedup_scratch,
        &mut ring_scratch,
        &marker_points,
    );
    frame
}

/// The mesh's own bounding-box diagonal, in world units -- the scale
/// [`meet_marker_points`]'s vertex-matching tolerance is fractioned from
/// (mirrors [`simplify_ring`]'s own scale-relative merge tolerance). `1e-6` for
/// an empty mesh so a zero tolerance never turns into "everything matches".
fn mesh_diagonal(mesh: &SolidMesh) -> f64 {
    let Some(&first) = mesh.positions.first() else {
        return 1e-6;
    };
    let (mut lo, mut hi) = (first, first);
    for &p in &mesh.positions {
        lo = lo.min(p);
        hi = hi.max(p);
    }
    (hi - lo).length().max(1e-6)
}

/// The index-wheel/symmetry parameters every panel draw needs, bundled purely
/// so [`render_panel`]'s own parameter list doesn't keep growing every time a
/// new wheel-relative overlay (the symmetry/mirror lines, on top of the index
/// wheel itself) needs another `DiagramConfig` field mirrored down.
#[derive(Debug, Clone, Copy)]
pub struct WheelConfig {
    /// Number of teeth on the index gear.
    pub gear_teeth: u32,
    /// Reference angle of the index gear, in degrees.
    pub gear_reference_angle: f32,
    /// Rotational symmetry order of the schedule.
    pub symmetry_order: u32,
    /// Whether the schedule carries mirror symmetry.
    pub mirror: bool,
}

/// One panel's whole draw pass: caption, index wheel, facet fill/edges, labels --
/// split out of [`render_diagram`] purely to keep that function short; see the
/// parent module's doc comment for the panel conventions this implements.
#[expect(
    clippy::too_many_arguments,
    reason = "a flat parameter list mirroring render_diagram's own locals -- there is \
              exactly one call site, so a bundling struct would only add indirection"
)]
fn render_panel(
    frame: &mut DiagramFrame,
    mesh: &SolidMesh,
    layout: &PanelLayout,
    facet_normal: &[Option<DVec3>],
    wheel: WheelConfig,
    style: &DiagramStyle,
    dedup_scratch: &mut Vec<DVec3>,
    ring_scratch: &mut Vec<DVec3>,
    marker_points: &[DVec3],
) {
    let (cap_w, _) = text_size(layout.kind.caption(), 1);
    draw_text(
        frame,
        layout.center_x - cap_w as f32 / 2.0,
        2.0,
        layout.kind.caption(),
        1,
        style.wheel_color,
        None,
    );

    if layout.kind.has_index_wheel() {
        draw_symmetry_and_mirror_lines(frame, layout, wheel, style);
        draw_index_wheel(
            frame,
            layout,
            wheel.gear_teeth,
            wheel.gear_reference_angle,
            style.wheel_color,
        );
    }

    let visible: fn(Vec3) -> bool = match layout.kind {
        PanelKind::Crown => crown_visible,
        PanelKind::Pavilion => pavilion_visible,
        PanelKind::Profile => profile_visible,
    };
    let project: fn(DVec3, &PanelLayout) -> (f32, f32, f32) = match layout.kind {
        PanelKind::Crown => crown_project,
        PanelKind::Pavilion => pavilion_project,
        PanelKind::Profile => profile_project,
    };

    let fill_ctx = PanelRenderCtx {
        mesh,
        layout,
        facet_normal,
        style,
        visible,
        project,
    };
    let (edge_points, edge_ranges, label_spots) =
        fill_panel_facets(frame, &fill_ctx, dedup_scratch, ring_scratch);
    draw_panel_edges(frame, layout, style, &edge_points, &edge_ranges);
    draw_panel_labels(frame, layout, style, label_spots);

    if layout.kind.has_index_wheel() {
        draw_selected_index_radials(
            frame,
            layout,
            wheel.gear_teeth,
            wheel.gear_reference_angle,
            style,
            &edge_ranges,
        );
    }

    // Meet-point markers -- crown-side points on the crown panel,
    // pavilion-side points on the pavilion panel (a profile panel shows no
    // index-wheel-relative content and is skipped, same set as the radials
    // above). See `meet_marker_points`'s own doc comment for how a point here
    // was resolved from `DiagramStyle::meet_marker_pairs`.
    let on_this_panel: fn(f64) -> bool = match layout.kind {
        PanelKind::Crown => |y| y >= 0.0,
        PanelKind::Pavilion => |y| y <= 0.0,
        PanelKind::Profile => |_| false,
    };
    for &point in marker_points {
        if !on_this_panel(point.y) {
            continue;
        }
        let screen = project(point, layout);
        draw_meet_marker(frame, screen, style.meet_marker_color, layout.clip);
    }
}

/// The immutable per-panel context [`fill_panel_facets`] needs, bundled purely to
/// keep that function (and its caller, [`render_panel`]) under clippy's
/// argument-count limit.
struct PanelRenderCtx<'a> {
    mesh: &'a SolidMesh,
    layout: &'a PanelLayout,
    facet_normal: &'a [Option<DVec3>],
    style: &'a DiagramStyle,
    visible: fn(Vec3) -> bool,
    project: fn(DVec3, &PanelLayout) -> (f32, f32, f32),
}

/// [`fill_panel_facets`]'s own output: the flattened edge-segment points, each
/// facet's `(facet_id, start, count)` range into them, and each facet's
/// `(facet_id, cx, cy, w, h)` label spot. Named purely so that function's return
/// type doesn't trip clippy's `type_complexity` lint.
type PanelFillOutput = (
    Vec<(f32, f32, f32)>,
    Vec<(usize, usize, usize)>,
    Vec<(usize, f32, f32, f32, f32)>, // facet_id, cx, cy, w, h
);

/// The facet fill pass of [`render_panel`]'s "two passes, exactly like
/// `raster::SolidRasterizer::render`" scheme: fills every visible facet, and
/// collects the edge segments/label spots the caller's later passes
/// ([`draw_panel_edges`]/[`draw_panel_labels`]) draw on top -- so an edge is
/// depth-tested against the complete panel rather than only the facets drawn before
/// it. Split out of `render_panel` purely to keep that function under clippy's
/// function-length lint.
fn fill_panel_facets(
    frame: &mut DiagramFrame,
    ctx: &PanelRenderCtx<'_>,
    dedup_scratch: &mut Vec<DVec3>,
    ring_scratch: &mut Vec<DVec3>,
) -> PanelFillOutput {
    let mut edge_points: Vec<(f32, f32, f32)> = Vec::new();
    let mut edge_ranges: Vec<(usize, usize, usize)> = Vec::new();
    let mut label_spots: Vec<(usize, f32, f32, f32, f32)> = Vec::new();

    for (facet_id, ring) in &ctx.mesh.rings {
        let Some(Some(normal_f64)) = ctx.facet_normal.get(*facet_id).copied() else {
            continue;
        };
        let normal = to_vec3(normal_f64);
        if !(ctx.visible)(normal) {
            continue;
        }
        simplify_ring(ring, dedup_scratch, ring_scratch);
        if ring_scratch.len() < 3 {
            continue;
        }
        let screen: Vec<(f32, f32, f32)> = ring_scratch
            .iter()
            .map(|&p| (ctx.project)(p, ctx.layout))
            .collect();

        let color = shade(normal, ctx.style);
        fill_polygon(
            frame,
            &screen,
            *facet_id,
            color,
            ctx.style,
            ctx.layout.clip,
            ctx.layout.kind.tag(),
        );
        edge_ranges.push((*facet_id, edge_points.len(), screen.len()));
        edge_points.extend_from_slice(&screen);

        let (min_x, max_x, min_y, max_y) = bounds(&screen);
        let (cx, cy) = (min_x.midpoint(max_x), min_y.midpoint(max_y));
        label_spots.push((*facet_id, cx, cy, max_x - min_x, max_y - min_y));
    }

    (edge_points, edge_ranges, label_spots)
}

/// Which of [`draw_panel_edges`]'s ordered passes an edge segment belongs to --
/// mirrors `raster::EdgePass` exactly (see that type's doc comment for why later
/// passes must win a shared edge, and for the pass ordering rationale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EdgePass {
    /// None of the below: the plain `edge_color`, drawn first.
    Ordinary,
    /// The one facet named in `style.hovered`.
    Hovered,
    /// A facet listed in `style.multi_selected`, not otherwise highlighted.
    MultiSelected,
    /// `selected` (a whole tier) and not otherwise highlighted.
    Selected,
    /// The one facet named in `style.selected_facet`.
    SelectedFacet,
    /// `pending`: drawn last, so it wins over every other pass too.
    Pending,
}

/// The edge-draw pass of [`render_panel`]'s "two passes" scheme -- see
/// [`fill_panel_facets`]'s doc comment. Split out of `render_panel` purely to keep
/// that function under clippy's function-length lint.
///
/// Draws in [`EdgePass`]'s six ordered sub-passes rather than once per facet, and
/// every highlighted sub-pass uses a wider stroke, so a highlighted facet's
/// boundary reads as a clean, intentional outline instead of being
/// half-overpainted by a neighbouring ordinary facet's dark edge.
fn draw_panel_edges(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    style: &DiagramStyle,
    edge_points: &[(f32, f32, f32)],
    edge_ranges: &[(usize, usize, usize)],
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
                EdgePass::Hovered => (hovered && !pending && !selected_facet, style.hover_color, 2),
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
                draw_edge(frame, a, b, color, layout.clip, width);
            }
        }
    }
}

const fn to_vec3(v: DVec3) -> Vec3 {
    Vec3::new(v.x as f32, v.y as f32, v.z as f32)
}
