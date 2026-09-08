//! Forward validation and schedule export: [`vertex_meet_groups`] is the
//! reverse of the solve direction (given real masts, report which planes
//! actually meet where), and [`build_reconstructed_schedule`] writes a
//! solver's output back into an exportable `.asc` schedule. Grouped together
//! because both consume a solve's *result* rather than participating in
//! producing one.

use super::{SolveStrategy, SolvedTier};
use crate::geometry::brep::GemPolyhedron;
use indicatrix_formats::asc::AscSchedule;

/// The geometric "meet structure" of an already-solved [`GemPolyhedron`].
///
/// For each vertex, the set of input plane indices that meet there -- derived
/// directly from the solid, independent of any textual meet instruction. The
/// forward-validation counterpart to
/// [`solve_meet_points`](super::solve_meet_points): given a schedule's real (or
/// hand-entered) masts, answers "which facets actually touch where", useful for
/// checking a hand-entered schedule (a facet expected to meet two neighbors
/// that doesn't share a vertex with them here is a real data-entry error).
#[must_use]
pub fn vertex_meet_groups(hull: &GemPolyhedron) -> Vec<Vec<usize>> {
    let mut groups: Vec<Vec<usize>> = vec![Vec::new(); hull.vertices.len()];
    for (plane_idx, poly) in hull.facet_polygons.iter().enumerate() {
        for &v in poly {
            groups[v as usize].push(plane_idx);
        }
    }
    groups
}

/// Wires solved mast distances back into an exportable `.asc` schedule.
///
/// `original` supplies every field the solver doesn't touch (gear, symmetry, mirror,
/// refractive index, headers, footnotes, tier names/indices/notes). `solved` must
/// have exactly one entry per `original.tiers` entry, in the same order. The result
/// is marked with [`indicatrix_formats::asc::mark_reconstructed`] so it can never be mistaken
/// for an authored-and-measured schedule.
///
/// # Panics
///
/// Panics if `solved.len() != original.tiers.len()` -- that indicates a caller bug,
/// not a data problem worth recovering from silently.
#[must_use]
pub fn build_reconstructed_schedule(original: &AscSchedule, solved: &[SolvedTier]) -> AscSchedule {
    assert_eq!(
        original.tiers.len(),
        solved.len(),
        "build_reconstructed_schedule: {} tiers in the schedule but {} solved masts",
        original.tiers.len(),
        solved.len()
    );

    let mut out = original.clone();
    for (tier, sol) in out.tiers.iter_mut().zip(solved) {
        tier.mast = sol.mast;
    }

    let any_fallback = solved.iter().any(|s| {
        !matches!(
            s.strategy,
            SolveStrategy::ScaleReference
                | SolveStrategy::DependencyOrder
                | SolveStrategy::JointGroup
        )
    });
    let note = if any_fallback {
        "solved via MeetPointSolver (vertex-incidence fixed point); at least one tier had no \
         usable meet vertex and kept an estimate -- verify every facet before cutting."
    } else {
        "solved via MeetPointSolver (vertex-incidence fixed point); every tier solved from a \
         meet vertex, but this is still a derived schedule -- verify before cutting."
    };
    indicatrix_formats::asc::mark_reconstructed(&mut out, note);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_meet_groups_reports_at_least_three_planes_per_vertex() {
        let planes = crate::geometry::cuts::StandardGemCuts::standard_round_brilliant();
        let hull =
            GemPolyhedron::from_planes(planes).expect("standard round brilliant must reconstruct");
        let groups = vertex_meet_groups(&hull);
        assert_eq!(groups.len(), hull.vertices.len());
        for g in &groups {
            assert!(
                g.len() >= 3,
                "every vertex of a valid polyhedron must be formed by at least 3 planes, got {}",
                g.len()
            );
        }
    }

    #[test]
    fn build_reconstructed_schedule_marks_output_and_writes_solved_masts() {
        use indicatrix_formats::asc::{parse_asc, to_asc_string};

        let original = parse_asc(
            "GemCad 5.0\ng 4 0.0\ny 1 n\nI 1.72\nH Test\n\
             a 90.000000 1.0 0 1 2 3\n\
             a 0.000000 0.0 n T\n",
        )
        .expect("must parse");

        let solved = vec![
            SolvedTier {
                mast: 1.0,
                strategy: SolveStrategy::ScaleReference,
                detail: "given".into(),
            },
            SolvedTier {
                mast: 0.6,
                strategy: SolveStrategy::DependencyOrder,
                detail: "vertex incidence".into(),
            },
        ];

        let reconstructed = build_reconstructed_schedule(&original, &solved);
        assert!((reconstructed.tiers[0].mast - 1.0).abs() < 1e-9);
        assert!((reconstructed.tiers[1].mast - 0.6).abs() < 1e-9);
        assert!(reconstructed.headers[0].starts_with("RECONSTRUCTED"));

        // Must still round-trip through the .asc writer/reader.
        let text = to_asc_string(&reconstructed);
        let reparsed = parse_asc(&text).expect("reconstructed schedule must itself parse");
        assert_eq!(reparsed, reconstructed);
    }

    #[test]
    #[should_panic(expected = "tiers in the schedule but")]
    fn build_reconstructed_schedule_panics_on_length_mismatch() {
        let original = indicatrix_formats::asc::parse_asc(
            "GemCad 5.0\ng 4 0.0\ny 1 n\nI 1.72\nH Test\na 90.000000 1.0 0 1 2 3\n",
        )
        .expect("must parse");
        let _ = build_reconstructed_schedule(&original, &[]);
    }
}
