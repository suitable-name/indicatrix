//! Small RGBA rasters for the wizard's canvases: the transmittance photo, the mask overlay, zone
//! outlines, the render and the colour-difference heat map.
//!
//! Everything is plain `Vec<u8>` so the
//! logic is testable; the window turns a raster into a Slint image.

use indicatrix_cut_core::rough_plan::photometry::{PixelMask, flag, linear_to_srgb};

/// An RGBA8 picture, row-major, origin at the top left.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Four bytes per pixel.
    pub data: Vec<u8>,
}

impl Rgba {
    /// A picture filled with one colour.
    #[must_use]
    pub fn filled(width: usize, height: usize, colour: [u8; 4]) -> Self {
        let mut data = Vec::with_capacity(width * height * 4);
        for _ in 0..width * height {
            data.extend_from_slice(&colour);
        }
        Self {
            width,
            height,
            data,
        }
    }

    /// The pixel at `(x, y)`.
    #[must_use]
    pub fn pixel(&self, x: usize, y: usize) -> [u8; 4] {
        let i = (y * self.width + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// Sets the pixel at `(x, y)`.
    pub fn set(&mut self, x: usize, y: usize, colour: [u8; 4]) {
        let i = (y * self.width + x) * 4;
        self.data[i..i + 4].copy_from_slice(&colour);
    }

    /// Mixes `colour` into the pixel at `(x, y)` with weight `alpha` (0 to 1).
    pub fn blend(&mut self, x: usize, y: usize, colour: [u8; 3], alpha: f32) {
        let a = alpha.clamp(0.0, 1.0);
        let i = (y * self.width + x) * 4;
        for (channel, &source) in self.data[i..i + 3].iter_mut().zip(&colour) {
            let mixed = f32::mul_add(f32::from(source), a, f32::from(*channel) * (1.0 - a));
            *channel = mixed.round().clamp(0.0, 255.0) as u8;
        }
        self.data[i + 3] = 255;
    }

    /// Whether the picture has no pixels.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }
}

/// The colour a linear value takes in a picture (sRGB encoding), 0 to 255.
#[must_use]
pub fn encode_linear(value: f32) -> u8 {
    if !value.is_finite() {
        return 0;
    }
    (linear_to_srgb(value.clamp(0.0, 1.0)) * 255.0)
        .round()
        .clamp(0.0, 255.0) as u8
}

/// A linear RGB picture (relative to the empty backlight) shown with exposure `gain`.
#[must_use]
pub fn linear_image(values: &[[f32; 3]], width: usize, height: usize, gain: f32) -> Rgba {
    let mut image = Rgba::filled(width, height, [0, 0, 0, 255]);
    for (i, value) in values.iter().take(width * height).enumerate() {
        let (x, y) = (i % width, i / width);
        image.set(
            x,
            y,
            [
                encode_linear(value[0] * gain),
                encode_linear(value[1] * gain),
                encode_linear(value[2] * gain),
                255,
            ],
        );
    }
    image
}

/// The mask flags in the order they are drawn (the first match wins), with their colour and name.
pub const FLAG_COLOURS: [(u8, [u8; 3], &str); 7] = [
    (flag::USER, [255, 0, 255], "Your brush"),
    (flag::INCLUSION, [255, 140, 0], "Inclusion"),
    (flag::GHOST, [160, 90, 255], "Ghost of an inclusion"),
    (flag::SATURATED, [255, 255, 0], "Saturated"),
    (flag::BELOW_NOISE, [0, 120, 255], "Below the noise"),
    (flag::EDGE_BAND, [0, 200, 200], "Edge band"),
    (flag::OUTSIDE_OUTLINE, [90, 90, 90], "Outside the stone"),
];

/// The overlay colour of a pixel's flags, or `None` for a clear pixel.
#[must_use]
pub fn flag_colour(bits: u8) -> Option<[u8; 3]> {
    FLAG_COLOURS
        .iter()
        .find(|(f, _, _)| bits & f != 0)
        .map(|(_, colour, _)| *colour)
}

/// Tints every flagged pixel of `image` (same size as `mask`) with its flag colour.
pub fn mask_overlay(image: &mut Rgba, mask: &PixelMask, alpha: f32) {
    if image.width != mask.width() || image.height != mask.height() {
        return;
    }
    for y in 0..mask.height() {
        for x in 0..mask.width() {
            if let Some(colour) = flag_colour(mask.get(x, y)) {
                image.blend(x, y, colour, alpha);
            }
        }
    }
}

/// Draws a polyline (points in picture pixels) one pixel wide; `closed` joins the last point to
/// the first. Parts outside the picture are clipped.
pub fn draw_polyline(image: &mut Rgba, points: &[[f64; 2]], closed: bool, colour: [u8; 3]) {
    if points.len() < 2 || image.is_empty() {
        return;
    }
    for pair in points.windows(2) {
        draw_line(image, pair[0], pair[1], colour);
    }
    if closed && points.len() > 2 {
        draw_line(image, points[points.len() - 1], points[0], colour);
    }
}

fn draw_line(image: &mut Rgba, from: [f64; 2], to: [f64; 2], colour: [u8; 3]) {
    if !(from[0].is_finite() && from[1].is_finite() && to[0].is_finite() && to[1].is_finite()) {
        return;
    }
    let length = (to[0] - from[0]).hypot(to[1] - from[1]);
    let steps = length.ceil().clamp(1.0, 20_000.0) as usize;
    for step in 0..=steps {
        let t = step as f64 / steps as f64;
        let x = (to[0] - from[0]).mul_add(t, from[0]).floor();
        let y = (to[1] - from[1]).mul_add(t, from[1]).floor();
        if x >= 0.0 && y >= 0.0 && (x as usize) < image.width && (y as usize) < image.height {
            image.blend(x as usize, y as usize, colour, 1.0);
        }
    }
}

/// Draws a filled disc (centre and radius in picture pixels).
pub fn draw_disc(image: &mut Rgba, centre: [f64; 2], radius: f64, colour: [u8; 3], alpha: f32) {
    if image.is_empty() || !centre[0].is_finite() || !centre[1].is_finite() {
        return;
    }
    let r = radius.clamp(0.5, 200.0);
    let x_lo = (centre[0] - r).floor().max(0.0) as usize;
    let y_lo = (centre[1] - r).floor().max(0.0) as usize;
    let x_hi = ((centre[0] + r).ceil().max(0.0) as usize).min(image.width - 1);
    let y_hi = ((centre[1] + r).ceil().max(0.0) as usize).min(image.height - 1);
    for y in y_lo..=y_hi {
        for x in x_lo..=x_hi {
            if (x as f64 + 0.5 - centre[0]).hypot(y as f64 + 0.5 - centre[1]) <= r {
                image.blend(x, y, colour, alpha);
            }
        }
    }
}

const HEAT_STOPS: [(f32, [f32; 3]); 6] = [
    (0.0, [10.0, 10.0, 40.0]),
    (1.0, [20.0, 60.0, 160.0]),
    (2.0, [30.0, 150.0, 150.0]),
    (4.0, [90.0, 190.0, 70.0]),
    (6.0, [230.0, 200.0, 50.0]),
    (10.0, [220.0, 50.0, 40.0]),
];

/// The heat-map colour of a CIEDE2000 difference: dark blue at 0, green near 4, yellow at 6, red
/// from 10 up.
#[must_use]
pub fn heat_colour(delta_e: f32) -> [u8; 3] {
    let d = if delta_e.is_finite() {
        delta_e.max(0.0)
    } else {
        0.0
    };
    let last = HEAT_STOPS[HEAT_STOPS.len() - 1];
    if d >= last.0 {
        return [last.1[0] as u8, last.1[1] as u8, last.1[2] as u8];
    }
    for pair in HEAT_STOPS.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if d <= b.0 {
            let t = (d - a.0) / (b.0 - a.0);
            let mix = |c: usize| (b.1[c] - a.1[c]).mul_add(t, a.1[c]).round() as u8;
            return [mix(0), mix(1), mix(2)];
        }
    }
    [last.1[0] as u8, last.1[1] as u8, last.1[2] as u8]
}

/// The color of a pixel that was not compared.
pub const NOT_COMPARED: [u8; 3] = [32, 32, 32];

/// A heat-map picture of per-pixel differences; pixels that are not `valid` are dark grey.
#[must_use]
pub fn heat_image(delta: &[f32], valid: &[bool], width: usize, height: usize) -> Rgba {
    let mut image = Rgba::filled(width, height, [0, 0, 0, 255]);
    for i in 0..width * height {
        let colour = match (delta.get(i), valid.get(i)) {
            (Some(&d), Some(&true)) => heat_colour(d),
            _ => NOT_COMPARED,
        };
        image.set(i % width, i / width, [colour[0], colour[1], colour[2], 255]);
    }
    image
}

/// The part of `image` that a zoom of `zoom` (1, 2, 4, ...) centred at `centre` (fractions of
/// the picture) shows, as a picture of its own.
///
/// `width / zoom` by `height / zoom`, shifted to stay inside the picture.
#[must_use]
pub fn crop_zoom(image: &Rgba, centre: [f64; 2], zoom: u32) -> Rgba {
    let zoom = zoom.max(1) as usize;
    if zoom == 1 || image.is_empty() {
        return image.clone();
    }
    let (w, h) = ((image.width / zoom).max(1), (image.height / zoom).max(1));
    let x0 = f64::mul_add(centre[0], image.width as f64, -(w as f64 / 2.0))
        .round()
        .clamp(0.0, (image.width - w) as f64) as usize;
    let y0 = f64::mul_add(centre[1], image.height as f64, -(h as f64 / 2.0))
        .round()
        .clamp(0.0, (image.height - h) as f64) as usize;
    let mut out = Rgba::filled(w, h, [0, 0, 0, 255]);
    for y in 0..h {
        for x in 0..w {
            out.set(x, y, image.pixel(x0 + x, y0 + y));
        }
    }
    out
}

/// The picture as PNG bytes.
///
/// # Errors
///
/// A sentence when the picture is empty or the encoder fails.
pub fn to_png(image: &Rgba) -> Result<Vec<u8>, String> {
    if image.is_empty() {
        return Err("The picture has no pixels.".to_owned());
    }
    let (width, height) = (
        u32::try_from(image.width).map_err(|_| "The picture is too wide.".to_owned())?,
        u32::try_from(image.height).map_err(|_| "The picture is too tall.".to_owned())?,
    );
    let buffer = image::RgbaImage::from_raw(width, height, image.data.clone())
        .ok_or_else(|| "The picture has no pixels.".to_owned())?;
    let mut bytes = Vec::new();
    buffer
        .write_to(
            &mut std::io::Cursor::new(&mut bytes),
            image::ImageFormat::Png,
        )
        .map_err(|e| format!("Could not encode the picture: {e}"))?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn linear_values_are_encoded_like_srgb() {
        assert_eq!(encode_linear(0.0), 0);
        assert_eq!(encode_linear(1.0), 255);
        assert_eq!(encode_linear(2.0), 255);
        assert_eq!(encode_linear(f32::NAN), 0);
        let mid = encode_linear(0.2140);
        assert!((i32::from(mid) - 128).abs() <= 1, "{mid}");
    }

    #[test]
    fn a_linear_image_applies_the_gain() {
        let image = linear_image(&[[0.1, 0.1, 0.1], [0.4, 0.2, 0.0]], 2, 1, 1.0);
        let boosted = linear_image(&[[0.1, 0.1, 0.1], [0.4, 0.2, 0.0]], 2, 1, 4.0);
        assert!(boosted.pixel(0, 0)[0] > image.pixel(0, 0)[0]);
        assert_eq!(image.pixel(1, 0)[2], 0);
        assert_eq!(image.pixel(0, 0)[3], 255);
    }

    #[test]
    fn blending_mixes_and_clamps() {
        let mut image = Rgba::filled(2, 2, [100, 100, 100, 255]);
        image.blend(0, 0, [200, 0, 100], 0.5);
        assert_eq!(image.pixel(0, 0), [150, 50, 100, 255]);
        image.blend(1, 1, [200, 0, 100], 5.0);
        assert_eq!(image.pixel(1, 1), [200, 0, 100, 255]);
        assert_eq!(image.pixel(1, 0), [100, 100, 100, 255]);
    }

    #[test]
    fn mask_flags_get_their_colours_in_priority_order() {
        let mut mask = PixelMask::new(3, 1);
        mask.set(0, 0, flag::SATURATED);
        mask.set(1, 0, flag::SATURATED | flag::USER);
        let mut image = Rgba::filled(3, 1, [10, 10, 10, 255]);
        mask_overlay(&mut image, &mask, 1.0);
        assert_eq!(image.pixel(0, 0)[..3], [255, 255, 0]);
        assert_eq!(image.pixel(1, 0)[..3], [255, 0, 255]);
        assert_eq!(image.pixel(2, 0)[..3], [10, 10, 10]);
        assert_eq!(flag_colour(0), None);
        let mut other = Rgba::filled(2, 2, [1, 1, 1, 255]);
        mask_overlay(&mut other, &mask, 1.0);
        assert_eq!(other.pixel(0, 0)[..3], [1, 1, 1], "wrong size is ignored");
    }

    #[test]
    fn every_flag_has_a_legend_entry() {
        for bit in [
            flag::SATURATED,
            flag::BELOW_NOISE,
            flag::OUTSIDE_OUTLINE,
            flag::EDGE_BAND,
            flag::INCLUSION,
            flag::GHOST,
            flag::USER,
        ] {
            assert!(FLAG_COLOURS.iter().any(|(f, _, _)| *f == bit));
        }
    }

    #[test]
    fn polylines_draw_and_clip() {
        let mut image = Rgba::filled(10, 10, [0, 0, 0, 255]);
        draw_polyline(&mut image, &[[1.0, 1.0], [8.0, 1.0]], false, [255, 0, 0]);
        assert_eq!(image.pixel(1, 1)[..3], [255, 0, 0]);
        assert_eq!(image.pixel(5, 1)[..3], [255, 0, 0]);
        assert_eq!(image.pixel(5, 2)[..3], [0, 0, 0]);
        draw_polyline(
            &mut image,
            &[[-50.0, 5.0], [500.0, 5.0]],
            false,
            [0, 255, 0],
        );
        assert_eq!(image.pixel(0, 5)[..3], [0, 255, 0]);
        assert_eq!(image.pixel(9, 5)[..3], [0, 255, 0]);
        let mut closed = Rgba::filled(10, 10, [0, 0, 0, 255]);
        draw_polyline(
            &mut closed,
            &[[1.0, 1.0], [8.0, 1.0], [8.0, 8.0]],
            true,
            [9, 9, 9],
        );
        assert_eq!(closed.pixel(4, 4)[..3], [9, 9, 9], "the closing diagonal");
        draw_polyline(
            &mut closed,
            &[[f64::NAN, 0.0], [1.0, 1.0]],
            false,
            [1, 2, 3],
        );
    }

    #[test]
    fn discs_cover_a_round_patch() {
        let mut image = Rgba::filled(20, 20, [0, 0, 0, 255]);
        draw_disc(&mut image, [10.0, 10.0], 3.0, [255, 255, 255], 1.0);
        assert_eq!(image.pixel(10, 10)[0], 255);
        assert_eq!(image.pixel(10, 15)[0], 0);
        let lit = (0..20)
            .flat_map(|y| (0..20).map(move |x| (x, y)))
            .filter(|&(x, y)| image.pixel(x, y)[0] == 255)
            .count();
        assert!((24..=34).contains(&lit), "{lit}");
    }

    #[test]
    fn the_heat_ramp_runs_from_blue_to_red() {
        assert_eq!(heat_colour(0.0), [10, 10, 40]);
        assert_eq!(heat_colour(100.0), [220, 50, 40]);
        assert_eq!(heat_colour(f32::NAN), heat_colour(0.0));
        let blue = |d| i32::from(heat_colour(d)[2]);
        let red = |d| i32::from(heat_colour(d)[0]);
        assert!(blue(1.0) > blue(10.0));
        assert!(red(10.0) > red(1.0));
        let c = heat_colour(3.0);
        assert!(c[1] > 150, "{c:?}");
    }

    #[test]
    fn a_heat_image_greys_out_what_was_not_compared() {
        let image = heat_image(&[0.0, 5.0, 1.0], &[true, true, false], 3, 1);
        assert_eq!(image.pixel(2, 0)[..3], NOT_COMPARED);
        assert_ne!(image.pixel(0, 0)[..3], image.pixel(1, 0)[..3]);
        let short = heat_image(&[], &[], 2, 1);
        assert_eq!(short.pixel(0, 0)[..3], NOT_COMPARED);
    }

    #[test]
    fn cropping_follows_the_centre_and_stays_inside() {
        let mut image = Rgba::filled(8, 8, [0, 0, 0, 255]);
        image.set(6, 6, [255, 0, 0, 255]);
        let whole = crop_zoom(&image, [0.5, 0.5], 1);
        assert_eq!(whole, image);
        let corner = crop_zoom(&image, [1.0, 1.0], 2);
        assert_eq!((corner.width, corner.height), (4, 4));
        assert_eq!(corner.pixel(2, 2)[..3], [255, 0, 0]);
        let top = crop_zoom(&image, [0.0, 0.0], 2);
        assert_eq!(top.pixel(2, 2)[..3], [0, 0, 0]);
        let tiny = crop_zoom(&Rgba::filled(1, 1, [1, 2, 3, 255]), [0.5, 0.5], 8);
        assert_eq!(tiny.width, 1);
    }

    #[test]
    fn pngs_start_with_the_signature() {
        let bytes = to_png(&Rgba::filled(3, 2, [1, 2, 3, 255])).unwrap();
        assert_eq!(&bytes[..4], b"\x89PNG");
        assert!(to_png(&Rgba::filled(0, 0, [0; 4])).is_err());
    }
}
