//! The optical numbers the Retarget dialog shows under the table.
//!
//! Three columns, so the cutter can see what the material change does and what the retarget
//! wins back:
//!
//! 1. the current design in its current material,
//! 2. the current design, unchanged, in the target material,
//! 3. the retargeted design in the target material.
//!
//! Each column is windowing, brilliance and extinction in percent, from the same fast
//! table-up measurement the Optimize tab scores with
//! (`indicatrix_cut_core::evaluate_objective_under`, about 2 ms) under the lighting preset
//! the viewport uses. They are comparable with each other, not with a full tilt sweep.

use super::validity::StoneAnalysis;
use glam::DVec3;
use indicatrix::{
    geometry::{GpuFacetPlane, StoneGeometry, tool::ToolPrimitive},
    optics::{LightingPreset, materials::GemMaterial},
};
use indicatrix_cut_core::{ObjectiveFidelity, evaluate_objective_geom_under};

/// What shows when a number is not available.
pub const NOT_AVAILABLE: &str = "n/a";

/// One column of the metrics table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MetricColumn {
    /// Percentage of light leaking straight out through the pavilion.
    pub windowing_pct: f32,
    /// Percentage of light returned to the eye with the table facing up.
    pub brilliance_pct: f32,
    /// Percentage of light trapped or lost.
    pub extinction_pct: f32,
}

/// The three columns; each is `None` when it could not be measured.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct RetargetMetrics {
    /// The current design in its current material (needs the design to name a material).
    pub current_in_current: Option<MetricColumn>,
    /// The current design, unchanged, in the target material.
    pub current_in_target: Option<MetricColumn>,
    /// The retargeted design in the target material.
    pub retargeted_in_target: Option<MetricColumn>,
}

/// Measures one column for the flat `planes` (`(normal, offset)`, the design's own
/// convention) in `gem` under `lighting`.
#[must_use]
pub fn measure_column(
    planes: &[(DVec3, f64)],
    gem: &GemMaterial,
    lighting: LightingPreset,
) -> MetricColumn {
    measure_column_with_tools(planes, &[], gem, lighting)
}

/// [`measure_column`] for the stone `analysis` describes, concave tools included.
///
/// The column is the figure of the stone the cutter holds and the Compare window draws,
/// not of its flat shell. Without concave tiers it is [`measure_column`] of the analysis'
/// planes, bit for bit.
#[must_use]
pub fn measure_analysis_column(
    analysis: &StoneAnalysis,
    gem: &GemMaterial,
    lighting: LightingPreset,
) -> MetricColumn {
    measure_column_with_tools(&analysis.planes, &analysis.tools(), gem, lighting)
}

/// [`measure_column`] with the concave `tools` cut out of `planes`.
#[must_use]
pub fn measure_column_with_tools(
    planes: &[(DVec3, f64)],
    tools: &[ToolPrimitive],
    gem: &GemMaterial,
    lighting: LightingPreset,
) -> MetricColumn {
    let gpu: Vec<GpuFacetPlane> = planes
        .iter()
        .map(|&(normal, offset)| {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "f32 is the GPU plane precision; offsets are in the normal float range"
            )]
            let d = -offset as f32;
            GpuFacetPlane::new(normal.as_vec3(), d)
        })
        .collect();
    let components = evaluate_objective_geom_under(
        StoneGeometry {
            planes: &gpu,
            tools,
        },
        gem,
        ObjectiveFidelity::Fast,
        lighting,
    );
    MetricColumn {
        windowing_pct: components.windowing_pct,
        brilliance_pct: components.tilt_brilliance_pct,
        extinction_pct: components.extinction_pct,
    }
}

fn cell(column: Option<MetricColumn>, pick: fn(MetricColumn) -> f32) -> String {
    column.map_or_else(
        || NOT_AVAILABLE.to_string(),
        |column| format!("{:.1} %", pick(column)),
    )
}

impl RetargetMetrics {
    /// The nine table cells, row by row (windowing, brilliance, extinction), each row in the
    /// column order of this struct's fields.
    #[must_use]
    pub fn cells(&self) -> Vec<String> {
        let columns = [
            self.current_in_current,
            self.current_in_target,
            self.retargeted_in_target,
        ];
        let rows: [fn(MetricColumn) -> f32; 3] = [
            |column| column.windowing_pct,
            |column| column.brilliance_pct,
            |column| column.extinction_pct,
        ];
        rows.iter()
            .flat_map(|&pick| columns.iter().map(move |&column| cell(column, pick)))
            .collect()
    }

    /// `true` when at least one column was measured.
    #[must_use]
    pub const fn any(&self) -> bool {
        self.current_in_current.is_some()
            || self.current_in_target.is_some()
            || self.retargeted_in_target.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn column(w: f32, b: f32, e: f32) -> MetricColumn {
        MetricColumn {
            windowing_pct: w,
            brilliance_pct: b,
            extinction_pct: e,
        }
    }

    #[test]
    fn cells_run_row_by_row_in_column_order() {
        let metrics = RetargetMetrics {
            current_in_current: Some(column(1.0, 2.0, 3.0)),
            current_in_target: None,
            retargeted_in_target: Some(column(4.0, 5.0, 6.0)),
        };
        assert_eq!(
            metrics.cells(),
            vec![
                "1.0 %", "n/a", "4.0 %", // windowing
                "2.0 %", "n/a", "5.0 %", // brilliance
                "3.0 %", "n/a", "6.0 %", // extinction
            ]
        );
        assert!(metrics.any());
        assert!(!RetargetMetrics::default().any());
    }

    #[test]
    fn the_standard_brilliant_measures_in_range_in_two_materials() {
        use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};

        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let solved = design.solve().expect("every tier is pinned");
        let planes = design.planes_from_solved(&solved);
        let lighting = LightingPreset::RingLights;
        for gem in [GemMaterial::diamond(), GemMaterial::sapphire()] {
            let column = measure_column(&planes, &gem, lighting);
            for value in [
                column.windowing_pct,
                column.brilliance_pct,
                column.extinction_pct,
            ] {
                assert!((0.0..=100.0).contains(&value), "{}: {value}", gem.name);
            }
        }
    }

    /// Retarget's columns measure the stone with its concave tools; a flat design's column
    /// is exactly the planes-only one it always was.
    #[test]
    fn the_analysis_column_measures_the_stone_with_its_tools() {
        use crate::retarget::validity::analyze;
        use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};

        let gem = GemMaterial::diamond();
        let lighting = LightingPreset::RingLights;

        let flat_design = Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let flat = analyze(&flat_design, false).expect("the standard brilliant closes");
        assert_eq!(flat.tools().len(), 0);
        assert_eq!(
            measure_analysis_column(&flat, &gem, lighting),
            measure_column(&flat.planes, &gem, lighting)
        );

        let concave = analyze(&Design::concave_fixture(), false).expect("the fixture closes");
        assert_eq!(concave.tools().len(), 12, "one tool per placement");
        assert_ne!(
            measure_analysis_column(&concave, &gem, lighting),
            measure_column(&concave.planes, &gem, lighting),
            "the grooves and dimples change the column"
        );
    }
}
