//! Read-only measurement for the census (follow-ups plan 2026-10-19, section 4.3).
//!
//! Nothing here judges a candidate or changes a verdict: the product never calls it. The
//! module is a child of `network` (declared there with `#[path]`) so it can read the
//! baseline's private fields without widening any of them.
use crate::design::Design;
use crate::meet_preservation::{
    MeetPreservationBaseline, PreservationPolicy, PreservationViolation, ShapeMeasures,
    tangent_changes,
};
use indicatrix::geometry::meet_solver::SolveControl;

/// What the baseline captured, as plain counts for one census row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BaselineStructure {
    /// Protected junctions of the original network.
    pub junctions: usize,
    /// Boundary edges of the original face cycles.
    pub edges: usize,
    /// Face cycles of the original (one per live face).
    pub faces: usize,
    /// Authored surfaces that were live (a face of the stone) at capture.
    pub live_facets: usize,
    /// Tiers whose inward reach could not be read at capture; a turned one of them is
    /// granted the half-length fallback.
    pub reach_fallbacks_possible: usize,
}

/// Shape deltas of a candidate against the baseline's original, and the travel the gate
/// would grant the candidate's own turn. Every delta is `candidate - original`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeDeltas {
    /// Change of length/width.
    pub d_aspect: f64,
    /// Change of the table size in percentage points; `None` when either stone has no table.
    pub d_table_pts: Option<f64>,
    /// Change of the crown height in width units; `None` when either stone has no crown.
    pub d_crown: Option<f64>,
    /// Change of the pavilion depth in width units; `None` when either has no pavilion.
    pub d_pavilion: Option<f64>,
    /// Change of the girdle height in width units; `None` when either has no girdle.
    pub d_girdle: Option<f64>,
    /// Largest original-junction displacement, in units of the stone's extent (the gate's own
    /// `PreservationFacts::shape_distance`).
    pub junction_disp: f64,
    /// The turn's travel in width units (what the depth and girdle bounds are measured in).
    pub travel_width: f64,
    /// The same travel in units of the stone's extent (what the junction bound is measured in).
    pub travel_scale: f64,
}

impl ShapeDeltas {
    /// Depth change left after the turn's travel: `max(|d_crown|, |d_pavilion|) -
    /// travel_width`; `None` when the stone has neither measure.
    #[must_use]
    pub fn excess_depth(&self) -> Option<f64> {
        [self.d_crown, self.d_pavilion]
            .into_iter()
            .flatten()
            .map(f64::abs)
            .reduce(f64::max)
            .map(|delta| delta - self.travel_width)
    }

    /// Girdle height change left after the travel: `|d_girdle| - travel_width`.
    #[must_use]
    pub fn excess_girdle(&self) -> Option<f64> {
        self.d_girdle.map(|delta| delta.abs() - self.travel_width)
    }

    /// Junction displacement left after the travel: `junction_disp - travel_scale`.
    #[must_use]
    pub const fn excess_junction(&self) -> f64 {
        self.junction_disp - self.travel_scale
    }
}

const fn delta(before: Option<f64>, after: Option<f64>) -> Option<f64> {
    match (before, after) {
        (Some(a), Some(b)) => Some(b - a),
        _ => None,
    }
}

impl MeetPreservationBaseline {
    /// The original stone's shape measures (width units, table in percentage points).
    #[must_use]
    pub const fn shape(&self) -> ShapeMeasures {
        self.shape
    }

    /// The original stone's width in model units.
    #[must_use]
    pub const fn width(&self) -> f64 {
        self.width
    }

    /// Counts of what the capture found.
    #[must_use]
    pub fn structure(&self) -> BaselineStructure {
        BaselineStructure {
            junctions: self.junctions.len(),
            edges: self.edges.len(),
            faces: self.cycles.len(),
            live_facets: self.live.len(),
            reach_fallbacks_possible: self.tier_reach.iter().filter(|r| r.is_none()).count(),
        }
    }

    /// The largest junction slack in units of the stone's extent.
    ///
    /// It is how far the original design itself misses its own meets: zero up to rounding for an
    /// exact design, the masts' quantisation for an imported one, at most the identification
    /// radius.
    #[must_use]
    pub fn max_slack(&self) -> f64 {
        self.junctions.iter().map(|j| j.slack).fold(0.0, f64::max) / self.scale
    }

    /// The identification radius in units of the stone's extent.
    #[must_use]
    pub fn identify_radius(&self) -> f64 {
        self.identify / self.scale
    }

    /// The raw shape deltas of `candidate`, measured under [`PreservationPolicy::CENSUS`]
    /// (limits no stone reaches), with the travel the gate would grant its own turn.
    ///
    /// The candidate is solved and its topology still has to correspond to the original's:
    /// the measurement needs the gate's junction mapping for `junction_disp`.
    ///
    /// The travel is read here and the policy's own turn grant switched off (the census
    /// limits are far wider than any grant), so the baseline's turn-fallback counter is not
    /// touched by a measurement.
    ///
    /// # Errors
    /// The [`PreservationViolation`] that stops the measurement: a candidate that is stale,
    /// does not solve, loses or detaches an original meet or facet, or loses a measure the
    /// original had.
    pub fn measure_deltas(&self, candidate: &Design) -> Result<ShapeDeltas, PreservationViolation> {
        let policy = PreservationPolicy {
            bounds_follow_turn: false,
            ..PreservationPolicy::CENSUS
        };
        let facts = self.validate_controlled(candidate, policy, &SolveControl::default())?;
        let travel = self.turn_travel(&tangent_changes(&self.original, candidate));
        let travel = if travel.is_finite() && travel > 0.0 {
            travel
        } else {
            0.0
        };
        let (before, after) = (self.shape, facts.shape);
        Ok(ShapeDeltas {
            d_aspect: after.aspect - before.aspect,
            d_table_pts: delta(before.table_percent, after.table_percent),
            d_crown: delta(before.crown, after.crown),
            d_pavilion: delta(before.pavilion, after.pavilion),
            d_girdle: delta(before.girdle, after.girdle),
            junction_disp: facts.shape_distance,
            travel_width: travel / self.width,
            travel_scale: travel / self.scale,
        })
    }
}
