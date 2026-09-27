//! Shared record types for the meet-solver validation harness: the `AscRow` input
//! record (including the printed proportions used by the ratio-anchored and
//! externally-verified reports), the `ConstraintKind`/`TierResult` per-tier
//! classification, and the `DesignResult` per-design accumulator.

use indicatrix::geometry::meet_solver::SolveStrategy;

/// Worker count for the parallel solve below. Fixed rather than queried from the
/// system (`std::thread::available_parallelism`) so a run's shape doesn't silently
/// change between machines -- this is a throwaway probe re-run often while iterating,
/// not a shipped tool, so a hardcoded figure matching the dev machine (16 cores) is
/// the simpler choice.
pub const THREADS: usize = 16;

pub struct AscRow {
    pub detail_id: i64,
    pub content: Vec<u8>,
    /// Printed crown-height/width and pavilion-depth/width proportions from
    /// `diagram_details` (`NULL` for a design the source never recorded them for).
    /// Only used by [`solve_one_ratio_anchored`]; [`solve_one`] (the original
    /// tier-0-bootstrap measurement) ignores these entirely.
    pub cw_ratio: Option<f64>,
    pub pw_ratio: Option<f64>,
    /// Printed `Vol/W^3`, `L/W` and `H/W`, used (together with the two ratios
    /// above) only by Report C's [`solve_one_verified`] as the external
    /// verification targets.
    pub volume: Option<f64>,
    pub lw_ratio: Option<f64>,
    pub hw_ratio: Option<f64>,
}

/// Which kind of information actually determined a tier's constraint, *before* the
/// bootstrap fallback (see `main`) might override tier 0. Used only for reporting --
/// separates "the schedule stated this" from "the solver had to infer it".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstraintKind {
    ScaleReference,
    MeetNamed,
    MeetExisting,
}

#[derive(Clone)]
pub struct TierResult {
    pub strategy: SolveStrategy,
    pub rel_err: f64,
    pub kind: ConstraintKind,
    /// Only meaningful when `kind == ConstraintKind::MeetNamed`: `Some(true)` if
    /// every name this tier's stated `"Meet <names>"` instruction referenced
    /// resolved to a known tier (via the *exact same* `name_to_tier`/`girdle_tier`/
    /// `resolve_name` logic `solve_meet_points` itself uses internally -- see
    /// `classify_named_resolution` below), `Some(false)` if one or more didn't,
    /// `None` for any other `kind`. This is what splits `MeetNamed` into the
    /// `MeetNamed-resolved` / `MeetNamed-unresolved` buckets in the report: real
    /// `.asc` meet text is hand-typed free prose ("Meet the girdle", "Meet 2 and
    /// the culet", "Meet P1, P4, P4, Form, PCP") where a majority of stated names
    /// never resolve to anything at all, and the original evidence for the
    /// vertex-incidence model was measured only on the subset that does resolve --
    /// so the two populations must be reported separately, never blended.
    pub named_resolved: Option<bool>,
    /// Whether the solver's constructive pass actually settled this tier via its
    /// resolved named references (as opposed to falling back to the rank-1 prior
    /// because the references hadn't settled yet, or no incident level existed).
    /// Read from the solver's own `detail` string.
    pub used_named: bool,
    /// For a rank-1 fallback on a tier with resolved refs: why (from the solver's
    /// detail string). 'u' = refs not settled at release, 'n' = no incident
    /// feasible level, ' ' = not a named fallback.
    pub fallback_cause: char,
}

/// Everything one design's solve contributes to the aggregate report. Produced by
/// [`solve_one`], which is the unit of work parallelized across `THREADS` workers.
#[derive(Default)]
pub struct DesignResult {
    pub parse_ok: bool,
    /// True iff the design had no stated scale-reference tier at all, so the harness
    /// had to bootstrap one from the file's own tier-0 real mast (see `solve_one`).
    /// Only meaningful when `parse_ok`.
    pub no_scale_reference: bool,
    pub tier_results: Vec<TierResult>,
    /// Worst meet-derived tier's relative error, or `None` if the design had none to
    /// score. Only meaningful when `parse_ok`.
    pub worst_err: Option<f64>,
    /// Which `ConstraintKind`s appear anywhere in this design's tiers -- for the
    /// per-design bucket counts (a design can and often does mix kinds).
    pub has_meet_named: bool,
    pub has_meet_existing: bool,
    pub has_scale_reference: bool,
    /// Only meaningful for [`solve_one_ratio_anchored`]'s results: true iff every
    /// block that needed an anchor at all got it from a printed `C/W`/`P/W` ratio
    /// (or a schedule's own stated scale reference) -- i.e. this design's solve
    /// needed *no* real-recorded-mast fallback anywhere. False whenever at least
    /// one block's ratio was missing and had to fall back to its own tier-0 real
    /// mast (a harness-only crutch unavailable to the ~2,700 catalogued designs
    /// with no `.asc` file at all -- see the module doc comment).
    pub fully_ratio_anchored: bool,
    /// Reports C and D only ([`solve_one_verified`] /
    /// [`solve_one_ratio_anchored_verified`]): the externally-verified repair
    /// search's own accounting. `None` for the other reports.
    pub verify: Option<indicatrix::geometry::meet_solver::VerifiedSolveReport>,
}
