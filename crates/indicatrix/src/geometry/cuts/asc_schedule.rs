//! Reconstructs facet planes directly from a parsed `.asc` cutting schedule's
//! real per-tier mast (depth) values -- the accurate counterpart to
//! [`super::database_angles`]'s fabricated proportions.

use glam::Vec3;
use indicatrix_formats::asc::AscSchedule;

use super::{CutError, StandardGemCuts};
use crate::geometry::{brep::GemPolyhedron, plane::GpuFacetPlane};

impl StandardGemCuts {
    /// Generates facet planes directly from a parsed `GemCAD` `.asc` cutting schedule.
    ///
    /// Uses [`indicatrix_formats::asc::parse_asc`]'s output and its real per-tier mast
    /// (depth) values as plane offsets, instead of the fabricated proportions
    /// [`Self::from_database_angles`] has to invent when only `angle_settings` (angle
    /// + index, no depth) is available.
    ///
    /// # Conventions (determined empirically against the real corpus; see the
    /// module-level report this function was built for)
    ///
    /// - **Angle sign** decides crown vs. pavilion: `.asc` angles are signed, and
    ///   negative means pavilion, matching `GemCAD`'s own convention (confirmed by 74
    ///   real files that carry both an explicit `-0.000000` pavilion culet and a
    ///   `0.000000` crown table in the same schedule -- the sign, not just the
    ///   magnitude, is meaningful). A tier whose angle is *unsigned* zero (the common
    ///   case -- about 98% of zero-angle tiers in the sampled corpus never bother
    ///   signing it, even for a culet) inherits the crown/pavilion side of the most
    ///   recent tier that did carry a nonzero (or explicitly signed) angle, since
    ///   `.asc` files consistently group a schedule's pavilion tiers before its crown
    ///   tiers; defaults to crown if it's the very first tier.
    /// - **Magnitude**: `theta` uses `angle_deg.abs()` -- the sign has already been
    ///   consumed above to pick crown vs. pavilion, so re-applying it to `sin`/`cos`
    ///   would rotate the facet to the wrong azimuthal quadrant.
    /// - **Plane offset**: `d = -mast.abs()`. `GemPolyhedron::from_planes` requires
    ///   every `d < 0` (the origin must lie inside every half-space); the file's mast
    ///   values are positive magnitudes in all but a handful of designs (a rare,
    ///   single-tier `"B"`-named exception with a negative mast at a near-zero angle,
    ///   seen in ~2.6% of sampled files), and even there the intent is a real
    ///   physical depth, not a sign-bearing offset, so the magnitude is what belongs
    ///   in the half-space equation.
    /// - **Azimuth**: `phi = 2*pi*(index + gear_reference_angle)/gear_teeth_abs()`
    ///   via [`Self::index_to_azimuth`] -- see that function's doc comment for
    ///   the reference-angle convention (tooth-denominated, additive, derived
    ///   from real corpus data) and why it never flips the existing handedness
    ///   of index growth. A tier with no listed index (rare -- a bare `angle
    ///   mast` with only a name) still produces one plane at `phi = 0` (the
    ///   reference angle is not applied to an absent index), the same
    ///   convention [`Self::from_database_angles`] uses for an unlisted
    ///   Table/Culet.
    /// - **`mirror` is not applied here.** `schedule.mirror` (the `y` line's
    ///   second field) is a symmetry *descriptor*, not a geometric transform:
    ///   every `a` record already lists every index-wheel position its facet
    ///   actually occurs at, mirror image included whenever the design has
    ///   one (see `indicatrix_formats::asc`'s module docs on tier records, which fold
    ///   more than one `n <name>` group into a single tier's `indices`
    ///   precisely so this holds). This is confirmed by
    ///   `indicatrix_cut_core::orbit`'s own corpus measurement (600 of 2,881
    ///   designs sampled, 10,006 tiers classified against a rotation+mirror
    ///   orbit model): `mirror` is used there only to *predict* a facet's
    ///   expected orbit-mate positions for editing operations
    ///   (`Design::add_orbit_member`/`remove_orbit_member`), never to rewrite
    ///   a tier's recorded `indices` -- 92.0% of sampled designs are already
    ///   fully consistent with their stated indices under that model, and the
    ///   remaining `mixed_fold`/`partial` tiers are designs whose file
    ///   genuinely doesn't carry a complete mirror pair. Applying `mirror` as
    ///   a transform in this function would double-reflect the (common)
    ///   schedules that already list both mirror images explicitly, and would
    ///   fabricate facets for the ones that don't.
    #[must_use]
    pub fn from_asc_schedule(schedule: &AscSchedule) -> Vec<GpuFacetPlane> {
        let gear_teeth = (schedule.gear_teeth_abs().max(1)) as f32;
        let gear_reference_angle = schedule.gear_reference_angle as f32;
        let mut planes = Vec::with_capacity(schedule.facet_plane_count());

        // Most recently resolved crown/pavilion side, used to break the tie for a
        // tier whose angle is unsigned zero. See the doc comment above.
        let mut last_side_is_crown = true;

        for tier in &schedule.tiers {
            let is_crown = if tier.angle_deg == 0.0 {
                if tier.angle_deg.is_sign_negative() {
                    false
                } else {
                    last_side_is_crown
                }
            } else {
                tier.angle_deg > 0.0
            };
            last_side_is_crown = is_crown;

            let theta = (tier.angle_deg.abs() as f32).to_radians();
            let sin_theta = theta.sin();
            let cos_theta = theta.cos();
            let d = -(tier.mast.abs() as f32);

            if tier.indices.is_empty() {
                let n = if is_crown {
                    Vec3::new(0.0, cos_theta, sin_theta)
                } else {
                    Vec3::new(0.0, -cos_theta, sin_theta)
                };
                planes.push(GpuFacetPlane::new(n, d));
                continue;
            }

            for &idx in &tier.indices {
                let phi = Self::index_to_azimuth(idx as f32, gear_teeth, gear_reference_angle);
                let (sin_phi, cos_phi) = (phi.sin(), phi.cos());
                let n = if is_crown {
                    Vec3::new(sin_theta * cos_phi, cos_theta, sin_theta * sin_phi)
                } else {
                    Vec3::new(sin_theta * cos_phi, -cos_theta, sin_theta * sin_phi)
                };
                planes.push(GpuFacetPlane::new(n, d));
            }
        }

        dedup_planes(planes)
    }

    /// Builds a validated B-Rep solid directly from a parsed `.asc` schedule.
    ///
    /// Uses [`Self::from_asc_schedule`] plus [`GemPolyhedron::from_planes`] and a
    /// check of [`GemPolyhedron::untouched_planes`] as the correctness oracle --
    /// exactly the same two-part gate [`Self::reconstruct_validated_brep`] uses for
    /// `angle_settings`-derived reconstructions.
    ///
    /// Unlike that path, there's no good "fabricated shape" fallback available here:
    /// the whole point of this function is that its offsets are the file's own real
    /// mast values, not invented proportions, so a validation failure means either the
    /// parse, the source file itself (e.g. a near-duplicate tier left in from a design
    /// revision), or the crown/pavilion sign convention above is off *for this
    /// particular file* -- silently substituting `standard_round_brilliant()` would
    /// hide that. Callers that need a guaranteed-renderable result on failure should
    /// fall back to [`Self::reconstruct_validated_brep`] (via `angle_settings`)
    /// themselves.
    ///
    /// # Errors
    ///
    /// Returns [`GemPolyhedron::from_planes`]'s error verbatim if the planes don't
    /// reconstruct into a valid, closed, finite solid at all, or a descriptive error
    /// if they do but leave one or more planes untouched (over-constrained --
    /// most often a near-duplicate tier revision in the source file, occasionally a
    /// zero-angle tier that resolved to the wrong side).
    pub fn reconstruct_validated_brep_from_asc(
        schedule: &AscSchedule,
    ) -> Result<GemPolyhedron, CutError> {
        let planes = Self::from_asc_schedule(schedule);
        let plane_count = planes.len();
        let hull = GemPolyhedron::from_planes(planes)?;
        let untouched = hull.untouched_planes();
        if untouched.is_empty() {
            Ok(hull)
        } else {
            Err(CutError::OverConstrained {
                untouched_count: untouched.len(),
                plane_count,
                untouched,
            })
        }
    }
}

/// Drops planes that are near-duplicates of an earlier one in the list (same normal
/// and offset within a coarse tolerance), keeping the first occurrence.
///
/// Real `.asc` files occasionally list the same index position twice across two
/// separate tier rows that happen to share an identical angle and mast (a data-entry
/// artifact in the original hand-authored schedules, e.g. index `0` appearing in both
/// a `-90 ... 0 32` row and a later `-90 ... 0` row at the same mast). Two planes with
/// the same normal and offset are the same half-space -- geometrically redundant, not
/// a real second facet -- and `GemPolyhedron::from_planes` correctly rejects them
/// outright (two coincident half-spaces have coincident dual points, which the dual
/// convex hull cannot use). Removing the redundant copy here does not change the
/// resulting solid at all, only avoids handing `from_planes` a construction it cannot
/// use.
///
/// A genuine `O(P^2)` tolerance compare against every already-kept plane, not a
/// quantized-bin lookup: `P` is small (real schedules top out in the low hundreds of
/// planes), so the quadratic cost is negligible, and it avoids the quantized
/// approach's own failure mode -- two planes whose true values are close but land in
/// ADJACENT bins (e.g. offsets half a quantum apart, straddling a bin edge) hash to
/// different keys and are kept as spurious distinct entries, which then makes
/// `GemPolyhedron::from_planes` reject the schedule outright on coincident dual
/// points.
///
/// When two kept-and-new planes ARE recognized as the same half-space, the
/// one with the smaller `|d|` (the tighter-fitting plane) survives, not simply whichever
/// was seen first -- iterating `planes` in its own original order still makes the
/// result deterministic regardless of input order, since the comparison itself (which
/// of the two has the smaller `|d|`) does not depend on which arrived first.
pub(super) fn dedup_planes(planes: Vec<GpuFacetPlane>) -> Vec<GpuFacetPlane> {
    // A genuine tolerance rather than a bin size (~5e-4, coarser than
    // brep.rs's own coincidence epsilon): two planes within one quantum of each
    // other in offset are always recognized as the same half-space, including
    // across a would-be bin edge.
    const QUANT: f32 = 1.0 / 2048.0;
    const OFFSET_EPSILON: f32 = QUANT;
    // Tightened to the f32 rounding scale: a true duplicate plane's
    // normal survives sin/cos/normalize with rounding noise on the order of 1e-7,
    // not a fraction of a degree. `1.0 - 1e-5` (~0.26 degrees) still catches genuine
    // duplicates with margin while never merging two facets a real schedule intends to
    // be distinct -- a looser epsilon like `1.0 - QUANT` (~1.79 degrees) would silently
    // collapse two tiers at the same index/mast whose angles genuinely differ by as
    // little as ~1 degree into a single facet, dropping a real half-space.
    const NORMAL_DOT_EPSILON: f32 = 1.0 - 1e-5;

    let mut kept: Vec<GpuFacetPlane> = Vec::with_capacity(planes.len());
    for plane in planes {
        let duplicate_of = kept.iter().position(|k| {
            let dot = k.normal[0].mul_add(
                plane.normal[0],
                k.normal[1].mul_add(plane.normal[1], k.normal[2] * plane.normal[2]),
            );
            dot >= NORMAL_DOT_EPSILON && (k.d - plane.d).abs() <= OFFSET_EPSILON
        });
        match duplicate_of {
            Some(idx) if plane.d.abs() < kept[idx].d.abs() => kept[idx] = plane,
            Some(_) => {}
            None => kept.push(plane),
        }
    }
    kept
}
