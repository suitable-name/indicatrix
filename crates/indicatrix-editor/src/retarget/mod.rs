//! "Retarget for material" -- proposing and applying a new set of facet angles when
//! a design's material changes.
//!
//! Pure Rust, no Slint types: [`build_proposal`] takes a `Design` and a resolved
//! target material and returns a [`RetargetProposal`] the dialog shows for review;
//! [`apply`] turns an accepted proposal into the one `Edit::RetargetAngles` the
//! caller pushes through `History`, exactly like every other edit in this crate.
//!
//! The desktop dialog uses the richer pieces on top of that:
//!
//! - [`plan`]: [`build_plan`] is the cheap synchronous half (which tiers move, the
//!   guarded new angles, the per-row margin and risk). Nothing in it solves the design.
//! - [`check`]: the half that needs a solved stone and runs on a worker thread. It
//!   re-anchors the masts so every facet turns about its girdle-side edge (the girdle
//!   keeps its outline and height), refits the table and culet heights, judges the
//!   result with [`validity`]'s gate and measures the optics ([`metrics`]).
//! - [`anchors`]: [`apply_with_anchors`] turns a proposal plus the re-anchored masts
//!   into ONE undoable `Edit::Batch` (angles and masts together).
//! - [`search`]: the desktop dialog's Optimize mode. It starts from the Shift result and
//!   searches every crown and pavilion angle inside a range (pinned facets included, each
//!   turning about its girdle edge), then runs the validity gate on the ranked results.
//!   [`RetargetMode::Optimize`] below is the older, simpler path the web app still uses.
//!
//! # The two algorithms
//!
//! - [`RetargetMode::Shift`] (default): every pavilion tier's angle moves by
//!   `retarget_angle_deg`, which keeps that tier's margin over the critical angle
//!   exactly fixed; every crown tier moves by [`CrownShift`]'s policy -- by default it
//!   follows the pavilion's vertical stretch so the stone keeps its silhouette, with a
//!   fraction of the shift or the critical-angle ratio as opt-ins. A shifted angle
//!   never crosses the horizontal and stays within 1 to 89.5 degrees (see
//!   `indicatrix_cut_core::optics_hints::guard_retargeted_angle_deg`). Flat tiers (the
//!   table, the culet) are never changed: the plan lists them, the proposal does not.
//!   Girdle tiers are never touched -- not even listed: a tier at (or near)
//!   plus-or-minus 90 degrees from the girdle plane classifies as `Block::Girdle`,
//!   structural rather than optical.
//! - [`RetargetMode::Optimize`] seeds a clone of the design with the `Shift` angles
//!   above, then runs `optimize_design` unchanged (only the objective material
//!   differs) over whatever tiers that search already treats as free
//!   (non-`ScaleReference`). Since an anchored tier's angle can never actually move
//!   under that search, retargeting a currently-anchored tier via this mode is
//!   refused up front ([`RetargetError::AnchoredTiers`]). Only the web app takes this path
//!   now; the desktop dialog uses [`search`], which varies pinned tiers too and so never
//!   returns that refusal.
//!
//! # Why `apply` also takes the `Design`
//!
//! [`RetargetProposal::rows`] carries each row's `old_angle` for display, but
//! [`apply`] re-reads the CURRENT `angle_deg` from `design` when it builds the
//! `Edit::RetargetAngles` -- trust runtime state, not a caller's claimed previous
//! value. This also means a proposal is safe to apply even if the design moved
//! slightly between building it and pressing Apply; a row naming a tier index the
//! design names is dropped rather than panicking.

use indicatrix::{
    geometry::meet_solver::{Block, MeetConstraint, classify_blocks},
    optics::materials::GemMaterial,
};
use indicatrix_cut_core::{
    AngleChange, Design, DesignSolveError, Edit, OptimizeConfig, ResolvedMaterial, Risk,
    SearchHooks, critical_angle_deg, optics_hints::is_horizontal_angle_deg, optimize_design,
};

pub mod anchors;
pub mod check;
pub mod metrics;
pub mod plan;
mod refit;
pub mod search;
#[cfg(test)]
mod tests;
pub mod validity;
pub mod view;

pub use anchors::{AnchorChange, apply_with_anchors};
pub use check::GirdleAllowance;
pub use plan::{PlanRow, RetargetPlan, build_plan};

/// How a crown tier's angle follows the pavilion critical-angle shift.
///
/// There are three rules; when more than one is set the precedence is
/// `scale_by_ratio` > `follow_pavilion` > `fraction`.
///
/// - **Follow the pavilion** (`follow_pavilion`, the default): the pavilion's steeper or
///   shallower angle stretches the stone vertically at the girdle edge by
///   `s = tan(P') / tan(P)` (`P` the representative pavilion angle, `P'` its shifted
///   one). Every crown angle gets the same stretch, `theta' = atan(s * tan(theta))`, so the
///   stone keeps its silhouette (crown height against pavilion depth) and its table size.
///   A design without a pavilion, or one whose pavilion does not move, has `s = 1`.
/// - **Fraction** (`fraction`): the crown moves by that fraction of the same delta every
///   pavilion tier shifts by: `critical_angle_deg(n_to) - critical_angle_deg(n_from)` is
///   one constant number, not per-tier, because the margin-preserving shift formula
///   reduces to that constant added to `theta` regardless of a tier's starting angle.
///   `0.0` leaves the crown alone.
/// - **Ratio** (`scale_by_ratio`): scale the raw angle by the ratio of the two critical
///   angles: `theta' = theta * critical_angle_deg(n_to) / critical_angle_deg(n_from)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CrownShift {
    /// The fraction of the pavilion critical-angle delta a crown tier moves by (used
    /// only when neither of the other two rules is set).
    pub fraction: f64,
    /// Scale the crown angle by the critical-angle ratio instead (wins over both other
    /// rules).
    pub scale_by_ratio: bool,
    /// Let every crown angle follow the pavilion's vertical stretch (wins over
    /// `fraction`).
    pub follow_pavilion: bool,
}

impl CrownShift {
    /// The policy that leaves every crown angle where it is ("the crown stays"): the
    /// behaviour before the crown followed the pavilion.
    #[must_use]
    pub const fn fixed() -> Self {
        Self {
            fraction: 0.0,
            scale_by_ratio: false,
            follow_pavilion: false,
        }
    }
}

impl Default for CrownShift {
    fn default() -> Self {
        Self {
            fraction: 0.0,
            scale_by_ratio: false,
            follow_pavilion: true,
        }
    }
}

/// Which of the two algorithms builds the proposal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RetargetMode {
    /// The deterministic critical-angle shift -- always available, never fails to
    /// solve (it never even calls `Design::solve`).
    Shift,
    /// Seeds from the shift above, then runs `optimize_design` (unchanged) with
    /// `config` against the resolved target material.
    Optimize(OptimizeConfig),
}

/// One reviewable row of a [`RetargetProposal`] -- one per pavilion or crown tier that
/// actually moves (flat tiers and girdle tiers are never listed, see the module doc
/// comment).
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetRow {
    /// This tier's position in `design.tiers`.
    pub tier_index: usize,
    /// The tier's block (never `Block::Girdle`).
    pub block: Block,
    /// The tier's name.
    pub name: String,
    /// The tier's angle when the proposal was built.
    pub old_angle: f64,
    /// The proposed angle.
    pub new_angle: f64,
    /// `new_angle`'s margin in the TARGET material: the critical-angle margin for a
    /// pavilion row, the crown-window estimate for a crown row.
    pub margin_deg: f64,
    /// `margin_deg`'s windowing risk in the target material.
    pub risk: Risk,
}

/// What [`build_proposal`] returns: one row per retargeted tier, the resolved
/// target material the rows were computed against, and any caller-facing notes.
#[derive(Debug, Clone, PartialEq)]
pub struct RetargetProposal {
    /// One row per retargeted tier, in schedule order.
    pub rows: Vec<RetargetRow>,
    /// The resolved target material the rows were computed against.
    pub target: ResolvedMaterial,
    /// Caller-facing notes (the material move, rows held at a limit, the Optimize
    /// caveat).
    pub notes: Vec<String>,
}

/// Why [`build_proposal`] could not build a proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RetargetError {
    /// `RetargetMode::Optimize` needs a real solved baseline before it can search
    /// at all -- propagated from `optimize_design`/`Design::solve` verbatim.
    Solve(DesignSolveError),
    /// `RetargetMode::Optimize` was asked to retarget one or more tiers currently
    /// `ScaleReference` (so `optimize_design` can never move them). `(tier_index,
    /// name)` per anchored tier, in schedule order. The desktop dialog never gets this
    /// ([`search`] varies pinned tiers about their girdle edges); the web app still does.
    AnchoredTiers(Vec<(usize, String)>),
}

impl std::fmt::Display for RetargetError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Solve(err) => write!(f, "design does not solve: {err}"),
            Self::AnchoredTiers(tiers) => {
                write!(f, "adopt these tiers' meet constraint before optimizing: ")?;
                for (i, (index, name)) in tiers.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    write!(f, "#{index} \"{name}\"")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for RetargetError {}

/// `true` iff `constraint` is a `ScaleReference` -- the predicate both
/// [`build_proposal`]'s anchored-tier check and `optimize::free_tier_indices` key off.
const fn is_scale_reference(constraint: &MeetConstraint) -> bool {
    matches!(constraint, MeetConstraint::ScaleReference(_))
}

/// The guarded shift for tier `index` under its own [`Block`] -- see
/// [`plan::shift_angle`]. Girdle tiers and flat tiers come back unchanged, so this stays
/// a total function. `stretch` is the design's [`plan::pavilion_stretch`], which the
/// caller works out once for all the tiers.
fn shifted_angle(
    design: &Design,
    index: usize,
    block: Block,
    n_from: f64,
    n_to: f64,
    crown: CrownShift,
    stretch: f64,
) -> f64 {
    plan::shift_angle(
        design.tiers[index].angle_deg,
        block,
        n_from,
        n_to,
        crown,
        stretch,
    )
    .angle_deg
}

/// Every pavilion/crown tier `build_proposal` moves (girdle tiers and flat tiers
/// excluded, see the module doc comment), plus the [`Block`] classification the caller
/// needs to interpret it.
///
/// Shared with `callbacks::retarget_actions`'s off-thread `RetargetMode::Optimize` wiring
/// so that module never reimplements `Block` classification itself.
///
/// The scope only ever holds positions in `design.tiers`: concave tiers (their own list,
/// `design.concave_tiers`) have no angle the material retarget could shift (a concave
/// tier's angle is authored against the tool, not a critical-angle margin), so they are
/// never part of it and the returned `Block` list has one entry per FLAT tier.
#[must_use]
pub fn retarget_scope(design: &Design) -> (Vec<usize>, Vec<Block>) {
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let scope = (0..design.tiers.len())
        .filter(|&i| {
            blocks[i] != Block::Girdle && !is_horizontal_angle_deg(design.tiers[i].angle_deg)
        })
        .collect();
    (scope, blocks)
}

/// The `scope` tiers (see [`retarget_scope`]) still pinned `ScaleReference`.
///
/// [`RetargetMode::Optimize`]'s up-front refusal check, factored out so
/// `callbacks::retarget_actions` can run this same synchronous check before handing the
/// actual search off to a worker thread.
#[must_use]
pub fn anchored_tiers_in(design: &Design, scope: &[usize]) -> Vec<(usize, String)> {
    scope
        .iter()
        .filter(|&&i| is_scale_reference(&design.tiers[i].constraint))
        .map(|&i| (i, design.tiers[i].name.clone()))
        .collect()
}

/// Clones `design` and seeds every `scope` tier with its critical-angle-shifted
/// angle (see [`shifted_angle`]) -- the deterministic baseline
/// [`RetargetMode::Optimize`] starts its search from.
///
/// Shared with
/// `callbacks::retarget_actions`'s off-thread wiring so the seeded design handed to
/// the worker thread is built by the exact same code the synchronous
/// [`build_optimized_angles`] path uses.
#[must_use]
pub fn seed_shift_design(
    design: &Design,
    scope: &[usize],
    blocks: &[Block],
    n_from: f64,
    n_to: f64,
    crown: CrownShift,
) -> Design {
    let mut seeded = design.clone();
    let stretch = plan::pavilion_stretch(design, n_from, n_to);
    for &i in scope {
        seeded.tiers[i].angle_deg =
            shifted_angle(design, i, blocks[i], n_from, n_to, crown, stretch);
    }
    seeded
}

/// Turns a finished (or cancelled) Optimize search's [`AngleChange`]s into
/// [`RetargetRow`]s.
///
/// `seeded`'s own angle for every `scope` tier, overridden by whichever tiers `changes`
/// actually touched, with each row's `old_angle` read from `original` (the design as it
/// stood before any shift/search ran) -- exactly [`build_optimized_angles`]'s own
/// post-processing, factored out so `callbacks::retarget_actions`'s off-thread completion
/// handler can build the same rows a synchronous call would have.
#[must_use]
pub fn rows_from_outcome(
    original: &Design,
    seeded: &Design,
    scope: &[usize],
    blocks: &[Block],
    n_to: f64,
    changes: &[AngleChange],
) -> Vec<RetargetRow> {
    let mut final_angles: Vec<f64> = scope.iter().map(|&i| seeded.tiers[i].angle_deg).collect();
    for change in changes {
        if let Some(slot) = scope.iter().position(|&i| i == change.index) {
            final_angles[slot] = change.to_deg;
        }
    }
    let entries: Vec<(usize, Block, f64)> = scope
        .iter()
        .zip(&final_angles)
        .map(|(&index, &angle)| (index, blocks[index], angle))
        .collect();
    plan::legacy_rows(original, &entries, n_to)
}

/// A one-line summary of the material move, plus (for `Optimize`) a note that
/// the search only ever touches free tiers.
///
/// `pub` so
/// `callbacks::retarget_actions`'s off-thread completion handler can build the
/// identical notes a synchronous [`build_proposal`] call would have.
///
/// `stretch` is the design's [`plan::pavilion_stretch`]; when the crown follows the pavilion
/// ([`CrownShift::follow_pavilion`], and `scale_by_ratio` is not set) and the stretch is not
/// `1.0`, a note says so.
#[must_use]
pub fn build_notes(
    mode: RetargetMode,
    n_from: f64,
    n_to: f64,
    crown: CrownShift,
    stretch: f64,
) -> Vec<String> {
    let delta = critical_angle_deg(n_to) - critical_angle_deg(n_from);
    let mut notes = vec![format!(
        "Retargeting from n_D {n_from:.4} to n_D {n_to:.4}: critical angle moves by {delta:+.3} deg."
    )];
    // `pavilion_stretch` returns exactly 1.0 when there is nothing to stretch.
    if crown.follow_pavilion && !crown.scale_by_ratio && stretch.to_bits() != 1.0_f64.to_bits() {
        notes.push(format!(
            "Crown tiers follow the pavilion's stretch: every crown angle's tangent is scaled by {stretch:.3}, so the stone keeps its silhouette and its table size."
        ));
    }
    if matches!(mode, RetargetMode::Optimize(_)) {
        notes.push(
            "Optimize mode seeds from the critical-angle shift, then searches free tiers only; anchored tiers keep their seeded angle."
                .to_string(),
        );
    }
    notes
}

/// Builds a retarget proposal for `design` against `target`, per `mode` -- see the
/// module doc comment for both algorithms.
///
/// `target.n_d` is the "to" index. The "from" index resolves through
/// [`Design::effective_refractive_index_with`] against `custom_materials`, NOT the
/// built-ins-only [`Design::effective_refractive_index`]. A design on a custom
/// catalogue material (e.g., "Garnet 1.74") must retarget against its own real
/// recorded index, not whatever the built-ins fallback would substitute (the
/// legacy schedule RI or 1.54). Every proposed angle shifts from that number, so
/// an error moves every facet on the stone.
///
/// Pass `&[]` only when there is genuinely no catalogue in scope -- which, outside
/// this module's own tests, there never is.
///
/// # Errors
///
/// - [`RetargetError::AnchoredTiers`] (`Optimize` only): one or more pavilion/crown
///   tiers this proposal would otherwise retarget are still `ScaleReference`-pinned.
///   Adopt them first, then call this again.
/// - [`RetargetError::Solve`] (`Optimize` only): `design` itself does not solve.
///
/// `RetargetMode::Shift` never fails: pure angle arithmetic, never calls `Design::solve`.
#[must_use = "a proposal must be shown to the user before it is applied"]
pub fn build_proposal(
    design: &Design,
    target: &ResolvedMaterial,
    crown: CrownShift,
    mode: RetargetMode,
    custom_materials: &[GemMaterial],
) -> Result<RetargetProposal, RetargetError> {
    build_proposal_impl(
        design,
        design.effective_refractive_index_with(custom_materials),
        target,
        crown,
        mode,
    )
}

/// The shared body of [`build_proposal`] --
/// everything past resolving `n_from`, which is now the caller's job (see
/// [`build_proposal`]'s own doc comment for why the catalogue is threaded in
/// callers instead of one taking a `MaterialLookup`).
fn build_proposal_impl(
    design: &Design,
    n_from: f64,
    target: &ResolvedMaterial,
    crown: CrownShift,
    mode: RetargetMode,
) -> Result<RetargetProposal, RetargetError> {
    match mode {
        RetargetMode::Shift => Ok(plan::build_plan_from(design, n_from, target, crown).proposal()),
        RetargetMode::Optimize(config) => {
            // Every pavilion/crown tier that moves, in schedule order. Girdle tiers and
            // flat tiers are never part of this set -- not filtered out later, never
            // considered in the first place.
            let (scope, blocks) = retarget_scope(design);
            let final_angles =
                build_optimized_angles(design, n_from, target, &scope, crown, &config)?;
            let entries: Vec<(usize, Block, f64)> = scope
                .iter()
                .zip(&final_angles)
                .map(|(&index, &angle)| (index, blocks[index], angle))
                .collect();
            Ok(RetargetProposal {
                rows: plan::legacy_rows(design, &entries, target.n_d),
                target: target.clone(),
                notes: build_notes(
                    mode,
                    n_from,
                    target.n_d,
                    crown,
                    plan::pavilion_stretch(design, n_from, target.n_d),
                ),
            })
        }
    }
}

/// [`build_proposal`]'s `RetargetMode::Optimize` half, split out to keep
/// `build_proposal` itself short. Seeds a clone of `design` with the shift angles
/// for every tier in `scope`, runs `optimize_design` unchanged against
/// `target.gem`, then reads back the final angle for each `scope` tier (the seeded
/// angle, overridden by `AngleChange::to_deg` for whichever tiers actually moved).
///
/// `n_from` is the caller's already-resolved source index (custom-material-aware).
/// Recomputing it here via the built-ins-only `Design::effective_refractive_index()`
/// would silently seed from the wrong index for a design on a custom catalogue
/// material.
///
/// # Errors
///
/// See [`build_proposal`]'s `# Errors` section.
fn build_optimized_angles(
    design: &Design,
    n_from: f64,
    target: &ResolvedMaterial,
    scope: &[usize],
    crown: CrownShift,
    config: &OptimizeConfig,
) -> Result<Vec<f64>, RetargetError> {
    let anchored = anchored_tiers_in(design, scope);
    if !anchored.is_empty() {
        return Err(RetargetError::AnchoredTiers(anchored));
    }

    let n_to = target.n_d;
    let blocks = classify_blocks(&design.meet_tier_inputs());
    let seeded = seed_shift_design(design, scope, &blocks, n_from, n_to, crown);

    let outcome = optimize_design(&seeded, &target.gem, config, &SearchHooks::default())
        .map_err(RetargetError::Solve)?;

    let mut final_angles: Vec<f64> = scope.iter().map(|&i| seeded.tiers[i].angle_deg).collect();
    for change in &outcome.changes {
        if let Some(slot) = scope.iter().position(|&i| i == change.index) {
            final_angles[slot] = change.to_deg;
        }
    }
    Ok(final_angles)
}

/// Turns an accepted [`RetargetProposal`] into the one `Edit::RetargetAngles` the
/// caller pushes through `History::apply`.
///
/// This moves angles only; a caller that also wants every facet to keep turning about its
/// girdle-side edge uses [`apply_with_anchors`] with the masts [`check`] computed.
///
/// See the module doc comment for why `design` (the CURRENT design, not
/// necessarily the one `build_proposal` was called against) is read here rather
/// than trusting each row's `old_angle`.
#[must_use]
pub fn apply(design: &Design, proposal: &RetargetProposal) -> Edit {
    let changes = proposal
        .rows
        .iter()
        .filter(|row| row.tier_index < design.tiers.len())
        .map(|row| {
            let current = design.tiers[row.tier_index].angle_deg;
            (row.tier_index, current, row.new_angle)
        })
        .collect();
    Edit::RetargetAngles { changes }
}
