//! What the Checks tab shows of the analysis: the metrics rows, the tilt curves, and the
//! tab-separated table "Copy Data" puts on the clipboard.

use std::{fmt::Write as _, sync::Arc};

use indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG;
use indicatrix_editor::view_model::curve_path::full_axis_curve_path;
use indicatrix_web_core::solve::TiltAxisData;

use super::{Kind, MetricsReading, Slots, TiltReading, inputs::Key};
use crate::app_state::AppState;

/// The number of points on each tilt curve (-90 to +90 degrees in 1 degree steps).
pub const TILT_POINTS: usize = 181;

/// The tilt in degrees away from table-up that curve point `index` stands for
/// (`-90.0` for point 0, `0.0` for point 90, `90.0` for point 180).
#[must_use]
pub fn tilt_at(index: usize) -> f32 {
    index as f32 - 90.0
}

/// Where a measurement stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnalysisStatus {
    /// There is no current solve to measure.
    NeedsSolve,
    /// Nothing was measured yet.
    NotComputed,
    /// A measurement is running (or queued).
    Busy,
    /// Numbers exist but describe an earlier design or lighting.
    Stale,
    /// The numbers describe the design as it is.
    Current,
    /// The last measurement failed.
    Failed,
}

impl AnalysisStatus {
    /// The status as the Slint int: 0 needs solve, 1 not computed, 2 busy, 3 stale,
    /// 4 current, 5 failed.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::NeedsSolve => 0,
            Self::NotComputed => 1,
            Self::Busy => 2,
            Self::Stale => 3,
            Self::Current => 4,
            Self::Failed => 5,
        }
    }
}

/// One line of the metrics list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricRow {
    /// Title Case name.
    pub label: &'static str,
    /// The value with its unit.
    pub value: String,
    /// One sentence about what it means.
    pub meaning: &'static str,
}

/// The Metrics section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricsView {
    /// Where the measurement stands.
    pub status: AnalysisStatus,
    /// A sentence under the rows (why they are missing or old, or what lit them).
    pub message: String,
    /// The five rows; empty until a measurement exists. Shown dimmed while stale or busy.
    pub rows: Vec<MetricRow>,
}

/// The Tilt Curves page.
#[derive(Debug, Clone, PartialEq)]
pub struct TiltView {
    /// Where the sweep stands.
    pub status: AnalysisStatus,
    /// A sentence about the curves or why there are none.
    pub message: String,
    /// The running sweep's completion (`0.0..=1.0`), `None` when idle.
    pub progress: Option<f32>,
    /// The running sweep's stage line, empty when idle.
    pub stage: String,
    /// The curves, if any were swept (old ones stay while a new sweep runs).
    pub curves: Option<std::sync::Arc<TiltReading>>,
}

/// How a stored result relates to the app as it stands.
fn freshness<T>(slots: &Slots<T>, current: Option<&Key>) -> AnalysisStatus {
    if slots.busy.is_some() {
        return AnalysisStatus::Busy;
    }
    match (&slots.result, current) {
        (Some((_, stored)), Some(now)) if stored == now => AnalysisStatus::Current,
        (Some(_), _) => AnalysisStatus::Stale,
        (None, _) if slots.error.is_some() => AnalysisStatus::Failed,
        (None, None) => AnalysisStatus::NeedsSolve,
        (None, Some(_)) => AnalysisStatus::NotComputed,
    }
}

/// The Metrics section for `state`.
pub(super) fn metrics_view(state: &AppState) -> MetricsView {
    let current = Key::new(state, Kind::Metrics);
    let inner = state.analysis.shared.lock();
    let slots = &inner.metrics;
    let status = freshness(slots, current.as_ref());
    let rows = slots
        .result
        .as_ref()
        .map(|(reading, _)| metric_rows(reading))
        .unwrap_or_default();
    let under_hdr = slots
        .result
        .as_ref()
        .is_some_and(|(reading, _)| reading.under_hdr);
    let message = match status {
        AnalysisStatus::NeedsSolve => "Solve the design to measure it.".to_owned(),
        AnalysisStatus::NotComputed => "Tap Compute to measure this stone.".to_owned(),
        AnalysisStatus::Busy => "Measuring...".to_owned(),
        AnalysisStatus::Stale => {
            "These numbers are from before the last change. Compute them again.".to_owned()
        }
        AnalysisStatus::Current if under_hdr => "Scored under the HDR map.".to_owned(),
        AnalysisStatus::Current => String::new(),
        AnalysisStatus::Failed => format!(
            "The measurement failed: {}",
            slots.error.as_deref().unwrap_or("unknown error")
        ),
    };
    drop(inner);
    MetricsView {
        status,
        message,
        rows,
    }
}

/// The Tilt Curves page for `state`.
pub(super) fn tilt_view(state: &AppState) -> TiltView {
    let current = Key::new(state, Kind::Tilt);
    let inner = state.analysis.shared.lock();
    let slots = &inner.tilt;
    let status = freshness(slots, current.as_ref());
    let progress = slots.busy.as_ref().and_then(|busy| busy.progress.clone());
    let message = match status {
        AnalysisStatus::NeedsSolve => "Solve the design to sweep its tilt curves.".to_owned(),
        AnalysisStatus::NotComputed => "Tap Run to sweep the stone through every tilt.".to_owned(),
        AnalysisStatus::Busy => "Sweeping...".to_owned(),
        AnalysisStatus::Stale => {
            "These curves are from before the last change. Run the sweep again.".to_owned()
        }
        AnalysisStatus::Current => {
            "Scored under the lighting rig at the current light; an HDR map is not used here."
                .to_owned()
        }
        AnalysisStatus::Failed => format!(
            "The sweep failed: {}",
            slots.error.as_deref().unwrap_or("unknown error")
        ),
    };
    let curves = slots
        .result
        .as_ref()
        .map(|(reading, _)| Arc::clone(reading));
    drop(inner);
    TiltView {
        status,
        message,
        progress: progress.as_ref().map(|(fraction, _)| *fraction),
        stage: progress.map(|(_, stage)| stage).unwrap_or_default(),
        curves,
    }
}

/// The five metric rows of `reading`, in the order the desktop lists them.
#[must_use]
pub fn metric_rows(reading: &MetricsReading) -> Vec<MetricRow> {
    vec![
        MetricRow {
            label: "Brilliance",
            value: format!("{:.1} %", reading.brilliance_pct),
            meaning: "The share of the light that returns to your eye.",
        },
        MetricRow {
            label: "Fire",
            value: format!("{:.1}", reading.fire_index),
            meaning: "How strongly the stone splits white light into colors.",
        },
        MetricRow {
            label: "Scintillation",
            value: format!("{:.1} %", reading.scintillation_pct),
            meaning: "How much the bright and dark flashes change as the stone moves.",
        },
        MetricRow {
            label: "Windowing",
            value: format!("{:.1} %", reading.windowing_pct),
            meaning: "Light that leaks out through the pavilion instead of returning.",
        },
        MetricRow {
            label: "Extinction",
            value: format!("{:.1} %", reading.extinction_pct),
            meaning: "Light that is lost, or comes back too dim to see.",
        },
    ]
}

impl TiltReading {
    /// The Slint-facing names of the swept axes: "0°", "45°", "90°", "135°".
    #[must_use]
    pub fn axis_names() -> Vec<String> {
        PROFILE_AZIMUTHS_DEG
            .iter()
            .map(|azimuth| format!("{azimuth:.0}\u{b0}"))
            .collect()
    }

    /// The axis at `index`, `None` when out of range.
    #[must_use]
    pub fn axis(&self, index: usize) -> Option<&TiltAxisData> {
        self.axes.get(index)
    }

    /// The SVG path strings (brilliance, extinction, windowing) of axis `index`, in the
    /// 100 x 100 box the chart's `Path` uses (x spans the whole tilt range, y is
    /// `100 - value`). `None` for an unknown axis or curves of the wrong length.
    #[must_use]
    pub fn paths(&self, index: usize) -> Option<[String; 3]> {
        let axis = self.axes.get(index)?;
        let curve = |values: &[f32]| -> Option<String> {
            let array: &[f32; TILT_POINTS] = values.try_into().ok()?;
            Some(full_axis_curve_path(array))
        };
        Some([
            curve(&axis.brilliance)?,
            curve(&axis.extinction)?,
            curve(&axis.windowing)?,
        ])
    }

    /// The whole sweep as a tab-separated table for the clipboard: a header row, then one
    /// row per degree of tilt with brilliance, extinction and windowing (in percent) for
    /// every axis.
    #[must_use]
    pub fn to_tsv(&self) -> String {
        let mut out = String::from("Tilt (deg)");
        for axis in &self.axes {
            let azimuth = axis.azimuth_deg;
            let _ = write!(
                out,
                "\tBrilliance {azimuth:.0}\tExtinction {azimuth:.0}\tWindowing {azimuth:.0}"
            );
        }
        out.push('\n');
        for index in 0..TILT_POINTS {
            let _ = write!(out, "{:.0}", tilt_at(index));
            for axis in &self.axes {
                let at = |values: &[f32]| values.get(index).copied().unwrap_or(f32::NAN);
                let _ = write!(
                    out,
                    "\t{:.2}\t{:.2}\t{:.2}",
                    at(&axis.brilliance),
                    at(&axis.extinction),
                    at(&axis.windowing)
                );
            }
            out.push('\n');
        }
        out
    }
}
