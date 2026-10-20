//! Conservative optical claims and a small, deterministic optics/shape frontier.
use super::{CandidateKind, CandidateRole, SearchCandidate};
use indicatrix_cut_core::meet_preservation::{MeetPreservationBaseline, PreservationPolicy};

/// A full-fidelity weighted score must improve by this many percentage-score points.
/// This prevents rounding noise from being presented as an improvement.
pub(super) const IMPROVEMENT_FLOOR: f32 = 0.05;

/// The least and the greatest of `values`.
fn range(values: impl Iterator<Item = f64>) -> (f64, f64) {
    values.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), value| {
        (lo.min(value), hi.max(value))
    })
}

/// The role of every point of a frontier given as `(score, shape_distance)` in score order
/// (best score first; lower is better for both numbers).
///
/// - the first point is [`CandidateRole::BestOptics`];
/// - the point with the least shape distance is [`CandidateRole::Closest`] (unless it is also
///   the first, which keeps the better-optics name);
/// - [`CandidateRole::Balanced`] is the remaining point nearest the ideal corner of the
///   frontier, where both numbers are scaled to `0..=1` over the frontier's own range and
///   added. Ties go to the earlier (better-scoring) point. A frontier of one or two points
///   has none.
pub(super) fn roles(frontier: &[(f32, f64)]) -> Vec<Option<CandidateRole>> {
    let mut roles = vec![None; frontier.len()];
    if frontier.is_empty() {
        return roles;
    }
    let closest = (0..frontier.len())
        .min_by(|&a, &b| frontier[a].1.total_cmp(&frontier[b].1))
        .unwrap_or(0);
    roles[0] = Some(CandidateRole::BestOptics);
    if closest != 0 {
        roles[closest] = Some(CandidateRole::Closest);
    }
    let (score_lo, score_hi) = range(frontier.iter().map(|point| f64::from(point.0)));
    let (distance_lo, distance_hi) = range(frontier.iter().map(|point| point.1));
    let scaled = |value: f64, lo: f64, hi: f64| {
        if hi > lo {
            (value - lo) / (hi - lo)
        } else {
            0.0
        }
    };
    let cost = |index: usize| {
        scaled(f64::from(frontier[index].0), score_lo, score_hi)
            + scaled(frontier[index].1, distance_lo, distance_hi)
    };
    let balanced = (0..frontier.len())
        .filter(|&index| index != 0 && index != closest)
        .min_by(|&a, &b| cost(a).total_cmp(&cost(b)));
    if let Some(index) = balanced {
        roles[index] = Some(CandidateRole::Balanced);
    }
    roles
}

pub(super) fn select(
    candidates: Vec<SearchCandidate>,
    baseline: &MeetPreservationBaseline,
    original_score: f32,
    capacity: usize,
) -> Vec<SearchCandidate> {
    if !original_score.is_finite() {
        return Vec::new();
    }
    let mut measured: Vec<_> = candidates
        .into_iter()
        .filter_map(|mut candidate| {
            if !candidate.numbers.score.is_finite() {
                return None;
            }
            if candidate.angles.is_empty() && candidate.anchors.is_empty() {
                return None;
            }
            if candidate.kind == CandidateKind::Optimized
                && candidate.numbers.score + IMPROVEMENT_FLOOR >= original_score
            {
                return None;
            }
            let facts = baseline
                .validate(&candidate.design, PreservationPolicy::RETARGET)
                .ok()?;
            candidate.preservation = Some(facts);
            candidate.validity.warnings.push(facts.summary());
            Some((candidate, facts.shape_distance))
        })
        .collect();
    measured.sort_by(|a, b| {
        a.0.numbers
            .score
            .total_cmp(&b.0.numbers.score)
            .then_with(|| a.1.total_cmp(&b.1))
    });
    let mut frontier: Vec<(SearchCandidate, f64)> = Vec::new();
    for (candidate, distance) in measured {
        if frontier
            .iter()
            .any(|(other, d)| *d <= distance && other.numbers.score <= candidate.numbers.score)
        {
            continue;
        }
        if frontier.iter().any(|(other, _)| {
            other.angles == candidate.angles && other.anchors == candidate.anchors
        }) {
            continue;
        }
        frontier.push((candidate, distance));
    }
    let points: Vec<(f32, f64)> = frontier
        .iter()
        .map(|(c, d)| (c.numbers.score, *d))
        .collect();
    let roles = roles(&points);
    for ((candidate, _), role) in frontier.iter_mut().zip(&roles) {
        candidate.role = *role;
    }
    let capacity = capacity.max(1);
    if frontier.len() <= capacity {
        return frontier.into_iter().map(|(c, _)| c).collect();
    }
    // Keep the three named roles, best optics first, and drop the rest; with room for only
    // two, the extremes.
    let mut indices: Vec<usize> = [
        CandidateRole::BestOptics,
        CandidateRole::Closest,
        CandidateRole::Balanced,
    ]
    .into_iter()
    .filter_map(|role| roles.iter().position(|&assigned| assigned == Some(role)))
    .collect();
    indices.truncate(capacity);
    indices.sort_unstable();
    frontier
        .into_iter()
        .enumerate()
        .filter(|(i, _)| indices.contains(i))
        .map(|(_, (c, _))| c)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_frontier_points_are_best_balanced_and_closest() {
        // Strictly better score means strictly larger distance: this is what a frontier is.
        let roles = roles(&[(10.0, 0.30), (12.0, 0.10), (20.0, 0.00)]);
        assert_eq!(
            roles,
            vec![
                Some(CandidateRole::BestOptics),
                Some(CandidateRole::Balanced),
                Some(CandidateRole::Closest),
            ]
        );
    }

    #[test]
    fn balanced_is_the_knee_not_the_middle_index() {
        // Five points; the third trades little optics for a lot of distance, the second is the
        // knee: close to the ideal corner on both numbers.
        let roles = roles(&[
            (10.0, 0.50),
            (10.5, 0.05),
            (18.0, 0.04),
            (19.0, 0.02),
            (20.0, 0.00),
        ]);
        assert_eq!(roles[0], Some(CandidateRole::BestOptics));
        assert_eq!(roles[1], Some(CandidateRole::Balanced));
        assert_eq!(roles[4], Some(CandidateRole::Closest));
        assert_eq!(roles[2], None);
        assert_eq!(roles[3], None);
    }

    #[test]
    fn a_short_frontier_has_no_balanced_point() {
        assert_eq!(roles(&[]), Vec::new());
        assert_eq!(roles(&[(10.0, 0.2)]), vec![Some(CandidateRole::BestOptics)]);
        assert_eq!(
            roles(&[(10.0, 0.2), (12.0, 0.1)]),
            vec![
                Some(CandidateRole::BestOptics),
                Some(CandidateRole::Closest)
            ]
        );
    }

    #[test]
    fn the_closest_point_keeps_the_better_optics_name_when_it_is_also_first() {
        // The first point is also the nearest one.
        let roles = roles(&[(10.0, 0.00), (12.0, 0.10), (20.0, 0.30)]);
        assert_eq!(roles[0], Some(CandidateRole::BestOptics));
        assert_eq!(
            roles
                .iter()
                .filter(|r| **r == Some(CandidateRole::Closest))
                .count(),
            0
        );
        assert_eq!(roles[1], Some(CandidateRole::Balanced));
        assert_eq!(roles[2], None);
    }
}
