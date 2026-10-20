//! The spectral attribution map as a transparent overlay layer, in the analytical overlay's
//! own palette.
//!
//! Display-referred and straight alpha like [`super::optical_overlay`]; it never enters
//! radiance or denoising. The three layer switches select the same things they do for the
//! analytical map (windowing, extinction, brilliance); surface glare is drawn as a grey
//! hatch whenever any of the three is on, and the fourth switch (the analytical map's Fire
//! slot, which a spectral map cannot attribute) draws an orange ring stipple on pixels whose
//! paths mostly scattered.

use super::{
    attribution_map::{
        AttributionMap, AttributionPixel, CLASS_GLARE, CLASS_KILLED, CLASS_RETURNED, CLASS_SHADOW,
        CLASS_WINDOWED,
    },
    optical_overlay::{OpticalPresentation, palette},
};

/// Which estimator the shares come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ShareMode {
    /// The fraction of samples (comparable to the analytical map's fractions).
    #[default]
    Probability,
    /// The fraction of delivered luminance.
    Energy,
}

/// The surface-glare color (a neutral grey no layer uses).
const GLARE_COLOR: [f32; 3] = [0.75, 0.75, 0.75];

/// The scattered-path ring color.
const SCATTER_COLOR: [f32; 3] = [1.0, 0.55, 0.05];

/// Pixels with fewer samples than this are drawn at half alpha ("Converging").
const CONVERGING_BELOW: u32 = 4;

/// A pixel shows the scattered ring when at least this share of its hits scattered.
const SCATTER_RING_SHARE: f32 = 0.5;

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
/// spanning `extent` projection pixels from the origin. The same mapping the analytical
/// overlay uses, so both sources draw the same patterns at the same beauty positions.
fn projection_coord(coord: usize, extent: u32, size: u32) -> u32 {
    let scaled = coord as u64 * u64::from(extent) / u64::from(size.max(1));
    u32::try_from(scaled).unwrap_or(u32::MAX)
}

/// The share of pixel `p` in class `k` under `share`.
fn class_share(p: &AttributionPixel, k: usize, share: ShareMode) -> f32 {
    match share {
        ShareMode::Probability => p.probability_share(k),
        ShareMode::Energy => p.energy_share(k),
    }
}

/// The summed share of the extinction classes (everything that is not glare, windowed or
/// returned).
fn extinction_share(p: &AttributionPixel, share: ShareMode) -> f32 {
    (CLASS_SHADOW..=CLASS_KILLED)
        .map(|k| class_share(p, k, share))
        .sum()
}

/// Whether projection pixel `(x, y)` is on the scattered-ring stipple: the eight pixels
/// around the centre of every 6 x 6 cell.
const fn on_scatter_ring(x: u32, y: u32) -> bool {
    let (dx, dy) = ((x % 6).abs_diff(3), (y % 6).abs_diff(3));
    let reach = if dx > dy { dx } else { dy };
    reach == 1
}

/// Generates a transparent separate layer for `map`.
///
/// `extent` is how many beauty pixels the map spans (the beauty size for a live frame, the
/// map's own size for an export), as for
/// [`optical_rgba_with_extent`](super::optical_overlay::optical_rgba_with_extent). Alpha is
/// the share of samples that met the stone, times the selected share and the opacity; a
/// pixel with fewer than four samples is drawn at half alpha. An empty or inconsistent map
/// gives an empty vector.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the overlay bytes are pinned by exact-byte tests and exports; a fused multiply-add could move a byte at a rounding boundary"
)]
pub fn attribution_rgba(
    map: &AttributionMap,
    style: OpticalPresentation,
    share: ShareMode,
    extent: [u32; 2],
) -> Vec<u8> {
    if map.width == 0
        || map.height == 0
        || (map.width as usize).checked_mul(map.height as usize) != Some(map.pixels.len())
    {
        return Vec::new();
    }
    let palette = palette(style.high_contrast);
    let any_class = style.layers[..3].iter().any(|&on| on);
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
        let x = projection_coord(i % width, extent[0], map.width);
        let y = projection_coord(i / width, extent[1], map.height);
        let shares = [
            class_share(p, CLASS_WINDOWED, share),
            extinction_share(p, share),
            class_share(p, CLASS_RETURNED, share),
        ];
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
            let layer_shade = if pattern { 0.6 } else { 1.0 };
            selected += shares[k];
            for c in 0..3 {
                rgb[c] += shares[k] * palette[k][c] * layer_shade;
            }
        }
        if any_class {
            let glare = class_share(p, CLASS_GLARE, share);
            let glare_shade = if (x + y).is_multiple_of(5) { 0.6 } else { 1.0 };
            selected += glare;
            for c in 0..3 {
                rgb[c] += glare * GLARE_COLOR[c] * glare_shade;
            }
        }
        if selected > 0.0 {
            for c in &mut rgb {
                *c /= selected;
            }
        }
        let scattered = p.scattered as f32 / p.hits as f32;
        if style.layers[3] && scattered >= SCATTER_RING_SHARE && on_scatter_ring(x, y) {
            rgb = SCATTER_COLOR;
            selected = selected.max(scattered);
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
        let coverage = p.hits as f32 / p.samples.max(1) as f32;
        let opacity = if style.map_mode {
            1.0
        } else {
            style.opacity.clamp(0.0, 1.0)
        };
        let converging = if p.samples < CONVERGING_BELOW {
            0.5
        } else {
            1.0
        };
        dst[3] = to_byte(255.0 * coverage * selected * opacity * converging);
    }
    out
}

/// The share of `total` in class `k` under `share`, as a whole percent.
fn percent(total: &AttributionPixel, k: usize, share: ShareMode) -> f32 {
    100.0 * class_share(total, k, share)
}

/// One line for the overlay's summary: the whole map's shares and its sample depth.
///
/// "W" is windowed, "R" returned, "E" every extinction class together, "Glare" the surface
/// reflection; the shares come from `share` (probability or energy). The sample figure is
/// the mean number of samples traced per map pixel.
#[must_use]
pub fn attribution_summary(map: &AttributionMap, share: ShareMode) -> String {
    let total = map.totals();
    let per_pixel = total.samples as f32 / map.pixels.len().max(1) as f32;
    let samples = per_pixel.round();
    if total.hits == 0 {
        return format!("Spectral Shares: None | {samples:.0} samples");
    }
    format!(
        "Spectral Shares: W {:.0}% \u{b7} R {:.0}% \u{b7} E {:.0}% \u{b7} Glare {:.0}% | {samples:.0} samples",
        percent(&total, CLASS_WINDOWED, share),
        percent(&total, CLASS_RETURNED, share),
        100.0 * extinction_share(&total, share),
        percent(&total, CLASS_GLARE, share),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        color::metrics::{ExtinctionReason, SpectralClass},
        renderer::attribution_map::Flags,
    };

    const SIZE: u32 = 7;
    const WINDOWED: Option<SpectralClass> = Some(SpectralClass::Windowed);
    const RETURNED: Option<SpectralClass> = Some(SpectralClass::Returned);
    const GLARE: Option<SpectralClass> = Some(SpectralClass::SurfaceGlare);

    fn blank() -> AttributionMap {
        AttributionMap::new(SIZE, SIZE)
    }

    /// Adds `hits` samples of `class` and `misses` misses to the pixel at `(x, y)`.
    fn fill(
        map: &mut AttributionMap,
        (x, y): (u32, u32),
        class: Option<SpectralClass>,
        (hits, misses): (u32, u32),
        flags: Flags,
    ) {
        for _ in 0..hits {
            map.add(y * SIZE + x, class, flags, 1.0);
        }
        for _ in 0..misses {
            map.add(y * SIZE + x, None, Flags::default(), 0.0);
        }
    }

    fn style(layers: [bool; 4], opacity: f32) -> OpticalPresentation {
        OpticalPresentation {
            layers,
            opacity,
            ..OpticalPresentation::default()
        }
    }

    fn at(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
        let i = ((y * SIZE + x) * 4) as usize;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    fn within(a: [u8; 4], b: [u8; 4]) -> bool {
        a.iter().zip(b).all(|(&p, q)| p.abs_diff(q) <= 1)
    }

    #[test]
    fn attribution_rgba_is_transparent_off_stone_and_scales_with_samples() {
        let mut map = blank();
        let none = Flags::default();
        fill(&mut map, (1, 0), WINDOWED, (4, 0), none);
        fill(&mut map, (3, 3), WINDOWED, (2, 2), none);
        fill(&mut map, (2, 2), WINDOWED, (2, 0), none);
        let rgba = attribution_rgba(
            &map,
            style([true, false, false, false], 1.0),
            ShareMode::Probability,
            [SIZE, SIZE],
        );
        assert_eq!(rgba.len(), (SIZE * SIZE * 4) as usize);
        // Full coverage, enough samples: the windowing palette color at full alpha.
        assert_eq!(at(&rgba, 1, 0), [0, 166, 255, 255]);
        // Half of the samples missed the stone: half the alpha, same color.
        assert_eq!(at(&rgba, 3, 3), [0, 166, 255, 128]);
        // Two samples are "converging": half alpha.
        assert_eq!(at(&rgba, 2, 2), [0, 166, 255, 128]);
        // Off the stone: fully transparent.
        assert_eq!(at(&rgba, 5, 5), [0, 0, 0, 0]);
    }

    #[test]
    fn layers_select_classes_and_glare_needs_a_class_layer() {
        let mut map = blank();
        let none = Flags::default();
        fill(&mut map, (1, 0), RETURNED, (4, 0), none);
        fill(&mut map, (1, 1), GLARE, (4, 0), none);
        let windowing = style([true, false, false, false], 0.5);
        let rgba = attribution_rgba(&map, windowing, ShareMode::Probability, [SIZE, SIZE]);
        assert_eq!(at(&rgba, 1, 0)[3], 0, "returned is not windowing");
        // Glare is grey and half transparent at opacity 0.5.
        assert_eq!(at(&rgba, 1, 1), [191, 191, 191, 128]);
        let none_on = style([false; 4], 0.5);
        let rgba = attribution_rgba(&map, none_on, ShareMode::Probability, [SIZE, SIZE]);
        assert!(rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 0));
    }

    #[test]
    fn probability_and_energy_shares_mix_the_palette_differently() {
        let mut map = blank();
        let none = Flags::default();
        // One returned sample worth 3, one windowed sample worth 1.
        map.add(SIZE + 4, RETURNED, none, 3.0);
        map.add(SIZE + 4, WINDOWED, none, 1.0);
        map.add(SIZE + 4, None, none, 0.0);
        map.add(SIZE + 4, None, none, 0.0);
        let layers = style([true, false, true, false], 1.0);
        let probability = attribution_rgba(&map, layers, ShareMode::Probability, [SIZE, SIZE]);
        let energy = attribution_rgba(&map, layers, ShareMode::Energy, [SIZE, SIZE]);
        // Half the samples hit, so alpha is 128 in both; the colors differ.
        assert!(within(at(&probability, 4, 1), [26, 191, 185, 128]));
        assert!(within(at(&energy, 4, 1), [38, 204, 150, 128]));
    }

    #[test]
    fn scattered_paths_draw_an_orange_ring_under_the_fourth_switch() {
        let mut map = blank();
        let scattered = Flags {
            scattered: true,
            fluoresced: false,
        };
        for pos in [(2, 3), (3, 3)] {
            fill(&mut map, pos, RETURNED, (4, 0), scattered);
        }
        let ring = style([false, false, false, true], 1.0);
        let rgba = attribution_rgba(&map, ring, ShareMode::Probability, [SIZE, SIZE]);
        assert_eq!(at(&rgba, 2, 3), [255, 140, 13, 255]);
        // The centre of the cell is not on the ring.
        assert_eq!(at(&rgba, 3, 3)[3], 0);
        // With the switch off nothing is drawn.
        let off = attribution_rgba(
            &map,
            style([false; 4], 1.0),
            ShareMode::Probability,
            [SIZE, SIZE],
        );
        assert!(off.as_chunks::<4>().0.iter().all(|p| p[3] == 0));
    }

    #[test]
    fn map_mode_fills_hit_pixels_and_an_inconsistent_map_draws_nothing() {
        let mut map = blank();
        fill(&mut map, (1, 0), RETURNED, (4, 0), Flags::default());
        let mut map_mode = style([true, false, false, false], 0.1);
        map_mode.map_mode = true;
        let rgba = attribution_rgba(&map, map_mode, ShareMode::Probability, [SIZE, SIZE]);
        // Nothing selected: the neutral grey at full alpha, as for the analytical map.
        assert_eq!(at(&rgba, 1, 0), [46, 46, 46, 255]);
        assert_eq!(at(&rgba, 5, 5), [0, 0, 0, 0]);
        map.pixels.pop();
        let broken = attribution_rgba(&map, map_mode, ShareMode::Probability, [SIZE, SIZE]);
        assert_eq!(broken.len(), 0);
    }

    #[test]
    fn the_summary_names_the_shares_and_the_sample_depth() {
        let mut map = AttributionMap::new(1, 1);
        let none = Flags::default();
        for _ in 0..2 {
            map.add(0, RETURNED, none, 1.0);
        }
        map.add(0, WINDOWED, none, 1.0);
        map.add(
            0,
            Some(SpectralClass::Extinct(ExtinctionReason::SideExit)),
            none,
            1.0,
        );
        assert_eq!(
            attribution_summary(&map, ShareMode::Probability),
            "Spectral Shares: W 25% \u{b7} R 50% \u{b7} E 25% \u{b7} Glare 0% | 4 samples"
        );
        let empty = AttributionMap::new(2, 2);
        assert_eq!(
            attribution_summary(&empty, ShareMode::Energy),
            "Spectral Shares: None | 0 samples"
        );
    }

    #[test]
    fn patterns_follow_the_projection_not_the_map_resolution() {
        let build = |size: u32| {
            let mut map = AttributionMap::new(size, size);
            for i in 0..size * size {
                for _ in 0..4 {
                    map.add(i, WINDOWED, Flags::default(), 1.0);
                }
            }
            map
        };
        let layers = style([true, true, true, false], 0.5);
        let draft = attribution_rgba(&build(8), layers, ShareMode::Probability, [32, 32]);
        let refined = attribution_rgba(&build(16), layers, ShareMode::Probability, [32, 32]);
        for y in 0..8usize {
            for x in 0..8usize {
                let d = (y * 8 + x) * 4;
                let r = ((2 * y) * 16 + 2 * x) * 4;
                assert_eq!(draft[d..d + 4], refined[r..r + 4], "pixel {x},{y}");
            }
        }
    }
}
