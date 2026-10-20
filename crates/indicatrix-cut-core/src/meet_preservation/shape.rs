//! Hard proportion limits, distinct from optical penalties. All dimensions are normalized.
use super::{Plane, PreservationPolicy, PreservationViolation};
use indicatrix::geometry::stone_metrics::{SolidMesh, StoneProportions, measure_solid};

/// Shape measurements in width units (table in percentage points).
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ShapeMeasures {
    /// Length divided by width.
    pub aspect: f64,
    /// Table width percentage, absent when there is no table.
    pub table_percent: Option<f64>,
    /// Crown height divided by width.
    pub crown: Option<f64>,
    /// Pavilion depth divided by width.
    pub pavilion: Option<f64>,
    /// Girdle height divided by width.
    pub girdle: Option<f64>,
}
impl ShapeMeasures {
    pub(super) fn measure(
        planes: &[Plane],
        mesh: &SolidMesh,
    ) -> Result<Self, PreservationViolation> {
        let m = measure_solid(planes).ok_or(PreservationViolation::Unrepresentable)?;
        if !m.width_axis.is_finite() || m.width_axis <= 0.0 {
            return Err(PreservationViolation::Unrepresentable);
        }
        let p = StoneProportions::from_solid(&m, mesh, planes);
        Ok(Self {
            aspect: m.length_axis / m.width_axis,
            table_percent: p.table_percent,
            crown: m.crown_height.map(|h| h / m.width_axis),
            pavilion: m.pavilion_depth.map(|h| h / m.width_axis),
            girdle: m.girdle_thickness.map(|h| h / m.width_axis),
        })
    }
    pub(super) fn validate(
        self,
        next: Self,
        policy: PreservationPolicy,
    ) -> Result<(), PreservationViolation> {
        let optional = |before: Option<f64>, after: Option<f64>, limit: f64| match (before, after) {
            (Some(a), Some(b)) => a.is_finite() && b.is_finite() && (a - b).abs() <= limit,
            (None, None) => true,
            // Missing geometry is not a zero-valued measurement. New contacts remain allowed
            // when they introduce a measurable feature within the policy's absolute limit.
            (None, Some(b)) => b.is_finite() && b.abs() <= limit,
            (Some(_), None) => false,
        };
        if ![
            policy.max_aspect_change,
            policy.max_table_percent_change,
            policy.max_depth_change,
            policy.max_girdle_change,
            policy.max_junction_displacement,
        ]
        .into_iter()
        .all(|x| x.is_finite() && x >= 0.0)
            || !next.aspect.is_finite()
            || (self.aspect - next.aspect).abs() > policy.max_aspect_change
            || !optional(
                self.table_percent,
                next.table_percent,
                policy.max_table_percent_change,
            )
            || !optional(self.crown, next.crown, policy.max_depth_change)
            || !optional(self.pavilion, next.pavilion, policy.max_depth_change)
            || !optional(self.girdle, next.girdle, policy.max_girdle_change)
        {
            return Err(PreservationViolation::ShapeLimit);
        }
        Ok(())
    }
}
