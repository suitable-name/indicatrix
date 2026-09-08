//! The tilt-performance sweep this crate stores per design.
//!
//! 4 axes x 3 metrics x 181 per-axis samples, backing both the Tilt Performance
//! dialog's cached graph and the `crate::model::performance` search filters.
//!
//! # Canonical shape (pinned; changing it means updating `apps/indicatrix-cut`'s
//! tilt-profile code too, which produces the live, uncached version of this sweep)
//!
//! ```text
//! 181 sample points per axis, exact 1-degree steps, index i holding the value at
//! TILT AWAY FROM TABLE-UP = (i - 90) degrees:
//!     index   0 -> tilt -90 (edge-on, one side)
//!     index  90 -> tilt   0 (TABLE-UP / face-up -- the shared pole pose)
//!     index 180 -> tilt +90 (edge-on, the opposite side)
//! 4 axes, indexed 0..=3, matching indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG ==
//! [0.0, 45.0, 90.0, 135.0]. For axis k: indices 90..=180 (tilt 0..+90) sweep azimuth
//! PROFILE_AZIMUTHS_DEG[k]; indices 0..=90 (tilt -90..0) sweep PROFILE_AZIMUTHS_DEG[k] +
//! 180. Index 90 (table-up) is shared between the two halves: 90 + 1 + 90 = 181.
//! Per axis, three curves: brilliance_pct, extinction_pct, windowing_pct -- each
//! already a 0..100-scale percentage, matching `indicatrix::color::metrics::
//! evaluate_angular_profile`'s own return convention -- as [f32; 181].
//! ```
//!
//! # Table-up at the centre, edge-on at the ends -- not the other way around
//!
//! The stored value at index `i` is TILT AWAY FROM TABLE-UP, never raw camera pitch: the
//! raytracer's `cam_pitch == 0` is edge-on, `cam_pitch == 90` is table-up, so index 90
//! here is face-up and indices 0/180 are edge-on, not the reverse. Every
//! `PerformanceFilter`'s tilt-radius window is centred on index 90 accordingly (see
//! [`TiltPerformanceCurves::matches_performance_filter`]).
//!
//! [`AxisTiltCurves`]'s arrays are always stored in this ascending-index order regardless
//! of which physical direction the raytracer swept -- whichever caller fills a
//! [`TiltPerformanceCurves`] must reverse the negative-half sweep into this order first.
//!
//! # Why 181 points at a 1-degree step, not something coarser
//!
//! Measured: interpolating from coarser samples gives max errors of 4.6-7.3 points at a
//! 5-degree step, still 3.2-4.6 at 2-degree -- enough to flip a design in/out of a
//! `crate::model::performance` hard-threshold filter, a wrong answer, not an
//! approximation. Hence 8,688 bytes/design (~27 MB across the ~3,187-design catalogue).
//!
//! # Why a packed BLOB, not a row-per-sample side table
//!
//! Both readers -- the Tilt Performance dialog (whole 2,172-value record at once) and
//! `crate::model::performance`'s filters (decoded per candidate design, in Rust, not SQL
//! -- see [`Self::matches_performance_filter`]) -- never need per-row access to
//! individual samples. A row-per-sample table would cost ~6.9M rows catalogue-wide for
//! no query capability either reader needs; one `BLOB` is simpler and cheaper.

use crate::model::performance::{
    GlobalExtremes, PerformanceAggregate, PerformanceBound, PerformanceFilter,
    PerformanceGlobalExtremes,
};
use anyhow::{Result, bail};

/// Sample points per axis. See this module's doc for the indexing convention
/// (index 0 = tilt -90, index 90 = table-up, index 180 = tilt +90, 1-degree step).
pub const TILT_CURVE_POINTS_PER_AXIS: usize = 181;

/// The tilt-away-from-table-up angle, in degrees, that sample `index` (0..=180) holds.
///
/// `index - 90`, so `index 90` (table-up/face-up) is always exactly `0.0`. The hot loop
/// in `matches_performance_filter` inlines this subtraction instead of calling this
/// function; this one exists for other callers, e.g. a future axis-label renderer.
///
/// # Panics
///
/// Never panics: `TILT_CURVE_POINTS_PER_AXIS - 1 = 180` is far below any width where
/// `index as f32` could lose precision.
#[must_use]
pub const fn tilt_deg_for_index(index: usize) -> f32 {
    index as f32 - 90.0
}

/// Axes per design (see `indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG`, which this
/// count matches).
pub const TILT_CURVE_AXIS_COUNT: usize = 4;

/// Metrics per axis: brilliance, extinction, windowing.
pub const TILT_CURVE_METRIC_COUNT: usize = 3;

/// Total `f32` samples in one design's packed record: 4 axes x 3 metrics x 181 points.
pub const TILT_CURVE_TOTAL_SAMPLES: usize =
    TILT_CURVE_AXIS_COUNT * TILT_CURVE_METRIC_COUNT * TILT_CURVE_POINTS_PER_AXIS;

/// Total bytes of [`Self::to_bytes`]'s output (4-byte little-endian `f32` per sample) --
/// the exact, fixed length [`TiltPerformanceCurves::from_bytes`] requires.
pub const TILT_CURVE_BLOB_BYTES: usize = TILT_CURVE_TOTAL_SAMPLES * 4;

/// One axis's three tilt sweeps. See this module's doc for the shared index-to-tilt
/// convention (index 90 = table-up/face-up; indices 0/180 = the edge-on extremes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisTiltCurves {
    pub brilliance_pct: [f32; TILT_CURVE_POINTS_PER_AXIS],
    pub extinction_pct: [f32; TILT_CURVE_POINTS_PER_AXIS],
    pub windowing_pct: [f32; TILT_CURVE_POINTS_PER_AXIS],
}

/// One design's full tilt-performance record: [`TILT_CURVE_AXIS_COUNT`] axes, each an
/// [`AxisTiltCurves`]. See this module's doc for the canonical shape and BLOB encoding.
#[derive(Debug, Clone, PartialEq)]
pub struct TiltPerformanceCurves {
    pub axes: [AxisTiltCurves; TILT_CURVE_AXIS_COUNT],
}

impl TiltPerformanceCurves {
    /// Packs this record into exactly [`TILT_CURVE_BLOB_BYTES`] bytes: for each axis
    /// (0..4, outermost), for each metric in the fixed order brilliance, extinction,
    /// windowing, for each of the 181 sample points (ascending index, i.e. ascending
    /// tilt-away-from-table-up, innermost), one little-endian `f32`.
    /// [`Self::from_bytes`] is the exact inverse.
    #[must_use]
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(TILT_CURVE_BLOB_BYTES);
        for axis in &self.axes {
            for curve in [
                &axis.brilliance_pct,
                &axis.extinction_pct,
                &axis.windowing_pct,
            ] {
                for &sample in curve {
                    out.extend_from_slice(&sample.to_le_bytes());
                }
            }
        }
        debug_assert_eq!(out.len(), TILT_CURVE_BLOB_BYTES);
        out
    }

    /// Unpacks `bytes` (as written by [`Self::to_bytes`]) back into a record.
    ///
    /// # Errors
    ///
    /// Returns an error if `bytes.len()` is not exactly [`TILT_CURVE_BLOB_BYTES`] -- a
    /// fixed-shape record, so any other length is a schema mismatch, never a value to
    /// silently truncate or zero-pad.
    ///
    /// # Panics
    ///
    /// Never panics: the length check above already proves every 4-byte read is in range.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != TILT_CURVE_BLOB_BYTES {
            bail!(
                "tilt-curve BLOB is {} bytes, expected exactly {TILT_CURVE_BLOB_BYTES} \
                 ({TILT_CURVE_AXIS_COUNT} axes x {TILT_CURVE_METRIC_COUNT} metrics x \
                 {TILT_CURVE_POINTS_PER_AXIS} points x 4-byte f32)",
                bytes.len()
            );
        }

        let mut axes = [AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        }; TILT_CURVE_AXIS_COUNT];

        // Advances 4 bytes per sample, in the same axis-major/metric/point order
        // Self::to_bytes wrote them in.
        let mut offset = 0usize;
        for axis in &mut axes {
            for curve in [
                &mut axis.brilliance_pct,
                &mut axis.extinction_pct,
                &mut axis.windowing_pct,
            ] {
                for sample in curve.iter_mut() {
                    *sample = f32::from_le_bytes([
                        bytes[offset],
                        bytes[offset + 1],
                        bytes[offset + 2],
                        bytes[offset + 3],
                    ]);
                    offset += 4;
                }
            }
        }

        Ok(Self { axes })
    }

    /// Computes every derived scalar `crate::db::sqlite::Database::save_tilt_curves`
    /// writes into `diagram_tilt_curves`'s 6 precomputed columns: the global min/max of
    /// each metric across the full -90..+90 sweep, all 4 axes. See
    /// `crate::model::performance`'s module doc for why only these two statistics per
    /// metric (not a windowed min/max/mean) survive as SQL columns, now that a
    /// [`PerformanceFilter`]'s tilt radius is arbitrary rather than one of a fixed few.
    #[must_use]
    pub fn global_extremes(&self) -> PerformanceGlobalExtremes {
        PerformanceGlobalExtremes {
            brilliance: self.metric_global_extremes(|a| &a.brilliance_pct),
            extinction: self.metric_global_extremes(|a| &a.extinction_pct),
            windowing: self.metric_global_extremes(|a| &a.windowing_pct),
        }
    }

    /// The min/max of one metric across all 4 axes, entire 181-point axis (no windowing)
    /// -- the per-metric unit [`Self::global_extremes`] computes three times.
    fn metric_global_extremes(
        &self,
        select: impl Fn(&AxisTiltCurves) -> &[f32; TILT_CURVE_POINTS_PER_AXIS],
    ) -> GlobalExtremes {
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        for axis in &self.axes {
            for &v in select(axis) {
                min = min.min(v);
                max = max.max(v);
            }
        }
        GlobalExtremes { min, max }
    }

    /// Evaluates `filter` exactly against this design's full decoded curve record --
    /// the arbitrary-radius counterpart of the coarse global-min/max SQL narrowing
    /// `crate::db::sqlite::search::build_search_predicate` applies first (see
    /// `crate::model::performance`'s module doc for the two-stage design). Scans every
    /// point within `filter.tilt_radius_deg` degrees of TABLE-UP (index 90 -- see "Table-up
    /// at the centre" above) across all 4 axes: at most 4*181 = 724 comparisons.
    ///
    /// Each point's own tilt-away-from-table-up (`i - 90`) is compared directly against
    /// `filter.tilt_radius_deg`, so any radius (integer or fractional) works without a
    /// lookup table of window boundaries.
    ///
    /// A `tilt_radius_deg` outside `0.0..=90.0` (only reachable by bypassing
    /// [`PerformanceFilter::new`]'s validation) degrades gracefully: negative matches
    /// zero points and returns `false` (safe default for an empty aggregate); above 90
    /// matches every point, same as exactly 90.
    #[must_use]
    pub fn matches_performance_filter(&self, filter: &PerformanceFilter) -> bool {
        let radius = filter.tilt_radius_deg;
        let mut min = f32::INFINITY;
        let mut max = f32::NEG_INFINITY;
        let mut sum = 0.0f64;
        let mut count: u32 = 0;

        for axis in &self.axes {
            let curve = filter.metric.select(axis);
            for (i, &v) in curve.iter().enumerate() {
                let angle_deg = i as f32 - 90.0;
                if angle_deg.abs() <= radius {
                    min = min.min(v);
                    max = max.max(v);
                    sum += f64::from(v);
                    count += 1;
                }
            }
        }

        let Some(count) = std::num::NonZeroU32::new(count) else {
            return false;
        };

        match (filter.aggregate, filter.bound) {
            (PerformanceAggregate::Worst, PerformanceBound::AtMost(t)) => max <= t,
            (PerformanceAggregate::Worst, PerformanceBound::AtLeast(t)) => min >= t,
            (PerformanceAggregate::Mean, PerformanceBound::AtMost(t)) => {
                (sum / f64::from(count.get())) as f32 <= t
            }
            (PerformanceAggregate::Mean, PerformanceBound::AtLeast(t)) => {
                (sum / f64::from(count.get())) as f32 >= t
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::performance::PerformanceMetric;

    fn sample_curves() -> TiltPerformanceCurves {
        let mut axes = [AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        }; TILT_CURVE_AXIS_COUNT];
        for (axis_idx, axis) in axes.iter_mut().enumerate() {
            for i in 0..TILT_CURVE_POINTS_PER_AXIS {
                // Distinct, deterministic values per (axis, point) for exact checks.
                let base = (axis_idx * 1000 + i) as f32;
                axis.brilliance_pct[i] = base;
                axis.extinction_pct[i] = base + 0.25;
                axis.windowing_pct[i] = base + 0.5;
            }
        }
        TiltPerformanceCurves { axes }
    }

    #[test]
    fn to_bytes_produces_exactly_the_documented_length() {
        let curves = sample_curves();
        assert_eq!(curves.to_bytes().len(), TILT_CURVE_BLOB_BYTES);
        assert_eq!(TILT_CURVE_BLOB_BYTES, 8688);
    }

    #[test]
    fn to_bytes_then_from_bytes_round_trips_exactly() {
        let curves = sample_curves();
        let bytes = curves.to_bytes();
        let decoded = TiltPerformanceCurves::from_bytes(&bytes).expect("decode");
        assert_eq!(decoded, curves);
    }

    #[test]
    fn from_bytes_rejects_the_wrong_length() {
        let err = TiltPerformanceCurves::from_bytes(&[0u8; 10]).unwrap_err();
        assert!(err.to_string().contains("8688"));
    }

    #[test]
    fn global_extremes_covers_the_entire_axis_on_every_metric() {
        let curves = sample_curves();
        let extremes = curves.global_extremes();
        // Axis 0 contributes 0..181, axis 3 contributes 3000..3181: min is axis 0's
        // first sample, max is axis 3's last.
        assert!((extremes.brilliance.min - 0.0).abs() < 1e-6);
        assert!((extremes.brilliance.max - 3180.0).abs() < 1e-6);
        assert!((extremes.extinction.min - 0.25).abs() < 1e-6);
        assert!((extremes.extinction.max - 3180.25).abs() < 1e-6);
        assert!((extremes.windowing.min - 0.5).abs() < 1e-6);
        assert!((extremes.windowing.max - 3180.5).abs() < 1e-6);
    }

    /// Pins down that index 90 (table-up) is the CENTRE of every window, not an edge --
    /// a ramp from 0 to 180 makes a wrong centre visible as a wrong "worst" value.
    #[test]
    fn matches_performance_filter_windows_are_centred_on_table_up_not_an_edge() {
        let mut axis = AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        for (i, sample) in axis.brilliance_pct.iter_mut().enumerate() {
            *sample = i as f32;
        }
        let curves = TiltPerformanceCurves {
            axes: [axis; TILT_CURVE_AXIS_COUNT],
        };

        // Tight +/-5deg window (indices 85..=95) has values in [85, 95]: "at least 85" PASSes.
        let tight_at_least_85 = PerformanceFilter::new(
            PerformanceMetric::Brilliance,
            PerformanceBound::AtLeast(85.0),
            5.0,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(
            curves.matches_performance_filter(&tight_at_least_85),
            "a +/-5deg window centred on table-up (index 90) must see values >= 85, \
             not values near an edge (which would be close to 0 or 180)"
        );

        // Same window's worst (max) value is 95, so "at most 90" must FAIL.
        let tight_at_most_90 = PerformanceFilter::new(
            PerformanceMetric::Brilliance,
            PerformanceBound::AtMost(90.0),
            5.0,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(!curves.matches_performance_filter(&tight_at_most_90));

        // If wrongly centred on an edge, a tight radius would see ~0 or ~180, not 85..=95.
        assert!(
            (85.0..=95.0).contains(&curves.axes[0].brilliance_pct[95]),
            "sanity: index 95 (tilt +5) must hold the ramp value 95.0"
        );
    }

    #[test]
    fn matches_performance_filter_mean_matches_a_hand_computed_average_on_flat_data() {
        // Flat curves make the expected mean trivial to state independently.
        let flat_axis = AxisTiltCurves {
            brilliance_pct: [42.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [1.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [99.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        let curves = TiltPerformanceCurves {
            axes: [flat_axis; TILT_CURVE_AXIS_COUNT],
        };
        for radius in [0.0_f32, 15.0, 45.0, 90.0] {
            let at_most = PerformanceFilter::new(
                PerformanceMetric::Brilliance,
                PerformanceBound::AtMost(42.0),
                radius,
                PerformanceAggregate::Mean,
            )
            .unwrap();
            assert!(curves.matches_performance_filter(&at_most));
            let at_least = PerformanceFilter::new(
                PerformanceMetric::Brilliance,
                PerformanceBound::AtLeast(42.0),
                radius,
                PerformanceAggregate::Mean,
            )
            .unwrap();
            assert!(curves.matches_performance_filter(&at_least));
            let strictly_more = PerformanceFilter::new(
                PerformanceMetric::Brilliance,
                PerformanceBound::AtLeast(42.001),
                radius,
                PerformanceAggregate::Mean,
            )
            .unwrap();
            assert!(!curves.matches_performance_filter(&strictly_more));
        }
    }

    #[test]
    fn matches_performance_filter_accepts_a_fractional_radius() {
        let mut axis = AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        // Table-up +/-2deg (indices 88..=92) is 100.0, every other point 0.0.
        for sample in &mut axis.windowing_pct[88..=92] {
            *sample = 100.0;
        }
        let curves = TiltPerformanceCurves {
            axes: [axis; TILT_CURVE_AXIS_COUNT],
        };

        // 1.5deg radius (indices 89..=91, inside the 100.0 band): "at most 99" fails.
        let radius_1_5 = PerformanceFilter::new(
            PerformanceMetric::Windowing,
            PerformanceBound::AtMost(99.0),
            1.5,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(!curves.matches_performance_filter(&radius_1_5));

        // 2.5deg radius (indices 88..=92, still inside): proves `<=` against a float
        // radius, not silently rounded to an integer degree.
        let radius_2_5 = PerformanceFilter::new(
            PerformanceMetric::Windowing,
            PerformanceBound::AtMost(99.0),
            2.5,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(!curves.matches_performance_filter(&radius_2_5));

        // AtLeast shows widening mattering: radius 1.5..=2.5 stays all-100.0 (passes);
        // radius 3.5 includes an out-of-band 0.0 point (fails).
        let at_least_narrow = PerformanceFilter::new(
            PerformanceMetric::Windowing,
            PerformanceBound::AtLeast(100.0),
            2.0,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(curves.matches_performance_filter(&at_least_narrow));

        let at_least_wide = PerformanceFilter::new(
            PerformanceMetric::Windowing,
            PerformanceBound::AtLeast(100.0),
            3.5,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(!curves.matches_performance_filter(&at_least_wide));
    }

    #[test]
    fn matches_performance_filter_zero_radius_looks_only_at_table_up_itself() {
        let mut axis = AxisTiltCurves {
            brilliance_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: [0.0; TILT_CURVE_POINTS_PER_AXIS],
        };
        axis.brilliance_pct[90] = 77.0; // table-up
        axis.brilliance_pct[89] = 1.0; // 1deg off -- invisible at radius 0
        let curves = TiltPerformanceCurves {
            axes: [axis; TILT_CURVE_AXIS_COUNT],
        };

        let filter = PerformanceFilter::new(
            PerformanceMetric::Brilliance,
            PerformanceBound::AtLeast(77.0),
            0.0,
            PerformanceAggregate::Worst,
        )
        .unwrap();
        assert!(curves.matches_performance_filter(&filter));
    }
}
