//! Per-pixel attribution of the spectral renderer's own samples.
//!
//! Every traced sample is labelled by `color::metrics::classify_path` and credited to its
//! pixel as a count and as its delivered luminance. Nothing here touches radiance or
//! denoising: the map is a side record of the same samples, kept only while the overlay
//! asks for it.
//!
//! Writers own disjoint pixels (one thread per row), so no merge is needed; the sums are
//! added in sample order within a pixel and are therefore the same for any thread count.

use crate::{
    color::metrics::{ExtinctionReason, MAX_DIAGNOSTIC_PIXELS, SpectralClass},
    optics::raytracer::PathEvents,
};

/// Classes a pixel keeps: see the `CLASS_*` indices.
pub const ATTRIBUTION_CLASSES: usize = 9;

/// Surface glare (the specular reflection off the outside of the stone).
pub const CLASS_GLARE: usize = 0;
/// Out through a pavilion facet.
pub const CLASS_WINDOWED: usize = 1;
/// Up and out, visibly returned to the observer.
pub const CLASS_RETURNED: usize = 2;
/// Exited upward into the observer's own head shadow.
pub const CLASS_SHADOW: usize = 3;
/// Exited upward but not towards a light.
pub const CLASS_UNLIT: usize = 4;
/// Exited neither up nor down.
pub const CLASS_SIDE_EXIT: usize = 5;
/// Left the stone without an exit event, plus any reason without a class of its own.
pub const CLASS_NO_INTERSECTION: usize = 6;
/// Still inside when the bounce budget ran out.
pub const CLASS_BUDGET: usize = 7;
/// Killed by Russian roulette or absorbed by a scattering event.
pub const CLASS_KILLED: usize = 8;

/// The slot of `class` in a pixel's arrays.
#[must_use]
pub const fn class_index(class: SpectralClass) -> usize {
    match class {
        SpectralClass::SurfaceGlare => CLASS_GLARE,
        SpectralClass::Windowed => CLASS_WINDOWED,
        SpectralClass::Returned => CLASS_RETURNED,
        SpectralClass::Extinct(reason) => match reason {
            ExtinctionReason::ObserverShadow => CLASS_SHADOW,
            ExtinctionReason::InsufficientIllumination => CLASS_UNLIT,
            ExtinctionReason::SideExit => CLASS_SIDE_EXIT,
            ExtinctionReason::BounceBudget => CLASS_BUDGET,
            ExtinctionReason::RussianRoulette | ExtinctionReason::ScatterAbsorbed => CLASS_KILLED,
            ExtinctionReason::NoIntersection
            | ExtinctionReason::EntryBlocked
            | ExtinctionReason::Invalid
            | ExtinctionReason::None => CLASS_NO_INTERSECTION,
        },
    }
}

/// Flags carried beside the class of one sample.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Flags {
    /// A Henyey-Greenstein scattering event happened on the path.
    pub scattered: bool,
    /// The fluorescence vertex fired.
    pub fluoresced: bool,
}

impl Flags {
    /// The flags `events` recorded.
    #[must_use]
    pub const fn from_events(events: &PathEvents) -> Self {
        Self {
            scattered: events.scattered,
            fluoresced: events.fluoresced,
        }
    }
}

/// What the samples of one pixel did.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AttributionPixel {
    /// Samples traced, including the ones that missed the stone.
    pub samples: u32,
    /// Samples whose camera ray met the stone (the sum of `counts`).
    pub hits: u32,
    /// Hits per class, see the `CLASS_*` indices.
    pub counts: [u32; ATTRIBUTION_CLASSES],
    /// Delivered luminance (the sample's `xyz.y`) per class; non-finite values are dropped.
    pub y_sum: [f32; ATTRIBUTION_CLASSES],
    /// Sum of squared luminance per class, for the energy share's standard error.
    pub y_sq_sum: [f32; ATTRIBUTION_CLASSES],
    /// Hits whose path scattered.
    pub scattered: u32,
    /// Hits whose path fluoresced.
    pub fluoresced: u32,
}

impl AttributionPixel {
    /// Credits one sample: `class` is `None` for a camera ray that missed the stone.
    ///
    /// A non-finite `y` still counts as a hit of its class, the renderer's own rule
    /// (`add_finite_sample` drops the value, not the sample).
    #[expect(
        clippy::suboptimal_flops,
        reason = "the plain `+= y * y` keeps the pinned attribution sums bit-identical"
    )]
    pub fn add(&mut self, class: Option<SpectralClass>, flags: Flags, y: f32) {
        self.samples = self.samples.saturating_add(1);
        let Some(class) = class else {
            return;
        };
        let k = class_index(class);
        self.hits = self.hits.saturating_add(1);
        self.counts[k] = self.counts[k].saturating_add(1);
        if y.is_finite() {
            self.y_sum[k] += y;
            self.y_sq_sum[k] += y * y;
        }
        self.scattered += u32::from(flags.scattered);
        self.fluoresced += u32::from(flags.fluoresced);
    }

    /// Adds every count and sum of `other` (used for the whole-image totals).
    fn merge(&mut self, other: &Self) {
        self.samples = self.samples.saturating_add(other.samples);
        self.hits = self.hits.saturating_add(other.hits);
        for (a, b) in self.counts.iter_mut().zip(other.counts) {
            *a = a.saturating_add(b);
        }
        for (a, b) in self.y_sum.iter_mut().zip(other.y_sum) {
            *a += b;
        }
        for (a, b) in self.y_sq_sum.iter_mut().zip(other.y_sq_sum) {
            *a += b;
        }
        self.scattered = self.scattered.saturating_add(other.scattered);
        self.fluoresced = self.fluoresced.saturating_add(other.fluoresced);
    }

    /// The share of hits in class `k`, 0 to 1 (0 with no hits).
    #[must_use]
    pub const fn probability_share(&self, k: usize) -> f32 {
        if self.hits == 0 {
            return 0.0;
        }
        self.counts[k] as f32 / self.hits as f32
    }

    /// The share of delivered luminance in class `k`, 0 to 1 (0 when nothing was delivered).
    #[must_use]
    pub fn energy_share(&self, k: usize) -> f32 {
        let total: f32 = self.y_sum.iter().sum();
        if total > 0.0 {
            self.y_sum[k] / total
        } else {
            0.0
        }
    }

    /// The binomial standard error of the probability share of class `k`.
    #[must_use]
    pub fn probability_error(&self, k: usize) -> f32 {
        if self.hits == 0 {
            return 0.0;
        }
        let p = self.probability_share(k);
        (p * (1.0 - p) / self.hits as f32).sqrt()
    }

    /// The standard error of the mean luminance credited to class `k` per hit, in the same
    /// units as `y_sum / hits` (0 with fewer than two hits).
    #[must_use]
    pub fn energy_error(&self, k: usize) -> f32 {
        if self.hits < 2 {
            return 0.0;
        }
        let n = self.hits as f32;
        let mean = self.y_sum[k] / n;
        let variance = (-mean).mul_add(mean, self.y_sq_sum[k] / n).max(0.0) * n / (n - 1.0);
        (variance / n).sqrt()
    }
}

/// The attribution of one frame (or export tile), one [`AttributionPixel`] per image pixel.
#[derive(Clone, Debug)]
pub struct AttributionMap {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// [`crate::color::metrics::SPECTRAL_ATTRIBUTION_VERSION`] the labels were made under.
    pub version: u32,
    /// Row-major pixels, `width * height` of them.
    pub pixels: Vec<AttributionPixel>,
}

impl AttributionMap {
    /// A cleared map of `width` x `height`, or `None` for an empty or oversized one
    /// (more than [`MAX_DIAGNOSTIC_PIXELS`] pixels).
    #[must_use]
    pub fn checked(width: u32, height: u32) -> Option<Self> {
        let count = (width as usize).checked_mul(height as usize)?;
        if count == 0 || count > MAX_DIAGNOSTIC_PIXELS {
            return None;
        }
        Some(Self {
            width,
            height,
            version: crate::color::metrics::SPECTRAL_ATTRIBUTION_VERSION,
            pixels: vec![AttributionPixel::default(); count],
        })
    }

    /// [`Self::checked`], with an empty `0 x 0` map standing in for an invalid size (an
    /// empty map draws nothing and accepts no sample).
    #[must_use]
    pub fn new(width: u32, height: u32) -> Self {
        Self::checked(width, height).unwrap_or(Self {
            width: 0,
            height: 0,
            version: crate::color::metrics::SPECTRAL_ATTRIBUTION_VERSION,
            pixels: Vec::new(),
        })
    }

    /// Credits one sample to `pixel` (row-major index); an index outside the map is ignored.
    pub fn add(&mut self, pixel: u32, class: Option<SpectralClass>, flags: Flags, y: f32) {
        if let Some(p) = self.pixels.get_mut(pixel as usize) {
            p.add(class, flags, y);
        }
    }

    /// Forgets every sample, keeping the size.
    pub fn clear(&mut self) {
        self.pixels.fill(AttributionPixel::default());
    }

    /// The sum over all pixels, in row-major order.
    #[must_use]
    pub fn totals(&self) -> AttributionPixel {
        let mut total = AttributionPixel::default();
        for p in &self.pixels {
            total.merge(p);
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RETURNED: Option<SpectralClass> = Some(SpectralClass::Returned);
    const WINDOWED: Option<SpectralClass> = Some(SpectralClass::Windowed);

    fn map_with_samples() -> AttributionMap {
        let mut map = AttributionMap::new(2, 2);
        let flags = Flags::default();
        map.add(0, RETURNED, flags, 2.0);
        map.add(0, WINDOWED, flags, 1.0);
        map.add(0, None, flags, 5.0);
        map.add(
            1,
            Some(SpectralClass::Extinct(ExtinctionReason::RussianRoulette)),
            Flags {
                scattered: true,
                fluoresced: false,
            },
            0.5,
        );
        map.add(3, Some(SpectralClass::SurfaceGlare), flags, f32::NAN);
        map
    }

    #[test]
    fn an_invalid_size_gives_an_empty_map_that_ignores_samples() {
        for (w, h) in [(0, 4), (4, 0), (2_000, 2_000)] {
            assert!(AttributionMap::checked(w, h).is_none());
            let mut map = AttributionMap::new(w, h);
            assert_eq!(map.pixels.len(), 0);
            map.add(0, RETURNED, Flags::default(), 1.0);
            assert_eq!(map.totals().samples, 0);
        }
        assert!(AttributionMap::checked(1024, 1024).is_some());
    }

    #[test]
    fn classes_conserve_hits() {
        let map = map_with_samples();
        for p in &map.pixels {
            assert_eq!(p.counts.iter().sum::<u32>(), p.hits);
        }
        let total = map.totals();
        assert_eq!((total.samples, total.hits), (5, 4));
        assert_eq!(total.counts.iter().sum::<u32>(), total.hits);
        // The miss counts as a sample but credits no luminance anywhere.
        let delivered: f32 = total.y_sum.iter().sum();
        assert!((delivered - 3.5).abs() <= 1e-6 * 3.5, "{delivered}");
        assert_eq!(total.scattered, 1);
    }

    #[test]
    fn a_non_finite_luminance_is_counted_but_not_summed() {
        let map = map_with_samples();
        let p = &map.pixels[3];
        assert_eq!(p.counts[CLASS_GLARE], 1);
        assert_eq!(p.y_sum[CLASS_GLARE].to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn extinction_reasons_land_in_their_own_slots() {
        use ExtinctionReason as R;
        let slot = |r| class_index(SpectralClass::Extinct(r));
        assert_eq!(slot(R::ObserverShadow), CLASS_SHADOW);
        assert_eq!(slot(R::InsufficientIllumination), CLASS_UNLIT);
        assert_eq!(slot(R::SideExit), CLASS_SIDE_EXIT);
        assert_eq!(slot(R::BounceBudget), CLASS_BUDGET);
        assert_eq!(slot(R::RussianRoulette), CLASS_KILLED);
        assert_eq!(slot(R::ScatterAbsorbed), CLASS_KILLED);
        assert_eq!(slot(R::NoIntersection), CLASS_NO_INTERSECTION);
        assert_eq!(slot(R::Invalid), CLASS_NO_INTERSECTION);
    }

    #[test]
    fn shares_sum_to_one() {
        let map = map_with_samples();
        let p = &map.pixels[0];
        let probability: f32 = (0..ATTRIBUTION_CLASSES)
            .map(|k| p.probability_share(k))
            .sum();
        let energy: f32 = (0..ATTRIBUTION_CLASSES).map(|k| p.energy_share(k)).sum();
        assert!((probability - 1.0).abs() <= 1e-6, "{probability}");
        assert!((energy - 1.0).abs() <= 1e-6, "{energy}");
        assert!((p.energy_share(CLASS_RETURNED) - 2.0 / 3.0).abs() <= 1e-6);
        // An empty pixel has no shares and no error.
        let empty = AttributionPixel::default();
        assert_eq!(empty.probability_share(CLASS_RETURNED), 0.0);
        assert_eq!(empty.energy_share(CLASS_RETURNED), 0.0);
        assert_eq!(empty.probability_error(CLASS_RETURNED), 0.0);
        assert_eq!(empty.energy_error(CLASS_RETURNED), 0.0);
    }

    #[test]
    fn the_standard_errors_follow_the_textbook_forms() {
        let mut p = AttributionPixel::default();
        for y in [1.0, 3.0, 1.0, 3.0] {
            p.add(RETURNED, Flags::default(), y);
        }
        // p = 1, so the binomial error is zero.
        assert_eq!(p.probability_error(CLASS_RETURNED), 0.0);
        // Mean 2, sample variance 4/3, standard error sqrt(1/3).
        let expected = (1.0f32 / 3.0).sqrt();
        assert!((p.energy_error(CLASS_RETURNED) - expected).abs() <= 1e-5);
        p.add(WINDOWED, Flags::default(), 1.0);
        let binomial = (0.8f32 * 0.2 / 5.0).sqrt();
        assert!((p.probability_error(CLASS_RETURNED) - binomial).abs() <= 1e-6);
    }

    #[test]
    fn clear_forgets_the_samples_and_keeps_the_size() {
        let mut map = map_with_samples();
        map.clear();
        assert_eq!((map.width, map.height, map.pixels.len()), (2, 2, 4));
        assert_eq!(map.totals(), AttributionPixel::default());
    }

    #[test]
    fn sums_do_not_depend_on_how_rows_were_split_between_writers() {
        // Two rows written by separate owners equal the same samples added in one pass.
        let samples = [(0u32, 1.5f32), (1, 0.25), (0, 2.0), (1, 4.0)];
        let mut together = AttributionMap::new(1, 2);
        for (row, y) in samples {
            together.add(row, RETURNED, Flags::default(), y);
        }
        let mut split = AttributionMap::new(1, 2);
        for (row, pixel) in split.pixels.iter_mut().enumerate() {
            for (r, y) in samples {
                if r as usize == row {
                    pixel.add(RETURNED, Flags::default(), y);
                }
            }
        }
        assert_eq!(together.pixels, split.pixels);
    }
}
