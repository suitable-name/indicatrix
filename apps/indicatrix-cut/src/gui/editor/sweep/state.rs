//! What the Angle Sweep dialog remembers between callbacks: the sweep that is running, the
//! rows being shown, which lines the chart draws, and the selected and hovered rows.
//!
//! Plain data and decisions, no Slint types, so they are tested without a window.

use super::run::RunHandle;
use indicatrix_editor::sweep::{SweepMetric, SweepOutcome, SweepRow, nearest_row};

/// The figures drawn as lines when the dialog is first opened.
pub(super) const DEFAULT_SHOWN: [SweepMetric; 3] = [
    SweepMetric::Brilliance,
    SweepMetric::Windowing,
    SweepMetric::Extinction,
];

/// The angle "Use this angle" would set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pick {
    /// The row in the table.
    pub row: usize,
    /// The tier (position in the design) the sweep varied.
    pub tier: usize,
    /// The angle to set, degrees, signed as the tier stores it (a pavilion tier's is
    /// negative). Anything shown to a person uses its magnitude.
    pub angle_deg: f64,
}

/// The dialog's state.
pub(super) struct Dialog {
    /// The design tier (position) of each entry of the tier combo.
    pub tiers: Vec<usize>,
    /// The sweep that is running, if one is.
    pub run: Option<RunHandle>,
    /// The design's generation when the shown or running sweep started; "Use this angle"
    /// refuses once the design has moved on.
    pub generation: u64,
    /// The finished or stopped sweep being shown.
    pub outcome: Option<SweepOutcome>,
    /// The figures whose lines the chart draws.
    pub shown: Vec<SweepMetric>,
    /// The selected row (a click on the table or the chart).
    pub selected: Option<usize>,
    /// The row under the pointer on the chart.
    pub hover: Option<usize>,
}

impl Dialog {
    pub(super) fn new() -> Self {
        Self {
            tiers: Vec::new(),
            run: None,
            generation: 0,
            outcome: None,
            shown: DEFAULT_SHOWN.to_vec(),
            selected: None,
            hover: None,
        }
    }

    /// The rows being shown; none before a sweep has finished.
    pub(super) fn rows(&self) -> &[SweepRow] {
        self.outcome
            .as_ref()
            .map_or(&[][..], |outcome| outcome.rows.as_slice())
    }

    /// Forgets the rows being shown, and the selection and hover with them.
    pub(super) fn clear_results(&mut self) {
        self.outcome = None;
        self.selected = None;
        self.hover = None;
    }

    /// The figures the table has a column for: the fast ones, the yield when the rough has
    /// a volume, and the tilt averages when the sweep asked for them.
    pub(super) fn columns(&self) -> Vec<SweepMetric> {
        self.outcome.as_ref().map_or_else(Vec::new, columns_of)
    }

    /// The figures drawn as lines: the shown ones that have a column.
    pub(super) fn lines(&self) -> Vec<SweepMetric> {
        self.columns()
            .into_iter()
            .filter(|metric| self.shown.contains(metric))
            .collect()
    }

    /// Shows or hides the line of `metric`.
    pub(super) fn toggle(&mut self, metric: SweepMetric) {
        if let Some(at) = self.shown.iter().position(|shown| *shown == metric) {
            self.shown.remove(at);
        } else {
            self.shown.push(metric);
        }
    }

    /// Selects row `index`; `false` (and nothing changes) when there is no such row.
    pub(super) fn select(&mut self, index: usize) -> bool {
        if index < self.rows().len() {
            self.selected = Some(index);
            true
        } else {
            false
        }
    }

    /// Points at the row nearest to `fraction` of the way across the plot (`None` for a
    /// pointer that has left it). Returns whether the hovered row changed.
    pub(super) fn hover_at(&mut self, fraction: Option<f64>) -> bool {
        let hovered = fraction.and_then(|fraction| nearest_row(self.rows(), fraction));
        let changed = hovered != self.hover;
        self.hover = hovered;
        changed
    }

    /// The angle "Use this angle" sets: the selected row, when it is a valid angle other
    /// than the one the design already has.
    pub(super) fn pick(&self) -> Option<Pick> {
        let outcome = self.outcome.as_ref()?;
        let row = self.selected?;
        let selected = outcome.rows.get(row)?;
        (selected.is_valid() && !selected.is_current).then_some(Pick {
            row,
            tier: outcome.tier,
            angle_deg: selected.angle_deg,
        })
    }

    /// Records that row `row` was made the design's angle: it is now the current row, and
    /// `generation` is the design's generation after that edit (the other rows stay true,
    /// because each is the design with only that tier's angle changed).
    pub(super) fn mark_applied(&mut self, row: usize, generation: u64) {
        if let Some(outcome) = &mut self.outcome {
            for (at, entry) in outcome.rows.iter_mut().enumerate() {
                entry.is_current = at == row;
            }
            if let Some(angle_deg) = outcome.rows.get(row).map(|entry| entry.angle_deg) {
                outcome.current_deg = angle_deg;
            }
        }
        self.generation = generation;
    }
}

/// The figures `outcome` has a column for.
pub(super) fn columns_of(outcome: &SweepOutcome) -> Vec<SweepMetric> {
    SweepMetric::ALL
        .into_iter()
        .filter(|metric| match metric {
            SweepMetric::TiltBrilliance
            | SweepMetric::TiltWindowing
            | SweepMetric::TiltExtinction => outcome.tilt_average,
            SweepMetric::Yield => outcome.rows.iter().any(|row| metric.value(row).is_some()),
            _ => true,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_editor::sweep::{RowMetrics, TiltAverages};

    fn figures(yield_pct: Option<f64>, tilt: bool) -> RowMetrics {
        RowMetrics {
            brilliance_pct: 80.0,
            windowing_pct: 10.0,
            extinction_pct: 5.0,
            fire_index: 1.0,
            scintillation_pct: 20.0,
            yield_pct,
            warning_count: 0,
            has_girdle: true,
            tilt: tilt.then_some(TiltAverages {
                brilliance_pct: 70.0,
                windowing_pct: 12.0,
                extinction_pct: 8.0,
            }),
        }
    }

    fn row(angle_deg: f64, is_current: bool, valid: bool) -> SweepRow {
        SweepRow {
            angle_deg,
            is_current,
            metrics: valid.then(|| figures(Some(30.0), false)),
            notes: Vec::new(),
        }
    }

    fn dialog_with(rows: Vec<SweepRow>, tilt_average: bool) -> Dialog {
        let mut dialog = Dialog::new();
        dialog.outcome = Some(SweepOutcome {
            tier: 5,
            tier_name: "Pavilion Main".to_owned(),
            current_deg: -41.0,
            requested: rows.len(),
            cancelled: false,
            tilt_average,
            rows,
        });
        dialog
    }

    /// A pavilion sweep, flattest first like the engine's: 40, 41 (the current one) and 42
    /// degrees, stored negative.
    fn three_rows() -> Vec<SweepRow> {
        vec![
            row(-40.0, false, false),
            row(-41.0, true, true),
            row(-42.0, false, true),
        ]
    }

    #[test]
    fn the_chart_starts_with_the_three_main_lines() {
        let dialog = dialog_with(three_rows(), false);
        assert_eq!(
            dialog.lines(),
            vec![
                SweepMetric::Brilliance,
                SweepMetric::Windowing,
                SweepMetric::Extinction
            ]
        );
    }

    #[test]
    fn a_line_is_toggled_on_and_off_and_needs_a_column() {
        let mut dialog = dialog_with(three_rows(), false);
        dialog.toggle(SweepMetric::Fire);
        assert!(dialog.lines().contains(&SweepMetric::Fire));
        dialog.toggle(SweepMetric::Fire);
        assert!(!dialog.lines().contains(&SweepMetric::Fire));
        // A tilt line is asked for but the sweep has no tilt column, so nothing is drawn.
        dialog.toggle(SweepMetric::TiltBrilliance);
        assert!(!dialog.lines().contains(&SweepMetric::TiltBrilliance));
    }

    #[test]
    fn tilt_columns_only_come_with_a_tilt_sweep_and_yield_only_with_a_value() {
        let plain = dialog_with(three_rows(), false).columns();
        assert_eq!(plain.len(), 6);
        assert!(!plain.contains(&SweepMetric::TiltBrilliance));
        let tilted = dialog_with(three_rows(), true).columns();
        assert_eq!(tilted.len(), 9);

        let mut no_volume = row(-41.0, true, true);
        if let Some(metrics) = &mut no_volume.metrics {
            metrics.yield_pct = None;
        }
        let columns = dialog_with(vec![no_volume], false).columns();
        assert!(!columns.contains(&SweepMetric::Yield));
        assert_eq!(Dialog::new().columns(), Vec::<SweepMetric>::new());
    }

    #[test]
    fn only_a_valid_row_other_than_the_current_one_can_be_used() {
        let mut dialog = dialog_with(three_rows(), false);
        assert_eq!(dialog.pick(), None, "nothing is selected");
        assert!(dialog.select(1));
        assert_eq!(
            dialog.pick(),
            None,
            "the current angle is already the design's"
        );
        assert!(dialog.select(0));
        assert_eq!(dialog.pick(), None, "the angle gives no stone");
        assert!(dialog.select(2));
        assert_eq!(
            dialog.pick(),
            Some(Pick {
                row: 2,
                tier: 5,
                angle_deg: -42.0
            }),
            "the pick keeps the stored sign, which is what the edit takes"
        );
        assert!(!dialog.select(3), "there is no fourth row");
        assert_eq!(dialog.selected, Some(2));
    }

    #[test]
    fn the_pointer_finds_the_nearest_row_and_reports_a_change_only_once() {
        let mut dialog = dialog_with(three_rows(), false);
        assert!(dialog.hover_at(Some(0.0)));
        assert_eq!(dialog.hover, Some(0));
        assert!(!dialog.hover_at(Some(0.05)), "still the first row");
        assert!(dialog.hover_at(Some(1.0)));
        assert_eq!(dialog.hover, Some(2));
        assert!(dialog.hover_at(None), "the pointer left the plot");
        assert_eq!(dialog.hover, None);
        assert!(!dialog.hover_at(None));
    }

    #[test]
    fn using_an_angle_moves_the_current_mark_and_the_generation() {
        let mut dialog = dialog_with(three_rows(), false);
        dialog.generation = 3;
        dialog.mark_applied(2, 4);
        let rows = dialog.rows();
        assert!(rows[2].is_current && !rows[1].is_current && !rows[0].is_current);
        assert_eq!(dialog.generation, 4);
        assert_eq!(dialog.outcome.as_ref().map(|o| o.current_deg), Some(-42.0));
    }

    #[test]
    fn clearing_the_results_forgets_the_selection_too() {
        let mut dialog = dialog_with(three_rows(), false);
        assert!(dialog.select(0));
        dialog.hover_at(Some(0.5));
        dialog.clear_results();
        assert_eq!(dialog.rows(), Vec::<SweepRow>::new().as_slice());
        assert_eq!((dialog.selected, dialog.hover), (None, None));
    }
}
