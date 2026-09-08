//! Tier/block classification: which of crown, pavilion or girdle each tier
//! belongs to, plus the crown/pavilion side convention ([`tier_sides`]) shared
//! with the normal-vector construction in `candidates`.

use super::MeetTierInput;

/// Which block a tier belongs to.
///
/// Classified like [`solve_meet_points`](super::solve_meet_points)'s arrangement:
/// by the sign/magnitude of the tier's normal `y`-component, a pure function of
/// `angle_deg` and crown/pavilion side (same for every index instance of a tier).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Block {
    Crown,
    Pavilion,
    Girdle,
}

/// Classifies every tier's [`Block`] in one pass, per [`tier_sides`]'s unsigned-zero rule.
#[must_use]
pub fn classify_blocks(tiers: &[MeetTierInput]) -> Vec<Block> {
    let sides = tier_sides(tiers);
    tiers
        .iter()
        .zip(&sides)
        .map(|(t, &crown)| {
            let theta = t.angle_deg.abs().to_radians();
            let y = if crown { theta.cos() } else { -theta.cos() };
            if y.abs() <= 1e-6 {
                Block::Girdle
            } else if y > 0.0 {
                Block::Crown
            } else {
                Block::Pavilion
            }
        })
        .collect()
}

/// Crown/pavilion side per tier, honoring the unsigned-zero inheritance rule.
pub(super) fn tier_sides(tiers: &[MeetTierInput]) -> Vec<bool> {
    let mut sides = Vec::with_capacity(tiers.len());
    let mut last_crown = true;
    for tier in tiers {
        let crown = if tier.angle_deg == 0.0 {
            if tier.angle_deg.is_sign_negative() {
                false
            } else {
                last_crown
            }
        } else {
            tier.angle_deg > 0.0
        };
        last_crown = crown;
        sides.push(crown);
    }
    sides
}

#[cfg(test)]
mod tests {
    use super::{super::MeetConstraint, *};

    #[test]
    fn classify_blocks_matches_angle_sign_and_girdle_threshold() {
        let tiers = vec![
            MeetTierInput {
                angle_deg: 45.0,
                indices: vec![],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -45.0,
                indices: vec![],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
            MeetTierInput {
                angle_deg: 90.0,
                indices: vec![],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
            MeetTierInput {
                angle_deg: -90.0,
                indices: vec![],
                constraint: MeetConstraint::MeetExisting,
                names: vec![],
            },
        ];
        let blocks = classify_blocks(&tiers);
        assert_eq!(
            blocks,
            vec![Block::Crown, Block::Pavilion, Block::Girdle, Block::Girdle]
        );
    }
}
