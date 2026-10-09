//! The pose choice for a zoned rough: which of a stone's box-symmetric poses shows the colour the
//! cutter wants face-up.
//!
//! A box-model [`PlacedStone`]'s pose is only defined up to the half turns of its bounding box
//! (`PlacedStone::pose`). For a uniform rough that does not matter; for a zoned one it decides
//! which zone ends up under the table. This module scores the (up to four) poses with a cheap
//! face-up predictor and picks one by the cutter's goal. The choice is stored in the vault's
//! `stone_pose_choice` table; `PlacedStone` and the saved plan are unchanged.
//!
//! # The predictor
//!
//! Light that makes a stone sparkle enters the table, goes down to the pavilion, and comes back
//! out along about the same way, so the colour face-up is dominated by the path along the table
//! normal. The predictor takes nine such lines (one through the stone's centre and eight in a
//! ring at half the half-extent), each starting at the table and running down to where a
//! brilliant's pavilion would turn it round, measures how long each is inside each zone with
//! the exact kernel (`indicatrix::optics::zoning::zone_lengths`), and averages the per-line
//! fractions weighted by the line lengths. The stone's real face-up optical path is
//! `MODEL_UNIT_FACE_UP_PATH * mm_per_unit` (the render's own rule, also used for the zone
//! swatches), split between the zones by those fractions.
//!
//! The lines start at the table and stop short of the culet away from the axis, so the
//! predictor is NOT symmetric under turning the stone upside down: which end of a layered or
//! slanted zoning lies at the table matters, as it does in the finished stone. Pavilion depth
//! at a given distance from the axis follows a straight cone (`CROWN_FRACTION` of the height
//! above the girdle, the rest below); the real pavilion of the design is not consulted, which
//! is the "cheap" in the cheap predictor.

use super::frame::{POSE_COUNT, candidate_poses};
use crate::rough_plan::{PlacedStone, RoughLayout, fit::StonePose};
use glam::DVec3;
use indicatrix::{
    optics::zoning::{MAX_ZONES, ZonedAbsorption, zone_lengths},
    render_setup::MODEL_UNIT_FACE_UP_PATH,
};

/// How many lines the predictor traces besides the centre one: a ring of eight.
const RING_LINES: usize = 8;
/// The ring's radius as a fraction of the stone's extent along the ring's axis (half of the
/// half-extent, so a ring line sits halfway between the axis and the girdle).
const RING_RADIUS_FRACTION: f64 = 0.25;
/// The share of the stone's height above the girdle (crown and table) in the predictor's
/// brilliant-proportions model; the rest is pavilion.
///
/// A standard round brilliant has a crown of
/// about a quarter of its total height.
pub const CROWN_FRACTION: f64 = 0.25;

/// How far down the stone a line at `rho` (its distance from the axis as a fraction of the
/// half-extent, `0` on the axis, `1` at the girdle) runs before the pavilion turns it round, as
/// a fraction of the stone's height: the whole crown plus the pavilion down to its cone.
fn line_depth_fraction(rho: f64) -> f64 {
    (1.0 - CROWN_FRACTION).mul_add(1.0 - rho.clamp(0.0, 1.0), CROWN_FRACTION)
}
/// A pose replaces the canonical one only when its score is better by more than this
/// (fractions are in `[0, 1]`), so ties and numerical noise keep the planner's own pose.
const SCORE_MARGIN: f64 = 1e-9;

/// The predicted face-up make-up of one pose.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FaceUp {
    /// The share of the face-up path inside each zone (index 0 is the base zone, then the
    /// zones in order); the entries sum to 1 (to 0 when the lines miss every zone, which a
    /// stone inside the rough's zoning never does).
    pub fractions: [f64; MAX_ZONES + 1],
    /// The face-up optical path in mm: `MODEL_UNIT_FACE_UP_PATH * mm_per_unit`.
    pub path_mm: f64,
}

/// What the cutter wants from the pose.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PoseGoal {
    /// Keep the planner's own pose (the default).
    #[default]
    KeepCanonical,
    /// "Best colour" for a target zone: the pose with the most of zone `n` face-up (zone `0`
    /// is the base zone; for a watermelon the core is the last zone).
    MostOfZone(usize),
    /// The pose with the least of zone `n` face-up (to hide a pale rind, for example).
    LeastOfZone(usize),
}

impl PoseGoal {
    /// The score of a prediction; larger is better. `None` for [`Self::KeepCanonical`].
    fn score(self, face_up: &FaceUp) -> Option<f64> {
        let share = |zone: usize| face_up.fractions.get(zone).copied().unwrap_or(0.0);
        match self {
            Self::KeepCanonical => None,
            Self::MostOfZone(zone) => Some(share(zone)),
            Self::LeastOfZone(zone) => Some(-share(zone)),
        }
    }
}

/// The extent of the stone along caliper axis `i` of `pose`, in mm, from the stone's bounding
/// box in the rough axes (exact for the axis-aligned box poses; a bound for a free pose).
fn extent_along(axis: [f64; 3], size_mm: [f64; 3]) -> f64 {
    axis.iter().zip(size_mm).map(|(a, s)| a.abs() * s).sum()
}

/// The predicted face-up make-up of `stone` placed with `pose` inside the zoned rough.
///
/// `pose` is one of the stone's poses (see `pose_variant`); the table normal is its `axes[1]`
/// and the stone is as high as its bounding box is along `stone.table_axis`.
#[must_use]
pub fn face_up_prediction(
    rough_zoned: &ZonedAbsorption,
    stone: &PlacedStone,
    pose: &StonePose,
) -> FaceUp {
    let centre = DVec3::from_array(pose.center_mm);
    let normal = DVec3::from_array(pose.axes[1]).normalize_or_zero();
    let across_u = DVec3::from_array(pose.axes[0]).normalize_or_zero();
    let across_v = DVec3::from_array(pose.axes[2]).normalize_or_zero();
    let height = stone.stone_size_mm[stone.table_axis.index()];
    let radius_u = RING_RADIUS_FRACTION * extent_along(pose.axes[0], stone.stone_size_mm);
    let radius_v = RING_RADIUS_FRACTION * extent_along(pose.axes[2], stone.stone_size_mm);

    let ring_rho = 2.0 * RING_RADIUS_FRACTION;
    let mut sum = [0.0_f64; MAX_ZONES + 1];
    let mut weight = 0.0_f64;
    for k in 0..=RING_LINES {
        let (origin, rho) = if k == 0 {
            (centre, 0.0)
        } else {
            let angle = std::f64::consts::TAU * (k - 1) as f64 / RING_LINES as f64;
            (
                centre + across_u * (radius_u * angle.cos()) + across_v * (radius_v * angle.sin()),
                ring_rho,
            )
        };
        let top = origin + normal * (0.5 * height);
        let bottom = top - normal * (height * line_depth_fraction(rho));
        let lengths = zone_lengths(rough_zoned, top, bottom);
        let total: f64 = lengths.iter().sum();
        if total > 0.0 {
            // Weighting each line's fractions by its length makes the result the share of the
            // whole face-up path, not of the average line.
            for (acc, length) in sum.iter_mut().zip(lengths) {
                *acc += length;
            }
            weight += total;
        }
    }
    let fractions = if weight > 0.0 {
        sum.map(|s| s / weight)
    } else {
        [0.0; MAX_ZONES + 1]
    };
    FaceUp {
        fractions,
        path_mm: f64::from(MODEL_UNIT_FACE_UP_PATH) * pose.mm_per_unit,
    }
}

/// The pose index (`0..POSE_COUNT`) `goal` picks for `stone`.
///
/// An exact single-stone fit (`exact_fit`) has a unique pose and always gives `0`; so does
/// [`PoseGoal::KeepCanonical`]. Otherwise the best-scoring pose wins, but only if it beats the
/// planner's own pose by more than a tiny margin; ties go to the lowest index.
#[must_use]
pub fn choose_pose(
    rough_zoned: &ZonedAbsorption,
    stone: &PlacedStone,
    exact_fit: bool,
    goal: PoseGoal,
) -> u8 {
    if goal == PoseGoal::KeepCanonical {
        return 0;
    }
    let mut best_index = 0_u8;
    let mut best_score = f64::NEG_INFINITY;
    let mut canonical_score = f64::NEG_INFINITY;
    for (index, pose) in candidate_poses(&stone.pose, exact_fit) {
        let face_up = face_up_prediction(rough_zoned, stone, &pose);
        let Some(score) = goal.score(&face_up) else {
            continue;
        };
        if index == 0 {
            canonical_score = score;
            best_score = score;
        } else if score > best_score + SCORE_MARGIN && score > canonical_score + SCORE_MARGIN {
            best_index = index;
            best_score = score;
        }
    }
    debug_assert!(best_index < POSE_COUNT);
    best_index
}

/// [`choose_pose`] for every stone of `layout`, in stone order.
#[must_use]
pub fn choose_poses(
    rough_zoned: &ZonedAbsorption,
    layout: &RoughLayout,
    goal: PoseGoal,
) -> Vec<u8> {
    layout
        .stones
        .iter()
        .map(|stone| choose_pose(rough_zoned, stone, layout.exact_fit, goal))
        .collect()
}
