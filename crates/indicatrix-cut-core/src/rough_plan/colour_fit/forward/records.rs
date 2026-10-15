//! The stored result of the forward trace: path records per (view, pixel, bin).

use indicatrix::optics::zoning::MAX_ZONES;

use crate::rough_plan::photometry::WorkingGrid;

/// Lengths per record: the base zone and up to [`MAX_ZONES`] shaped zones.
pub const LENGTHS: usize = MAX_ZONES + 1;

/// The code of a neutral (grey) chroma factor of a stored record: exactly 1.
pub const CHROMA_NEUTRAL: u8 = 127;

/// The largest code of a chroma factor (codes `0..=254`; 255 is never written).
pub const CHROMA_MAX_CODE: u8 = 254;

/// The widest chroma factor a record can carry: the factors span `1 / 1.5 ..= 1.5`.
pub const CHROMA_LIMIT: f64 = 1.5;

/// `ln(CHROMA_LIMIT)`.
const CHROMA_LOG_SPAN: f64 = 0.405_465_108_108_164_4;

/// The largest relative error of a stored chroma factor inside `1 / 1.5 ..= 1.5`: half a code step.
///
/// The 255 codes are log-spaced, `factor = 1.5^((code - 127) / 127)`, so neighbouring factors
/// differ by `1.5^(1/127) - 1 = 0.3197 %` and rounding to the nearest code errs by at most half of
/// that: 0.160 % (ZC-13). Code 127 is exactly 1.
pub const CHROMA_MAX_REL_ERROR: f64 = 0.001_6;

/// The factor a chroma code stands for (`code` must be at most [`CHROMA_MAX_CODE`]).
///
/// Evaluation reads these through a table built once per evaluation, so the value never depends on
/// the thread or on the record.
#[must_use]
pub fn chroma_decode(code: u8) -> f64 {
    ((f64::from(code) - f64::from(CHROMA_NEUTRAL)) / f64::from(CHROMA_NEUTRAL) * CHROMA_LOG_SPAN)
        .exp()
}

/// The code nearest to the factor `chroma` (in the log scale).
///
/// Saturates at `1 / 1.5` and `1.5`;
/// a factor that is not positive (a dead channel) gets code 0. See [`chroma_in_range`] to tell
/// whether the factor was representable.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "steps are clamped to -limit..=limit and shifted by limit, so the code is in 0..=2 * limit"
)]
pub fn chroma_encode(chroma: f64) -> u8 {
    if chroma.is_nan() || chroma <= 0.0 {
        return 0;
    }
    let steps = (chroma.ln() / CHROMA_LOG_SPAN * f64::from(CHROMA_NEUTRAL)).round();
    let limit = f64::from(CHROMA_NEUTRAL);
    (steps.clamp(-limit, limit) + limit) as u8
}

/// Whether `chroma` lies inside the stored range `1 / 1.5 ..= 1.5`.
#[must_use]
pub fn chroma_in_range(chroma: f64) -> bool {
    (1.0 / CHROMA_LIMIT..=CHROMA_LIMIT).contains(&chroma)
}

/// One cluster of paths of a pixel and wavelength bin.
///
/// `lengths[z]` is the path length in mm inside zone `z` (0 is the base zone); `weight` is the
/// mean over the pixel's samples of the path throughput (Fresnel, microfacet and roulette
/// weights) times the relative light level at the exit, averaged over the three colour channels.
/// `chroma[c]` is the factor by which channel `c` differs from that average (the exit light of an
/// edge-lit panel is not grey, ZC-13): the weight of the record in channel `c` is
/// `weight * chroma[c]`. Records carry no absorption: for any candidate absorption the
/// transmitted share of the record in channel `c` is
/// `weight * chroma[c] * exp(-sum_z alpha_z * lengths[z])`.
///
/// This is the working form (compression, tests, [`RecordSlice::get`]). A [`ViewRecords`] stores
/// the records flat, `n_zones` lengths and the weight per record, plus three one-byte chroma codes
/// per record when the light is not grey (see [`ViewRecords::chroma`]), so a uniform stone costs
/// 8 bytes per record on a grey panel and 11 on a coloured one, a four-zone stone 24 and 27 (ZC-8);
/// entries of `lengths` from `n_zones` on are zero.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PathRecord {
    /// The path length in each zone, mm.
    pub lengths: [f32; LENGTHS],
    /// The weight, with the channel-mean light level folded in and normalised by the sample count.
    pub weight: f32,
    /// The per-channel factor of the weight (red, green, blue); `[1.0; 3]` for a grey light.
    /// From the quantised store ([`RecordSlice::get`]) it carries the stored codes' values.
    pub chroma: [f32; 3],
}

/// The records of one (pixel, traced bin): a view into the flat store of a [`ViewRecords`].
#[derive(Debug, Clone, Copy)]
pub struct RecordSlice<'a> {
    data: &'a [f32],
    stride: usize,
    /// Three chroma codes per record, or empty when the view's light is grey.
    chroma: &'a [u8],
}

impl<'a> RecordSlice<'a> {
    /// The number of records.
    #[must_use]
    pub const fn len(&self) -> usize {
        // `stride` is at least 2 (one length and the weight); a zero stride only comes from a
        // hand-built empty slice and then there is nothing to count.
        match self.data.len().checked_div(self.stride) {
            Some(count) => count,
            None => 0,
        }
    }

    /// Whether there are no records.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Record `i`; panics when `i >= len()`.
    #[must_use]
    pub fn get(&self, i: usize) -> PathRecord {
        let record = &self.data[i * self.stride..(i + 1) * self.stride];
        let zones = self.stride - 1;
        let mut lengths = [0.0_f32; LENGTHS];
        lengths[..zones].copy_from_slice(&record[..zones]);
        #[expect(
            clippy::cast_possible_truncation,
            reason = "f32 is the record storage precision; decoded chroma is within 1 / 1.5 ..= 1.5"
        )]
        let chroma = self.chroma.get(3 * i..3 * i + 3).map_or([1.0; 3], |codes| {
            [
                chroma_decode(codes[0]) as f32,
                chroma_decode(codes[1]) as f32,
                chroma_decode(codes[2]) as f32,
            ]
        });
        PathRecord {
            lengths,
            weight: record[zones],
            chroma,
        }
    }

    /// The records in order.
    pub fn iter(&self) -> impl Iterator<Item = PathRecord> + '_ {
        (0..self.len()).map(|i| self.get(i))
    }

    /// The raw records, `n_zones` lengths and then the weight each (the evaluator's hot loop).
    pub(super) fn raw(&self) -> std::slice::ChunksExact<'a, f32> {
        self.data.chunks_exact(self.stride.max(1))
    }

    /// The chroma codes, three per record in record order; empty when every record of the view is
    /// grey (the evaluator then takes the weight alone for all channels).
    pub(super) const fn chroma_codes(&self) -> &'a [u8] {
        self.chroma
    }
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
    /// Share of the exit light that came from the Lambertian mean (flagged), or whose colour
    /// differed from the camera ray's own pixel by more than the stored chroma range (a factor
    /// outside `1 / 1.5 ..= 1.5` in some channel, [`CHROMA_LIMIT`]).
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
    /// How many bins were traced: `bins` for a dispersive stone when
    /// [`ForwardOptions::trace_dispersion`](super::ForwardOptions) is set, 1 otherwise (the paths
    /// are traced at one index, the d-line index of a dispersive stone, and the records are
    /// shared by all bins).
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
    /// The Monte-Carlo estimate of the pixel's mean transmittance at the pixel's reference
    /// absorption `alpha_ref = 1 / mean path length` (optical depth 1 at the weighted mean path
    /// of the pixel's first batch of samples), a scalar. Zero for dropped pixels.
    pub mc_mean: Vec<f32>,
    /// The variance of that estimate (of the mean, not of one sample).
    pub mc_variance: Vec<f32>,
    /// The Monte-Carlo estimate of the pixel's mean transmittance of the clear stone (absorption
    /// 0) under the channel MEAN of the light: the sum of the record weights. It is the quantity
    /// the adaptive sampling converges on. For a coloured panel (`chroma` non-empty) the clear
    /// prediction of channel `c` is this times the record-weighted mean factor of that channel
    /// (within the codec range, a few percent); the exact per-channel figure is `evaluate` at
    /// zero absorption.
    pub mc_mean_clear: Vec<f32>,
    /// The variance of that estimate (of the mean, not of one sample).
    pub mc_variance_clear: Vec<f32>,
    /// What each pixel lost.
    pub loss: Vec<PixelLoss>,
    /// CSR offsets into the records (counted in records, not floats): the records of pixel `p`,
    /// traced bin `b` are record numbers from `offsets[p * traced_bins + b]` up to (not
    /// including) `offsets[p * traced_bins + b + 1]`.
    pub offsets: Vec<u32>,
    /// Floats per stored record: `n_zones` lengths and the weight.
    pub stride: usize,
    /// All records, pixel-major, then bin, flat: `stride` floats per record (the lengths of the
    /// zones in use, then the weight).
    pub records: Vec<f32>,
    /// The per-channel colour of the records' light (ZC-13): three codes (red, green, blue) per
    /// record, in record order, each standing for the factor [`chroma_decode`] by which the
    /// record's weight is multiplied in that channel. **Empty when every code is
    /// [`CHROMA_NEUTRAL`]** (a grey panel, a uniform surround, a frame whose colour varies by less
    /// than the quantisation): such a view costs nothing and evaluates exactly as before. The
    /// codes are log-spaced over `1 / 1.5 ..= 1.5`, so a stored factor is within
    /// [`CHROMA_MAX_REL_ERROR`] (0.16 %) of the traced one.
    pub chroma: Vec<u8>,
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
    pub fn pixel_records(&self, p: usize, bin: usize, traced_bins: usize) -> RecordSlice<'_> {
        let i = p * traced_bins + bin;
        let (first, last) = (self.offsets[i] as usize, self.offsets[i + 1] as usize);
        RecordSlice {
            data: &self.records[first * self.stride..last * self.stride],
            stride: self.stride,
            chroma: if self.chroma.is_empty() {
                &[]
            } else {
                &self.chroma[3 * first..3 * last]
            },
        }
    }

    /// The number of stored records.
    #[must_use]
    pub const fn record_count(&self) -> usize {
        match self.records.len().checked_div(self.stride) {
            Some(count) => count,
            None => 0,
        }
    }

    /// Checks the invariants of the CSR store: `offsets` has `pixels * traced_bins + 1` entries,
    /// starts at 0, never decreases and ends at the record count; every (pixel, bin) holds at
    /// most `max_records` records; the float store is a whole number of records. Returns what is
    /// wrong, or `Ok`.
    ///
    /// # Errors
    ///
    /// A static description of the first violated invariant.
    pub fn check_offsets(
        &self,
        traced_bins: usize,
        max_records: usize,
    ) -> Result<(), &'static str> {
        let expected = self
            .pixel_count()
            .checked_mul(traced_bins)
            .and_then(|n| n.checked_add(1));
        if expected != Some(self.offsets.len()) {
            return Err("the offsets do not have one entry per (pixel, bin) plus one");
        }
        if self.stride < 2 || !self.records.len().is_multiple_of(self.stride) {
            return Err("the record store is not a whole number of records");
        }
        if self.offsets.first() != Some(&0) {
            return Err("the first offset is not zero");
        }
        if self.offsets.windows(2).any(|w| w[0] > w[1]) {
            return Err("the offsets are not non-decreasing");
        }
        if self.offsets.last().map(|&o| o as usize) != Some(self.record_count()) {
            return Err("the last offset is not the record count");
        }
        if !self.chroma.is_empty() {
            if self.record_count().checked_mul(3) != Some(self.chroma.len()) {
                return Err("the chroma store is not three codes per record");
            }
            if self.chroma.iter().any(|&code| code > CHROMA_MAX_CODE) {
                return Err("a chroma code is out of range");
            }
        }
        if self
            .offsets
            .windows(2)
            .any(|w| (w[1] - w[0]) as usize > max_records)
        {
            return Err("a (pixel, bin) holds more records than the record budget");
        }
        Ok(())
    }

    /// The heap bytes held.
    #[must_use]
    pub const fn memory_bytes(&self) -> usize {
        self.status.len()
            + self.samples.len() * 2
            + (self.mc_mean.len()
                + self.mc_variance.len()
                + self.mc_mean_clear.len()
                + self.mc_variance_clear.len())
                * 4
            + self.loss.len() * std::mem::size_of::<PixelLoss>()
            + self.offsets.len() * 4
            + self.records.len() * 4
            + self.chroma.len()
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
    /// Formula: per view, `pixels * (1 + 2 + 4 * 4 + 16 + 4 * traced_bins)` bytes of per-pixel
    /// data plus `4 * (n_zones + 1) * records` bytes, with
    /// `records <= pixels * traced_bins * max_records` (8 bytes per record for a uniform stone,
    /// 24 for four shaped zones), plus 3 bytes per record of chroma codes in a view whose light is
    /// not grey (ZC-13).
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
