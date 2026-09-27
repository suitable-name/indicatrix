//! The index wheel (tick ring, tooth labels, tooth hit-tagging) and the
//! symmetry/mirror-line and selected-facet radial overlays drawn on top of it.

use super::{
    DiagramFrame, DiagramStyle, PanelKind,
    fill::draw_edge,
    labels::{draw_text, text_size},
    layout::{PanelLayout, wheel_direction},
    render::WheelConfig,
};

/// Draws the index wheel: one tick per tooth, a longer tick every 8th (every 4th
/// for a gear of 64 teeth or fewer, so a small wheel doesn't end up with only one
/// or two labeled ticks), with the tooth number at every long tick.
pub fn draw_index_wheel(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    gear_teeth: u32,
    gear_reference_angle: f32,
    color: [u8; 3],
) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    let major_every = if gear_teeth <= 64 { 4 } else { 8 };
    let inner_r = layout.wheel_radius_px * 0.94;
    let minor_outer_r = layout.wheel_radius_px;
    let major_outer_r = layout.wheel_radius_px * 1.08;
    let label_r = layout.wheel_radius_px * 1.2;

    for tooth in 0..gear_teeth {
        let phi =
            2.0 * std::f32::consts::PI * (tooth as f32 + gear_reference_angle) / gear_teeth as f32;
        let (su, sv) = wheel_direction(phi, mirrored);
        let is_major = tooth % major_every == 0;
        let outer_r = if is_major {
            major_outer_r
        } else {
            minor_outer_r
        };
        let from = (
            su.mul_add(inner_r, layout.center_x),
            sv.mul_add(-inner_r, layout.center_y),
            0.0,
        );
        let to = (
            su.mul_add(outer_r, layout.center_x),
            sv.mul_add(-outer_r, layout.center_y),
            0.0,
        );
        draw_edge(frame, from, to, color, layout.clip, 1);
        // Tag a small hit region at this tick's midpoint so hover/click can
        // resolve "which tooth" from a generous target, not the 1px stroke.
        let mid_r = inner_r.midpoint(outer_r);
        let mid_x = su.mul_add(mid_r, layout.center_x).round() as i32;
        let mid_y = sv.mul_add(-mid_r, layout.center_y).round() as i32;
        tag_tooth_hit(frame, mid_x, mid_y, tooth, 4);

        if is_major {
            let label = tooth.to_string();
            let (w, h) = text_size(&label, 1);
            let lx = su.mul_add(label_r, layout.center_x) - w as f32 / 2.0;
            let ly = sv.mul_add(-label_r, layout.center_y) - h as f32 / 2.0;
            // `label_r` alone puts the two horizontal ticks' labels (3 and 9
            // o'clock) past the panel's own column edge -- `wheel_radius_px*1.2`
            // exceeds `col_width/2` whenever the wheel is sized against a
            // width-limited panel. The ticks themselves stay exactly where they
            // are; only the label's drawing origin is pulled back inside the
            // clip rect so the full glyph string survives rather than being cut
            // off mid-digit.
            let lx = lx.clamp(layout.clip.0 as f32, (layout.clip.2 as f32) - w as f32);
            let ly = ly.clamp(layout.clip.1 as f32, (layout.clip.3 as f32) - h as f32);
            draw_text(frame, lx, ly, &label, 1, color, Some(layout.clip));
        }
    }
}

/// Fills a `(2*radius+1)` square of [`DiagramFrame::tooth_at`]'s backing buffer
/// around `(cx, cy)` with `tooth + 1` -- the hit-region primitive
/// [`draw_index_wheel`]'s tagging pass uses per tick. Out-of-bounds pixels are
/// silently skipped, same as every other per-pixel primitive in this module.
fn tag_tooth_hit(frame: &mut DiagramFrame, cx: i32, cy: i32, tooth: u32, radius: i32) {
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            if let Some(idx) = frame.idx(cx + dx, cy + dy) {
                frame.tooth[idx] = tooth + 1;
            }
        }
    }
}

/// Draws `wheel.symmetry_order` evenly spaced radial guide lines
/// (the schedule's own rotational symmetry, `ScheduleMeta::symmetry_order`) and,
/// when `wheel.mirror` is set, the design's mirror axis -- always the vertical
/// line through the panel centre, since index 0 sits at screen "up" on BOTH
/// panels regardless of `mirrored` (see the parent module's own doc comment: only
/// `screen_right` flips between crown and pavilion, never `screen_up`).
/// Drawn before the index wheel and the facet body so both sit on top of these
/// as background reference lines, not the other way around.
pub fn draw_symmetry_and_mirror_lines(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    wheel: WheelConfig,
    style: &DiagramStyle,
) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    let line_len = layout.wheel_radius_px * 1.05;
    let center = (layout.center_x, layout.center_y, 0.0);

    if wheel.symmetry_order > 1 {
        for k in 0..wheel.symmetry_order {
            let phi = 2.0 * std::f32::consts::PI * (k as f32) / (wheel.symmetry_order as f32);
            let (su, sv) = wheel_direction(phi, mirrored);
            let tip = (
                su.mul_add(line_len, layout.center_x),
                sv.mul_add(-line_len, layout.center_y),
                0.0,
            );
            draw_edge(
                frame,
                center,
                tip,
                style.symmetry_line_color,
                layout.clip,
                1,
            );
        }
    }

    if wheel.mirror {
        let top = (layout.center_x, layout.center_y - line_len, 0.0);
        let bottom = (layout.center_x, layout.center_y + line_len, 0.0);
        draw_edge(frame, top, bottom, style.mirror_line_color, layout.clip, 2);
    }
}

/// Draws a radial line from the panel centre to a highlighted facet's own
/// index-wheel tooth (`DiagramStyle::facet_index_on_gear`), linking "here is
/// tooth N on the wheel" to "here is the facet sitting at that tooth". Only
/// ever called for a crown/pavilion panel (see `super::render::render_panel`): a
/// profile panel has no index wheel to point at.
///
/// One radial per highlighted facet, in the SAME highlight color
/// `super::render::draw_panel_edges` would give that facet's own boundary, at the
/// same precedence (a more specific highlight wins over a coarser one).
pub fn draw_selected_index_radials(
    frame: &mut DiagramFrame,
    layout: &PanelLayout,
    gear_teeth: u32,
    gear_reference_angle: f32,
    style: &DiagramStyle,
    edge_ranges: &[(usize, usize, usize)],
) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    for &(facet_id, _start, _count) in edge_ranges {
        let color = if style.selected_facet == Some(facet_id as u32) {
            style.selected_facet_color
        } else if style.selected.get(facet_id).copied().unwrap_or(false) {
            style.selected_color
        } else if style.multi_selected.contains(&(facet_id as u32)) {
            style.multi_selected_color
        } else {
            continue;
        };
        let Some(&index) = style.facet_index_on_gear.get(facet_id) else {
            continue;
        };
        let phi = 2.0 * std::f32::consts::PI * (index as f32 + gear_reference_angle)
            / gear_teeth.max(1) as f32;
        let (su, sv) = wheel_direction(phi, mirrored);
        let center = (layout.center_x, layout.center_y, 0.0);
        let tip = (
            su.mul_add(layout.wheel_radius_px, layout.center_x),
            sv.mul_add(-layout.wheel_radius_px, layout.center_y),
            0.0,
        );
        draw_edge(frame, center, tip, color, layout.clip, 2);
    }
}
