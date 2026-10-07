//! A sweep's chart: the SVG path data of each line, the position of a row on the x axis,
//! the row under the pointer and the readout text.
//!
//! The chart is drawn the way the tilt chart is (`Path::commands` in a 0..100 by 0..100
//! viewbox, filled into the element). The x axis is the angle, as a magnitude (the flattest
//! on the left, whichever side of the girdle the tier is on), over the sweep's whole
//! range, valid rows or not. Every line is scaled to its own lowest and highest value,
//! because the figures have different units; the readout and [`series_range_text`] give
//! the real numbers.

use super::{SweepMetric, SweepRow};
use std::fmt::Write as _;

/// Empty margin at the left and right of the viewbox, percent, so the end points and
/// their strokes stay inside the chart.
const MARGIN_X: f64 = 2.0;

/// Empty margin at the top and bottom of the viewbox, percent.
const MARGIN_Y: f64 = 6.0;

/// The width the lines use inside the margins, percent.
const PLOT_WIDTH: f64 = 100.0 - 2.0 * MARGIN_X;

/// The height the lines use inside the margins, percent.
const PLOT_HEIGHT: f64 = 100.0 - 2.0 * MARGIN_Y;

/// Two values this close are the same height.
const FLAT_SPAN: f64 = 1e-9;

/// Where `angle_deg` sits on the x axis, in 0..100. The axis runs over magnitudes, so a
/// pavilion tier's `-41` sits between its `-40` and `-42` the way a person counts them.
fn x_percent(angle_deg: f64, low: f64, high: f64) -> f64 {
    if high - low < FLAT_SPAN {
        50.0
    } else {
        ((angle_deg.abs() - low) / (high - low)).mul_add(PLOT_WIDTH, MARGIN_X)
    }
}

/// The smallest and the largest magnitude among the rows' angles.
fn angle_extent(rows: &[SweepRow]) -> Option<(f64, f64)> {
    let low = rows
        .iter()
        .map(|row| row.angle_deg.abs())
        .reduce(f64::min)?;
    let high = rows
        .iter()
        .map(|row| row.angle_deg.abs())
        .reduce(f64::max)?;
    Some((low, high))
}

/// Where row `index` sits on the x axis of the chart, in 0..100; `50` when the sweep has
/// one angle only or `index` is not a row.
#[must_use]
pub fn chart_x_percent(rows: &[SweepRow], index: usize) -> f64 {
    match (angle_extent(rows), rows.get(index)) {
        (Some((low, high)), Some(row)) => x_percent(row.angle_deg, low, high),
        _ => 50.0,
    }
}

/// Where the design's own angle sits on the x axis, in 0..100; `None` when its row has
/// not finished.
#[must_use]
pub fn current_x_percent(rows: &[SweepRow]) -> Option<f64> {
    let index = rows.iter().position(|row| row.is_current)?;
    Some(chart_x_percent(rows, index))
}

/// The lowest and highest value of `metric` over the valid rows.
fn value_extent(rows: &[SweepRow], metric: SweepMetric) -> Option<(f64, f64)> {
    let mut values = rows.iter().filter_map(|row| metric.value(row));
    let first = values.next()?;
    Some(values.fold((first, first), |(low, high), value| {
        (low.min(value), high.max(value))
    }))
}

/// The SVG path data (`M x y L x y ...`) of `metric`'s line.
///
/// The viewbox is 0..100 by 0..100 with y growing downward. Rows without the figure break
/// the line. The line is scaled so the lowest value is at the bottom and the highest at
/// the top; a figure that is the same in every row is a line across the middle. Empty when
/// no row has the figure.
#[must_use]
pub fn chart_path(rows: &[SweepRow], metric: SweepMetric) -> String {
    let (Some((angle_low, angle_high)), Some((low, high))) =
        (angle_extent(rows), value_extent(rows, metric))
    else {
        return String::new();
    };
    let span = high - low;
    let mut commands = String::new();
    let mut pen_down = false;
    for row in rows {
        let Some(value) = metric.value(row) else {
            pen_down = false;
            continue;
        };
        let x = x_percent(row.angle_deg, angle_low, angle_high);
        let y = if span < FLAT_SPAN {
            50.0
        } else {
            ((value - low) / span).mul_add(-PLOT_HEIGHT, 100.0 - MARGIN_Y)
        };
        if !commands.is_empty() {
            commands.push(' ');
        }
        let command = if pen_down { 'L' } else { 'M' };
        let _ = write!(commands, "{command} {x:.2} {y:.2}");
        pen_down = true;
    }
    commands
}

/// The row nearest to `fraction` (0 at the left edge of the plot, 1 at the right edge).
#[must_use]
pub fn nearest_row(rows: &[SweepRow], fraction: f64) -> Option<usize> {
    let (low, high) = angle_extent(rows)?;
    let target = fraction.clamp(0.0, 1.0) * 100.0;
    rows.iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| {
            let da = (x_percent(a.angle_deg, low, high) - target).abs();
            let db = (x_percent(b.angle_deg, low, high) - target).abs();
            da.total_cmp(&db)
        })
        .map(|(index, _)| index)
}

/// The readout for row `index`: its angle (a magnitude) and the figures of the lines on
/// the chart (`metrics`), or why the row is not valid.
#[must_use]
pub fn hover_text(rows: &[SweepRow], index: usize, metrics: &[SweepMetric]) -> String {
    let Some(row) = rows.get(index) else {
        return String::new();
    };
    let mut text = format!("{:.2}\u{b0}", row.angle_deg.abs());
    if row.is_current {
        text.push_str(" (current)");
    }
    if !row.is_valid() {
        text.push_str(": not valid");
        if !row.notes.is_empty() {
            text.push_str(" - ");
            text.push_str(&row.notes_text());
        }
        return text;
    }
    for (position, metric) in metrics.iter().enumerate() {
        text.push_str(if position == 0 { ": " } else { ", " });
        let _ = write!(
            text,
            "{} {}{}",
            metric.label(),
            metric.cell_text(row),
            metric.unit()
        );
    }
    text
}

/// What a line's height means: "Brilliance 77.10 to 79.60 %", or a note when no row has
/// the figure.
#[must_use]
pub fn series_range_text(rows: &[SweepRow], metric: SweepMetric) -> String {
    value_extent(rows, metric).map_or_else(
        || format!("{}: no values", metric.label()),
        |(low, high)| format!("{} {low:.2} to {high:.2}{}", metric.label(), metric.unit()),
    )
}
