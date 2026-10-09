//! The stored result of the forward trace: path records per (view, pixel, bin).

use indicatrix::optics::zoning::MAX_ZONES;

use crate::rough_plan::photometry::WorkingGrid;

/// Lengths per record: the base zone and up to [`MAX_ZONES`] shaped zones.
pub const LENGTHS: usize = MAX_ZONES + 1;

/// One cluster of paths of a pixel and wavelength bin.
///
/// `lengths[z]` is the path length in mm inside zone `z` (0 is the base zone); `weight` is the
/// mean over the pixel's samples of the path throughput (Fresnel, microfacet and roulette
/// weights) times the relative light level at the exit. Records carry no absorption: for any
/// candidate absorption the transmitted share of the record is
/// `weight * exp(-sum_z alpha_z * lengths[z])`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathRecord {
    /// The path length in each zone, mm.
    pub lengths: [f32; LENGTHS],
    /// The weight, with the light level folded in and normalised by the sample count.
    pub weight: f32,
}

/// Bits of [`ViewRecords::status`].
pub mod status {
    /// The pixel was masked and not traced.
    pub const SKIPPED: u8 = 1;
    /// Dropped: more than the allowed share of the exit light came from the Lambertian mean
    /// (exit directions the white frame never saw).
    pub const FLAGGED_LIGHT: u8 = 1 << 1;
    /// Dropped: too much of the weight went through an inclusion.
    pub const INCLUSION: u8 = 1 << 2;
    /// Kept, but the Monte-Carlo error stayed above its target after the maximum samples; the
    /// fit should down-weight it by [`ViewRecords::mc_variance`](super::ViewRecords).
    pub const NOT_CONVERGED: u8 = 1 << 3;
    /// Any of these means the pixel has no records and must not be used.
    pub const DROPPED: u8 = SKIPPED | FLAGGED_LIGHT | INCLUSION;
}

/// The fractions of the paths a pixel lost, per sample and bin.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PixelLoss {
    /// Weight of paths cut off at the maximum depth.
    pub depth: f32,
    /// Weight of paths dropped by a microfacet sample on the wrong side, or by a damaged mesh.
    pub invalid: f32,
    /// Weight of paths removed because they passed an inclusion (before re-normalising).
    pub inclusion: f32,
    /// Share of the exit light that came from the Lambertian mean (flagged).
    pub flagged: f32,
}

/// One camera-ray evaluation point of the spectral integral: a wavelength with the weight that
/// turns the transmitted share at that wavelength into camera RGB.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EvalPoint {
    /// The traced bin whose records apply (always 0 when the index is not dispersive).
    pub bin: usize,
    /// The wavelength in nm.
    pub lambda_nm: f64,
    /// `S_c(lambda) * backlight(lambda) * d_lambda / reference_c`; the weights of all points
    /// sum to 1 per channel, so a transmittance of 1 everywhere gives camera RGB (1, 1, 1).
    pub weight: [f64; 3],
}

/// The wavelength setup of a trace.
#[derive(Debug, Clone, PartialEq)]
pub struct SpectralSetup {
    /// The first wavelength of the range, nm.
    pub lambda_min_nm: f64,
    /// The last wavelength of the range, nm.
    pub lambda_max_nm: f64,
    /// The number of spectral bins.
    pub bins: usize,
    /// How many bins were traced: `bins` for a dispersive stone, 1 otherwise (the paths do not
    /// depend on the wavelength then, so the records are shared).
    pub traced_bins: usize,
    /// The evaluation points, in bin order.
    pub eval_points: Vec<EvalPoint>,
}

/// Where the records came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheStatus {
    /// No cache directory was given.
    NotUsed,
    /// Read from the disk cache; nothing was traced.
    Hit,
    /// Traced and written to the cache.
    Stored,
    /// Traced; writing the cache failed (the result is still good).
    StoreFailed,
}

/// Counters of one trace.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ForwardStats {
    /// Whether the cache was used.
    pub cache: CacheStatus,
    /// Camera-ray samples traced, summed over pixels (each one path per traced bin).
    pub samples: u64,
    /// The largest per-zone length range inside one compressed cluster, mm. The transmittance
    /// error of the compression is about `(alpha * range / 2)^2 / 2` for the largest `alpha`.
    pub max_cluster_range_mm: f32,
}

/// The records of one view on its working grid.
#[derive(Debug, Clone, PartialEq)]
pub struct ViewRecords {
    /// The rig view index.
    pub view: usize,
    /// The working grid; pixel `(x, y)` has index `y * width + x`.
    pub grid: WorkingGrid,
    /// [`status`] bits per pixel.
    pub status: Vec<u8>,
    /// Samples used per pixel.
    pub samples: Vec<u16>,
    /// The Monte-Carlo estimate of the pixel's mean transmittance at the reference absorption
    /// ([`ForwardOptions::reference_alpha_per_mm`](super::ForwardOptions)), a scalar.
    pub mc_mean: Vec<f32>,
    /// The variance of that estimate (of the mean, not of one sample).
    pub mc_variance: Vec<f32>,
    /// What each pixel lost.
    pub loss: Vec<PixelLoss>,
    /// CSR offsets into `records`: the records of pixel `p`, traced bin `b` are
    /// `records[offsets[p * traced_bins + b] .. offsets[p * traced_bins + b + 1]]`.
    pub offsets: Vec<u32>,
    /// All records, pixel-major, then bin.
    pub records: Vec<PathRecord>,
}

impl ViewRecords {
    /// The number of working pixels.
    #[must_use]
    pub const fn pixel_count(&self) -> usize {
        self.grid.width * self.grid.height
    }

    /// Whether pixel `p` has usable records.
    #[must_use]
    pub fn is_valid(&self, p: usize) -> bool {
        self.status[p] & status::DROPPED == 0
    }

    /// The records of pixel `p` in traced bin `bin` (`traced_bins` is
    /// [`SpectralSetup::traced_bins`]).
    #[must_use]
    pub fn pixel_records(&self, p: usize, bin: usize, traced_bins: usize) -> &[PathRecord] {
        let i = p * traced_bins + bin;
        &self.records[self.offsets[i] as usize..self.offsets[i + 1] as usize]
    }

    /// The heap bytes held.
    #[must_use]
    pub const fn memory_bytes(&self) -> usize {
        self.status.len()
            + self.samples.len() * 2
            + (self.mc_mean.len() + self.mc_variance.len()) * 4
            + self.loss.len() * std::mem::size_of::<PixelLoss>()
            + self.offsets.len() * 4
            + self.records.len() * std::mem::size_of::<PathRecord>()
    }
}

/// The result of [`trace_rig`](super::trace_rig).
#[derive(Debug, Clone, PartialEq)]
pub struct ForwardRecords {
    /// The wavelength setup.
    pub spectral: SpectralSetup,
    /// How many zones the lengths use: 1 (the base) plus the shaped zones. Entries of
    /// `lengths` from here on are zero.
    pub n_zones: usize,
    /// One entry per requested view, in the order requested.
    pub views: Vec<ViewRecords>,
    /// Counters.
    pub stats: ForwardStats,
}

impl ForwardRecords {
    /// The heap bytes held by all views.
    ///
    /// Formula: per view, `pixels * (1 + 2 + 4 + 4 + 16 + 4 * traced_bins)` bytes of per-pixel
    /// data plus `24 * (records)` bytes, with `records <= pixels * traced_bins * max_records`
    /// (24 bytes per [`PathRecord`]).
    #[must_use]
    pub fn memory_bytes(&self) -> usize {
        self.views.iter().map(ViewRecords::memory_bytes).sum()
    }

    /// The mean share of the weight cut off at the maximum depth, over the valid pixels of all
    /// views (each pixel counts once).
    #[must_use]
    pub fn lost_depth_fraction(&self) -> f64 {
        let (mut sum, mut count) = (0.0_f64, 0_u64);
        for view in &self.views {
            for (p, loss) in view.loss.iter().enumerate() {
                if view.is_valid(p) {
                    sum += f64::from(loss.depth);
                    count += 1;
                }
            }
        }
        if count == 0 { 0.0 } else { sum / count as f64 }
    }

    /// The number of valid pixels over all views.
    #[must_use]
    pub fn valid_pixels(&self) -> usize {
        self.views
            .iter()
            .map(|v| (0..v.pixel_count()).filter(|&p| v.is_valid(p)).count())
            .sum()
    }
}
