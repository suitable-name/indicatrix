//! Meet-point solver.
//!
//! Derives `GemCAD` "mast" (facet plane offset) distances from angles, index
//! positions, gear, and meet constraints alone -- the inverse of the forward
//! `from_asc_schedule` plane construction in [`super::cuts`].
//!
//! # The meet model: vertex incidence, not tangency
//!
//! Every facet plane is `n . x = mast` with `n` fixed by angle, index and gear, and
//! `mast` the unknown. The format's semantics is **vertex incidence**, not
//! *tangency* (cutting inward until first touching the solid, `mast = max_v n . v`,
//! the support function): the recorded mast is essentially never that maximum
//! (0.7% of ~39,000 measured tiers across 2,881 real designs). Instead the facet is
//! cut *past* first touch until its plane passes exactly through a vertex of the
//! arrangement formed by the other facets' planes -- true for 96.2% of tiers
//! (within 0.5% relative, every other tier pinned at its true mast). Among the
//! arrangement's candidate vertex "levels" ordered by `n . v` descending, the true
//! one is level 1 (first past first touch) for 77.5% of tiers, within the first
//! three for 94.4%.
//!
//! # What meets alone cannot determine: per-block anchors
//!
//! A design's plane arrangement has continuous degrees of freedom that preserve
//! *every* vertex incidence: girdle planes are vertical (`n.y = 0`), so shifting a
//! whole crown or pavilion block, or the girdle's own radial scale, coherently is
//! undetectable from meets alone (verified empirically: a stable fixed point of
//! this solver's refinement stage). These free parameters are exactly the
//! dimensions a designer chooses and a printed diagram states (the `C/W`, `P/W`,
//! `H/W` numbers on every `GemCAD` printout). A caller must therefore supply one
//! [`MeetConstraint::ScaleReference`] per block that the schedule itself doesn't
//! anchor; masts within each block are then meet-derived from that anchor.
//! [`meet_tier_inputs_from_asc`] classifies stated anchors, and
//! [`apply_ratio_anchors`] fills in whatever's left unanchored from a design's
//! printed `C/W`/`P/W` proportions -- the only source available with no `.asc`
//! file, and one with real residual error (see its own doc comment); a caller with
//! real recorded masts to bootstrap from should prefer those.
//!
//! # Solving strategy
//!
//! Which vertex a tier's plane passes through depends on where every *other*
//! tier's plane sits, and real schedules can have genuinely mutual dependencies.
//! The solve runs in three phases:
//!
//! 1. **Constructive pass in file order** ([`SolveStrategy::DependencyOrder`]):
//!    tiers settle one at a time in schedule order, each against the arrangement
//!    of everything settled so far -- the shallowest candidate vertex level
//!    incident to every resolved `"Meet <names>"` reference when the schedule
//!    states one, the rank-1 level otherwise. A tier that cannot settle yet is
//!    retried on later passes.
//! 2. **Block estimate**: every still-unsettled tier gets a per-block least-squares
//!    estimate (`mast ~ a*cos(theta) + b*sin(theta)`, the planes-through-a-circle
//!    model) as a starting point.
//! 3. **Nearest-level refinement** ([`SolveStrategy::JointGroup`]): Jacobi sweeps
//!    over the full arrangement where every meet-derived tier snaps to the
//!    shallowest candidate vertex level incident to its resolved named references
//!    when one exists, else the level nearest its current mast. The true
//!    configuration is a stable fixed point of this update; the sweeps both settle
//!    mutually-dependent groups and polish phase-1 values.
//!
//! `"Meet <names>"` references are resolved by [`MeetNameResolver`], which handles
//! the corpus's informal reference styles (unnamed girdle/culet/table references,
//! compound `"1-2-G1"` vertex specs, connective prose, case and side-prefix
//! mismatches).
//!
//! All geometry runs in `f64` on a deterministic candidate-vertex primitive (every
//! well-conditioned plane triple, solved directly, filtered by feasibility) -- no
//! convex-hull library, no hashed iteration, byte-identical results run to run.
//!
//! # External verification: printed proportions
//!
//! A wrong solve is *self-consistent* -- every tier still lands on a real meet
//! vertex -- so nothing internal to the arrangement separates it from the truth
//! (several internal discriminators were tried and rejected; see the NOTE
//! comments in this module and in `candidates`, `solve`, `verify`). What does
//! separate them is **external**: the proportion figures printed on every real
//! diagram (`Vol/W^3`, `L/W`, `C/W`, `P/W`, `H/W`). The true configuration
//! reproduces them to ~0.1% median while a wrong solve is off by ~30%
//! ([`super::stone_metrics`] holds the calibration). [`solve_meet_points_verified`]
//! exploits this with a greedy repair search over phase-1 vertex-level picks,
//! scored by [`ExternalProportions::combined_deviation`].
//!
//! # Validating a schedule (forward direction)
//!
//! [`vertex_meet_groups`] is the reverse capability: given an already-solved
//! [`GemPolyhedron`], it reports which input planes actually touch at each
//! vertex -- geometric ground truth independent of a file's `G`-field text.
//!
//! # Cost envelope
//!
//! [`candidates::enumerate_candidate_vertices`] -- every well-conditioned
//! triple of the arrangement's `P` planes, solved and feasibility-tested -- is
//! `O(P^3)` per call. Phase 1 (the constructive pass) avoids paying that per
//! tier via `phase1_cache`'s incremental candidate set, but phase 3
//! (refinement) re-enumerates the *entire* arrangement from scratch every
//! sweep, up to [`MAX_REFINE_SWEEPS`] times -- a deliberate choice, not an
//! oversight: an owner-move-invalidated persistent cache across phase-3 sweeps
//! was implemented and measured, and rejected (see the NOTE at the top of
//! `candidates`) -- on a real 103-tier design, 97-100 of its 103 tiers move on
//! *every* sweep (it never converges within the cap), so almost nothing ever
//! qualified for the cheap path and the cache's own bookkeeping cost more than
//! it saved (a measured 27% regression). Do not reintroduce that cache without
//! new evidence the corpus's move pattern has changed.
//!
//! [`MAX_PLANES`] (400) hard-caps what gets solved at all -- above it,
//! [`solve_meet_points`] and [`solve_meet_points_verified`] both return
//! immediately with [`SolveStrategy::Failed`] tiers rather than attempting a
//! solve. No design in the 2,881-design corpus this crate is calibrated
//! against comes anywhere near that cap. [`solve_meet_points_verified`]'s own
//! repair search documents its per-design cost model directly (see
//! `VERIFY_RUN_BUDGET` in `verify`): one pipeline run costs roughly
//! `tiers * planes^3`, and since tier count itself grows with plane count,
//! that is effectively `planes^4` -- which is why the verified search's
//! per-design run budget is `VERIFY_RUN_BUDGET / total_planes^4` (clamped to
//! `[1, VERIFY_MAX_RUNS]`, `VERIFY_MAX_RUNS_CALIBRATED` with adjustable
//! anchors): a ~60-plane design gets the search's full budget (up to 120
//! runs), a ~150-plane design gets roughly 20, and a 300+-plane design is left
//! with essentially the one plain-solve run -- the budget itself is what keeps
//! a large design from spending minutes in the repair search rather than any
//! change in per-run cost.
//!
//! Measured figures, not modeled estimates: [`solve_meet_points_verified`]'s
//! own corpus probe (see that function's doc comment, Report C) ran a mean of
//! 68.4 pipeline runs per design over the full 2,881-design corpus (fixed
//! anchors) in about 29 minutes at 16 threads -- averaging on the order of
//! **~0.1 s per pipeline run** across the corpus's real, heavily
//! small-plane-count-weighted distribution (the run-budget scaling above is
//! exactly what keeps that average low despite the heavy tail: without it,
//! the ~190 corpus designs above 100 planes would each burn many core-minutes
//! per design, per that same comment). The one measured single-design,
//! single-run-class data point at the heavy end: the real 103-tier design
//! used in the threading and caching experiments above took 5.5-6.1 s
//! single-threaded end to end (see the NOTE at the top of `candidates`).
//!
//! Practical expectation for a caller: a design in the roughly 60-150-plane
//! range (typical of the corpus) solves in a small fraction of a second to
//! low seconds even under [`solve_meet_points_verified`]'s repair search;
//! a design pushing into the 300+-plane range should be expected to take
//! several seconds and to receive little or no repair search regardless (the
//! budget above has already reduced it near to the plain solve). Neither
//! entry point streams progress or supports cancellation mid-solve -- a
//! caller driving this from an interactive UI should run it off the main
//! thread and size any timeout to the plane count, not assume a fixed budget.
//!
//! # Module layout
//!
//! Split by seam, not by size: [`blocks`] classifies each tier into
//! crown/pavilion/girdle; [`anchors`] fills in per-block scale references from
//! printed proportions; [`names`] resolves stated `"Meet <names>"` text against
//! tier names; [`candidates`] is the deterministic candidate-vertex primitive
//! (plane triples -> feasible vertices -> `n . v` levels); `phase1_cache` is
//! [`solve`]'s incremental candidate-vertex cache; [`solve`] holds the
//! three-phase pipeline ([`SolveContext`](solve::SolveContext),
//! [`solve_meet_points`]); `verify` layers the externally-verified repair search
//! ([`solve_meet_points_verified`]) on top; `validation` holds the reverse and
//! schedule-export helpers.

use indicatrix_formats::asc::AscSchedule;

mod anchors;
mod blocks;
mod candidates;
mod names;
mod phase1_cache;
mod solve;
mod validation;
mod verify;

pub use anchors::apply_ratio_anchors;
pub use blocks::{Block, classify_blocks};
pub use candidates::tier_instance_normals;
pub use names::{MeetNameResolver, ResolvedNames, TokenResolution};
pub use solve::solve_meet_points;
pub use validation::{build_reconstructed_schedule, vertex_meet_groups};
pub use verify::{VERIFY_ACCEPT_TOL, VerifiedSolveReport, solve_meet_points_verified};

/// Half-extent of the bounding box standing in for the uncut rough stone. Real
/// `.asc` masts sit close to 1.0, so this never masquerades as a real facet; it only
/// keeps the candidate-vertex feasibility test well-defined before the arrangement
/// closes up.
const BLANK_HALF_EXTENT: f64 = 64.0;

/// Feasibility slack: a candidate vertex may poke this far (absolute; masts are ~1)
/// beyond a plane before that plane's tier counts as violated.
const EPS_FEAS: f64 = 1e-5;

/// A plane within this absolute distance of a vertex counts as passing through it
/// (used to test incidence with named meet references).
const EPS_INCIDENT: f64 = 1e-4;

/// Two candidate `n . v` values within this absolute distance belong to one vertex
/// "level".
const LEVEL_TOL: f64 = 1e-5;

/// Minimum `|determinant|` for a triple of unit plane normals to define a candidate
/// vertex.
const MIN_TRIPLE_DET: f64 = 1e-6;

/// Designs with more facet planes than this are not solved (the candidate
/// enumeration is cubic in the plane count). No design in the 2,881-design corpus
/// comes anywhere near it.
const MAX_PLANES: usize = 400;

/// Maximum constructive sweeps (phase 1). Values snap onto exact vertex levels,
/// so convergence, when it happens, is exact; the cap only guards against cycling.
const MAX_CONSTRUCTIVE_SWEEPS: usize = 64;

/// Maximum nearest-level refinement sweeps (phase 3).
///
/// Many runs never satisfy the `1e-12` convergence test -- masts drift or
/// oscillate between vertex levels instead of contracting -- so extra sweeps
/// mostly waste time rather than improve accuracy; convergent runs typically
/// settle within one or two sweeps anyway. Capping at 4 measured better on every
/// corpus accuracy metric than higher caps while cutting phase-3 CPU time by
/// roughly 2.7x. Tried a fixed-point acceleration of the vertex map instead: it
/// mostly failed to converge within 20,000 iterations, and slightly worsened
/// accuracy on the corpus median when it did; rejected.
const MAX_REFINE_SWEEPS: usize = 4;

/// A picked level more than this many times the design's scale prior almost
/// certainly means the region isn't really bounded there yet (the pick hit
/// bounding-blank geometry); such picks are rejected.
const BLANK_DOMINATION_FACTOR: f64 = 4.0;

/// Default scale assumed when a solve has no scale-reference tier at all.
const DEFAULT_PLAUSIBLE_SCALE: f64 = 1.0;

/// One facet tier awaiting a solved mast distance.
///
/// Mirrors the geometry-relevant fields of [`indicatrix_formats::asc::AscTier`], plus the
/// constraint that determines its mast. Kept independent of `AscTier` so this
/// solver isn't coupled to one file format.
#[derive(Debug, Clone)]
pub struct MeetTierInput {
    /// Signed angle from the girdle plane, in degrees (`GemCAD` convention:
    /// negative is pavilion, non-negative is crown). An unsigned `0.0` inherits
    /// the previous tier's side, per
    /// [`super::cuts::StandardGemCuts::from_asc_schedule`]'s convention.
    pub angle_deg: f64,
    /// Index-wheel positions this tier's facet occurs at. Empty means a single
    /// facet at azimuth 0 (e.g. an unlisted table/culet).
    pub indices: Vec<f64>,
    pub constraint: MeetConstraint,
    /// Every name this tier is known by in the source schedule (e.g. `["P1"]`, or
    /// `["c", "d"]` for a tier folded from more than one named group). Used only
    /// to resolve [`MeetConstraint::MeetNamed`]'s references back to tier indices.
    pub names: Vec<String>,
}

/// How a tier's mast distance is determined.
///
/// `PartialEq` is derived for `indicatrix-cut-core`'s benefit: `indicatrix_cut_core::design::ConstraintTier`
/// stores this as editable state and needs it for undo/redo round-trip tests.
#[derive(Debug, Clone, PartialEq)]
pub enum MeetConstraint {
    /// Cut past first touch until the plane passes through a meet vertex of the
    /// other facets' arrangement, with no further information about *which*
    /// vertex. Covers "Cut to centerpoint", "Table", and any unnamed meet-style
    /// instruction; solved with the rank-1 prior (see the module docs).
    MeetExisting,
    /// Like [`Self::MeetExisting`], but the schedule states explicitly which
    /// facets this tier closes against (a real `.asc` `"G Meet P1, P2, G1"`
    /// instruction). Resolved against every tier's [`MeetTierInput::names`] via
    /// [`MeetNameResolver`]; an unresolved token is dropped, and a tier with no
    /// resolved tokens degrades to [`Self::MeetExisting`]'s rank-1 handling.
    MeetNamed(Vec<String>),
    /// An externally supplied target mast: "Set girdle thickness" / "Set stone
    /// size" / "Level girdle", or a caller-supplied per-block dimension (see the
    /// module docs on anchors). Not derivable from geometry.
    ScaleReference(f64),
}

/// Builds [`MeetTierInput`]s from a parsed `.asc` schedule, classifying each
/// tier's [`MeetConstraint`] from its `G`-field text.
///
/// See [`indicatrix_formats::asc::AscTier::meet_instruction`]: `Meet` -> `MeetNamed`;
/// `ScaleReference`/`LevelGirdle` -> `ScaleReference` (using the tier's recorded
/// `mast`); everything else -> `MeetExisting`.
///
/// Never fabricates a scale anchor: a block with no stated anchor still needs
/// one from the caller (see the module docs).
#[must_use]
pub fn meet_tier_inputs_from_asc(schedule: &AscSchedule) -> Vec<MeetTierInput> {
    schedule
        .tiers
        .iter()
        .map(|tier| {
            let constraint = match tier.meet_instruction() {
                Some(indicatrix_formats::asc::MeetInstruction::Meet(names)) => {
                    MeetConstraint::MeetNamed(names)
                }
                Some(
                    indicatrix_formats::asc::MeetInstruction::ScaleReference
                    | indicatrix_formats::asc::MeetInstruction::LevelGirdle,
                ) => MeetConstraint::ScaleReference(tier.mast),
                _ => MeetConstraint::MeetExisting,
            };
            MeetTierInput {
                angle_deg: tier.angle_deg,
                indices: tier.indices.clone(),
                constraint,
                names: tier.names().into_iter().map(str::to_string).collect(),
            }
        })
        .collect()
}

/// Which technique actually produced a tier's solved mast, for reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SolveStrategy {
    /// The mast was supplied directly (a [`MeetConstraint::ScaleReference`]).
    ScaleReference,
    /// Settled in the constructive fixed-point pass: a candidate meet vertex from
    /// already-settled planes, possibly polished by refinement sweeps afterwards.
    DependencyOrder,
    /// Part of a mutually-dependent remainder the constructive pass couldn't
    /// order; settled by refinement sweeps over the full arrangement.
    JointGroup,
    /// No usable candidate vertex; the mast is the per-block
    /// `a*cos(theta) + b*sin(theta)` estimate, not vertex-derived.
    LeastSquaresFallback,
    /// The solve could not produce even an estimate (e.g. exceeds [`MAX_PLANES`]).
    /// The returned mast is a placeholder and should not be trusted.
    Failed,
}

/// One tier's solved result.
#[derive(Debug, Clone)]
pub struct SolvedTier {
    pub mast: f64,
    pub strategy: SolveStrategy,
    /// Human-readable detail about how the value was obtained.
    pub detail: String,
}
