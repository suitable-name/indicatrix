//! Girdle-facet classification for the per-facet frosted/bruted-girdle finish.
//!
//! `optics::raytracer::FacetFinish::Frosted` and `renderer::buffers::encode_facet_finishes`
//! already render a per-facet finish; this module supplies *which* facets are the
//! girdle band for a design without a hand-authored answer (see
//! [`super::cuts::STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS`], which only covers
//! [`super::cuts::StandardGemCuts::standard_round_brilliant`]'s fixed construction order).
//!
//! Works from plane geometry rather than the cutting schedule:
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
const GIRDLE_NORMAL_Y_EPSILON: f32 = 1e-3;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts};
    use glam::Vec3;

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
