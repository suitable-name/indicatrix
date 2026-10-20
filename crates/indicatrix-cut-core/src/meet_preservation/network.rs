use super::{Plane, PreservationFacts, PreservationPolicy, PreservationViolation, provenance};
use crate::design::Design;
use glam::DVec3;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh},
};

#[path = "network_topology.rs"]
mod topology;
use topology::Role;

#[path = "census.rs"]
mod census;
pub use census::{BaselineStructure, ShapeDeltas};
#[path = "branch.rs"]
mod branch;
pub use branch::BranchDiagnosis;

/// One authoritative solve of a candidate: the geometry verdict and, when that verdict
/// is a refusal, the junction residuals of the same solved state.
#[derive(Debug)]
pub struct Assessment {
    /// The geometry verdict: what [`MeetPreservationBaseline::validate_controlled`] would
    /// answer when no requested turn is given; with one, the turn allowance is the smaller
    /// of the request's travel and the candidate's own ([`MeetPreservationBaseline::assess`]).
    pub(crate) verdict: Result<PreservationFacts, PreservationViolation>,
    /// What [`MeetPreservationBaseline::residuals`] would answer; `None` for an accepted
    /// state (nothing to correct) or one whose residuals cannot be formed.
    pub(crate) residuals: Option<Vec<f64>>,
}

#[derive(Debug, Clone, PartialEq)]
struct Junction {
    point: DVec3,
    support: Vec<usize>,
    basis: [usize; 3],
    /// The largest distance of a support plane from `point` (model units): the miss the
    /// design itself has at this meet, at most the identification radius.
    slack: f64,
}

/// The radius, as a fraction of the stone, at which meets are identified: plane-triple
/// points within it are one junction and a plane within it belongs to the junction.
///
/// The reproduction bar stays `2e-6` of the stone. `GemCad` 4.x files carry 5-decimal masts
/// whose meets miss by `2e-6 .. 2e-5` of the stone (`shah.asc`; `texasstar`, `trichecker12`
/// and `forfun` written by `GemCad` 5 stay below `2e-7` in `f64` and below `2e-6` in `f32`);
/// identifying at the reproduction bar split those meets or refused 54 of the first 100
/// library designs (census 2026-10-20). `5e-5` is the meet solver's own incidence scale,
/// 0.5 micrometres on a 10 mm stone.
const IDENTIFY: f64 = 5e-5;

/// Immutable, operation-start geometry. Uses production f32-converted planes and mesh rings.
#[derive(Debug, Clone, PartialEq)]
pub struct MeetPreservationBaseline {
    original: Design,
    pub(crate) telemetry: std::sync::Arc<super::telemetry::Telemetry>,
    keys: Vec<provenance::SurfaceKey>,
    junctions: Vec<Junction>,
    edges: Vec<(usize, usize)>,
    live: Vec<usize>,
    scale: f64,
    tolerance: f64,
    /// The identification radius, `IDENTIFY` of the stone (model units).
    identify: f64,
    /// Per junction, the original residual rows of [`Self::residuals`]; a row above the
    /// tolerance is the design's own slack, which a repair aims at instead of zero.
    row_targets: Vec<Vec<f64>>,
    radial_extent: f64,
    gauge_junctions: Vec<usize>,
    girdle_height: f64,
    cycles: Vec<(usize, Vec<usize>)>,
    shape: super::ShapeMeasures,
    width: f64,
    /// Each tier's inward reach from the girdle outline in the original solved geometry
    /// (model units), `None` when it cannot be read (`reach::tier_reaches`).
    tier_reach: Vec<Option<f64>>,
}

/// The undirected boundary edges of the face cycles, each once.
fn boundary_edges(cycles: &[(usize, Vec<usize>)]) -> Vec<(usize, usize)> {
    let mut edges = Vec::new();
    for (_, cycle) in cycles {
        for i in 0..cycle.len() {
            let (a, b) = (cycle[i], cycle[(i + 1) % cycle.len()]);
            let edge = (a.min(b), a.max(b));
            if !edges.contains(&edge) {
                edges.push(edge);
            }
        }
    }
    edges
}

fn vertices(mesh: &SolidMesh, tolerance: f64) -> Vec<DVec3> {
    let mut points: Vec<DVec3> = Vec::new();
    for (_, ring) in &mesh.rings {
        for &point in ring {
            if !points.iter().any(|p| p.distance(point) <= tolerance) {
                points.push(point);
            }
        }
    }
    points
}

/// A solve failure of a *candidate* is not a failure to establish the original meets.
fn blame_candidate(error: PreservationViolation) -> PreservationViolation {
    match error {
        PreservationViolation::InvalidBaseline(reason) => {
            PreservationViolation::CandidateTopology(reason)
        }
        other => other,
    }
}

fn polygon_area(ring: &[DVec3]) -> f64 {
    let Some(&origin) = ring.first() else {
        return 0.0;
    };
    ring.windows(2)
        .skip(1)
        .map(|p| (p[0] - origin).cross(p[1] - origin))
        .sum::<DVec3>()
        .length()
        * 0.5
}

impl MeetPreservationBaseline {
    /// Solve and capture before any Shift/refit/seed. Incomplete and ambiguous inputs fail.
    ///
    /// # Errors
    /// Returns [`PreservationViolation::InvalidBaseline`] (or another violation) when the
    /// design cannot be solved, is not a closed stone, or has ambiguous meets.
    pub fn capture(design: &Design) -> Result<Self, PreservationViolation> {
        Self::capture_with(design, None)
    }

    /// Capture using the worker's cancellation flag.
    ///
    /// # Errors
    /// As [`Self::capture`], plus [`PreservationViolation::Cancelled`] when the flag is set.
    pub fn capture_with(
        design: &Design,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<Self, PreservationViolation> {
        Self::capture_controlled(
            design,
            &cancel.map_or_else(
                indicatrix::geometry::meet_solver::SolveControl::default,
                indicatrix::geometry::meet_solver::SolveControl::with_cancel,
            ),
        )
    }

    /// Capture with solver-level cancellation, including closure-based editor workers.
    ///
    /// # Errors
    /// As [`Self::capture`], plus [`PreservationViolation::Cancelled`] when the control
    /// reports cancellation.
    pub fn capture_controlled(
        design: &Design,
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
    ) -> Result<Self, PreservationViolation> {
        let telemetry = std::sync::Arc::new(super::telemetry::Telemetry::default());
        let _capture = telemetry.phase(0);
        let solved = {
            let _solve = telemetry.phase(2);
            super::solve_checked_controlled(design, control)?
        };
        let planes = design.planes_from_solved(&solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return Err(PreservationViolation::InvalidBaseline(
                "the stone is not closed".into(),
            ));
        };
        let raw = vertices(&mesh, 0.0);
        let low = raw
            .iter()
            .copied()
            .fold(DVec3::splat(f64::INFINITY), DVec3::min);
        let high = raw
            .iter()
            .copied()
            .fold(DVec3::splat(f64::NEG_INFINITY), DVec3::max);
        let scale = (high - low).max_element();
        if !scale.is_finite() || scale <= 0.0 {
            return Err(PreservationViolation::InvalidBaseline(
                "non-finite or empty geometry".into(),
            ));
        }
        // Separate incidence accuracy from edge/area floors. No visual weld tolerance.
        let tolerance = scale * 2e-6;
        let identify = scale * IDENTIFY;
        let surfaces = provenance::surfaces(design, &solved, &planes)?;
        let all: Vec<Plane> = surfaces.iter().map(|(_, p)| *p).collect();
        if all.iter().any(|(n, d)| !n.is_finite() || !d.is_finite()) {
            return Err(PreservationViolation::InvalidBaseline(
                "non-finite plane".into(),
            ));
        }
        // Junctions and faces come from the f64 arrangement at the identification
        // radius, not from the welded display mesh (see the topology module docs).
        let junctions =
            topology::arrangement_junctions(&all, scale, identify, Role::Baseline, control)?;
        let cycles = topology::face_cycles(&all, &junctions, scale, identify, Role::Baseline)?;
        let row_targets = junctions
            .iter()
            .map(|j| topology::support_rows(&all, j, scale).map(|(_, rows)| rows))
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| {
                PreservationViolation::InvalidBaseline("rank-deficient junction".into())
            })?;
        let edges = boundary_edges(&cycles);
        let points: Vec<_> = junctions.iter().map(|j| j.point).collect();
        let tier_reach = super::reach::tier_reaches(design, &surfaces, &points, &cycles);
        let live = (0..all.len())
            .filter(|&surface| {
                cycles
                    .iter()
                    .any(|(_, cycle)| topology::supports_face(&junctions, cycle, surface))
            })
            .collect();
        let radial_extent = junctions
            .iter()
            .map(|j| j.point.x.hypot(j.point.z))
            .fold(0.0, f64::max);
        let gauge_junctions: Vec<_> = junctions
            .iter()
            .enumerate()
            .filter(|(_, j)| (j.point.x.hypot(j.point.z) - radial_extent).abs() <= identify)
            .map(|(i, _)| i)
            .collect();
        let girdle_height = gauge_junctions
            .iter()
            .map(|&i| junctions[i].point.y)
            .sum::<f64>()
            / gauge_junctions.len().max(1) as f64;
        let shape = super::ShapeMeasures::measure(&planes, &mesh)?;
        let width = (high.x - low.x).min(high.z - low.z);
        Ok(Self {
            original: design.clone(),
            telemetry: telemetry.clone(),
            keys: surfaces.into_iter().map(|(k, _)| k).collect(),
            junctions,
            edges,
            live,
            scale,
            tolerance,
            identify,
            row_targets,
            radial_extent,
            gauge_junctions,
            girdle_height,
            cycles,
            shape,
            width,
            tier_reach,
        })
    }

    /// The operation's original authored state, useful for freshness checks.
    #[must_use]
    pub const fn original(&self) -> &Design {
        &self.original
    }

    /// Cumulative phase measurements, including rejected projection work.
    #[must_use]
    pub fn diagnostics(&self) -> super::SearchDiagnostics {
        self.telemetry.snapshot()
    }

    pub(crate) const fn scale(&self) -> f64 {
        self.scale
    }

    /// Whether only the editable angle/pinned-depth parameters differ from the source.
    pub(crate) fn same_authoring(&self, candidate: &Design) -> bool {
        provenance::same_authoring(&self.original, candidate)
    }

    /// Exact geometry gate. Added contacts are allowed; original correspondence is injective.
    ///
    /// # Errors
    /// Returns the [`PreservationViolation`] that refuses the candidate (stale, detached
    /// junction, lost facet, collapsed edge, shape limit, girdle gauge, ...).
    pub fn validate(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
    ) -> Result<PreservationFacts, PreservationViolation> {
        self.validate_with(candidate, policy, None)
    }

    /// Cancellable counterpart used by worker searches.
    ///
    /// # Errors
    /// As [`Self::validate`], plus [`PreservationViolation::Cancelled`] when the flag is set.
    pub fn validate_with(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        cancel: Option<&std::sync::atomic::AtomicBool>,
    ) -> Result<PreservationFacts, PreservationViolation> {
        self.validate_controlled(
            candidate,
            policy,
            &cancel.map_or_else(
                indicatrix::geometry::meet_solver::SolveControl::default,
                indicatrix::geometry::meet_solver::SolveControl::with_cancel,
            ),
        )
    }

    /// Validate with the same cancellation control used by the authoring solve.
    ///
    /// # Errors
    /// As [`Self::validate`], plus [`PreservationViolation::Cancelled`] when the control
    /// reports cancellation.
    pub fn validate_controlled(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
    ) -> Result<PreservationFacts, PreservationViolation> {
        self.validate_geometry(candidate, policy, control, true)
            .map_err(|error| self.annotate(error))
    }

    /// Solve once and judge the result and, when it is refused, also form its residuals,
    /// so a projection step does not pay a second solve of the same design.
    ///
    /// `requested_turn` is the per-tier tangent change of the REQUEST
    /// ([`super::tangent_changes`] of the projection's seed, or of the caller's own request),
    /// which a policy that follows the turn is widened by: the grant is the smaller of the
    /// request's travel and the candidate's own, so a repair can neither enlarge the
    /// allowance beyond the request nor keep one for a turn it undid. `None` reads the turn
    /// from `candidate` itself.
    pub(crate) fn assess(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
        requested_turn: Option<&[f64]>,
    ) -> Assessment {
        let refused = |error: PreservationViolation| Assessment {
            verdict: Err(self.annotate(error)),
            residuals: None,
        };
        if control.is_cancelled() {
            return refused(PreservationViolation::Cancelled);
        }
        if !provenance::same_authoring(&self.original, candidate) {
            return refused(PreservationViolation::StaleCandidate);
        }
        let _validation = self.telemetry.phase(3);
        let solved = {
            let _solve = self.telemetry.phase(2);
            match super::solve_checked_controlled(candidate, control) {
                Ok(solved) => solved,
                Err(error) => return refused(blame_candidate(error)),
            }
        };
        let verdict = self
            .validate_solved(candidate, &solved, policy, control, requested_turn)
            .map_err(|error| self.annotate(error));
        let residuals = if verdict.is_err() {
            self.residuals_solved(candidate, &solved, control)
        } else {
            None
        };
        Assessment { verdict, residuals }
    }

    pub(super) fn validate_geometry(
        &self,
        candidate: &Design,
        policy: PreservationPolicy,
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
        check_authoring: bool,
    ) -> Result<PreservationFacts, PreservationViolation> {
        if control.is_cancelled() {
            return Err(PreservationViolation::Cancelled);
        }
        if check_authoring && !provenance::same_authoring(&self.original, candidate) {
            return Err(PreservationViolation::StaleCandidate);
        }
        let _validation = self.telemetry.phase(3);
        let solved = {
            let _solve = self.telemetry.phase(2);
            super::solve_checked_controlled(candidate, control).map_err(blame_candidate)?
        };
        self.validate_solved(candidate, &solved, policy, control, None)
    }

    /// The travel a turn implies, in model units: the largest product of a turned tier's
    /// inward reach from the outline and its tangent change.
    ///
    /// `turn` has one tangent change per tier ([`super::tangent_changes`]); the reaches were
    /// read by [`Self::capture`] in the original solved geometry. A tier whose reach could not
    /// be read is granted the owner's fallback, half the stone's length. Pure arithmetic: the
    /// fallback grants are counted by [`Self::turn_allowance`].
    pub(crate) fn turn_travel(&self, turn: &[f64]) -> f64 {
        let half_length = 0.5 * self.shape.aspect * self.width;
        super::reach::turn_travel(&self.tier_reach, half_length, turn)
    }

    /// The travel the gate grants `candidate` under a policy that follows the turn, in model
    /// units: the travel of the candidate's own turn or, with a `requested_turn`, the smaller
    /// of the request's travel and its own.
    ///
    /// Every turned tier of the turn that was granted whose reach could not be read is
    /// counted in [`super::SearchDiagnostics::turn_reach_fallbacks`], so a run that was
    /// granted the half-length fallback says so.
    fn turn_allowance(&self, candidate: &Design, requested_turn: Option<&[f64]>) -> f64 {
        let own_turn = super::tangent_changes(&self.original, candidate);
        let own = self.turn_travel(&own_turn);
        let requested = requested_turn.map(|turn| (self.turn_travel(turn), turn));
        let (travel, granted) = match requested {
            Some((travel, turn)) if travel <= own => (travel, turn),
            _ => (own, own_turn.as_slice()),
        };
        let fallbacks = self
            .tier_reach
            .iter()
            .zip(granted)
            .filter(|(reach, change)| reach.is_none() && **change > 0.0)
            .count();
        self.telemetry.turn_reach_fallbacks(fallbacks);
        travel
    }

    /// The geometry gate proper, on an already solved candidate. `requested_turn` as in
    /// [`Self::assess`].
    fn validate_solved(
        &self,
        candidate: &Design,
        solved: &[SolvedTier],
        policy: PreservationPolicy,
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
        requested_turn: Option<&[f64]>,
    ) -> Result<PreservationFacts, PreservationViolation> {
        // A facet turned about the girdle outline carries the points it reaches with it; a
        // policy that follows the turn grants that travel on top of its constants
        // (`PreservationPolicy::for_turn`). The turn is the REQUEST's where the projection
        // hands one over, and never more than the candidate's own: a repair can neither
        // enlarge the allowance nor keep one for a turn it undid, and a state the projection
        // accepts also passes the finished-state gates (`validate`), which read the turn from
        // the candidate's own angles. A policy that does not follow the turn is granted
        // nothing, so it neither measures the turn nor counts a fallback it never used.
        let travel = if policy.bounds_follow_turn {
            self.turn_allowance(candidate, requested_turn)
        } else {
            0.0
        };
        let policy = policy.for_turn(travel / self.width, travel / self.scale);
        let planes = candidate.planes_from_solved(solved);
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            return Err(PreservationViolation::Unrepresentable);
        };
        let surfaces = provenance::surfaces(candidate, solved, &planes)?;
        if surfaces.iter().map(|(k, _)| k).ne(self.keys.iter()) {
            return Err(PreservationViolation::StaleCandidate);
        }
        let all: Vec<_> = surfaces.iter().map(|(_, p)| *p).collect();
        let junctions = topology::arrangement_junctions(
            &all,
            self.scale,
            self.identify,
            Role::Candidate,
            control,
        )?;
        let cycles =
            topology::face_cycles(&all, &junctions, self.scale, self.identify, Role::Candidate)?;
        let points: Vec<_> = junctions.iter().map(|j| j.point).collect();
        for &facet in &self.live {
            if !cycles
                .iter()
                .any(|(_, cycle)| topology::supports_face(&junctions, cycle, facet))
            {
                return Err(PreservationViolation::LostFacet(facet));
            }
        }
        let (mapped, max_residual, distance) =
            self.map_junctions(&all, &points, &planes, control)?;
        self.check_edges(&all, &points, &mapped)?;
        self.check_cycles(&cycles, &junctions, &mapped)?;
        let shape = super::ShapeMeasures::measure(&planes, &mesh)?;
        self.shape.validate(shape, policy)?;
        if policy.fixed_outline
            && self.junctions.iter().enumerate().any(|(j, original)| {
                let now = points[mapped[j]];
                (now.x - original.point.x).hypot(now.z - original.point.z) > self.tolerance
            })
        {
            return Err(PreservationViolation::ShapeLimit);
        }
        if policy.lock_girdle_gauge {
            self.check_girdle_gauge(&points, &mapped)?;
        }
        if !policy.max_junction_displacement.is_finite()
            || distance > policy.max_junction_displacement
        {
            return Err(PreservationViolation::ShapeLimit);
        }
        Ok(PreservationFacts {
            original_meets: self.junctions.len(),
            max_normalized_residual: max_residual,
            shape_distance: distance,
            added_meets: points.len().saturating_sub(self.junctions.len()),
            max_residual_units: max_residual * self.scale,
            max_residual_mm: self
                .original
                .girdle_diameter_mm
                .filter(|d| d.is_finite() && *d > 0.0)
                .map(|mm| max_residual * self.scale * mm / self.width),
            shape,
        })
    }

    /// Map every original junction to exactly one candidate point. Returns the candidate
    /// point index per original junction, the largest plane residual and the largest
    /// junction displacement (both normalised by the stone scale).
    fn map_junctions(
        &self,
        all: &[Plane],
        points: &[DVec3],
        planes: &[Plane],
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
    ) -> Result<(Vec<usize>, f64, f64), PreservationViolation> {
        let mut mapped = Vec::new();
        let mut max_residual: f64 = 0.0;
        let mut distance: f64 = 0.0;
        for (j, junction) in self.junctions.iter().enumerate() {
            if control.is_cancelled() {
                return Err(PreservationViolation::Cancelled);
            }
            // The original's own miss at this meet is not asked to close.
            let bound = self.tolerance.max(junction.slack);
            let matches: Vec<_> = points
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    junction
                        .support
                        .iter()
                        .all(|&s| (all[s].0.dot(**p) - all[s].1).abs() <= bound)
                        && planes.iter().all(|(n, d)| n.dot(**p) <= d + bound)
                })
                .collect();
            if matches.len() != 1 || mapped.contains(&matches[0].0) {
                return Err(PreservationViolation::DetachedJunction(j));
            }
            let (index, point) = matches[0];
            mapped.push(index);
            distance = distance.max(point.distance(junction.point) / self.scale);
            for &s in &junction.support {
                max_residual =
                    max_residual.max((all[s].0.dot(*point) - all[s].1).abs() / self.scale);
            }
        }
        Ok((mapped, max_residual, distance))
    }

    /// No original edge may collapse or lose two independent common boundary planes.
    fn check_edges(
        &self,
        all: &[Plane],
        points: &[DVec3],
        mapped: &[usize],
    ) -> Result<(), PreservationViolation> {
        for (edge, &(a, b)) in self.edges.iter().enumerate() {
            let (p, q) = (points[mapped[a]], points[mapped[b]]);
            if p.distance(q) <= self.tolerance {
                return Err(PreservationViolation::CollapsedEdge(edge));
            }
            // Both endpoints must still lie together on at least two independent boundary planes.
            let common: Vec<_> = self.junctions[a]
                .support
                .iter()
                .copied()
                .filter(|s| self.junctions[b].support.contains(s))
                .collect();
            if !common.iter().any(|&s| {
                common
                    .iter()
                    .any(|&t| all[s].0.cross(all[t].0).length() > 1e-6)
            }) {
                return Err(PreservationViolation::CollapsedEdge(edge));
            }
        }
        Ok(())
    }

    /// Preserve each original face's oriented boundary cycle. Added edge subdivision
    /// vertices are ignored, but original vertices may neither swap nor reverse order.
    fn check_cycles(
        &self,
        cycles: &[(usize, Vec<usize>)],
        junctions: &[Junction],
        mapped: &[usize],
    ) -> Result<(), PreservationViolation> {
        for (surface, cycle) in &self.cycles {
            let valid = cycles.iter().any(|(_, ring)| {
                if !topology::supports_face(junctions, ring, *surface) {
                    return false;
                }
                let mut observed = Vec::new();
                for point in ring {
                    if let Some(j) = cycle.iter().find(|&&j| *point == mapped[j])
                        && observed.last() != Some(j)
                    {
                        observed.push(*j);
                    }
                }
                if observed.len() > 1 && observed.first() == observed.last() {
                    observed.pop();
                }
                observed.len() == cycle.len()
                    && (0..cycle.len()).any(|offset| {
                        (0..cycle.len()).all(|i| observed[i] == cycle[(i + offset) % cycle.len()])
                    })
            });
            if !valid {
                return Err(PreservationViolation::LostFacet(*surface));
            }
        }
        Ok(())
    }

    /// The girdle radius and height must stay within the capture tolerance.
    fn check_girdle_gauge(
        &self,
        points: &[DVec3],
        mapped: &[usize],
    ) -> Result<(), PreservationViolation> {
        let radial = mapped
            .iter()
            .map(|&i| points[i].x.hypot(points[i].z))
            .fold(0.0, f64::max);
        let height = self
            .gauge_junctions
            .iter()
            .map(|&i| points[mapped[i]].y)
            .sum::<f64>()
            / self.gauge_junctions.len().max(1) as f64;
        if (radial - self.radial_extent).abs() > self.tolerance {
            return Err(PreservationViolation::GirdleGauge {
                reason: format!(
                    "the girdle radius moved by {:.4} % of the stone",
                    (radial - self.radial_extent).abs() / self.scale * 100.0
                ),
            });
        }
        if (height - self.girdle_height).abs() > self.tolerance {
            return Err(PreservationViolation::GirdleGauge {
                reason: format!(
                    "the girdle height moved by {:.4} % of the stone",
                    (height - self.girdle_height).abs() / self.scale * 100.0
                ),
            });
        }
        Ok(())
    }

    fn annotate(&self, error: PreservationViolation) -> PreservationViolation {
        let indices = match &error {
            PreservationViolation::DetachedJunction(j) => {
                self.junctions.get(*j).map(|j| j.support.clone())
            }
            PreservationViolation::LostFacet(f) => Some(vec![*f]),
            PreservationViolation::CollapsedEdge(e) => self.edges.get(*e).map(|(a, b)| {
                let mut names = self.junctions[*a].support.clone();
                names.extend(&self.junctions[*b].support);
                names.sort_unstable();
                names.dedup();
                names
            }),
            _ => None,
        };
        let Some(indices) = indices else {
            return error;
        };
        let surfaces = indices
            .iter()
            .filter_map(|&i| self.keys.get(i))
            .map(|key| match key {
                provenance::SurfaceKey::Preform(i) => format!("rough face {}", i + 1),
                provenance::SurfaceKey::Facet(id, instance) => {
                    let name = self
                        .original
                        .tiers
                        .iter()
                        .enumerate()
                        .find(|(i, _)| self.original.tier_id_at_or_synthetic(*i) == *id)
                        .map_or("unknown tier", |(_, t)| t.name.as_str());
                    format!("{name} / facet {}", instance + 1)
                }
            })
            .collect();
        PreservationViolation::AtSurfaces {
            detail: error.to_string(),
            surfaces,
        }
    }

    pub(super) fn has_live_preform(&self) -> bool {
        self.live
            .iter()
            .any(|&i| matches!(self.keys[i], provenance::SurfaceKey::Preform(_)))
    }

    /// Residuals at moving junctions, using the actual authored-state solve.
    /// Additional supports must agree and no other plane may cut the junction away.
    pub(crate) fn residuals(
        &self,
        candidate: &Design,
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
    ) -> Option<Vec<f64>> {
        let solved = {
            let _solve = self.telemetry.phase(2);
            super::solve_checked_controlled(candidate, control).ok()?
        };
        self.residuals_solved(candidate, &solved, control)
    }

    /// [`Self::residuals`] for an already solved candidate.
    fn residuals_solved(
        &self,
        candidate: &Design,
        solved: &[SolvedTier],
        control: &indicatrix::geometry::meet_solver::SolveControl<'_>,
    ) -> Option<Vec<f64>> {
        let planes = candidate.planes_from_solved(solved);
        let surfaces = provenance::surfaces(candidate, solved, &planes).ok()?;
        if surfaces.iter().map(|(key, _)| key).ne(self.keys.iter()) {
            return None;
        }
        let all: Vec<_> = surfaces.iter().map(|(_, p)| *p).collect();
        if all.iter().any(|(n, d)| !n.is_finite() || !d.is_finite()) {
            return None;
        }
        let mut residuals = Vec::new();
        let mut points = Vec::new();
        for (junction, targets) in self.junctions.iter().zip(&self.row_targets) {
            if control.is_cancelled() {
                return None;
            }
            let (p, rows) = topology::support_rows(&all, junction, self.scale)?;
            points.push(p);
            // A row the original already missed by more than the tolerance aims at that
            // miss (an imported mast's quantisation), not at a closure the design never
            // had; every other row aims at zero, bit for bit as before.
            residuals.extend(rows.iter().zip(targets).map(|(&row, &target)| {
                if target.abs() * self.scale > self.tolerance {
                    row - target
                } else {
                    row
                }
            }));
            // Incidence alone can describe an intersection outside the stone. A single
            // maximum violation is zero exactly when every non-support half-space holds;
            // it avoids a large Jacobian full of inactive zero rows. New contacts are allowed.
            let outside = all
                .iter()
                .enumerate()
                .filter(|(s, _)| !junction.support.contains(s))
                .map(|(_, (n, d))| (n.dot(p) - d) / self.scale)
                .fold(0.0, f64::max);
            residuals.push(outside);
        }
        let radial = points.iter().map(|p| p.x.hypot(p.z)).fold(0.0, f64::max);
        let height = self
            .gauge_junctions
            .iter()
            .map(|&i| points[i].y)
            .sum::<f64>()
            / self.gauge_junctions.len().max(1) as f64;
        residuals.push((radial - self.radial_extent) / self.scale);
        residuals.push((height - self.girdle_height) / self.scale);
        Some(residuals)
    }
}

#[cfg(test)]
#[path = "network_tests.rs"]
mod tests;
