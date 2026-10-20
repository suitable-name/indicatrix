//! Girdle-facet classification for the per-facet frosted/bruted-girdle finish.
//!
//! `optics::raytracer::FacetFinish::Frosted` and `renderer::buffers::encode_facet_finishes`
//! already render a per-facet finish; this module supplies *which* facets are the
//! girdle band for a design without a hand-authored answer (see
//! [`super::cuts::STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`], which only covers
//! [`super::cuts::StandardGemCuts::standard_round_brilliant`]'s fixed construction order).
//!
//! Works from plane geometry rather than the cutting instructions:
//! [`super::meet_solver::classify_blocks`] needs a `&[MeetTierInput]`, which not
//! every caller has, while a girdle facet's normal is by construction
//! perpendicular to the stone's `+Y` axis and directly readable from the
//! [`GpuFacetPlane`]. Same "normal is exactly horizontal" rule as
//! `classify_blocks` (there `y.abs() <= 1e-6` on an exact schedule angle); see
//! [`GIRDLE_NORMAL_Y_EPSILON`] for why this module's threshold differs.

use super::plane::GpuFacetPlane;
use crate::optics::raytracer::FacetFinish;

/// How close a (unit, `f32`) plane normal's `y`-component must be to zero to
/// count as girdle (perpendicular to the stone's `+Y` axis).
///
/// `classify_blocks` uses `y.abs() <= 1e-6` on an exact `f64` schedule angle;
/// here `y` is an `f32` normal that passed through angle -> `sin`/`cos` ->
/// `normalize`, leaving rounding noise even for an intended-exact 90 degree
/// facet (~5e-8 measured). `1e-3` clears that noise floor with margin below
/// the shallowest real non-girdle facet in either built-in cut (SRB
/// `|y| ~= 0.737`; `emerald_cut` `|y| ~= 0.602`).
///
/// Purpose: classify `f32` plane normals (as carried by [`GpuFacetPlane`]), which
/// is why it is 1000x looser than `classify_blocks`' exact-angle test, and why it
/// is unrelated to the 85 degree girdle-tier pick in `meet_solver::names` (a
/// naming heuristic over schedule angles).
///
/// Also backs [`super::stone_metrics::measure_solid`]'s own girdle-thickness
/// threshold -- that function's planes come from the SAME `GpuFacetPlane` f32
/// normals (widened to `f64` via [`super::plane::GpuFacetPlane::to_halfspace_f64`],
/// which does not add precision, only represent the existing rounding noise in a
/// wider type), so a second, independently-tuned threshold there could
/// disagree with this one on a facet in the gap between the two -- `is_girdle_plane`
/// would classify it as girdle (frosted) while `measure_solid` excluded it from the
/// girdle-thickness band. One shared constant makes that impossible by construction.
pub(super) const GIRDLE_NORMAL_Y_EPSILON: f32 = 1e-3;

/// True iff `plane`'s normal is close enough to horizontal (perpendicular to the
/// stone's `+Y` symmetry axis) to count as a girdle facet. See
/// [`GIRDLE_NORMAL_Y_EPSILON`].
fn is_girdle_plane(plane: &GpuFacetPlane) -> bool {
    plane.normal[1].abs() <= GIRDLE_NORMAL_Y_EPSILON
}

/// Returns the plane indices (into `planes`, ascending) that classify as girdle
/// facets.
///
/// Deterministic and order-preserving (plain forward scan, no hashing). Empty
/// or all-non-girdle input returns an empty `Vec` rather than panicking.
///
/// Must reproduce [`super::cuts::STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`]'s
/// `33..49` on `standard_round_brilliant`'s planes -- see this module's tests.
#[must_use]
pub fn classify_girdle_plane_indices(planes: &[GpuFacetPlane]) -> Vec<usize> {
    planes
        .iter()
        .enumerate()
        .filter_map(|(i, p)| is_girdle_plane(p).then_some(i))
        .collect()
}

/// Builds a ready `Vec<FacetFinish>` for a bruted-girdle variant of `planes`.
///
/// Sized to `planes.len()`: girdle facets (per [`classify_girdle_plane_indices`])
/// get [`FacetFinish::Frosted`], everything else [`FacetFinish::Polished`].
/// Matches the shape `trace_spectral_ray_with_finish`'s `facet_finishes` and
/// `renderer::buffers::encode_facet_finishes` already consume.
#[must_use]
pub fn girdle_facet_finishes(planes: &[GpuFacetPlane]) -> Vec<FacetFinish> {
    planes
        .iter()
        .map(|p| {
            if is_girdle_plane(p) {
                FacetFinish::Frosted
            } else {
                FacetFinish::Polished
            }
        })
        .collect()
}

/// The per-facet finishes of a stone whose girdle toggle and authored frosting both apply.
///
/// `frosted` lists the plane indices (into `planes`) a design authors as frosted: whole tiers
/// and single facets alike, already resolved to planes. The girdle band is frosted as well
/// when `girdle_frosted` is on (see [`girdle_facet_finishes`]). An index past the end of
/// `planes` is ignored.
///
/// Returns an EMPTY list when no facet ends up frosted, which is
/// `trace_spectral_ray_with_finish`'s documented equivalent of the all-polished path: a stone
/// without any frosting keeps exactly the data (and so the bytes) it had before frosting
/// could be authored.
#[must_use]
pub fn facet_finishes_with_frosted(
    planes: &[GpuFacetPlane],
    girdle_frosted: bool,
    frosted: &[u32],
) -> Vec<FacetFinish> {
    if !girdle_frosted && frosted.is_empty() {
        return Vec::new();
    }
    let mut finishes = if girdle_frosted {
        girdle_facet_finishes(planes)
    } else {
        vec![FacetFinish::Polished; planes.len()]
    };
    for &index in frosted {
        if let Some(finish) = usize::try_from(index)
            .ok()
            .and_then(|index| finishes.get_mut(index))
        {
            *finish = FacetFinish::Frosted;
        }
    }
    if finishes.contains(&FacetFinish::Frosted) {
        finishes
    } else {
        Vec::new()
    }
}

/// A resolved finish list as the two pieces a wire scene holds: the girdle toggle and the
/// authored frosted planes.
///
/// [`facet_finishes_with_frosted`] on them rebuilds `finishes`, the list a scene snapshot
/// carries.
///
/// An empty list is `(false, [])`. A list that is exactly the girdle band
/// ([`girdle_facet_finishes`]) is `(true, [])`, the form a scene without authored frosting has
/// always had. Anything else is `(false, every frosted plane)`.
#[must_use]
pub fn finishes_wire_form(planes: &[GpuFacetPlane], finishes: &[FacetFinish]) -> (bool, Vec<u32>) {
    if finishes.is_empty() {
        return (false, Vec::new());
    }
    if finishes == girdle_facet_finishes(planes).as_slice() {
        return (true, Vec::new());
    }
    let frosted = finishes
        .iter()
        .enumerate()
        .filter(|(_, finish)| **finish == FacetFinish::Frosted)
        .filter_map(|(index, _)| u32::try_from(index).ok())
        .collect();
    (false, frosted)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts};
    use glam::Vec3;

    /// The wire form of a finish list rebuilds that list: no frosting stays empty, the plain
    /// girdle band keeps its old `(true, [])` form, and authored frosting is listed by plane.
    #[test]
    fn finishes_wire_form_round_trips_through_facet_finishes_with_frosted() {
        let planes = StandardGemCuts::standard_round_brilliant();
        assert_eq!(finishes_wire_form(&planes, &[]), (false, Vec::new()));
        let girdle = girdle_facet_finishes(&planes);
        assert_eq!(finishes_wire_form(&planes, &girdle), (true, Vec::new()));
        for (toggle, authored) in [(false, vec![2_u32, 7]), (true, vec![2, 7])] {
            let finishes = facet_finishes_with_frosted(&planes, toggle, &authored);
            let (girdle_flag, frosted) = finishes_wire_form(&planes, &finishes);
            assert_eq!(
                facet_finishes_with_frosted(&planes, girdle_flag, &frosted),
                finishes,
                "toggle {toggle}"
            );
        }
    }

    /// No toggle and no authored frosting is the plain path (an empty list); authored
    /// frosting marks exactly the named planes, ORs with the girdle band, and ignores an
    /// index past the end.
    #[test]
    fn facet_finishes_with_frosted_marks_exactly_the_named_planes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        assert_eq!(facet_finishes_with_frosted(&planes, false, &[]), Vec::new());
        assert_eq!(
            facet_finishes_with_frosted(&planes, true, &[]),
            girdle_facet_finishes(&planes)
        );
        let only = facet_finishes_with_frosted(&planes, false, &[0, 5, 9_999]);
        assert_eq!(only.len(), planes.len());
        for (i, finish) in only.iter().enumerate() {
            let expected = if i == 0 || i == 5 {
                FacetFinish::Frosted
            } else {
                FacetFinish::Polished
            };
            assert_eq!(*finish, expected, "index {i}");
        }
        let both = facet_finishes_with_frosted(&planes, true, &[0]);
        assert_eq!(both[0], FacetFinish::Frosted);
        for i in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
            assert_eq!(both[i], FacetFinish::Frosted, "girdle index {i}");
        }
        assert_eq!(
            facet_finishes_with_frosted(&planes, false, &[9_999]),
            Vec::new()
        );
    }

    /// Cross-check against the hand-verified [`STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`]
    /// constant (`33..49`); if this disagrees, the classifier is wrong, not the constant.
    #[test]
    fn matches_standard_round_brilliant_girdle_constant() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let got = classify_girdle_plane_indices(&planes);
        let expected: Vec<usize> = STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS.collect();
        assert_eq!(got, expected);
    }

    /// [`girdle_facet_finishes`] must mark exactly those same indices `Frosted`, and
    /// nothing else, on the same design.
    #[test]
    fn srb_finishes_mark_exactly_the_girdle_band() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let finishes = girdle_facet_finishes(&planes);
        assert_eq!(finishes.len(), planes.len());
        for (i, finish) in finishes.iter().enumerate() {
            let expected = if STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS.contains(&i) {
                FacetFinish::Frosted
            } else {
                FacetFinish::Polished
            };
            assert_eq!(*finish, expected, "index {i}");
        }
    }

    /// `emerald_cut`'s girdle band (`13..21`) sits at a different position than
    /// SRB's `33..49`, proving the classifier reads plane geometry, not position.
    #[test]
    fn emerald_cut_girdle_is_a_different_range_than_srb() {
        let planes = StandardGemCuts::emerald_cut();
        let got = classify_girdle_plane_indices(&planes);
        let expected: Vec<usize> = (13..21).collect();
        assert_eq!(got, expected);
        assert_ne!(
            got,
            STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS.collect::<Vec<_>>()
        );
    }

    /// A design with no near-horizontal facet must return an empty result, not panic.
    #[test]
    fn no_girdle_facets_returns_empty() {
        let planes = vec![
            GpuFacetPlane::new(Vec3::new(0.0, 1.0, 0.0), -1.0),
            GpuFacetPlane::new(Vec3::new(0.0, -1.0, 0.0), -1.0),
            GpuFacetPlane::new(Vec3::new(1.0, 1.0, 0.0), -1.0),
            GpuFacetPlane::new(Vec3::new(-1.0, 1.0, 0.0), -1.0),
        ];
        assert_eq!(classify_girdle_plane_indices(&planes), Vec::<usize>::new());
        assert_eq!(
            girdle_facet_finishes(&planes),
            vec![FacetFinish::Polished; planes.len()]
        );
    }

    /// An empty plane slice must not panic, for either entry point.
    #[test]
    fn empty_planes_does_not_panic() {
        assert_eq!(classify_girdle_plane_indices(&[]), Vec::<usize>::new());
        assert_eq!(girdle_facet_finishes(&[]), Vec::<FacetFinish>::new());
    }

    /// Same input, same output, bit-for-bit (plain forward scan, no hashing).
    #[test]
    fn deterministic_across_repeated_calls() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let a = classify_girdle_plane_indices(&planes);
        let b = classify_girdle_plane_indices(&planes);
        assert_eq!(a, b);

        let fa = girdle_facet_finishes(&planes);
        let fb = girdle_facet_finishes(&planes);
        assert_eq!(fa, fb);
    }
}
