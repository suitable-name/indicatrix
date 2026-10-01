//! The edge-highlight passes, facet-label stamping, and the orientation-marker
//! overlay drawn after the base fill/edge pass.

use super::{MIN_LABEL_SPAN, SolidRasterizer, SolidStyle, diagram2d, render::project};
use glam::DVec3;
use indicatrix::{geometry::stone_metrics::SolidMesh, optics::raytracer::Camera};

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
    /// The one facet named in `style.hovered`.
    Hovered,
    /// A facet listed in `style.multi_selected`, not otherwise highlighted.
    MultiSelected,
    /// A facet listed in `style.moved` (its tier's mast moved during a drag), not
    /// selected or pending -- a selection or pending highlight still wins its edges.
    Moved,
    /// `selected` (a whole tier) and not otherwise highlighted.
    Selected,
    /// The one facet named in `style.selected_facet` -- a stronger, more
    /// specific identification than the tier-level `Selected` pass.
    SelectedFacet,
    /// `pending`: wins over every pass above it.
    Pending,
    /// A facet listed in `style.provisional` (the uncommitted slice tier): drawn
    /// last and widest, so it wins every shared edge -- including against `Pending`.
    Provisional,
}

impl SolidRasterizer {
    /// Draws every visible facet's boundary in [`EdgePass`]'s ordered passes --
    /// ordinary facets first, then hover/multi-select/tier-select/single-facet-
    /// select/pending, each pass only overpainting a facet already drawn by an
    /// earlier one -- so a shared edge between an ordinary facet and a highlighted
    /// one always ends up in the highlighted color, never the other way around.
    /// Drawing edges once per facet in plain mesh-ring order instead would let
    /// whichever of a shared edge's two owning facets happens to be visited LAST
    /// silently overpaint the other's color (`draw_edge`'s depth test admits a
    /// coincident edge from either side) -- on roughly half of a highlighted
    /// facet's boundary the neighbour's plain dark edge would win, making the
    /// highlight look like a rendering glitch rather than a selection. Every
    /// highlighted pass also draws with a wider stroke (see [`EdgePass`]'s match
    /// arms below) so it reads clearly even where it doesn't win the fight.
    pub(super) fn draw_facet_edges(
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
        let is_moved = |facet_id: usize| style.moved.contains(&(facet_id as u32));
        let is_provisional = |facet_id: usize| style.provisional.contains(&(facet_id as u32));

        for pass in [
            EdgePass::Ordinary,
            EdgePass::Hovered,
            EdgePass::MultiSelected,
            EdgePass::Moved,
            EdgePass::Selected,
            EdgePass::SelectedFacet,
            EdgePass::Pending,
            EdgePass::Provisional,
        ] {
            for &(facet_id, start, count) in edge_ranges {
                let selected = is_selected(facet_id);
                let pending = is_pending(facet_id);
                let hovered = is_hovered(facet_id);
                let selected_facet = is_selected_facet(facet_id);
                let multi_selected = is_multi_selected(facet_id);
                let moved = is_moved(facet_id);
                let provisional = is_provisional(facet_id);
                let highlighted =
                    selected || pending || hovered || selected_facet || moved || provisional;
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
                    EdgePass::Moved => (
                        moved && !selected && !pending && !selected_facet && !provisional,
                        style.moved_color,
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
                    EdgePass::Pending => (pending && !provisional, style.pending_color, 2),
                    EdgePass::Provisional => (provisional, style.provisional_color, 3),
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

    /// Stamps each visible facet's own label (`style.facet_labels`,
    /// `facet_map::FacetMap::facet_label`'s output) at its screen centroid, but
    /// only where the facet is big enough to read ([`MIN_LABEL_SPAN`]) and still
    /// wins the depth test at its own centroid -- the same two guards
    /// `diagram2d::draw_panel_labels` uses, so a facet occluded by a nearer one
    /// never gets a floating label drawn on top of whatever DID win that pixel.
    pub(super) fn draw_facet_labels(
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

    /// Draws the orientation cue directly into the finished color buffer,
    /// after every facet/edge -- an overlay, not a lit surface, so it is never
    /// depth-tested against the mesh (it would otherwise vanish behind whatever
    /// facet happens to be nearest at that screen point). See
    /// [`SolidStyle::show_orientation_marker`]'s own doc comment for what it
    /// draws and why.
    pub(super) fn draw_orientation_marker(&mut self, mesh: &SolidMesh, camera: &Camera) {
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
