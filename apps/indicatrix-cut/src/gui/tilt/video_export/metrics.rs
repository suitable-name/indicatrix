//! Reading the tilt dialog's already-computed 181-point curves at an arbitrary
//! fractional tilt angle -- the video overlay's numbers must agree with the curve the
//! dialog draws, so this mirrors `performance_graph_dialog.slint`'s own
//! `interpolated_value` function exactly (Slint pure functions cannot be called from
//! Rust, so this is a deliberate, carefully-matched duplicate, not a shared
//! implementation).

/// Linearly interpolates `values` (a 181-point, 1°-spaced ±90° sweep; index 0 = -90°,
/// index 180 = +90°) at `tilt_deg`. Mirrors
/// `performance_graph_dialog.slint`'s `interpolated_value` bit-for-bit in its rounding
/// behaviour (clamp, then floor/frac split) so the video overlay never disagrees with
/// the dialog's own hover readout for the same angle.
#[must_use]
pub fn interpolated_value(values: &[f32], tilt_deg: f64) -> f32 {
    if values.len() < 181 {
        return 0.0;
    }
    let clamped = tilt_deg.clamp(-90.0, 90.0);
    let shifted = clamped + 90.0; // 0..180
    let lower_idx = shifted.floor().clamp(0.0, 179.0) as usize;
    if lower_idx >= 179 {
        return values[180];
    }
    let frac = (shifted - lower_idx as f64) as f32;
    values[lower_idx].mul_add(1.0 - frac, values[lower_idx + 1] * frac)
}

/// "Tilt brilliance" for the video overlay: the mean of the swept axis's full ±90°
/// brilliance curve -- how well the stone stays bright across the WHOLE sweep, not
/// just face-up.
///
/// Deliberately a different number from `indicatrix_cut_core::optimize::objective`'s
/// own `tilt_brilliance_pct` (that one averages a small fixed set of Optimize's own
/// measurement poses, and is not read from here) -- named the same because it
/// answers the same question ("how well does this stone stay bright off face-up")
/// for a video that already has this dialog's own 181-point sweep on hand, not
/// because the two are computed identically or will ever agree pixel-for-pixel.
/// Callers surfacing both numbers side by side should say so, the same honesty
/// convention `performance_graph_dialog.slint`'s own captions already follow for
/// its brilliance-vs-Optimize distinction.
#[must_use]
pub fn mean_tilt_brilliance(brilliance_values: &[f32]) -> f32 {
    if brilliance_values.is_empty() {
        0.0
    } else {
        brilliance_values.iter().sum::<f32>() / brilliance_values.len() as f32
    }
}

/// Which metrics the "Show performance values" toggle can overlay onto a frame --
/// fixed order, also the order rows are drawn top-to-bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Brilliance,
    Windowing,
    Extinction,
    TiltBrilliance,
    Angle,
}

/// One resolved metric reading for one frame: which metric, and its numeric value at
/// that frame's exact tilt angle.
#[derive(Debug, Clone, Copy)]
pub struct MetricReading {
    pub metric: Metric,
    pub value: f32,
}

/// Every metric curve `interpolated_value`/`mean_tilt_brilliance` need, captured once
/// (not per-frame -- the curve itself doesn't change during an export) before the
/// background render loop starts.
pub struct MetricCurves {
    pub brilliance: Vec<f32>,
    pub windowing: Vec<f32>,
    pub extinction: Vec<f32>,
}

/// Which metrics are selected -- one bool per [`Metric`] variant, matching
/// `TiltVideoExportModel`'s own five checkboxes.
#[derive(Debug, Clone, Copy, Default)]
pub struct MetricSelection {
    pub brilliance: bool,
    pub windowing: bool,
    pub extinction: bool,
    pub tilt_brilliance: bool,
    pub angle: bool,
}

impl MetricSelection {
    /// How many of the five metrics are selected -- what `overlay::select_overlay_layout`
    /// bases its layout choice on.
    #[must_use]
    pub const fn count(self) -> usize {
        // `as usize` (not `usize::from`, not yet usable in a `const fn` on this
        // toolchain) -- exact for `bool` (`false as usize == 0`, `true as usize == 1`).
        self.brilliance as usize
            + self.windowing as usize
            + self.extinction as usize
            + self.tilt_brilliance as usize
            + self.angle as usize
    }
}

/// Builds this frame's [`MetricReading`]s for every selected metric, in fixed
/// ([`Metric`]'s own declared) order.
#[must_use]
pub fn readings_for_frame(
    selection: MetricSelection,
    curves: &MetricCurves,
    tilt_deg: f64,
) -> Vec<MetricReading> {
    let mut out = Vec::with_capacity(selection.count());
    if selection.brilliance {
        out.push(MetricReading {
            metric: Metric::Brilliance,
            value: interpolated_value(&curves.brilliance, tilt_deg),
        });
    }
    if selection.windowing {
        out.push(MetricReading {
            metric: Metric::Windowing,
            value: interpolated_value(&curves.windowing, tilt_deg),
        });
    }
    if selection.extinction {
        out.push(MetricReading {
            metric: Metric::Extinction,
            value: interpolated_value(&curves.extinction, tilt_deg),
        });
    }
    if selection.tilt_brilliance {
        out.push(MetricReading {
            metric: Metric::TiltBrilliance,
            value: mean_tilt_brilliance(&curves.brilliance),
        });
    }
    if selection.angle {
        out.push(MetricReading {
            metric: Metric::Angle,
            value: tilt_deg as f32,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ramp() -> Vec<f32> {
        // index i -> value i, so interpolation is trivially checkable.
        (0..181).map(|i| i as f32).collect()
    }

    #[test]
    fn interpolated_value_reads_exact_measured_points() {
        let values = ramp();
        assert_eq!(interpolated_value(&values, -90.0), 0.0);
        assert_eq!(interpolated_value(&values, 0.0), 90.0);
        assert_eq!(interpolated_value(&values, 90.0), 180.0);
    }

    #[test]
    fn interpolated_value_interpolates_between_points() {
        let values = ramp();
        assert!((interpolated_value(&values, 0.5) - 90.5).abs() < 1e-4);
        assert!((interpolated_value(&values, -89.75) - 0.25).abs() < 1e-4);
    }

    #[test]
    fn interpolated_value_clamps_out_of_range_angles() {
        let values = ramp();
        assert_eq!(interpolated_value(&values, 200.0), 180.0);
        assert_eq!(interpolated_value(&values, -200.0), 0.0);
    }

    #[test]
    fn interpolated_value_needs_the_full_181_points() {
        assert_eq!(interpolated_value(&[1.0, 2.0], 0.0), 0.0);
    }

    #[test]
    fn mean_tilt_brilliance_averages_the_whole_curve() {
        let values = vec![0.0, 50.0, 100.0];
        assert!((mean_tilt_brilliance(&values) - 50.0).abs() < 1e-6);
        assert_eq!(mean_tilt_brilliance(&[]), 0.0);
    }

    #[test]
    fn metric_selection_count_matches_the_flags_set() {
        assert_eq!(MetricSelection::default().count(), 0);
        let all = MetricSelection {
            brilliance: true,
            windowing: true,
            extinction: true,
            tilt_brilliance: true,
            angle: true,
        };
        assert_eq!(all.count(), 5);
    }

    #[test]
    fn readings_for_frame_respects_selection_and_fixed_order() {
        let curves = MetricCurves {
            brilliance: ramp(),
            windowing: ramp(),
            extinction: ramp(),
        };
        let selection = MetricSelection {
            extinction: true,
            angle: true,
            ..Default::default()
        };
        let readings = readings_for_frame(selection, &curves, 0.0);
        assert_eq!(readings.len(), 2);
        assert_eq!(readings[0].metric, Metric::Extinction);
        assert_eq!(readings[1].metric, Metric::Angle);
        assert_eq!(readings[1].value, 0.0);
    }
}
