//! The figures a sweep scores per angle, and which angle is best in each.

use super::SweepRow;

/// Two figures closer than this read the same at the table's two decimals, so both are
/// the best when they tie for it.
const TIE_TOLERANCE: f64 = 0.005;

/// One figure of a sweep row.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SweepMetric {
    /// Table-up brilliance (higher is better).
    Brilliance,
    /// Table-up windowing (lower is better).
    Windowing,
    /// Table-up extinction (lower is better).
    Extinction,
    /// The fire index (higher is more fire).
    Fire,
    /// Scintillation (higher is more sparkle).
    Scintillation,
    /// The yield from the rough (higher is better).
    Yield,
    /// Tilt-averaged brilliance (higher is better).
    TiltBrilliance,
    /// Tilt-averaged windowing (lower is better).
    TiltWindowing,
    /// Tilt-averaged extinction (lower is better).
    TiltExtinction,
}

impl SweepMetric {
    /// Every figure, in table order: the fast ones, then the tilt averages.
    pub const ALL: [Self; 9] = [
        Self::Brilliance,
        Self::Windowing,
        Self::Extinction,
        Self::Fire,
        Self::Scintillation,
        Self::Yield,
        Self::TiltBrilliance,
        Self::TiltWindowing,
        Self::TiltExtinction,
    ];

    /// The figure at position `index` of [`Self::ALL`].
    #[must_use]
    pub fn from_index(index: usize) -> Option<Self> {
        Self::ALL.get(index).copied()
    }

    /// The position of the figure in [`Self::ALL`].
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Brilliance => 0,
            Self::Windowing => 1,
            Self::Extinction => 2,
            Self::Fire => 3,
            Self::Scintillation => 4,
            Self::Yield => 5,
            Self::TiltBrilliance => 6,
            Self::TiltWindowing => 7,
            Self::TiltExtinction => 8,
        }
    }

    /// The name of the figure in a sentence.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Brilliance => "Brilliance",
            Self::Windowing => "Windowing",
            Self::Extinction => "Extinction",
            Self::Fire => "Fire",
            Self::Scintillation => "Scintillation",
            Self::Yield => "Yield",
            Self::TiltBrilliance => "Tilt brilliance",
            Self::TiltWindowing => "Tilt windowing",
            Self::TiltExtinction => "Tilt extinction",
        }
    }

    /// The column title in the table.
    #[must_use]
    pub const fn column_title(self) -> &'static str {
        match self {
            Self::Brilliance => "Brilliance %",
            Self::Windowing => "Windowing %",
            Self::Extinction => "Extinction %",
            Self::Fire => "Fire",
            Self::Scintillation => "Scint. %",
            Self::Yield => "Yield %",
            Self::TiltBrilliance => "Tilt brill. %",
            Self::TiltWindowing => "Tilt wind. %",
            Self::TiltExtinction => "Tilt ext. %",
        }
    }

    /// The column name in the CSV header.
    #[must_use]
    pub const fn csv_name(self) -> &'static str {
        match self {
            Self::Brilliance => "brilliance_pct",
            Self::Windowing => "windowing_pct",
            Self::Extinction => "extinction_pct",
            Self::Fire => "fire_index",
            Self::Scintillation => "scintillation_pct",
            Self::Yield => "yield_pct",
            Self::TiltBrilliance => "tilt_brilliance_pct",
            Self::TiltWindowing => "tilt_windowing_pct",
            Self::TiltExtinction => "tilt_extinction_pct",
        }
    }

    /// The unit written after a value: `" %"`, or nothing for the fire index.
    #[must_use]
    pub const fn unit(self) -> &'static str {
        match self {
            Self::Fire => "",
            _ => " %",
        }
    }

    /// Whether a larger figure is the better one.
    #[must_use]
    pub const fn higher_is_better(self) -> bool {
        !matches!(
            self,
            Self::Windowing | Self::Extinction | Self::TiltWindowing | Self::TiltExtinction
        )
    }

    /// Whether the figure is one of the tilt averages, which only a sweep that asked
    /// for them carries.
    #[must_use]
    pub const fn is_tilt(self) -> bool {
        matches!(
            self,
            Self::TiltBrilliance | Self::TiltWindowing | Self::TiltExtinction
        )
    }

    /// The figure of `row`; `None` when the row is not valid or lacks the figure.
    #[must_use]
    pub fn value(self, row: &SweepRow) -> Option<f64> {
        let metrics = row.metrics.as_ref()?;
        Some(match self {
            Self::Brilliance => f64::from(metrics.brilliance_pct),
            Self::Windowing => f64::from(metrics.windowing_pct),
            Self::Extinction => f64::from(metrics.extinction_pct),
            Self::Fire => f64::from(metrics.fire_index),
            Self::Scintillation => f64::from(metrics.scintillation_pct),
            Self::Yield => return metrics.yield_pct,
            Self::TiltBrilliance => f64::from(metrics.tilt.as_ref()?.brilliance_pct),
            Self::TiltWindowing => f64::from(metrics.tilt.as_ref()?.windowing_pct),
            Self::TiltExtinction => f64::from(metrics.tilt.as_ref()?.extinction_pct),
        })
    }

    /// The figure of `row` as a table cell: two decimals, or `-` when there is none.
    #[must_use]
    pub fn cell_text(self, row: &SweepRow) -> String {
        self.value(row)
            .map_or_else(|| "-".to_owned(), |value| format!("{value:.2}"))
    }
}

/// Which figures of one row are the best of the sweep.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BestFlags {
    /// The row has the best brilliance.
    pub brilliance: bool,
    /// The row has the best (lowest) windowing.
    pub windowing: bool,
    /// The row has the best (lowest) extinction.
    pub extinction: bool,
    /// The row has the most fire.
    pub fire: bool,
    /// The row has the most scintillation.
    pub scintillation: bool,
    /// The row has the best yield.
    pub yield_pct: bool,
    /// The row has the best tilt-averaged brilliance.
    pub tilt_brilliance: bool,
    /// The row has the best (lowest) tilt-averaged windowing.
    pub tilt_windowing: bool,
    /// The row has the best (lowest) tilt-averaged extinction.
    pub tilt_extinction: bool,
}

impl BestFlags {
    /// Whether the row is the best in `metric`.
    #[must_use]
    pub const fn get(self, metric: SweepMetric) -> bool {
        match metric {
            SweepMetric::Brilliance => self.brilliance,
            SweepMetric::Windowing => self.windowing,
            SweepMetric::Extinction => self.extinction,
            SweepMetric::Fire => self.fire,
            SweepMetric::Scintillation => self.scintillation,
            SweepMetric::Yield => self.yield_pct,
            SweepMetric::TiltBrilliance => self.tilt_brilliance,
            SweepMetric::TiltWindowing => self.tilt_windowing,
            SweepMetric::TiltExtinction => self.tilt_extinction,
        }
    }

    const fn mark(&mut self, metric: SweepMetric) {
        match metric {
            SweepMetric::Brilliance => self.brilliance = true,
            SweepMetric::Windowing => self.windowing = true,
            SweepMetric::Extinction => self.extinction = true,
            SweepMetric::Fire => self.fire = true,
            SweepMetric::Scintillation => self.scintillation = true,
            SweepMetric::Yield => self.yield_pct = true,
            SweepMetric::TiltBrilliance => self.tilt_brilliance = true,
            SweepMetric::TiltWindowing => self.tilt_windowing = true,
            SweepMetric::TiltExtinction => self.tilt_extinction = true,
        }
    }
}

/// For every row, in which figures it is the best of the sweep.
///
/// Only valid rows compete. A row is best when it is within 0.005 of the best figure
/// (so rows that read the same at the table's two decimals are all marked), and a figure
/// that is the same in every row marks none: nothing stands out in it.
#[must_use]
pub fn best_flags(rows: &[SweepRow]) -> Vec<BestFlags> {
    let mut flags = vec![BestFlags::default(); rows.len()];
    for metric in SweepMetric::ALL {
        let values: Vec<Option<f64>> = rows.iter().map(|row| metric.value(row)).collect();
        let (low, high) = values
            .iter()
            .flatten()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), &value| {
                (low.min(value), high.max(value))
            });
        if high - low <= TIE_TOLERANCE {
            continue;
        }
        let best = if metric.higher_is_better() { high } else { low };
        for (flag, value) in flags.iter_mut().zip(&values) {
            if value.is_some_and(|value| (value - best).abs() <= TIE_TOLERANCE) {
                flag.mark(metric);
            }
        }
    }
    flags
}
