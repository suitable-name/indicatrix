//! Deterministic overlay-layout selection and the tilt-performance readout's own
//! draw code, for stamping it into each exported frame before PNG encoding.
//!
//! The glyph lookup itself is [`crate::gui::pixel_font`], shared with
//! `gui::solid_preview::diagram2d` -- see that module's own doc comment. Only the
//! scaling/drawing code below (`draw_glyph`/`draw_text`) and the overlay-specific
//! layout/colour logic are this module's own.

use super::metrics::{Metric, MetricReading};
use crate::gui::pixel_font;

const GLYPH_WIDTH: u32 = pixel_font::GLYPH_WIDTH as u32;
const GLYPH_HEIGHT: u32 = pixel_font::GLYPH_HEIGHT as u32;

/// Pixel width `text` occupies at `scale` (1 device px per glyph px at `scale == 1`),
/// including inter-glyph spacing but not a trailing gap.
#[must_use]
pub fn text_width(text: &str, scale: u32) -> u32 {
    let len = text.chars().count() as u32;
    if len == 0 {
        0
    } else {
        (len * (GLYPH_WIDTH + 1) - 1) * scale
    }
}

fn put_pixel(frame: &mut [u8], width: u32, height: u32, x: i32, y: i32, color: [u8; 3]) {
    if x < 0 || y < 0 || x as u32 >= width || y as u32 >= height {
        return;
    }
    let Some(idx) = (y as u32 * width + x as u32)
        .checked_mul(4)
        .map(|i| i as usize)
    else {
        return;
    };
    if idx + 3 >= frame.len() {
        return;
    }
    frame[idx] = color[0];
    frame[idx + 1] = color[1];
    frame[idx + 2] = color[2];
    frame[idx + 3] = 255;
}

#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one glyph's own draw request (the target buffer, its dimensions, the glyph's position/bitmap/scale/colour) -- bundling them into a struct would just move the same count into field access, matching this crate's existing convention for genuinely-this-shaped calls (e.g. bridge::export_thread::spawn_export)"
)]
fn draw_glyph(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    rows: [u8; 7],
    scale: u32,
    color: [u8; 3],
) {
    for (row_idx, bits) in rows.iter().enumerate() {
        for col in 0..GLYPH_WIDTH {
            if bits & (1 << (GLYPH_WIDTH - 1 - col)) == 0 {
                continue;
            }
            let px0 = x + (col * scale) as i32;
            let py0 = y + (row_idx as u32 * scale) as i32;
            for dy in 0..scale {
                for dx in 0..scale {
                    put_pixel(
                        frame,
                        width,
                        height,
                        px0 + dx as i32,
                        py0 + dy as i32,
                        color,
                    );
                }
            }
        }
    }
}

/// Draws `text` with its top-left at `(x, y)` straight into an RGBA8 `width x height`
/// buffer, `scale` device pixels per glyph pixel, at full opacity. Pixels that would
/// land outside the buffer are silently skipped rather than panicking.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one text run's own draw request (the target buffer, its dimensions, the text's position/content/scale/colour) -- see draw_glyph's identical reasoning just above"
)]
pub fn draw_text(
    frame: &mut [u8],
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    text: &str,
    scale: u32,
    color: [u8; 3],
) {
    let scale = scale.max(1);
    for (i, ch) in text.chars().enumerate() {
        let glyph_x = x + i as i32 * ((GLYPH_WIDTH as i32 + 1) * scale as i32);
        draw_glyph(
            frame,
            width,
            height,
            glyph_x,
            y,
            pixel_font::glyph(ch),
            scale,
            color,
        );
    }
}

/// Alpha-blends a solid `color` rectangle onto `frame` (`alpha` in `0..=255`) -- the
/// dark backing bar drawn behind each overlay row so light-coloured text stays legible
/// over a bright render.
#[expect(
    clippy::too_many_arguments,
    reason = "every argument is a distinct piece of one backing-bar draw request (the target buffer, its dimensions, the rectangle's position/size, its colour/alpha) -- see draw_glyph's identical reasoning above"
)]
pub fn fill_rect_alpha(
    frame: &mut [u8],
    width: u32,
    height: u32,
    rect_x: i32,
    rect_y: i32,
    rect_w: u32,
    rect_h: u32,
    color: [u8; 3],
    alpha: u8,
) {
    let alpha_frac = f32::from(alpha) / 255.0;
    for row in 0..rect_h {
        for col in 0..rect_w {
            let px = rect_x + col as i32;
            let py = rect_y + row as i32;
            if px < 0 || py < 0 || px as u32 >= width || py as u32 >= height {
                continue;
            }
            let Some(idx) = (py as u32 * width + px as u32)
                .checked_mul(4)
                .map(|byte_idx| byte_idx as usize)
            else {
                continue;
            };
            if idx + 3 >= frame.len() {
                continue;
            }
            for channel in 0..3 {
                let bg = f32::from(frame[idx + channel]);
                let fg = f32::from(color[channel]);
                frame[idx + channel] = bg.mul_add(1.0 - alpha_frac, fg * alpha_frac) as u8;
            }
        }
    }
}

/// The three overlay display types, richest to plainest -- see
/// [`select_overlay_layout`]'s own doc comment for how one is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlayLayout {
    NumbersOnly,
    ShortLabels,
    FullLabels,
}

/// Font scale (device px per glyph px) for an overlay drawn into a `height`-px-tall
/// frame -- grows with the frame so the readout stays proportionate from 128px up to
/// 8K, while `scale >= 2` always gives a `2 * 7 = 14px` tall glyph, comfortably above
/// a 10px floor.
#[must_use]
pub const fn font_scale(height: u32) -> u32 {
    let scaled = height / 180;
    if scaled < 2 { 2 } else { scaled }
}

/// The longest label any SINGLE metric can print at each layout tier -- the worst-case
/// per-metric width [`select_overlay_layout`] budgets against, since the layout choice
/// depends only on HOW MANY metrics are selected, never on which ones (this dialog's
/// own rule: the same layout must be picked whether the two selected metrics are
/// "Brilliance + Angle" or "Tilt Brilliance + Windowing").
const fn worst_case_sample(layout: OverlayLayout) -> &'static str {
    match layout {
        OverlayLayout::FullLabels => "TILT BRILLIANCE 100.0%",
        OverlayLayout::ShortLabels => "TBR 100.0%",
        OverlayLayout::NumbersOnly => "100.0%",
    }
}

/// Total pixel width `metric_count` metrics take at `layout`/`scale`, including one
/// glyph-width gap between adjacent metrics.
fn layout_width(layout: OverlayLayout, metric_count: usize, scale: u32) -> u32 {
    if metric_count == 0 {
        return 0;
    }
    let per_metric = text_width(worst_case_sample(layout), scale);
    let gap = (GLYPH_WIDTH + 2) * scale;
    per_metric * metric_count as u32 + gap * (metric_count as u32 - 1)
}

/// Picks the richest overlay layout whose worst-case text run fits within 90% of
/// `width` at [`font_scale`], falling back to [`OverlayLayout::NumbersOnly`] (which is
/// always the choice when `metric_count == 0`, though nothing is drawn in that case --
/// see `overlay::draw_overlay`). Depends only on `(width, height, metric_count)`, never
/// on which metrics are selected -- see [`worst_case_sample`]'s own doc comment.
#[must_use]
pub fn select_overlay_layout(width: u32, height: u32, metric_count: usize) -> OverlayLayout {
    if metric_count == 0 {
        return OverlayLayout::NumbersOnly;
    }
    let scale = font_scale(height);
    let budget = (f64::from(width) * 0.9) as u32;
    for layout in [OverlayLayout::FullLabels, OverlayLayout::ShortLabels] {
        if layout_width(layout, metric_count, scale) <= budget {
            return layout;
        }
    }
    OverlayLayout::NumbersOnly
}

/// Colour for one metric reading, matching the live viewport HUD's own colours
/// (`gem_viewport.slint`'s 5-point metrics row, `ui/theme.slint`'s palette) including
/// its value-dependent thresholds for windowing/extinction -- brilliance is always
/// `accent-emerald`, windowing turns `accent-ruby` above 10%, extinction turns
/// `accent-amber` above 12%, otherwise both read `text-secondary`. Tilt brilliance
/// (not part of the live HUD) uses `accent-purple`; the tilt angle itself (also not
/// part of the live HUD) uses `text-primary`.
#[must_use]
pub const fn metric_color(metric: Metric, value: f32) -> [u8; 3] {
    const EMERALD: [u8; 3] = [0x10, 0xB9, 0x81];
    const RUBY: [u8; 3] = [0xF4, 0x3F, 0x5E];
    const AMBER: [u8; 3] = [0xF5, 0x9E, 0x0B];
    const PURPLE: [u8; 3] = [0xA8, 0x55, 0xF7];
    const SECONDARY: [u8; 3] = [0x94, 0xA3, 0xB8];
    const PRIMARY: [u8; 3] = [0xF1, 0xF5, 0xF9];
    match metric {
        Metric::Brilliance => EMERALD,
        Metric::Windowing => {
            if value > 10.0 {
                RUBY
            } else {
                SECONDARY
            }
        }
        Metric::Extinction => {
            if value > 12.0 {
                AMBER
            } else {
                SECONDARY
            }
        }
        Metric::TiltBrilliance => PURPLE,
        Metric::Angle => PRIMARY,
    }
}

const fn full_label(metric: Metric) -> &'static str {
    match metric {
        Metric::Brilliance => "BRILLIANCE",
        Metric::Windowing => "WINDOWING",
        Metric::Extinction => "EXTINCTION",
        Metric::TiltBrilliance => "TILT BRILLIANCE",
        Metric::Angle => "TILT ANGLE",
    }
}

const fn short_label(metric: Metric) -> &'static str {
    match metric {
        Metric::Brilliance => "BRI",
        Metric::Windowing => "WIN",
        Metric::Extinction => "EXT",
        Metric::TiltBrilliance => "TBR",
        Metric::Angle => "ANG",
    }
}

/// Formats one metric's value the way the overlay prints it: a signed one-decimal
/// number for the tilt angle (no unit -- the bundled font carries no degree glyph), a
/// one-decimal percentage for every other metric.
fn format_value(metric: Metric, value: f32) -> String {
    if metric == Metric::Angle {
        format!("{value:.1}")
    } else {
        format!("{value:.1}%")
    }
}

/// The exact text drawn for one metric reading at `layout`.
#[must_use]
pub fn format_reading(metric: Metric, value: f32, layout: OverlayLayout) -> String {
    match layout {
        OverlayLayout::FullLabels => {
            format!("{} {}", full_label(metric), format_value(metric, value))
        }
        OverlayLayout::ShortLabels => {
            format!("{} {}", short_label(metric), format_value(metric, value))
        }
        OverlayLayout::NumbersOnly => format_value(metric, value),
    }
}

/// Draws every selected metric reading into `frame` (RGBA8, `width x height`), choosing
/// the layout via [`select_overlay_layout`] and stacking one reading per row in the
/// top-left corner, each with a translucent backing bar for legibility over a bright
/// render. A no-op for an empty `readings` (the "Show performance values" toggle off).
pub fn draw_overlay(frame: &mut [u8], width: u32, height: u32, readings: &[MetricReading]) {
    if readings.is_empty() {
        return;
    }
    let layout = select_overlay_layout(width, height, readings.len());
    let scale = font_scale(height);
    let row_h = GLYPH_HEIGHT * scale + scale * 2;
    let pad = scale as i32;
    for (row, reading) in readings.iter().enumerate() {
        let text = format_reading(reading.metric, reading.value, layout);
        let y = pad + row as i32 * row_h as i32;
        let bar_w = text_width(&text, scale) + 2 * scale + pad as u32;
        fill_rect_alpha(
            frame,
            width,
            height,
            0,
            y - pad / 2,
            bar_w,
            row_h,
            [15, 21, 34],
            170,
        );
        draw_text(
            frame,
            width,
            height,
            pad,
            y,
            &text,
            scale,
            metric_color(reading.metric, reading.value),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOLUTIONS: [(u32, u32); 5] = [
        (128, 128),
        (256, 256),
        (512, 512),
        (1920, 1080),
        (3840, 2160),
    ];

    /// Layout selection at 128/256/512/1080p/4K for 1..=5 selected metrics.
    #[test]
    fn select_overlay_layout_covers_every_required_resolution_and_metric_count() {
        for (w, h) in RESOLUTIONS {
            for count in 1..=5 {
                // Must not panic, and must return a layout every reading can be drawn
                // in -- the actual richness assertions below cover the interesting
                // cases.
                let _ = select_overlay_layout(w, h, count);
            }
        }
    }

    #[test]
    fn zero_metrics_is_always_numbers_only() {
        assert_eq!(
            select_overlay_layout(3840, 2160, 0),
            OverlayLayout::NumbersOnly
        );
    }

    #[test]
    fn a_tiny_128px_frame_with_many_metrics_falls_back_to_numbers_only() {
        assert_eq!(
            select_overlay_layout(128, 128, 5),
            OverlayLayout::NumbersOnly
        );
    }

    #[test]
    fn a_4k_frame_with_one_metric_affords_full_labels() {
        assert_eq!(
            select_overlay_layout(3840, 2160, 1),
            OverlayLayout::FullLabels
        );
    }

    /// More metrics at the SAME resolution must never pick a richer layout than fewer
    /// metrics -- adding a metric only ever tightens the budget.
    #[test]
    fn more_metrics_never_yields_a_richer_layout_at_the_same_resolution() {
        fn rank(layout: OverlayLayout) -> u8 {
            match layout {
                OverlayLayout::NumbersOnly => 0,
                OverlayLayout::ShortLabels => 1,
                OverlayLayout::FullLabels => 2,
            }
        }
        for (w, h) in RESOLUTIONS {
            let mut previous = u8::MAX;
            for count in 1..=5 {
                let rank_now = rank(select_overlay_layout(w, h, count));
                assert!(
                    rank_now <= previous,
                    "at {w}x{h}, {count} metrics picked a richer layout than {} metrics",
                    count - 1
                );
                previous = rank_now;
            }
        }
    }

    /// A higher resolution at the SAME metric count must never pick a poorer layout
    /// than a lower one -- more width only ever loosens the budget.
    #[test]
    fn higher_resolution_never_yields_a_poorer_layout_for_the_same_metric_count() {
        fn rank(layout: OverlayLayout) -> u8 {
            match layout {
                OverlayLayout::NumbersOnly => 0,
                OverlayLayout::ShortLabels => 1,
                OverlayLayout::FullLabels => 2,
            }
        }
        for count in 1..=5 {
            let mut previous = 0u8;
            for (w, h) in RESOLUTIONS {
                let rank_now = rank(select_overlay_layout(w, h, count));
                assert!(
                    rank_now >= previous,
                    "{w}x{h} at {count} metrics regressed below a smaller resolution"
                );
                previous = rank_now;
            }
        }
    }

    #[test]
    fn font_scale_never_drops_below_the_10px_floor() {
        for h in [1u32, 128, 512, 4320] {
            assert!(font_scale(h) * GLYPH_HEIGHT >= 10);
        }
    }

    #[test]
    fn text_width_scales_linearly_and_is_zero_for_empty_text() {
        assert_eq!(text_width("", 3), 0);
        assert_eq!(text_width("A", 1), GLYPH_WIDTH);
        assert_eq!(text_width("AB", 1), GLYPH_WIDTH * 2 + 1);
    }

    #[test]
    fn format_reading_matches_each_layouts_own_shape() {
        // 42.3 (not e.g. 42.05) so the expected `{:.1}` rounding is unambiguous
        // regardless of `f32`'s binary rounding of the literal itself.
        assert_eq!(
            format_reading(Metric::Brilliance, 42.3, OverlayLayout::NumbersOnly),
            "42.3%"
        );
        assert_eq!(
            format_reading(Metric::Brilliance, 42.3, OverlayLayout::ShortLabels),
            "BRI 42.3%"
        );
        assert_eq!(
            format_reading(Metric::Brilliance, 42.3, OverlayLayout::FullLabels),
            "BRILLIANCE 42.3%"
        );
        assert_eq!(
            format_reading(Metric::Angle, -12.3, OverlayLayout::NumbersOnly),
            "-12.3"
        );
    }

    #[test]
    fn metric_color_applies_the_hud_thresholds() {
        assert_eq!(
            metric_color(Metric::Windowing, 5.0),
            metric_color(Metric::Extinction, 5.0)
        );
        assert_ne!(
            metric_color(Metric::Windowing, 5.0),
            metric_color(Metric::Windowing, 20.0)
        );
        assert_ne!(
            metric_color(Metric::Extinction, 5.0),
            metric_color(Metric::Extinction, 20.0)
        );
    }

    #[test]
    fn draw_overlay_does_not_panic_on_a_1px_frame() {
        let mut frame = vec![0u8; 4];
        draw_overlay(
            &mut frame,
            1,
            1,
            &[MetricReading {
                metric: Metric::Angle,
                value: 0.0,
            }],
        );
    }

    #[test]
    fn draw_text_paints_at_least_one_opaque_pixel_for_visible_glyphs() {
        let (w, h) = (40, 20);
        let mut frame = vec![0u8; (w * h * 4) as usize];
        draw_text(&mut frame, w, h, 2, 2, "8", 2, [255, 255, 255]);
        assert!(frame.chunks(4).any(|px| px[3] == 255));
    }
}
