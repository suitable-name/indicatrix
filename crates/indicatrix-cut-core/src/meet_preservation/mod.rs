//! Geometry-only preservation of the original planar facet network.
//!
//! Certificates are deliberately not serialized: every materialization is solved again.
//!
//! # Concave tools
//!
//! A design with concave tiers is preserved like any other: what is captured and
//! validated is the planar facet network of the *shell* (the preform and the planar
//! tiers, `Design::tiers`), which the tools never enter. The concave tiers are fixed
//! authored state: a candidate is accepted only when it equals the original in
//! everything but the planar angles and pinned depths (`provenance::same_authoring`
//! compares `concave_tiers` as part of the design), so no optimizer or
//! retarget run can edit a tool. The tools are carving outside the network: they are
//! placed on each candidate's own shell when it is scored (`Design::tools_for_scoring`,
//! `optimize::ScoredStone`), so they follow the stone's frame, but they neither add
//! nor remove a planar surface and cannot detach a planar meet. Scoring and the yield
//! therefore stay on the carved stone, while the meet gate judges the shell the tools
//! are cut from. A meet that a tool happens to carve away is still a meet of the
//! network and is still preserved.
mod export;
mod network;
pub use export::check_schedule_precision;
mod provenance;
mod reach;
mod shape;
pub(crate) mod telemetry;
pub use shape::ShapeMeasures;
pub use telemetry::SearchDiagnostics;

use crate::design::Design;
use glam::DVec3;
use indicatrix::geometry::meet_solver::{SolveControl, SolveStrategy, SolvedTier};
pub use network::{BaselineStructure, BranchDiagnosis, MeetPreservationBaseline, ShapeDeltas};

/// A hard geometric failure, independent of optical quality.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreservationViolation {
    /// The owner cancelled before geometry validation completed.
    Cancelled,
    /// A geometry failure annotated with the original authored support names.
    AtSurfaces {
        detail: String,
        surfaces: Vec<String>,
    },
    /// The original network cannot be certified.
    InvalidBaseline(String),
    /// A tier relation forbids the requested change or cannot be evaluated; the reason is
    /// the complete user-facing sentence and has nothing to do with the original meets.
    Relation(String),
    /// The candidate's own facet boundaries could not be resolved at the preservation
    /// accuracy (a degenerate or vanished face); the original is not at fault.
    CandidateTopology(String),
    /// The authored design no longer corresponds to the captured network.
    StaleCandidate,
    /// A complete original support set no longer meets at one boundary point.
    DetachedJunction(usize),
    /// A previously live authored surface disappeared.
    LostFacet(usize),
    /// An original edge collapsed or left its face boundary.
    CollapsedEdge(usize),
    /// The requested deformation exceeds the operation's hard shape bound.
    ShapeLimit,
    /// The girdle outline or height moved while [`PreservationPolicy::lock_girdle_gauge`]
    /// holds it. `reason` names which and by how much.
    GirdleGauge { reason: String },
    /// Ordinary authored state cannot realize a projected result.
    Unrepresentable,
    /// The bounded numerical search stopped; this is not proof of infeasibility.
    ProjectionFailed { stage: String, reason: String },
}

impl std::fmt::Display for PreservationViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AtSurfaces { detail, surfaces } => {
                write!(f, "{detail} Affected surfaces: {}.", surfaces.join(", "))
            }
            Self::Cancelled => write!(f, "Meet-preserving search was cancelled."),
            Self::InvalidBaseline(reason) => {
                write!(f, "Cannot establish the original meets: {reason}")
            }
            Self::Relation(reason) => write!(f, "{reason}"),
            Self::CandidateTopology(reason) => write!(
                f,
                "The candidate's facet boundary could not be resolved: {reason}"
            ),
            Self::StaleCandidate => write!(
                f,
                "The design changed since the original meets were captured."
            ),
            Self::DetachedJunction(j) => write!(f, "Original junction #{} would separate.", j + 1),
            Self::LostFacet(p) => write!(f, "Original facet #{} would disappear.", p + 1),
            Self::CollapsedEdge(e) => {
                write!(f, "Original edge #{} would collapse or disconnect.", e + 1)
            }
            Self::ShapeLimit => write!(
                f,
                "The candidate exceeds the permitted change to the original shape."
            ),
            Self::GirdleGauge { reason } => {
                write!(f, "The candidate moves the girdle gauge: {reason}.")
            }
            Self::Unrepresentable => write!(
                f,
                "The authored constraints cannot reproduce the preserved meets."
            ),
            Self::ProjectionFailed { stage, reason } => write!(
                f,
                "Meet-preserving projection stopped during {stage}. Candidate geometry check: {reason}"
            ),
        }
    }
}
impl std::error::Error for PreservationViolation {}

impl PreservationViolation {
    /// A stable `snake_case` name of the violation for machine-readable output (the census).
    ///
    /// A failure annotated with surface names ([`Self::AtSurfaces`]) reports the kind of the
    /// failure it annotates: the annotation wraps a detached junction, a lost facet or a
    /// collapsed edge and keeps their text as its detail.
    #[must_use]
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Cancelled => "cancelled",
            Self::AtSurfaces { detail, .. } => {
                if detail.starts_with("Original junction") {
                    "detached_junction"
                } else if detail.starts_with("Original facet") {
                    "lost_facet"
                } else if detail.starts_with("Original edge") {
                    "collapsed_edge"
                } else {
                    "at_surfaces"
                }
            }
            Self::InvalidBaseline(_) => "invalid_baseline",
            Self::Relation(_) => "relation",
            Self::CandidateTopology(_) => "candidate_topology",
            Self::StaleCandidate => "stale_candidate",
            Self::DetachedJunction(_) => "detached_junction",
            Self::LostFacet(_) => "lost_facet",
            Self::CollapsedEdge(_) => "collapsed_edge",
            Self::ShapeLimit => "shape_limit",
            Self::GirdleGauge { .. } => "girdle_gauge",
            Self::Unrepresentable => "unrepresentable",
            Self::ProjectionFailed { .. } => "projection_failed",
        }
    }
}

/// Explicit finite displacement bounds, in units of the original stone's diameter.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreservationPolicy {
    /// Maximum displacement of any protected junction. Never widened to obtain a score; a
    /// Retarget's own facet turn adds the travel it implies, see
    /// [`Self::bounds_follow_turn`].
    pub max_junction_displacement: f64,
    /// Maximum absolute change of length/width.
    pub max_aspect_change: f64,
    /// Maximum absolute table-size change in percentage points.
    pub max_table_percent_change: f64,
    /// Maximum crown or pavilion height change in width units.
    pub max_depth_change: f64,
    /// Maximum girdle height change in width units.
    pub max_girdle_change: f64,
    /// Hold every original junction's x/z position (strong fixed-outline policy).
    /// No preset enables it; it is available to callers that build their own policy.
    pub fixed_outline: bool,
    /// Hold the girdle gauge: the largest junction radius (the outline's reach) and the
    /// mean height of the junctions on it must stay within the incidence tolerance
    /// (`2e-6` of the stone, the reproduction bar; meets themselves are identified at
    /// `5e-5` of the stone). Both presets hold it, so a design whose girdle planes
    /// are derived from a moving tier's meet (and whose radius therefore changes with
    /// that tier) is refused with [`PreservationViolation::GirdleGauge`] naming the
    /// quantity. This is independent of `max_aspect_change`, which bounds the
    /// length-to-width ratio and says nothing about the outline's absolute size. The
    /// projection's own residuals keep preferring the original gauge even when this
    /// is `false`; only the final acceptance test is relaxed.
    #[serde(default = "default_lock_girdle_gauge")]
    pub lock_girdle_gauge: bool,
    /// Whether the depth, girdle and junction bounds grow with the travel the REQUESTED
    /// facet turn implies.
    ///
    /// A Retarget's angle changes are the request itself, and with the girdle outline held
    /// ([`Self::lock_girdle_gauge`]) a facet turned from slope `t` to `t'` about the outline
    /// carries every point it reaches, `l` inward from the outline, by `l (tan t' - tan t)`
    /// up or down. A brilliant's pavilion mains reach from the outline almost to the axis: a
    /// ten-degree pavilion shift (diamond to sapphire) deepens the stone by 0.17 width units
    /// and moves the culet's junctions by as much, against constant bounds of 0.08. The gate
    /// therefore grants such a candidate ([`Self::for_turn`]) the largest per-tier product
    /// `reach_i x |d tan_i|` on top of `max_depth_change`, `max_girdle_change` and
    /// `max_junction_displacement` (owner decision 2026-10-19, tightened from one
    /// half-length times the largest tangent change):
    ///
    /// - `d tan_i` is read from the requested angles ([`tangent_changes`]: the caller's
    ///   request or the projection's seed, or the finished state a caller judges), never from
    ///   a probe the projection is repairing, and the grant is never more than the candidate's
    ///   own turn implies, so a repair can neither enlarge its own allowance nor keep one for
    ///   a turn it undid (and a state the projection accepts passes the finished-state gates,
    ///   which read the candidate's own angles);
    /// - only tiers the request turned count: a fixed tier contributes nothing, and a
    ///   relation follower counts with its own derived turn (its facet really turns);
    /// - `reach_i` is the tier's real inward reach from the outline in the original solved
    ///   geometry (`MeetPreservationBaseline` reads it at capture), with half the stone's
    ///   length as the fallback when a turned tier has no readable facet;
    /// - a tier steeper than [`TURN_TANGENT_CAP_DEG`] has its tangent capped there.
    ///
    /// The constants then bound what the projection's repair adds to the turn; the bound is
    /// never tighter than the constant. The meets, the girdle gauge, the table and the aspect
    /// are not relaxed. `Optimize` keeps its constant bounds: they are broad and its angle
    /// range is the request.
    #[serde(default)]
    pub bounds_follow_turn: bool,
}
const fn default_lock_girdle_gauge() -> bool {
    true
}
impl PreservationPolicy {
    /// Ordinary Optimize permits broad proportion changes.
    pub const OPTIMIZE: Self = Self {
        max_junction_displacement: 0.5,
        max_aspect_change: 1.0,
        max_table_percent_change: 40.0,
        max_depth_change: 0.5,
        max_girdle_change: 0.1,
        fixed_outline: false,
        lock_girdle_gauge: true,
        bounds_follow_turn: false,
    };
    /// The Retarget bound.
    ///
    /// The constants bound what the projection's repair may add to the requested turn (a
    /// conservative initial choice; corpus calibration is still required), and the depth,
    /// girdle and junction bounds follow the turn itself ([`Self::bounds_follow_turn`]),
    /// because a shift between refractive indices turns the pavilion by the critical-angle
    /// difference, many degrees between distant materials, and no constant can both hold a
    /// one-degree shift tight and admit a ten-degree one.
    pub const RETARGET: Self = Self {
        max_junction_displacement: 0.08,
        max_aspect_change: 0.1,
        max_table_percent_change: 8.0,
        max_depth_change: 0.08,
        max_girdle_change: 0.02,
        fixed_outline: false,
        lock_girdle_gauge: true,
        bounds_follow_turn: true,
    };
    /// The wide policy the census measures raw deltas under (follow-ups plan 2026-10-19,
    /// section 4.3). No stone reaches these limits, so a candidate is refused only for the
    /// topology and gauge reasons. Never a product default: nothing but the census uses it.
    pub const CENSUS: Self = Self {
        max_junction_displacement: 10.0,
        max_aspect_change: 10.0,
        max_table_percent_change: 100.0,
        max_depth_change: 10.0,
        max_girdle_change: 10.0,
        fixed_outline: false,
        lock_girdle_gauge: true,
        bounds_follow_turn: true,
    };

    /// This policy with the travel a facet turn implies granted on top of the constant
    /// bounds, when [`Self::bounds_follow_turn`] asks for it.
    ///
    /// `travel_width_units` is the largest per-tier product of a turned tier's inward reach
    /// from the outline and its tangent change, in width units (what the depth and girdle
    /// bounds are measured in), and `travel_scale_units` that same travel in units of the
    /// stone's extent (what the junction displacement is measured in);
    /// `MeetPreservationBaseline` forms both. The depth and girdle bounds grow by the first,
    /// the junction bound by the second, so the bound is never tighter than the constant. A
    /// policy that does not follow the turn, or a travel that is not a finite positive
    /// number, comes back as it is.
    #[must_use]
    pub const fn for_turn(self, travel_width_units: f64, travel_scale_units: f64) -> Self {
        if !self.bounds_follow_turn
            || !travel_width_units.is_finite()
            || !travel_scale_units.is_finite()
            || travel_width_units <= 0.0
            || travel_scale_units <= 0.0
        {
            return self;
        }
        Self {
            max_junction_displacement: self.max_junction_displacement + travel_scale_units,
            max_depth_change: self.max_depth_change + travel_width_units,
            max_girdle_change: self.max_girdle_change + travel_width_units,
            ..self
        }
    }
}

/// Facets steeper than this (degrees from horizontal) have their slope tangent capped at
/// its value when a turn's travel is measured.
///
/// The hinge model behind [`PreservationPolicy::bounds_follow_turn`] carries a point by
/// `l (tan t' - tan t)`, and the tangent grows without bound toward 90 degrees (its slope is
/// 15 per radian at 75 degrees and 130 at 85) while a steep facet's real inward reach shrinks
/// to its height over the tangent; the linear model no longer describes the travel there, and
/// a near-wall request would otherwise buy an unbounded allowance (owner decision
/// 2026-10-19). With the cap a tier turned past 75 degrees contributes at most its reach times
/// `tan 75 - tan t` (3.73 minus its old tangent), and one that starts beyond 75 degrees
/// contributes nothing.
pub const TURN_TANGENT_CAP_DEG: f64 = 75.0;

/// `tan |t|` with `|t|` capped at [`TURN_TANGENT_CAP_DEG`].
fn capped_tangent(angle_deg: f64) -> f64 {
    angle_deg.abs().min(TURN_TANGENT_CAP_DEG).to_radians().tan()
}

/// The slope-tangent change each tier of `requested` asks for against `original`, one entry
/// per tier: `|tan |t'| - tan |t||` for a turned tier, `0.0` otherwise.
///
/// It is how far, per unit of inward reach, a facet turned about the girdle outline carries
/// the points it reaches ([`PreservationPolicy::bounds_follow_turn`]). A tier counts only
/// when the request turned it: its requested angle differs from the original's, an optimizer
/// could turn it ([`crate::optimize::angle_is_variable`] on both sides: a flat has no slope and
/// a facet at or past the girdle limit is a wall whose tangent is no travel). A relation
/// follower counts with its own derived turn: its facet really turns and carries its points,
/// and the per-tier maximum in the travel never adds it to its driver's. Tangents are
/// capped at [`TURN_TANGENT_CAP_DEG`]. Pure arithmetic on the authored angles: `requested` is
/// a caller's request (`OptimizeOptions::turn_request`), the projection's seed, or the
/// finished state a caller judges.
#[must_use]
pub fn tangent_changes(original: &Design, requested: &Design) -> Vec<f64> {
    original
        .tiers
        .iter()
        .zip(&requested.tiers)
        .map(|(was, now)| {
            let turned = was.angle_deg.to_bits() != now.angle_deg.to_bits()
                && crate::optimize::angle_is_variable(was.angle_deg)
                && crate::optimize::angle_is_variable(now.angle_deg);
            if turned {
                (capped_tangent(now.angle_deg) - capped_tangent(was.angle_deg)).abs()
            } else {
                0.0
            }
        })
        .collect()
}

/// Accuracy facts for the exact production geometry just validated.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PreservationFacts {
    /// Number of protected junctions.
    pub original_meets: usize,
    /// Largest plane residual divided by the original diameter.
    pub max_normalized_residual: f64,
    /// Largest original-junction displacement divided by the original diameter.
    pub shape_distance: f64,
    /// Additional boundary vertices; no original junction is removed.
    pub added_meets: usize,
    /// Accuracy in authored design units.
    pub max_residual_units: f64,
    /// Accuracy in mm when the original design has a physical diameter.
    pub max_residual_mm: Option<f64>,
    /// Actual candidate proportions, after the authoritative solve.
    pub shape: ShapeMeasures,
}

impl PreservationFacts {
    /// Short end-user diagnostic; tolerances are accuracy measures, never permission to detach.
    #[must_use]
    pub fn summary(&self) -> String {
        let residual = self.max_residual_mm.map_or_else(
            || format!("{:.3e} design units", self.max_residual_units),
            |mm| format!("{mm:.3e} mm"),
        );
        format!(
            "{} original meets preserved; {} additional boundary vertices; maximum meet residual {residual}.",
            self.original_meets, self.added_meets
        )
    }
}

pub(crate) fn solve_checked_controlled(
    design: &Design,
    control: &SolveControl<'_>,
) -> Result<Vec<SolvedTier>, PreservationViolation> {
    if control.is_cancelled() {
        return Err(PreservationViolation::Cancelled);
    }
    let solved = design.solve_with(control).map_err(|e| match e {
        crate::design::DesignSolveError::Solve(
            indicatrix::geometry::meet_solver::SolveError::Cancelled,
        ) => PreservationViolation::Cancelled,
        other => PreservationViolation::InvalidBaseline(other.to_string()),
    })?;
    if solved.len() != design.tiers.len()
        || solved
            .iter()
            .any(|s| !s.mast.is_finite() || matches!(s.strategy, SolveStrategy::Failed))
    {
        return Err(PreservationViolation::InvalidBaseline(
            "incomplete solve".into(),
        ));
    }
    Ok(solved)
}

pub(crate) type Plane = (DVec3, f64);

#[cfg(test)]
mod tests;
