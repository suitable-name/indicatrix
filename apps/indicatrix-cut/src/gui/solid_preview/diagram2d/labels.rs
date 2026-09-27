//! Facet-label placement and the shared bitmap-text drawing primitives built on
//! `gui::pixel_font`'s glyph table.
//!
//! The glyph lookup itself (`glyph`/`GLYPH_WIDTH`/`GLYPH_HEIGHT`) lives in
//! `gui::pixel_font`, shared with `gui::tilt::video_export::overlay` -- see that
//! module's own doc comment. Everything in this file is this panel's own
//! scaling/drawing code, which stays here.

use super::{
    super::super::pixel_font::{GLYPH_HEIGHT, GLYPH_WIDTH, glyph},
    DiagramFrame, DiagramStyle, LEADER_LINE_LENGTH, LEADER_LINE_MIN_SPAN, MIN_LABEL_SPAN,
    fill::draw_edge,
    layout::PanelLayout,
};

/// The facet-label pass of `render_panel`: only where there's real room, and only
/// where this facet actually won the depth test at its own centroid (never label an
/// occluded facet). Split out of `render_panel` purely to keep that function under
/// clippy's function-length lint.
pub fn draw_panel_labels(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    style: &DiagramStyle,
    label_spots: Vec<(usize, f32, f32, f32, f32)>,
) {
    for (facet_id, cx, cy, w, h) in label_spots {
        if w < LEADER_LINE_MIN_SPAN || h < LEADER_LINE_MIN_SPAN {
            continue;
        }
        let Some(label) = style.facet_labels.get(facet_id) else {
            continue;
        };
        if label.is_empty() {
            continue;
        }
        let Some(idx) = frame.idx(cx.round() as i32, cy.round() as i32) else {
            continue;
        };
        if frame.pick[idx] != facet_id as u32 + 1 {
            continue;
        }

        // Too small to hold the label directly on the facet: draw a short leader
        // line radiating away from the panel center instead of dropping the
        // label, so a cutter can still trace it back to its facet. Reuses the
        // facet's own depth so the leader is occluded by anything truly in front
        // of it but always wins against the (f32::INFINITY) background.
        let (label_cx, label_cy) = if w < MIN_LABEL_SPAN || h < MIN_LABEL_SPAN {
            let (dir_x, dir_y) = {
                let (dx, dy) = (cx - layout.center_x, cy - layout.center_y);
                let dist = dx.hypot(dy).max(1e-3);
                (dx / dist, dy / dist)
            };
            let leader_end = (
                dir_x.mul_add(LEADER_LINE_LENGTH, cx),
                dir_y.mul_add(LEADER_LINE_LENGTH, cy),
            );
            let depth_here = frame.depth[idx];
            draw_edge(
                frame,
                (cx, cy, depth_here),
                (leader_end.0, leader_end.1, depth_here),
                style.edge_color,
                layout.clip,
                1,
            );
            leader_end
        } else {
            (cx, cy)
        };

        let (label_w, label_h) = text_size(label, 1);
        draw_text(
            frame,
            label_cx - label_w as f32 / 2.0,
            label_cy - label_h as f32 / 2.0,
            label,
            1,
            style.edge_color,
            Some(layout.clip),
        );
    }
}

/// The pixel size `text` occupies at `scale` (1 device pixel per glyph pixel at
/// `scale == 1`), including inter-character spacing but not a trailing gap.
///
/// `pub(super)` (re-exported from the parent module) -- `raster.rs`'s
/// orientation-marker/facet-label overlay needs the same measurement to center
/// its own labels.
pub fn text_size(text: &str, scale: u32) -> (u32, u32) {
    let len = text.chars().count() as u32;
    if len == 0 {
        return (0, (GLYPH_HEIGHT as u32) * scale);
    }
    let width = (len * (GLYPH_WIDTH as u32 + 1) - 1) * scale;
    (width, (GLYPH_HEIGHT as u32) * scale)
}

/// Draws `text` with its top-left at `(x, y)`, one glyph pixel per `scale`
/// device pixels, clipped to `clip` when given (otherwise the whole frame).
pub fn draw_text(
    frame: &mut DiagramFrame,
    x: f32,
    y: f32,
    text: &str,
    scale: u32,
    color: [u8; 3],
    clip: Option<(i32, i32, i32, i32)>,
) {
    let (width, height) = (frame.width, frame.height);
    draw_text_into_buffer(
        TextCanvas {
            color_buf: &mut frame.color,
            width,
            height,
        },
        x,
        y,
        text,
        scale,
        color,
        clip,
    );
}

/// The buffer-generic sibling of [`draw_text`]: draws `text` straight into a raw
/// RGBA8 `width x height` buffer rather than a [`DiagramFrame`]. `pub(super)`
/// (re-exported from the parent module) so `raster.rs`'s orientation-marker/
/// facet-label overlay can share this module's one bitmap font instead of
/// carrying a second copy -- see that module's own doc comment for what it draws.
/// One RGBA8 drawing target for [`draw_text_into_buffer`]: the pixels plus the two
/// dimensions needed to index them.
///
/// Bundled because the three always travel together and describe one thing, which
/// also keeps that function's parameter list within the workspace's own limit.
pub struct TextCanvas<'a> {
    /// RGBA8 pixels, `width * height * 4` bytes long.
    pub color_buf: &'a mut [u8],
    /// Row width in pixels.
    pub width: u32,
    /// Row count.
    pub height: u32,
}

pub fn draw_text_into_buffer(
    canvas: TextCanvas<'_>,
    x: f32,
    y: f32,
    text: &str,
    scale: u32,
    color: [u8; 3],
    clip: Option<(i32, i32, i32, i32)>,
) {
    let TextCanvas {
        color_buf,
        width,
        height,
    } = canvas;
    let clip = clip.unwrap_or((0, 0, width as i32 - 1, height as i32 - 1));
    let scale = scale.max(1) as i32;
    let mut pen_x = x.round() as i32;
    let pen_y = y.round() as i32;
    for ch in text.chars() {
        let rows = glyph(ch);
        for (row, bits) in rows.iter().enumerate() {
            for col in 0..GLYPH_WIDTH {
                if bits & (1 << (GLYPH_WIDTH - 1 - col)) == 0 {
                    continue;
                }
                let px0 = pen_x + col as i32 * scale;
                let py0 = pen_y + row as i32 * scale;
                for sy in 0..scale {
                    for sx in 0..scale {
                        let (px, py) = (px0 + sx, py0 + sy);
                        if px < clip.0 || py < clip.1 || px > clip.2 || py > clip.3 {
                            continue;
                        }
                        if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                            continue;
                        }
                        let idx = (py as u32 * width + px as u32) as usize;
                        let o = idx * 4;
                        color_buf[o] = color[0];
                        color_buf[o + 1] = color[1];
                        color_buf[o + 2] = color[2];
                        color_buf[o + 3] = 255;
                    }
                }
            }
        }
        pen_x += (GLYPH_WIDTH as i32 + 1) * scale;
    }
}
