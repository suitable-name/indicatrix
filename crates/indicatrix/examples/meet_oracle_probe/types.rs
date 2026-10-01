//! Shared tuning constants and record types for the meet-oracle probe: tolerance
//! knobs used by candidate enumeration and the core per-design analysis, plus the
//! `AscRow` input record, `Plane`/`Cand` arrangement primitives, and the
//! `TierStats`/`DesignResult` output accumulators.

use glam::DVec3;

/// Worker thread count for the parallel per-design analysis.
pub const THREADS: usize = 16;
/// Bounding-blank half extent (matches the solver's `BLANK_HALF_EXTENT`).
pub const BLANK: f64 = 64.0;
/// Feasibility slack: a vertex may poke this far (absolute, masts are ~1) beyond a
/// plane before that plane's tier counts as violated.
pub const EPS_FEAS: f64 = 1e-5;
/// Incidence tolerance: a plane within this absolute distance of a vertex counts as
/// passing through it.
pub const EPS_INCIDENT: f64 = 1e-4;
/// Relative error below which a candidate value counts as matching the true mast.
pub const MATCH_REL: f64 = 0.005;
/// Two candidate values within this (absolute) distance belong to one "level".
pub const LEVEL_TOL: f64 = 1e-5;
/// Designs with more planes than this are skipped (cubic triple enumeration).
pub const MAX_PLANES: usize = 400;
/// Minimum |det| for a triple of unit normals to define a candidate vertex.
pub const MIN_DET: f64 = 1e-6;

/// One `.asc` schedule row loaded from the database.
pub struct AscRow {
    /// Database id of the design the file belongs to.
    pub detail_id: i64,
    /// Raw bytes of the attached `.asc` file.
    pub content: Vec<u8>,
}

/// One plane of the design: unit normal, offset (mast), owning tier (usize::MAX for
/// the bounding blank).
#[derive(Clone, Copy)]
pub struct Plane {
    /// Unit normal.
    pub n: DVec3,
    /// Offset (mast) along the normal.
    pub m: f64,
    /// Index of the owning tier.
    pub owner: usize,
}

/// One candidate vertex of the arrangement: position, the single tier whose planes it
/// violates (`None` = feasible for the full solid), and the three owning tiers of the
/// planes that formed it.
pub struct Cand {
    /// Vertex position.
    pub v: DVec3,
    /// The single tier whose planes the vertex violates, if any.
    pub violated: Option<usize>,
    /// Owning tiers of the three planes that formed the vertex.
    pub owners: [usize; 3],
}

/// Per-tier outcome of the oracle analysis.
#[derive(Default)]
pub struct TierStats {
    /// E3b relative error, instance 0.
    pub err0: f64,
    /// E3b relative error, worst instance.
    pub err_worst: f64,
    /// Rank (0-based level index, descending by value) of the true level, instance 0.
    /// `None` when err0 >= MATCH_REL (no matching level).
    pub rank: Option<usize>,
    /// Rank after intersecting candidate values across all instances.
    pub rank_sym: Option<usize>,
    /// Total candidate levels (instance 0).
    pub n_levels: usize,
    /// Tangency overshoot: (max candidate value) / true mast, instance 0.
    pub tangency_ratio: f64,
    /// Rank of the deepest level at which every other tier's facet keeps >= 3
    /// corner vertices ("deepest safe cut").
    pub deepest_safe_rank: Option<usize>,
    /// E4: relative error restricted to candidates incident to every resolved named
    /// reference. `None` when the tier has no resolved named refs.
    pub e4_err: Option<f64>,
    /// How many of the tier's stated meet names resolved / total stated.
    pub named_resolved: Option<(usize, usize)>,
    /// True if this tier was reachable in the incremental simulation.
    pub reachable: bool,
    /// Relative mast error from the global true-incidence linear solve. `None` when
    /// the design's global system could not be assembled/solved for this tier.
    pub global_err: Option<f64>,
    /// Selection-rule experiment hits (others pinned at truth): did each rule's
    /// predicted level match the true mast within MATCH_REL?
    pub hit_rank1: bool,
    /// Hit flag of the named-reference rank-1 rule.
    pub hit_named_rank1: bool,
    /// Hit flag of the incidence-degree rule.
    pub hit_degree: bool,
    /// Hit flag of the named-reference incidence-degree rule.
    pub hit_named_degree: bool,
    /// Relative error of the named->rank1 rule's prediction (not just hit/miss).
    pub named_rank1_err: Option<f64>,
    /// True if the tier is meet-derived (scored at all).
    pub scored: bool,
}

/// Everything one design's analysis contributes to the aggregate report.
#[derive(Default)]
pub struct DesignResult {
    /// The schedule parsed successfully.
    pub parsed: bool,
    /// The design exceeded `MAX_PLANES` and was skipped.
    pub skipped_too_big: bool,
    /// The design's arrangement was degenerate and could not be analysed.
    pub degenerate: bool,
    /// Per-tier statistics.
    pub tiers: Vec<TierStats>,
    /// Every scored tier was reachable in the incremental simulation.
    pub all_reachable: bool,
    /// At least one tier was meet-derived and scored.
    pub any_scored: bool,
    /// Median relative mast error of the production solver, if it ran.
    pub solver_median_err: Option<f64>,
    /// Total degeneracy of the arrangement at the true masts.
    pub degeneracy_truth: Option<f64>,
    /// Total degeneracy of the arrangement at the solved masts.
    pub degeneracy_solved: Option<f64>,
}
