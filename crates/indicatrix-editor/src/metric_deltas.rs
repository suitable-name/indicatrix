//! "Deltas in words": the optical figures of two stones side by side, and a few plain
//! sentences about what changed between them.
//!
//! The compare window shows the five table-up figures of the "before" and the "after"
//! stone ([`OpticalFigures`]: brilliance, windowing, extinction, fire, scintillation) and
//! can add the tilt averages ([`TiltAverages`], the mean over four axes and 181 angles).
//! [`describe_metric_deltas`] turns the two sets into one to three short sentences,
//! [`metric_rows`] into the rows of the small table beside them.
//!
//! # Noise thresholds
//!
//! Two stones are never measured to the last digit: the table-up figures come from an
//! 18 x 18 grid of rays, of which a disc of roughly 250 cells is used, so one grid cell is
//! worth about 0.4 of a point. A change in the last few cells (a facet edge sliding across
//! a ray) is not something a cutter can see, so a figure that moved by less than its
//! threshold reads "about the same" instead of being reported as a gain or a loss. The
//! thresholds are named constants below, each with the reason for its size.
//!
//! The measuring functions ([`measure_table_up`], [`measure_tilt_average`]) are plain
//! calls into `indicatrix::color::metrics`; they hold no state and no thread, so the
//! desktop runs them on its own worker threads.

use crate::sweep::TiltAverages;
use indicatrix::{
    color::metrics::{
        AxisProfile, GemOpticalMetrics, SweepProgress, evaluate_all_axes_profiles_stepped_geom,
        evaluate_gem_optical_metrics_geom,
    },
    geometry::{GpuFacetPlane, StoneGeometry},
    optics::{materials::GemMaterial, raytracer::EnvironmentSource},
};
use std::f32::consts::FRAC_PI_2;

/// Brilliance changes under this many percentage points read "about the same".
///
/// Two points is about five grid cells of the table-up measurement: more than the cells
/// that flip when a facet edge slides, and less than a change a cutter can see in the
/// stone.
pub const BRILLIANCE_NOISE_POINTS: f32 = 2.0;

/// Windowing changes under this many percentage points read "about the same".
///
/// Sized like [`BRILLIANCE_NOISE_POINTS`]: windowing is counted over the same grid.
pub const WINDOWING_NOISE_POINTS: f32 = 2.0;

/// Extinction changes under this many percentage points read "about the same".
///
/// Sized like [`BRILLIANCE_NOISE_POINTS`]: extinction is the share of rays that are
/// neither returned nor leaked, counted over the same grid.
pub const EXTINCTION_NOISE_POINTS: f32 = 2.0;

/// Scintillation changes under this many percentage points read "about the same".
///
/// Scintillation is a contrast figure (how unevenly the grid cells return light, blended
/// with how much they flicker when the stone is turned a few degrees). It moves more than
/// the plain shares above for the same small change in the cut, so its threshold is a
/// point higher.
pub const SCINTILLATION_NOISE_POINTS: f32 = 3.0;

/// Fire changes under this share of the "before" figure read "about the same".
///
/// Fire is an index (about 20 for a round brilliant in diamond, see the fire scale in
/// `indicatrix::color::metrics`), not a percentage, so its threshold is relative: 5 % of
/// the "before" figure, which is about one index point for a typical stone.
pub const FIRE_NOISE_RELATIVE: f32 = 0.05;

/// Fire changes under this many index points read "about the same", whatever the
/// relative threshold says.
///
/// The fire index never drops below 0.1, and a stone with almost no fire would otherwise
/// report a jump from 0.1 to 0.3 as "three times the fire". Half an index point is a
/// small step on the fire scale.
pub const FIRE_NOISE_FLOOR: f32 = 0.5;

/// Changes of a tilt average under this many percentage points read "about the same".
///
/// A tilt average is the mean of 724 poses (four axes, 181 tilts each), so the grid
/// jitter of single poses largely cancels out. One point is already a difference worth
/// naming.
pub const TILT_NOISE_POINTS: f32 = 1.0;

/// The subject of the sentences in the compare window: the "after" side.
pub const DEFAULT_SUBJECT: &str = "The after design";

/// The camera pitch of the table-up measurement: looking straight down on the table.
const TABLE_UP_PITCH_RAD: f32 = FRAC_PI_2;

/// The five table-up figures of one stone.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OpticalFigures {
    /// Share of the light returned to the eye, percent (higher is better).
    pub brilliance_pct: f32,
    /// Share of the light that leaks out through the pavilion, percent (lower is better).
    pub windowing_pct: f32,
    /// Share of the light that is trapped or lost, percent (lower is better).
    pub extinction_pct: f32,
    /// The fire index (higher is more fire).
    pub fire_index: f32,
    /// Scintillation, percent (higher is more sparkle).
    pub scintillation_pct: f32,
}

impl From<GemOpticalMetrics> for OpticalFigures {
    fn from(metrics: GemOpticalMetrics) -> Self {
        Self {
            brilliance_pct: metrics.brilliance_pct,
            windowing_pct: metrics.windowing_pct,
            extinction_pct: metrics.extinction_pct,
            fire_index: metrics.fire_index,
            scintillation_pct: metrics.scintillation_pct,
        }
    }
}

/// Measures `planes` in `material` under `environment`, looking straight down on the
/// table. About 2 ms for a round brilliant.
#[must_use]
pub fn measure_table_up(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> OpticalFigures {
    measure_table_up_geom(StoneGeometry::planes_only(planes), material, environment)
}

/// [`measure_table_up`] for a stone that may carry concave tools (grooves, dimples).
///
/// With no tools this is bit-identical to [`measure_table_up`], which calls it.
#[must_use]
pub fn measure_table_up_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
) -> OpticalFigures {
    evaluate_gem_optical_metrics_geom(geom, material, 0.0, TABLE_UP_PITCH_RAD, environment).into()
}

/// The tilt performance of `planes` averaged over the four axes and 181 angles, or
/// `None` when `step` stopped the sweep.
///
/// `step` is called before each of the 724 evaluations (about 1.4 s in all) and stops the
/// sweep by answering `false`.
#[must_use]
pub fn measure_tilt_average(
    planes: &[GpuFacetPlane],
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    step: &mut dyn FnMut(SweepProgress) -> bool,
) -> Option<TiltAverages> {
    measure_tilt_average_geom(
        StoneGeometry::planes_only(planes),
        material,
        environment,
        step,
    )
}

/// [`measure_tilt_average`] for a stone that may carry concave tools.
///
/// With no tools this is bit-identical to [`measure_tilt_average`], which calls it.
#[must_use]
pub fn measure_tilt_average_geom(
    geom: StoneGeometry<'_>,
    material: &GemMaterial,
    environment: EnvironmentSource<'_>,
    step: &mut dyn FnMut(SweepProgress) -> bool,
) -> Option<TiltAverages> {
    evaluate_all_axes_profiles_stepped_geom(geom, material, environment, step)
        .map(|axes| average_axis_profiles(&axes))
}

/// The mean brilliance, windowing and extinction over every point of `axes`.
#[must_use]
pub fn average_axis_profiles(axes: &[AxisProfile]) -> TiltAverages {
    let mut brilliance = 0.0_f64;
    let mut windowing = 0.0_f64;
    let mut extinction = 0.0_f64;
    let mut samples = 0_u32;
    for axis in axes {
        for ((b, w), e) in axis
            .brilliance
            .iter()
            .zip(&axis.windowing)
            .zip(&axis.extinction)
        {
            brilliance += f64::from(*b);
            windowing += f64::from(*w);
            extinction += f64::from(*e);
            samples += 1;
        }
    }
    let samples = f64::from(samples.max(1));
    TiltAverages {
        brilliance_pct: (brilliance / samples) as f32,
        windowing_pct: (windowing / samples) as f32,
        extinction_pct: (extinction / samples) as f32,
    }
}

/// One of the figures the sentences talk about, in the order of importance.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeltaMetric {
    /// Brilliance: how bright the stone looks face-up.
    Brilliance,
    /// Windowing: see-through areas.
    Windowing,
    /// Extinction: dark areas.
    Extinction,
    /// Fire: the coloured flashes.
    Fire,
    /// Scintillation: the on-off sparkle.
    Scintillation,
}

impl DeltaMetric {
    /// Every figure, most important first: the three that decide how the stone looks,
    /// then fire, then scintillation.
    pub const ALL: [Self; 5] = [
        Self::Brilliance,
        Self::Windowing,
        Self::Extinction,
        Self::Fire,
        Self::Scintillation,
    ];

    /// The name of the figure.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Brilliance => "Brilliance",
            Self::Windowing => "Windowing",
            Self::Extinction => "Extinction",
            Self::Fire => "Fire",
            Self::Scintillation => "Scintillation",
        }
    }

    /// The name of the figure's tilt average.
    #[must_use]
    pub const fn tilt_label(self) -> &'static str {
        match self {
            Self::Brilliance => "Tilt brilliance",
            Self::Windowing => "Tilt windowing",
            Self::Extinction => "Tilt extinction",
            Self::Fire => "Tilt fire",
            Self::Scintillation => "Tilt scintillation",
        }
    }

    /// Whether a larger figure is the better one.
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        !matches!(self, Self::Windowing | Self::Extinction)
    }

    /// Whether the figure is one of the three that lead the sentences.
    #[must_use]
    pub const fn is_main(self) -> bool {
        matches!(self, Self::Brilliance | Self::Windowing | Self::Extinction)
    }

    /// The figure of `figures`.
    #[must_use]
    pub const fn value(self, figures: &OpticalFigures) -> f32 {
        match self {
            Self::Brilliance => figures.brilliance_pct,
            Self::Windowing => figures.windowing_pct,
            Self::Extinction => figures.extinction_pct,
            Self::Fire => figures.fire_index,
            Self::Scintillation => figures.scintillation_pct,
        }
    }

    /// The change in percentage points under which a table-up figure reads "about the
    /// same". Fire is relative and has its own rule, see [`judge`].
    const fn noise_points(self) -> f32 {
        match self {
            Self::Brilliance => BRILLIANCE_NOISE_POINTS,
            Self::Windowing => WINDOWING_NOISE_POINTS,
            Self::Extinction => EXTINCTION_NOISE_POINTS,
            Self::Scintillation => SCINTILLATION_NOISE_POINTS,
            Self::Fire => FIRE_NOISE_FLOOR,
        }
    }
}

/// Whether a change helps, hurts, or is lost in the noise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    /// Within the noise threshold: about the same.
    Same,
    /// A change for the better.
    Better,
    /// A change for the worse.
    Worse,
}

impl Tone {
    /// The word the table shows for the change.
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Same => "same",
            Self::Better => "better",
            Self::Worse => "worse",
        }
    }

    /// The number the Slint side uses for the colour: 0 same, 1 better, 2 worse.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Same => 0,
            Self::Better => 1,
            Self::Worse => 2,
        }
    }
}

/// How one figure changed from "before" to "after".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricChange {
    /// The figure.
    pub metric: DeltaMetric,
    /// After minus before: percentage points, or for fire the relative change in percent.
    pub change: f32,
    /// Whether the change is better, worse or within the noise.
    pub tone: Tone,
}

/// How `metric` changed from `before` to `after`; `tilt` selects the tilt average's
/// threshold. `None` when either figure is not a finite number.
#[must_use]
pub fn judge(metric: DeltaMetric, before: f32, after: f32, tilt: bool) -> Option<MetricChange> {
    if !before.is_finite() || !after.is_finite() {
        return None;
    }
    let difference = after - before;
    let (threshold, change) = if metric == DeltaMetric::Fire {
        let relative = FIRE_NOISE_RELATIVE * before;
        (
            relative.max(FIRE_NOISE_FLOOR),
            difference / before.max(FIRE_NOISE_FLOOR) * 100.0,
        )
    } else if tilt {
        (TILT_NOISE_POINTS, difference)
    } else {
        (metric.noise_points(), difference)
    };
    let tone = if difference.abs() <= threshold {
        Tone::Same
    } else if (difference > 0.0) == metric.higher_is_better() {
        Tone::Better
    } else {
        Tone::Worse
    };
    Some(MetricChange {
        metric,
        change,
        tone,
    })
}

/// The sentences about what changed between `before` and `after`, with "The after
/// design" as their subject: see [`describe_metric_deltas_for`].
#[must_use]
pub fn describe_metric_deltas(
    before: &OpticalFigures,
    after: &OpticalFigures,
    tilt: Option<(&TiltAverages, &TiltAverages)>,
) -> Vec<String> {
    describe_metric_deltas_for(DEFAULT_SUBJECT, before, after, tilt)
}

/// The sentences about what changed from `before` to `after`, with `subject` ("The after
/// design", "The current design") as the thing that changed.
///
/// One to three sentences about the table-up figures, most important first: the changes
/// in brilliance, windowing and extinction; then the changes in fire and scintillation
/// ("It also ..."); then the figures that stayed within their noise threshold ("Fire is
/// about the same."). When nothing moved beyond its threshold the answer is the single
/// sentence "No clear optical difference.". With `tilt` (the before and after tilt
/// averages) one more sentence follows, about the tilt averages.
#[must_use]
pub fn describe_metric_deltas_for(
    subject: &str,
    before: &OpticalFigures,
    after: &OpticalFigures,
    tilt: Option<(&TiltAverages, &TiltAverages)>,
) -> Vec<String> {
    let judged: Vec<MetricChange> = DeltaMetric::ALL
        .iter()
        .filter_map(|&metric| judge(metric, metric.value(before), metric.value(after), false))
        .collect();
    let mut lines = table_up_lines(subject, &judged);
    if let Some((tilt_before, tilt_after)) = tilt {
        lines.push(tilt_line(subject, tilt_before, tilt_after));
    }
    lines
}

/// The one to three sentences about the table-up figures.
fn table_up_lines(subject: &str, judged: &[MetricChange]) -> Vec<String> {
    if judged.is_empty() {
        return vec!["The optical figures could not be compared.".to_owned()];
    }
    let changed: Vec<&MetricChange> = judged
        .iter()
        .filter(|change| change.tone != Tone::Same)
        .collect();
    if changed.is_empty() {
        return vec!["No clear optical difference.".to_owned()];
    }
    let phrases = |main: bool| -> Vec<String> {
        changed
            .iter()
            .filter(|change| change.metric.is_main() == main)
            .map(|&&change| direction_phrase(change, false))
            .collect()
    };
    let (main, other) = (phrases(true), phrases(false));
    let mut lines = Vec::new();
    if !main.is_empty() {
        lines.push(format!("{subject} {}.", join_list(&main)));
    }
    if !other.is_empty() {
        let lead = if lines.is_empty() { subject } else { "It also" };
        lines.push(format!("{lead} {}.", join_list(&other)));
    }
    let same: Vec<&str> = judged
        .iter()
        .filter(|change| change.tone == Tone::Same)
        .map(|change| change.metric.label())
        .collect();
    lines.extend(same_line(&same));
    lines
}

/// The sentence about the tilt averages.
fn tilt_line(subject: &str, before: &TiltAverages, after: &TiltAverages) -> String {
    let pairs = [
        (
            DeltaMetric::Brilliance,
            before.brilliance_pct,
            after.brilliance_pct,
        ),
        (
            DeltaMetric::Windowing,
            before.windowing_pct,
            after.windowing_pct,
        ),
        (
            DeltaMetric::Extinction,
            before.extinction_pct,
            after.extinction_pct,
        ),
    ];
    let phrases: Vec<String> = pairs
        .iter()
        .filter_map(|&(metric, was, now)| judge(metric, was, now, true))
        .filter(|change| change.tone != Tone::Same)
        .map(|change| direction_phrase(change, true))
        .collect();
    if phrases.is_empty() {
        "Averaged over all tilts, there is no clear difference.".to_owned()
    } else {
        format!(
            "Averaged over all tilts, {} {}.",
            lower_first(subject),
            join_list(&phrases)
        )
    }
}

/// "is brighter face-up (+4 %)": what happened to one figure, in words and as a number.
fn direction_phrase(change: MetricChange, tilt: bool) -> String {
    let more = change.change > 0.0;
    let words = match (change.metric, tilt, more) {
        (DeltaMetric::Brilliance, false, true) => "is brighter face-up",
        (DeltaMetric::Brilliance, false, false) => "is less bright face-up",
        (DeltaMetric::Brilliance, true, true) => "returns more light",
        (DeltaMetric::Brilliance, true, false) => "returns less light",
        (DeltaMetric::Windowing, _, true) => "shows more windowing",
        (DeltaMetric::Windowing, _, false) => "shows less windowing",
        (DeltaMetric::Extinction, _, true) => "has more extinction",
        (DeltaMetric::Extinction, _, false) => "has less extinction",
        (DeltaMetric::Fire, _, true) => "shows more fire",
        (DeltaMetric::Fire, _, false) => "shows less fire",
        (DeltaMetric::Scintillation, _, true) => "sparkles more",
        (DeltaMetric::Scintillation, _, false) => "sparkles less",
    };
    format!("{words} ({:+.0} %)", change.change)
}

/// "Fire is about the same." / "Brilliance and windowing are about the same."; `None`
/// for an empty list.
fn same_line(labels: &[&str]) -> Option<String> {
    let (first, rest) = labels.split_first()?;
    let mut names = vec![(*first).to_owned()];
    names.extend(rest.iter().map(|label| label.to_lowercase()));
    let verb = if names.len() == 1 { "is" } else { "are" };
    Some(format!("{} {verb} about the same.", join_list(&names)))
}

/// "a", "a and b", "a, b and c".
fn join_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// `text` with its first letter in lower case.
fn lower_first(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_lowercase().chain(chars).collect()
    })
}

/// One row of the comparison table: a figure before and after, its change and whether
/// the change is better, worse or within the noise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetricRow {
    /// The figure's name.
    pub label: &'static str,
    /// The figure for the "before" stone, e.g. `62.4 %`.
    pub before: String,
    /// The figure for the "after" stone.
    pub after: String,
    /// The signed change, e.g. `+4.0 %`; for fire the relative change.
    pub change: String,
    /// Whether the change is better, worse or about the same.
    pub tone: Tone,
}

/// The table rows: the five table-up figures, then (with `tilt`) the three tilt averages.
#[must_use]
pub fn metric_rows(
    before: &OpticalFigures,
    after: &OpticalFigures,
    tilt: Option<(&TiltAverages, &TiltAverages)>,
) -> Vec<MetricRow> {
    let mut rows: Vec<MetricRow> = DeltaMetric::ALL
        .iter()
        .map(|&metric| {
            row(
                metric.label(),
                metric,
                metric.value(before),
                metric.value(after),
                false,
            )
        })
        .collect();
    if let Some((tilt_before, tilt_after)) = tilt {
        let pairs = [
            (
                DeltaMetric::Brilliance,
                tilt_before.brilliance_pct,
                tilt_after.brilliance_pct,
            ),
            (
                DeltaMetric::Windowing,
                tilt_before.windowing_pct,
                tilt_after.windowing_pct,
            ),
            (
                DeltaMetric::Extinction,
                tilt_before.extinction_pct,
                tilt_after.extinction_pct,
            ),
        ];
        rows.extend(
            pairs
                .iter()
                .map(|&(metric, was, now)| row(metric.tilt_label(), metric, was, now, true)),
        );
    }
    rows
}

/// What shows where a figure could not be measured.
const NOT_AVAILABLE: &str = "n/a";

fn row(label: &'static str, metric: DeltaMetric, before: f32, after: f32, tilt: bool) -> MetricRow {
    let judged = judge(metric, before, after, tilt);
    let change = judged.map_or_else(
        || NOT_AVAILABLE.to_owned(),
        |judged| {
            if metric == DeltaMetric::Fire {
                format!("{} %", signed(judged.change, 0))
            } else {
                format!("{} %", signed(judged.change, 1))
            }
        },
    );
    MetricRow {
        label,
        before: figure_text(metric, before),
        after: figure_text(metric, after),
        change,
        tone: judged.map_or(Tone::Same, |judged| judged.tone),
    }
}

/// A figure as a table cell: one decimal, with the percent sign unless it is the fire
/// index.
fn figure_text(metric: DeltaMetric, value: f32) -> String {
    if !value.is_finite() {
        NOT_AVAILABLE.to_owned()
    } else if metric == DeltaMetric::Fire {
        format!("{value:.1}")
    } else {
        format!("{value:.1} %")
    }
}

/// `value` with an explicit sign and `decimals` decimals; a value that rounds to zero
/// carries no sign ("0.0", never "-0.0").
fn signed(value: f32, decimals: usize) -> String {
    let text = format!("{value:+.decimals$}");
    if text[1..].chars().all(|c| c == '0' || c == '.') {
        text[1..].to_owned()
    } else {
        text
    }
}

#[cfg(test)]
mod tests;
