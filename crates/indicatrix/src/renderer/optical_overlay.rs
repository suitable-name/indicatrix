//! Pure display-referred, straight-alpha transfer. Never enters radiance or denoising.
use crate::color::metrics::{OpticalImage, scintillation_index, scintillation_spatial};
/// Version 1 Fire scale: weighted separation 0..0.1 degrees, fixed across all poses.
pub const FIRE_RANGE_DEG: f32 = 0.1;
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OpticalPresentation {
    /// Windowing, extinction, brilliance, dispersion potential.
    pub layers: [bool; 4],
    pub opacity: f32,
    pub map_mode: bool,
    pub high_contrast: bool,
    /// The scintillation layer (a fifth channel beside `layers`); drawn only on a map that
    /// computed it.
    pub scintillation: bool,
}
impl Default for OpticalPresentation {
    fn default() -> Self {
        Self {
            layers: [true, false, false, false],
            opacity: 0.45,
            map_mode: false,
            high_contrast: false,
            scintillation: false,
        }
    }
}

/// The windowing, extinction and brilliance layer colors (linear 0..1 RGB).
#[must_use]
pub(crate) const fn palette(high_contrast: bool) -> [[f32; 3]; 3] {
    if high_contrast {
        [[0.0, 0.85, 1.0], [1.0, 0.45, 0.9], [0.4, 1.0, 0.6]]
    } else {
        [[0.0, 0.65, 1.0], [0.85, 0.3, 0.75], [0.2, 0.85, 0.45]]
    }
}

/// The scintillation layer color: a white-violet that no other layer uses.
const SCINTILLATION_RGB: [f32; 3] = [0.85, 0.8, 1.0];

/// Whether projection pixel `(x, y)` carries the scintillation diamond pattern: both
/// diagonals at period 5, distinct from Windowing's hatch (7) and Fire's stipple (3).
const fn on_scintillation_diamond(x: u32, y: u32) -> bool {
    (x + y).is_multiple_of(5) || (x + 5 - y % 5).is_multiple_of(5)
}

/// An 8-bit channel from a value already scaled to `0..=255`: rounded, saturating (NaN is 0).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to 0..=255 before the cast"
)]
const fn to_byte(scaled: f32) -> u8 {
    scaled.round().clamp(0.0, 255.0) as u8
}

/// The projection (beauty) pixel coordinate of map pixel `coord`: a map of `size` pixels
/// spanning `extent` projection pixels from `origin`.
fn projection_coord(coord: usize, extent: u32, size: u32, origin: u32) -> u32 {
    let scaled = coord as u64 * u64::from(extent) / u64::from(size.max(1));
    origin.saturating_add(u32::try_from(scaled).unwrap_or(u32::MAX))
}

/// Generates a transparent separate layer. Integer patterns distinguish classes without hue.
///
/// The patterns are laid out on the map's own pixels, offset by its origin: right for an
/// export (one map pixel per beauty pixel) and for its row tiles. A map sampled below the
/// beauty resolution needs [`optical_rgba_with_extent`] instead.
#[must_use]
pub fn optical_rgba(map: &OpticalImage, style: OpticalPresentation) -> Vec<u8> {
    optical_rgba_with_extent(map, style, [map.width, map.height])
}

/// [`optical_rgba`] with the patterns evaluated on beauty-image coordinates.
///
/// `extent` is how many beauty pixels the map spans (the full beauty size for a draft
/// sampled at a lower resolution; `[map.width, map.height]` for an export or a tile, which
/// is one map pixel per beauty pixel). A draft and its refinement then draw the same
/// hatching and stipple instead of re-patterning when the refined map replaces the draft.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the overlay bytes are pinned by exact-byte tests and exports; a fused multiply-add could move a byte at a rounding boundary"
)]
pub fn optical_rgba_with_extent(
    map: &OpticalImage,
    style: OpticalPresentation,
    extent: [u32; 2],
) -> Vec<u8> {
    if map.width == 0
        || map.height == 0
        || map.samples == 0
        || (map.width as usize).checked_mul(map.height as usize) != Some(map.pixels.len())
    {
        return Vec::new();
    }
    let palette = palette(style.high_contrast);
    // The spatial term needs the finished map, so it is computed once, and only when the
    // layer can be drawn at all.
    let scintillation =
        (style.scintillation && map.scint_computed).then(|| scintillation_spatial(map));
    let width = map.width as usize;
    let mut out = vec![0; map.pixels.len() * 4];
    for (i, (p, dst)) in map
        .pixels
        .iter()
        .zip(out.as_chunks_mut::<4>().0.iter_mut())
        .enumerate()
    {
        if p.hits == 0 {
            continue;
        }
        let x = projection_coord(i % width, extent[0], map.width, map.origin[0]);
        let y = projection_coord(i / width, extent[1], map.height, map.origin[1]);
        let shares = p.fractions();
        let mut rgb = [0.0; 3];
        let mut selected = 0.0;
        for k in 0..3 {
            if !style.layers[k] {
                continue;
            }
            let pattern = match k {
                0 => (x + y).is_multiple_of(7),
                1 => (x + 7 - y % 7).is_multiple_of(7) || (x + y).is_multiple_of(7),
                _ => x.is_multiple_of(7) && y.is_multiple_of(7),
            };
            let shade = if pattern { 0.6 } else { 1.0 };
            selected += shares[k];
            for c in 0..3 {
                rgb[c] += shares[k] * palette[k][c] * shade;
            }
        }
        if selected > 0.0 {
            for c in &mut rgb {
                *c /= selected;
            }
        }
        if style.layers[3] && map.fire_computed && x.is_multiple_of(3) && y.is_multiple_of(3) {
            let fire = (p.fire_deg() / FIRE_RANGE_DEG).clamp(0.0, 1.0);
            if fire > 0.0 {
                rgb = [1.0, 0.55 + 0.4 * fire, 0.05];
                selected = selected.max(fire);
            }
        }
        if let Some(spatial) = &scintillation
            && on_scintillation_diamond(x, y)
            && let Some(index) = scintillation_index(map, spatial, i)
        {
            let strength = (index / 100.0).clamp(0.0, 1.0);
            if strength > 0.0 {
                rgb = SCINTILLATION_RGB;
                selected = selected.max(strength);
            }
        }
        if style.map_mode {
            for c in &mut rgb {
                *c = *c * selected + 0.18 * (1.0 - selected);
            }
            selected = 1.0;
        }
        for c in 0..3 {
            dst[c] = to_byte(rgb[c] * 255.0);
        }
        let coverage = p.hits as f32 / map.samples as f32;
        let opacity = if style.map_mode {
            1.0
        } else {
            style.opacity.clamp(0.0, 1.0)
        };
        dst[3] = to_byte(255.0 * coverage * selected * opacity);
    }
    out
}
/// Source-over in display space. Preserve the beauty alpha: transparent backgrounds stay clear.
#[expect(
    clippy::suboptimal_flops,
    reason = "the blended bytes are pinned by exact-byte tests and exports; a fused multiply-add could move a byte at a rounding boundary"
)]
pub fn composite_optical(beauty: &mut [u8], overlay: &[u8]) {
    for (dst, src) in beauty
        .as_chunks_mut::<4>()
        .0
        .iter_mut()
        .zip(overlay.as_chunks::<4>().0.iter())
    {
        if dst[3] == 0 {
            continue;
        }
        let alpha = f32::from(src[3]) / 255.0;
        for c in 0..3 {
            dst[c] = to_byte(f32::from(src[c]) * alpha + f32::from(dst[c]) * (1.0 - alpha));
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::metrics::OpticalPixel;

    const SIZE: u32 = 7;

    fn pixel(hits: u32, classes: [u32; 4]) -> OpticalPixel {
        OpticalPixel {
            hits,
            classes,
            ..OpticalPixel::default()
        }
    }

    /// A 7x7 map of empty pixels (no hits) with four samples per pixel.
    fn blank_map() -> OpticalImage {
        OpticalImage {
            width: SIZE,
            height: SIZE,
            samples: 4,
            fire_computed: false,
            scint_computed: false,
            origin: [0, 0],
            pixels: vec![OpticalPixel::default(); (SIZE * SIZE) as usize],
            total: OpticalPixel::default(),
        }
    }

    fn put(map: &mut OpticalImage, x: u32, y: u32, p: OpticalPixel) {
        map.pixels[(y * SIZE + x) as usize] = p;
    }

    fn at(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    fn style(layers: [bool; 4], opacity: f32) -> OpticalPresentation {
        OpticalPresentation {
            layers,
            opacity,
            map_mode: false,
            high_contrast: false,
            scintillation: false,
        }
    }

    #[test]
    fn compositor_preserves_alpha_and_clean_export_is_identity() {
        let mut beauty = [3, 4, 5, 0, 20, 40, 80, 128];
        composite_optical(&mut beauty, &[255, 0, 0, 255, 255, 0, 0, 0]);
        assert_eq!(beauty, [3, 4, 5, 0, 20, 40, 80, 128]);
    }

    #[test]
    fn compositor_blends_in_display_space_and_keeps_the_beauty_alpha() {
        let mut beauty = [0, 0, 255, 255];
        composite_optical(&mut beauty, &[200, 100, 0, 128]);
        assert_eq!(beauty, [100, 50, 127, 255]);
    }

    #[test]
    fn palette_patterns_and_alpha_follow_the_spec() {
        let mut map = blank_map();
        // Windowing everywhere it matters: on the hatch line, off it, and a pixel the
        // stone does not cover.
        put(&mut map, 0, 0, pixel(4, [4, 0, 0, 0]));
        put(&mut map, 1, 0, pixel(4, [4, 0, 0, 0]));
        put(&mut map, 2, 2, pixel(4, [0, 4, 0, 0]));
        put(&mut map, 0, 3, pixel(4, [0, 0, 4, 0]));
        put(&mut map, 1, 3, pixel(4, [0, 0, 4, 0]));
        let windowing = optical_rgba(&map, style([true, false, false, false], 0.5));
        // Off the (x + y) % 7 hatch: the palette colour, alpha = round(255 * 0.5).
        assert_eq!(at(&windowing, 1, 0), [0, 166, 255, 128]);
        // On the hatch the colour is shaded by 0.6.
        assert_eq!(at(&windowing, 0, 0), [0, 99, 153, 128]);
        // Another class selects nothing under a windowing-only style.
        assert_eq!(at(&windowing, 2, 2), [0, 0, 0, 0]);
        // No hits: fully transparent, whatever the style.
        assert_eq!(at(&windowing, 5, 5), [0, 0, 0, 0]);

        let extinction = optical_rgba(&map, style([false, true, false, false], 0.5));
        assert_eq!(at(&extinction, 0, 0)[3], 0, "windowing is not extinction");
        assert_eq!(at(&extinction, 2, 2)[3], 128);

        // Brilliance dots only at x % 7 == 0 && y % 7 == 0: (0, 0) is a dot, (1, 3) and
        // (0, 3) are not.
        put(&mut map, 0, 0, pixel(4, [0, 0, 4, 0]));
        let brilliance = optical_rgba(&map, style([false, false, true, false], 0.5));
        assert_eq!(at(&brilliance, 0, 0), [31, 130, 69, 128]);
        assert_eq!(at(&brilliance, 1, 3), [51, 217, 115, 128]);
        assert_eq!(at(&brilliance, 0, 3), [51, 217, 115, 128]);
    }

    #[test]
    fn opacity_is_clamped_and_map_mode_ignores_it() {
        let mut map = blank_map();
        put(&mut map, 1, 0, pixel(4, [4, 0, 0, 0]));
        put(&mut map, 2, 2, pixel(4, [0, 4, 0, 0]));
        put(&mut map, 3, 3, pixel(2, [0, 2, 0, 0]));
        let layers = [true, false, false, false];
        assert_eq!(at(&optical_rgba(&map, style(layers, 3.0)), 1, 0)[3], 255);
        assert_eq!(at(&optical_rgba(&map, style(layers, -1.0)), 1, 0)[3], 0);

        let mut map_mode = style(layers, 0.1);
        map_mode.map_mode = true;
        let rgba = optical_rgba(&map, map_mode);
        // Selected pixel keeps its palette colour at full alpha.
        assert_eq!(at(&rgba, 1, 0), [0, 166, 255, 255]);
        // A hit with nothing selected is the neutral grey 0.18, also at full alpha.
        assert_eq!(at(&rgba, 2, 2), [46, 46, 46, 255]);
        // Coverage still scales alpha: two of four samples hit.
        assert_eq!(at(&rgba, 3, 3)[3], 128);
        // A pixel with no hits stays clear even in map mode.
        assert_eq!(at(&rgba, 5, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn fire_is_a_stipple_on_every_third_pixel_and_needs_computed_fire() {
        let mut map = blank_map();
        for y in 0..SIZE {
            for x in 0..SIZE {
                let mut p = pixel(4, [0, 0, 4, 0]);
                // Four samples summing 0.4 degrees: 0.1 degrees per hit sample, full scale.
                p.fire_sum_deg = 0.4;
                put(&mut map, x, y, p);
            }
        }
        let fire_only = style([false, false, false, true], 1.0);
        assert!(
            optical_rgba(&map, fire_only)
                .as_chunks::<4>()
                .0
                .iter()
                .all(|p| p[3] == 0),
            "without computed Fire nothing is drawn"
        );
        map.fire_computed = true;
        let rgba = optical_rgba(&map, fire_only);
        let mut stippled = 0;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let px = at(&rgba, x, y);
                if x % 3 == 0 && y % 3 == 0 {
                    assert_eq!(px, [255, 242, 13, 255], "stipple at {x},{y}");
                    stippled += 1;
                } else {
                    assert_eq!(px[3], 0, "no Fire off the stipple at {x},{y}");
                }
            }
        }
        assert_eq!(stippled, 9);
    }

    /// A draft sampled at 64 pixels and its refinement at 128 draw the same hatching at the
    /// same beauty positions when both are laid out on the 256-pixel projection.
    #[test]
    fn patterns_follow_the_projection_not_the_map_resolution() {
        let build = |size: u32| OpticalImage {
            width: size,
            height: size,
            samples: 4,
            fire_computed: false,
            scint_computed: false,
            origin: [0, 0],
            pixels: vec![pixel(4, [4, 0, 0, 0]); (size * size) as usize],
            total: OpticalPixel::default(),
        };
        let layers = style([true, true, true, false], 0.5);
        let draft = optical_rgba_with_extent(&build(64), layers, [256, 256]);
        let refined = optical_rgba_with_extent(&build(128), layers, [256, 256]);
        for y in 0..64usize {
            for x in 0..64usize {
                let d = (y * 64 + x) * 4;
                let r = ((2 * y) * 128 + 2 * x) * 4;
                assert_eq!(draft[d..d + 4], refined[r..r + 4], "pixel {x},{y}");
            }
        }
        // And the default entry point keeps the map-pixel layout of exports and tiles.
        assert_eq!(
            optical_rgba(&build(64), layers),
            optical_rgba_with_extent(&build(64), layers, [64, 64])
        );
    }

    /// A map whose every pixel is uniformly brilliant (spatial term 0) and returned light at
    /// two of five yaw poses (temporal term at its cap), so the index is 40 everywhere.
    fn scintillating_map() -> OpticalImage {
        let mut map = blank_map();
        map.scint_computed = true;
        for p in &mut map.pixels {
            *p = pixel(4, [0, 0, 4, 0]);
            p.scint_returned = 2;
            p.scint_poses = 5;
        }
        map
    }

    fn scintillation_style(layers: [bool; 4]) -> OpticalPresentation {
        OpticalPresentation {
            scintillation: true,
            ..style(layers, 1.0)
        }
    }

    #[test]
    fn scintillation_bytes_only_where_computed() {
        let map = scintillating_map();
        let rgba = optical_rgba(&map, scintillation_style([false; 4]));
        let mut diamonds = 0;
        for y in 0..SIZE {
            for x in 0..SIZE {
                let on = (x + y) % 5 == 0 || (x + 5 - y % 5) % 5 == 0;
                if on {
                    // White-violet at the index (40) as alpha.
                    assert_eq!(at(&rgba, x, y), [217, 204, 255, 102], "diamond at {x},{y}");
                    diamonds += 1;
                } else {
                    assert_eq!(at(&rgba, x, y), [0, 0, 0, 0], "no pattern at {x},{y}");
                }
            }
        }
        assert!(diamonds > 10);

        // The flag off, or a map that did not compute the term: nothing is drawn.
        assert!(
            optical_rgba(&map, style([false; 4], 1.0))
                .iter()
                .all(|b| *b == 0)
        );
        let mut not_computed = scintillating_map();
        not_computed.scint_computed = false;
        assert!(
            optical_rgba(&not_computed, scintillation_style([false; 4]))
                .iter()
                .all(|b| *b == 0)
        );
        // A pixel without poses (centre ray missed at the base pose) is never drawn.
        let mut gap = scintillating_map();
        gap.pixels[0].scint_poses = 0;
        gap.pixels[0].scint_returned = 0;
        assert_eq!(
            at(&optical_rgba(&gap, scintillation_style([false; 4])), 0, 0),
            [0, 0, 0, 0]
        );
    }

    #[test]
    fn scintillation_leaves_the_four_existing_layers_untouched_off_the_diamond() {
        let map = scintillating_map();
        for layers in [
            [true, false, false, false],
            [false, false, true, false],
            [true, true, true, false],
        ] {
            let without = optical_rgba(&map, style(layers, 0.5));
            let with = optical_rgba(
                &map,
                OpticalPresentation {
                    scintillation: true,
                    ..style(layers, 0.5)
                },
            );
            for y in 0..SIZE {
                for x in 0..SIZE {
                    if !((x + y) % 5 == 0 || (x + 5 - y % 5) % 5 == 0) {
                        assert_eq!(at(&with, x, y), at(&without, x, y), "{layers:?} at {x},{y}");
                    }
                }
            }
        }
    }

    #[test]
    fn the_palette_function_is_the_former_inline_table() {
        assert_eq!(
            palette(false),
            [[0.0, 0.65, 1.0], [0.85, 0.3, 0.75], [0.2, 0.85, 0.45]]
        );
        assert_eq!(
            palette(true),
            [[0.0, 0.85, 1.0], [1.0, 0.45, 0.9], [0.4, 1.0, 0.6]]
        );
    }
}
