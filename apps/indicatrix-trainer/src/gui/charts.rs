//! Chart data: a ring of per-iteration metric points and their scaling into SVG paths.
//!
//! Every chart is drawn by the Slint `Path` element in a 1000 x 400 viewbox (larger values higher,
//! x = iteration). Rust owns the scaling so the UI only strokes a string; a figure that does not
//! exist in a point (`None`, NaN) leaves a gap instead of a zero.

use std::{collections::VecDeque, fmt::Write};

use crate::{
    ChartData,
    stats::{IterationMetricPoint, drop_ratio},
    ui_state::RATIO_GOAL,
};

/// Points kept per run.
pub const MAX_POINTS: usize = 2000;
/// Viewbox width of the chart paths.
pub const VIEW_W: f32 = 1000.0;
/// Viewbox height of the chart paths.
pub const VIEW_H: f32 = 400.0;
/// Direction-cosine goal (README: above 0.7 within 50 rounds).
pub const COSINE_GOAL: f32 = 0.7;
/// Half the width of the tick drawn for a lone point.
const TICK: f32 = 6.0;

/// Where a chart's numbers came out.
#[derive(Clone, Debug, PartialEq)]
pub struct Plot {
    pub commands: String,
    pub reference: String,
    pub y_min: f32,
    pub y_max: f32,
    pub x_first: f32,
    pub x_last: f32,
    pub latest: f32,
}

/// Scales a series into path commands. `None` when there is no finite value to draw.
///
/// The y range is the data range extended to include `reference`; a flat series gets a symmetric
/// margin so it draws as a centred line. A `step` series holds each value until the next x (for
/// figures that only change at a validation pass).
pub fn plot(xs: &[f32], ys: &[Option<f32>], reference: Option<f32>, step: bool) -> Option<Plot> {
    let samples: Vec<(f32, Option<f32>)> = xs
        .iter()
        .zip(ys)
        .filter(|(x, _)| x.is_finite())
        .map(|(&x, &y)| (x, y.filter(|v| v.is_finite())))
        .collect();
    let valid: Vec<(f32, f32)> = samples
        .iter()
        .filter_map(|&(x, y)| y.map(|y| (x, y)))
        .collect();
    let latest = valid.last()?.1;
    let mut lo = valid.iter().map(|p| p.1).fold(f32::INFINITY, f32::min);
    let mut hi = valid.iter().map(|p| p.1).fold(f32::NEG_INFINITY, f32::max);
    if let Some(goal) = reference.filter(|r| r.is_finite()) {
        lo = lo.min(goal);
        hi = hi.max(goal);
    }
    if hi - lo < 1e-9 {
        let pad = if lo.abs() > 1e-6 { lo.abs() * 0.1 } else { 1.0 };
        lo -= pad;
        hi += pad;
    }
    let x_lo = valid.iter().map(|p| p.0).fold(f32::INFINITY, f32::min);
    let x_hi = valid.iter().map(|p| p.0).fold(f32::NEG_INFINITY, f32::max);
    let x_span = x_hi - x_lo;
    let x_of = |x: f32| {
        if x_span > 1e-9 {
            (x - x_lo) / x_span * VIEW_W
        } else {
            VIEW_W / 2.0
        }
    };
    let y_of = |v: f32| VIEW_H * (1.0 - (v - lo) / (hi - lo));

    let mut runs: Vec<Vec<(f32, f32)>> = Vec::new();
    let mut open = false;
    for &(x, y) in &samples {
        match y {
            Some(v) => {
                if !open {
                    runs.push(Vec::new());
                    open = true;
                }
                if let Some(run) = runs.last_mut() {
                    run.push((x_of(x), y_of(v)));
                }
            }
            None => open = false,
        }
    }
    let reference = reference
        .filter(|r| r.is_finite())
        .map(|goal| {
            let y = y_of(goal);
            format!("M 0 {y:.1} L {VIEW_W:.0} {y:.1}")
        })
        .unwrap_or_default();
    Some(Plot {
        commands: runs_to_path(&runs, step),
        reference,
        y_min: lo,
        y_max: hi,
        x_first: x_lo,
        x_last: x_hi,
        latest,
    })
}

/// SVG path commands for the runs of drawn points (one `M` per run).
fn runs_to_path(runs: &[Vec<(f32, f32)>], step: bool) -> String {
    let mut path = String::new();
    for run in runs {
        if !path.is_empty() {
            path.push(' ');
        }
        match run.as_slice() {
            [] => {}
            [(x, y)] => {
                let _ = write!(path, "M {:.1} {y:.1} L {:.1} {y:.1}", x - TICK, x + TICK);
            }
            [(x0, y0), rest @ ..] => {
                let _ = write!(path, "M {x0:.1} {y0:.1}");
                let mut previous_y = *y0;
                for &(x, y) in rest {
                    if step {
                        let _ = write!(path, " L {x:.1} {previous_y:.1}");
                    }
                    let _ = write!(path, " L {x:.1} {y:.1}");
                    previous_y = y;
                }
            }
        }
    }
    path
}

/// A figure for an axis label or a card: enough digits to tell neighbours apart.
pub fn format_figure(value: f32) -> String {
    let magnitude = value.abs();
    if !value.is_finite() {
        "—".to_owned()
    } else if magnitude == 0.0 {
        "0".to_owned()
    } else if magnitude >= 1000.0 {
        format!("{value:.0}")
    } else if magnitude >= 100.0 {
        format!("{value:.1}")
    } else if magnitude >= 1.0 {
        format!("{value:.2}")
    } else if magnitude >= 0.01 {
        format!("{value:.3}")
    } else {
        format!("{value:.1e}")
    }
}

/// What a chart is called, explained and when it has nothing to show.
struct Spec {
    title: &'static str,
    unit: &'static str,
    empty_text: &'static str,
    help: String,
    reference: Option<(f32, String)>,
    step: bool,
}

/// Builds the chart for one series of the points.
fn build_chart(
    spec: &Spec,
    points: &[&IterationMetricPoint],
    value: impl Fn(&IterationMetricPoint) -> Option<f32>,
) -> ChartData {
    let xs: Vec<f32> = points.iter().map(|p| p.iteration as f32).collect();
    let ys: Vec<Option<f32>> = points.iter().map(|&p| value(p)).collect();
    let goal = spec.reference.as_ref().map(|(value, _)| *value);
    let mut chart = ChartData {
        title: spec.title.into(),
        unit: spec.unit.into(),
        empty_text: spec.empty_text.into(),
        help: spec.help.as_str().into(),
        ..ChartData::default()
    };
    if let Some(plot) = plot(&xs, &ys, goal, spec.step) {
        chart.has_data = true;
        chart.commands = plot.commands.into();
        chart.reference = plot.reference.into();
        chart.reference_label = spec
            .reference
            .as_ref()
            .map_or_else(String::new, |(_, label)| label.clone())
            .into();
        chart.latest = format_figure(plot.latest).into();
        chart.y_min = format_figure(plot.y_min).into();
        chart.y_max = format_figure(plot.y_max).into();
        chart.x_first = format!("{:.0}", plot.x_first).into();
        chart.x_last = format!("{:.0}", plot.x_last).into();
    }
    chart
}

/// The model's share of the oracle's validation drop in a point (`None` before a validation).
pub fn ratio_of(point: &IterationMetricPoint) -> Option<f32> {
    point
        .val_improvement
        .and_then(|model| drop_ratio(model, point.val_improvement_oracle))
}

/// Seconds the iteration took on the main thread (rollout plus the wait for the update; see
/// [`IterationMetricPoint::main_thread_secs`]); `None` when no time was recorded.
fn iteration_secs(point: &IterationMetricPoint) -> Option<f32> {
    let secs = point.main_thread_secs();
    (secs.is_finite() && secs > 0.0).then_some(secs)
}

/// The sentence under the seconds chart: where the latest iteration's time went.
fn phase_help(latest: Option<&IterationMetricPoint>) -> String {
    let base =
        "Wall time of one iteration (rollout plus the wait for the update); lower is faster.";
    latest.map_or_else(
        || base.to_owned(),
        |p| {
            format!(
                "{base} Latest: rollout {:.1} s (of which labels {:.1} s), update {:.1} s \
                 (waited {:.1} s).",
                p.rollout_secs, p.label_secs, p.update_secs, p.update_wait_secs
            )
        },
    )
}

/// The live points of the current run and, optionally, an imported run shown instead.
#[derive(Default)]
pub struct ChartStore {
    live: VecDeque<IterationMetricPoint>,
    imported: Option<Vec<IterationMetricPoint>>,
    generation: Option<u64>,
}

impl ChartStore {
    /// Appends a live point; the oldest falls out beyond [`MAX_POINTS`].
    pub fn push(&mut self, point: IterationMetricPoint) {
        self.live.push_back(point);
        while self.live.len() > MAX_POINTS {
            self.live.pop_front();
        }
    }

    pub fn clear(&mut self) {
        self.live.clear();
    }

    /// Records the run generation of a snapshot; returns true (and clears the live points) when
    /// it differs from the previous one.
    pub fn note_generation(&mut self, generation: u64) -> bool {
        let changed = self.generation.is_some_and(|g| g != generation);
        self.generation = Some(generation);
        if changed {
            self.live.clear();
        }
        changed
    }

    pub fn set_imported(&mut self, points: Option<Vec<IterationMetricPoint>>) {
        self.imported = points;
    }

    #[cfg(test)]
    pub fn live_len(&self) -> usize {
        self.live.len()
    }

    /// The reward chart and the six charts of the grid for what is on screen.
    pub fn render(&self) -> (ChartData, Vec<ChartData>) {
        let points: Vec<&IterationMetricPoint> = self.imported.as_ref().map_or_else(
            || self.live.iter().collect(),
            |imported| imported.iter().collect(),
        );
        let spec = |title, unit, empty_text, help: &str| Spec {
            title,
            unit,
            empty_text,
            help: help.to_owned(),
            reference: None,
            step: false,
        };
        let after_iteration = "Appears after the first iteration";
        let reward = build_chart(
            &Spec {
                reference: Some((0.0, "Zero".to_owned())),
                ..spec(
                    "Mean Reward per Step",
                    "",
                    after_iteration,
                    "Smoothed mean reward per environment step; higher is better.",
                )
            },
            &points,
            |p| Some(p.mean_step_reward),
        );
        let loss = build_chart(
            &spec(
                "Imitation Loss",
                "",
                "Appears after the first update",
                "Mean imitation loss of the update; lower is better.",
            ),
            &points,
            |p| {
                (p.imitation_loss.is_finite() && p.imitation_loss > 0.0).then_some(p.imitation_loss)
            },
        );
        let cosine = build_chart(
            &Spec {
                reference: Some((COSINE_GOAL, format!("Goal {COSINE_GOAL}"))),
                ..spec(
                    "Direction Cosine",
                    "",
                    "Appears after the first update",
                    "Cosine between the model's direction and the measured one; higher is better, above 0.7 within 50 rounds is the goal.",
                )
            },
            &points,
            |p| p.train_cosine,
        );
        let accepted = build_chart(
            &spec(
                "Accepted Steps",
                "%",
                after_iteration,
                "Share of steps that changed the design; higher means fewer wasted candidates.",
            ),
            &points,
            |p| Some(p.accepted_frac * 100.0),
        );
        let ratio = build_chart(
            &Spec {
                reference: Some((RATIO_GOAL, format!("Goal {RATIO_GOAL}"))),
                step: true,
                ..spec(
                    "Model / Oracle Ratio",
                    "",
                    "Appears after the first validation",
                    "Validation score drop of the model divided by the oracle's; higher is better, above 0.8 is the goal.",
                )
            },
            &points,
            ratio_of,
        );
        let evals = build_chart(
            &spec(
                "Evaluations per Second",
                "/s",
                after_iteration,
                "Candidate evaluations per second of one iteration; higher is faster.",
            ),
            &points,
            IterationMetricPoint::evaluations_per_sec,
        );
        let seconds = build_chart(
            &spec(
                "Seconds per Iteration",
                "s",
                after_iteration,
                &phase_help(points.last().copied()),
            ),
            &points,
            iteration_secs,
        );
        (reward, vec![loss, cosine, accepted, ratio, evals, seconds])
    }
}

/// A point with every required field set, for the tests of this module tree.
#[cfg(test)]
pub fn test_point(iteration: usize) -> IterationMetricPoint {
    let json = format!(
        r#"{{"iteration":{iteration},"episode":0,"total_steps":0,"steps_per_sec":1.0,
        "mean_step_reward":0.5,"policy_loss":0.0,"value_loss":0.0,"entropy":0.0,
        "clip_fraction":0.0,"solid_closed":true,"brilliance":1.0,"windowing":1.0,
        "extinction":1.0,"yield_pct":1.0}}"#
    );
    serde_json::from_str(&json).expect("a minimal metric point")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn some(values: &[f32]) -> Vec<Option<f32>> {
        values.iter().copied().map(Some).collect()
    }

    #[test]
    fn a_rising_series_runs_from_bottom_left_to_top_right() {
        let plot = plot(&[0.0, 10.0], &some(&[0.0, 1.0]), None, false).expect("plot");
        assert_eq!(plot.commands, "M 0.0 400.0 L 1000.0 0.0");
        assert_eq!((plot.y_min, plot.y_max), (0.0, 1.0));
        assert_eq!((plot.x_first, plot.x_last), (0.0, 10.0));
        assert_eq!(plot.latest, 1.0);
        assert_eq!(plot.reference, "");
    }

    #[test]
    fn nothing_to_draw_gives_none() {
        assert!(plot(&[], &[], None, false).is_none());
        assert!(plot(&[1.0, 2.0], &[None, Some(f32::NAN)], None, false).is_none());
    }

    #[test]
    fn gaps_start_a_new_run() {
        let ys = vec![Some(1.0), Some(2.0), None, Some(3.0), Some(4.0)];
        let plot = plot(&[0.0, 1.0, 2.0, 3.0, 4.0], &ys, None, false).expect("plot");
        assert_eq!(plot.commands.matches('M').count(), 2);
    }

    #[test]
    fn nan_values_leave_gaps_like_none() {
        let ys = vec![Some(1.0), Some(f32::NAN), Some(2.0), Some(3.0)];
        let plot = plot(&[0.0, 1.0, 2.0, 3.0], &ys, None, false).expect("plot");
        assert_eq!(plot.commands.matches('M').count(), 2);
        assert_eq!(plot.latest, 3.0);
    }

    #[test]
    fn a_single_point_is_a_centred_tick() {
        let plot = plot(&[5.0], &some(&[2.0]), None, false).expect("plot");
        assert_eq!(plot.commands, "M 494.0 200.0 L 506.0 200.0");
        assert_eq!((plot.x_first, plot.x_last), (5.0, 5.0));
    }

    #[test]
    fn a_flat_series_gets_a_symmetric_margin() {
        let plot = plot(&[0.0, 1.0, 2.0], &some(&[4.0, 4.0, 4.0]), None, false).expect("plot");
        assert!(plot.y_min < 4.0 && plot.y_max > 4.0);
        assert!(plot.commands.contains("200.0"));
        let zero = super::plot(&[0.0, 1.0], &some(&[0.0, 0.0]), None, false).expect("plot");
        assert_eq!((zero.y_min, zero.y_max), (-1.0, 1.0));
    }

    #[test]
    fn the_reference_extends_the_range_and_is_drawn() {
        let plot = plot(&[0.0, 1.0], &some(&[0.2, 0.4]), Some(0.8), false).expect("plot");
        assert_eq!(plot.y_max, 0.8);
        assert_eq!(plot.reference, "M 0 0.0 L 1000 0.0");
        let inside = super::plot(&[0.0, 1.0], &some(&[0.0, 1.0]), Some(0.5), false).expect("plot");
        assert_eq!(inside.reference, "M 0 200.0 L 1000 200.0");
    }

    #[test]
    fn step_series_hold_the_value_until_the_next_x() {
        let plot = plot(&[0.0, 10.0], &some(&[0.0, 1.0]), None, true).expect("plot");
        assert_eq!(plot.commands, "M 0.0 400.0 L 1000.0 400.0 L 1000.0 0.0");
    }

    #[test]
    fn figures_keep_a_useful_number_of_digits() {
        assert_eq!(format_figure(0.0), "0");
        assert_eq!(format_figure(1234.56), "1235");
        assert_eq!(format_figure(123.456), "123.5");
        assert_eq!(format_figure(std::f32::consts::PI), "3.14");
        assert_eq!(format_figure(0.12345), "0.123");
        assert_eq!(format_figure(0.000_123_4), "1.2e-4");
        assert_eq!(format_figure(f32::NAN), "—");
    }

    #[test]
    fn the_store_keeps_at_most_max_points() {
        let mut store = ChartStore::default();
        for i in 0..MAX_POINTS + 25 {
            store.push(test_point(i));
        }
        assert_eq!(store.live_len(), MAX_POINTS);
        assert_eq!(store.live.front().map(|p| p.iteration), Some(25));
    }

    #[test]
    fn a_new_generation_clears_the_live_points() {
        let mut store = ChartStore::default();
        assert!(!store.note_generation(1));
        store.push(test_point(1));
        assert!(!store.note_generation(1));
        assert_eq!(store.live_len(), 1);
        assert!(store.note_generation(2));
        assert_eq!(store.live_len(), 0);
    }

    #[test]
    fn rendering_shows_empty_states_without_data() {
        let store = ChartStore::default();
        let (reward, grid) = store.render();
        assert!(!reward.has_data);
        assert_eq!(grid.len(), 6);
        assert!(
            grid.iter()
                .all(|chart| !chart.has_data && !chart.empty_text.is_empty())
        );
    }

    #[test]
    fn rendering_draws_what_the_points_hold() {
        let mut store = ChartStore::default();
        for i in 1..=3 {
            let mut point = test_point(i);
            point.imitation_loss = 1.0 / i as f32;
            point.train_cosine = (i > 1).then_some(0.5);
            store.push(point);
        }
        let (reward, grid) = store.render();
        assert!(reward.has_data);
        assert!(grid[0].has_data, "loss");
        assert!(grid[1].has_data, "cosine");
        assert_eq!(grid[1].reference_label.as_str(), "Goal 0.7");
        assert!(!grid[3].has_data, "no validation yet");
        assert!(!grid[5].has_data, "no timings recorded");
    }

    #[test]
    fn an_imported_run_replaces_the_live_one_on_screen() {
        let mut store = ChartStore::default();
        store.push(test_point(1));
        store.set_imported(Some(Vec::new()));
        assert!(!store.render().0.has_data);
        store.set_imported(None);
        assert!(store.render().0.has_data);
    }

    #[test]
    fn the_ratio_needs_a_validation_and_a_usable_oracle() {
        let mut point = test_point(1);
        assert_eq!(ratio_of(&point), None);
        point.val_improvement = Some(4.0);
        point.val_improvement_oracle = Some(5.0);
        assert_eq!(ratio_of(&point), Some(0.8));
        point.val_improvement_oracle = Some(0.0);
        assert_eq!(ratio_of(&point), None);
    }

    #[test]
    fn the_phase_help_names_the_latest_timings() {
        assert!(phase_help(None).starts_with("Wall time"));
        let mut point = test_point(1);
        point.rollout_secs = 2.0;
        point.label_secs = 0.5;
        point.update_secs = 1.0;
        let text = phase_help(Some(&point));
        assert!(text.contains("rollout 2.0 s") && text.contains("update 1.0 s"));
        // No update_of_iteration: a file from before the wait field, the old sum.
        assert_eq!(iteration_secs(&point), Some(3.0));
        // An overlapping update: only the wait adds to the rollout.
        point.update_secs = 6.0;
        point.update_wait_secs = 0.5;
        point.update_of_iteration = Some(0);
        let text = phase_help(Some(&point));
        assert!(text.contains("update 6.0 s (waited 0.5 s)"), "{text}");
        assert_eq!(iteration_secs(&point), Some(2.5));
    }
}
