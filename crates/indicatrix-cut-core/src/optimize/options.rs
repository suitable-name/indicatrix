//! The optimizer's extended request and result types: [`OptimizeOptions`] (what
//! [`super::optimize_design_with`] may vary beyond [`super::OptimizeConfig`]'s search
//! knobs, and the guards it keeps) and [`OptimizeResult`] (the plain
//! [`super::OptimizeOutcome`] plus the mast changes and the ranked alternatives).
//!
//! # Why a second struct and not more fields on `OptimizeConfig`
//!
//! [`super::OptimizeConfig`] is `Copy` and is stored inside `RetargetMode`
//! (`indicatrix-editor`), which is `Copy` too; and `OptimizeOutcome` is built by full
//! struct literal in the web crates. Putting a `BTreeMap` into the first, or two more
//! fields into the second, would break those crates for no gain. So every new request
//! lives here, every new answer lives in [`OptimizeResult`], and the old types, the
//! old entry point ([`super::optimize_design`]) and the old apply function
//! ([`super::apply_optimize_outcome`]) behave exactly as before.

use super::{
    objective::{ObjectiveComponents, ToneGoal},
    search::{AngleChange, OptimizeOutcome},
};
use crate::design::Design;
use glam::DVec3;
use indicatrix::{
    color::metrics::FaceUpTone, geometry::meet_solver::MeetConstraint,
    optics::raytracer::LightingPreset,
};
use std::collections::BTreeMap;

/// What [`super::optimize_design_with`] may vary beyond the search knobs of
/// [`super::OptimizeConfig`], and the guards it keeps while it does.
///
/// [`Default`] asks for nothing extra: with it, `optimize_design_with` searches the
/// same space [`super::optimize_design`] does and returns the same outcome.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeOptions {
    /// Let `ScaleReference` tiers move too.
    ///
    /// Every imported design pins every tier to a `ScaleReference` mast, which the
    /// search otherwise never varies. When this is on, a `ScaleReference` tier that has
    /// an entry in [`Self::anchor_hinges`] joins the free set, and whenever its angle
    /// changes its mast is recomputed so the plane keeps passing through its hinge.
    /// Tiers with a tier target are never varied this way: the solver derives their
    /// mast from the target, not from the stored one.
    pub vary_anchored: bool,
    /// Per tier index, the point (in design units) the tier's first facet turns about
    /// when its angle changes. Only read when [`Self::vary_anchored`] is on. A tier
    /// without an entry is never varied as an anchored tier.
    pub anchor_hinges: BTreeMap<usize, DVec3>,
    /// Per tier index, the signed angle range in degrees (inclusive) the tier may be
    /// searched in. A candidate outside its range is rejected, and a coordinate step
    /// that overshoots is clamped to the range edge as long as it still moves the
    /// tier. A reversed pair is read as `(low, high)`; a non-finite pair is ignored.
    pub angle_bounds: BTreeMap<usize, (f64, f64)>,
    /// Keeps the girdle: when `Some(fraction)`, a candidate whose girdle band vanishes
    /// or is thinner than `fraction` times the starting design's band is rejected,
    /// and so is a candidate whose band is thinner than `fraction` times the starting
    /// band at its THINNEST point (or runs to a knife edge there, or loses a girdle
    /// wall), and a candidate that loses a table facet the starting design had.
    /// `None` (the default) keeps none of these guards. `Some(0.5)` means "at least
    /// half the girdle thickness".
    pub min_girdle_fraction: Option<f64>,
    /// The fraction the band's THINNEST point must keep, when it is not the same as
    /// [`Self::min_girdle_fraction`]. `None` (the default) applies `min_girdle_fraction` to
    /// both figures, the overall band and its thinnest point. Only read when
    /// `min_girdle_fraction` is `Some`.
    ///
    /// A caller whose two floors differ (the retarget's gate holds each to half of its own
    /// figure on the live design) sets both, so neither figure is held to the stricter floor.
    pub min_girdle_thinnest_fraction: Option<f64>,
    /// How many ranked alternatives to return in [`OptimizeResult::candidates`].
    /// `0` and `1` (the default) both mean the best one only. Each alternative beyond
    /// the first costs one more full-fidelity scoring (about 1.3 s on a small design).
    pub keep_candidates: usize,
    /// Two alternatives count as different only when some free angle differs by at
    /// least this many degrees. `None` uses [`super::OptimizeConfig::min_step_deg`].
    pub candidate_separation_deg: Option<f64>,
    /// Keeps the stone's look: when `Some`, every candidate's score (the fast one in the
    /// search and the final full one) adds [`ShapeTarget::penalty`] of the candidate's
    /// table size and crown-to-pavilion ratio, and so does the starting design's own
    /// score, so scores stay comparable. `None` (the default) adds nothing and every
    /// result is bit-for-bit what it was before this field existed. The reported
    /// [`ObjectiveComponents`] stay the raw optical figures; the penalty is in the
    /// score only.
    pub shape_target: Option<ShapeTarget>,
}

/// The table size and crown-to-pavilion ratio a search should stay near, and how hard.
///
/// The penalty added to a score is `weight * 100 * d`, where
/// `d = sqrt(((t' - t) / t)^2 + ((r' - r) / r)^2)` over the figures that are `Some` on
/// both sides (`t` the table percent, `r` the crown height over the pavilion depth; the
/// primed figures are the candidate's). A figure whose target is `None` or not positive
/// contributes nothing.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ShapeTarget {
    /// The table facet's width as a percentage of the stone's width, to stay near.
    pub table_percent: Option<f64>,
    /// The crown height over the pavilion depth, to stay near.
    pub crown_to_pavilion: Option<f64>,
    /// The score points one unit of relative drift costs per hundred; `0.0` or less
    /// switches the penalty off.
    pub weight: f32,
}

impl ShapeTarget {
    /// The relative drift `d` of a stone with these figures from the target: `0.0` on the
    /// target, `0.1` when one figure is 10 % off and the other is on it.
    #[must_use]
    pub fn distance(&self, table_percent: Option<f64>, crown_to_pavilion: Option<f64>) -> f64 {
        let term = |target: Option<f64>, now: Option<f64>| -> f64 {
            match (target, now) {
                (Some(t), Some(n)) if t.is_finite() && t > 0.0 && n.is_finite() => {
                    let drift = (n - t) / t;
                    drift * drift
                }
                _ => 0.0,
            }
        };
        (term(self.table_percent, table_percent) + term(self.crown_to_pavilion, crown_to_pavilion))
            .sqrt()
    }

    /// The score penalty for a stone with these figures: `weight * 100 * d`, `0.0` when
    /// the weight is not positive.
    #[must_use]
    pub fn penalty(&self, table_percent: Option<f64>, crown_to_pavilion: Option<f64>) -> f32 {
        if self.weight.is_nan() || self.weight <= 0.0 {
            return 0.0;
        }
        self.weight * 100.0 * self.distance(table_percent, crown_to_pavilion) as f32
    }
}

impl Default for OptimizeOptions {
    fn default() -> Self {
        Self {
            vary_anchored: false,
            anchor_hinges: BTreeMap::new(),
            angle_bounds: BTreeMap::new(),
            min_girdle_fraction: None,
            min_girdle_thinnest_fraction: None,
            keep_candidates: 1,
            candidate_separation_deg: None,
            shape_target: None,
        }
    }
}

impl OptimizeOptions {
    /// The hinge `design`'s tier `index` turns about, if this request lets that tier
    /// vary as an anchored tier: [`Self::vary_anchored`] is on, the tier is a
    /// `ScaleReference` tier with a finite hinge, and the tier has no tier target.
    pub(super) fn anchored_hinge(&self, design: &Design, index: usize) -> Option<DVec3> {
        if !self.vary_anchored {
            return None;
        }
        let tier = design.tiers.get(index)?;
        if !matches!(tier.constraint, MeetConstraint::ScaleReference(_))
            || design.tier_target(index).is_some()
        {
            return None;
        }
        self.anchor_hinges
            .get(&index)
            .copied()
            .filter(|hinge| hinge.is_finite())
    }

    /// The number of ranked results to keep, never below one.
    pub(super) fn candidate_capacity(&self) -> usize {
        self.keep_candidates.max(1)
    }
}

/// One accepted mast change: tier `index`'s `ScaleReference` mast moves.
///
/// The mast goes from `from_mast` to `to_mast`. It is always paired with an
/// [`AngleChange`] of the same tier, because the search only changes a mast to keep a
/// turning plane on its hinge.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MastChange {
    /// Tier position this refers to.
    pub index: usize,
    /// The tier's mast before the change, in design units.
    pub from_mast: f64,
    /// The tier's mast after the change, in design units.
    pub to_mast: f64,
}

/// One ranked alternative of an [`OptimizeResult`]: a complete set of changes and
/// what the design measures like after them, scored at full fidelity.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeCandidate {
    /// The angle changes, one per modified tier, in tier order.
    pub changes: Vec<AngleChange>,
    /// The mast changes that go with them (empty unless anchored tiers were varied).
    pub mast_changes: Vec<MastChange>,
    /// Windowing, extinction and tilt brilliance of the design after the changes.
    pub after: ObjectiveComponents,
    /// The combined score after the changes (lower is better).
    pub score: f32,
    /// `100.0 -` the yield percentage after the changes.
    pub yield_loss_pct: f32,
    /// The face-up tone after the changes (table-up pose, under the run's lighting
    /// preset), measured at full fidelity whether or not the tone was weighted. `None`
    /// only for a hand-built candidate.
    pub tone: Option<FaceUpTone>,
}

/// What [`super::optimize_design_with`] found: the familiar [`OptimizeOutcome`] for
/// the best result, the mast changes that go with it, and the ranked alternatives.
#[derive(Debug, Clone, PartialEq)]
pub struct OptimizeResult {
    /// The best result, in the same shape [`super::optimize_design`] returns.
    /// `outcome.changes` and `outcome.after` describe the first of [`Self::candidates`].
    pub outcome: OptimizeOutcome,
    /// The mast changes that go with `outcome.changes`. Empty unless
    /// [`OptimizeOptions::vary_anchored`] was on and an anchored tier moved.
    pub mast_changes: Vec<MastChange>,
    /// The best alternatives, best first, at most [`OptimizeOptions::keep_candidates`]
    /// of them. Every entry changes something and scores no worse than the starting
    /// design; empty when the search found nothing better.
    pub candidates: Vec<OptimizeCandidate>,
    /// The face-up tone of the starting design (table-up pose, under
    /// [`Self::lighting`]), always measured. `None` only when the search stopped before
    /// measuring (a result built by hand).
    pub tone_before: Option<FaceUpTone>,
    /// The tone goal the run optimized for: `Some` iff the weights carried a tone term
    /// (`tone_weight > 0.0`).
    pub tone_goal: Option<ToneGoal>,
    /// The lighting preset the run scored and toned under
    /// ([`super::OptimizeConfig::lighting`]), so a front end can label the swatches.
    pub lighting: LightingPreset,
    /// How many starts the search ran: `1` for a single descent (the default
    /// [`super::OptimizeConfig::starts`]), more for a multi-start run (possibly fewer
    /// than asked, see [`super::effective_starts`]).
    pub starts_run: usize,
    /// Which start the best result came from; `0` is the design's own descent (always
    /// `0` for a single-start run).
    pub best_start: usize,
}

impl OptimizeResult {
    /// A result that proposes no change; the tone fields are filled by the caller.
    pub(super) const fn unchanged(
        outcome: OptimizeOutcome,
        lighting: LightingPreset,
        tone_before: Option<FaceUpTone>,
        tone_goal: Option<ToneGoal>,
    ) -> Self {
        Self {
            outcome,
            mast_changes: Vec::new(),
            candidates: Vec::new(),
            tone_before,
            tone_goal,
            lighting,
            starts_run: 1,
            best_start: 0,
        }
    }

    /// The best alternative, if the search found one.
    #[must_use]
    pub fn best_candidate(&self) -> Option<&OptimizeCandidate> {
        self.candidates.first()
    }
}
