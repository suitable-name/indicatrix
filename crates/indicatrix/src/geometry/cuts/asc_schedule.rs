//! Reconstructs facet planes directly from the real per-tier mast (depth) values
//! in a parsed `.asc` file's cutting instructions -- the accurate counterpart to
//! [`super::database_angles`]'s fabricated proportions.

use glam::{DVec3, Vec3};
use indicatrix_formats::asc::AscSchedule;

use super::{CutError, StandardGemCuts};
use crate::geometry::{
    brep::GemPolyhedron,
    plane::{GpuFacetPlane, tier_is_crown_side},
};

impl StandardGemCuts {
    /// Generates facet planes directly from a parsed `GemCAD` `.asc` file's cutting instructions.
    ///
    /// Uses [`indicatrix_formats::asc::parse_asc`]'s output and its real per-tier mast
    /// (depth) values as plane offsets, instead of the fabricated proportions
    /// [`Self::from_database_angles`] has to invent when only `angle_settings` (angle
    /// + index, no depth) is available.
    ///
    /// # Conventions (determined empirically against the real corpus; see the
    /// module-level report this function was built for)
    ///
    /// - **Angle sign** decides crown vs. pavilion, via
    ///   [`tier_is_crown_side`](crate::geometry::plane::tier_is_crown_side):
    ///   negative is pavilion, positive is crown, and a zero angle is the table
    ///   unless it is a sign-negative zero (the culet). `parse_asc` already turned
    ///   the file's documented culet encoding (angle `0`, NEGATIVE distance) into
    ///   that sign-negative zero, so file order never matters.
    /// - **Magnitude**: `theta` uses `angle_deg.abs()` -- the sign has already been
    ///   consumed above to pick crown vs. pavilion, so re-applying it to `sin`/`cos`
    ///   would rotate the facet to the wrong azimuthal quadrant.
    /// - **Plane offset**: `d = -mast.abs()`. `GemPolyhedron::from_planes` requires
    ///   every `d < 0` (the origin must lie inside every half-space). After parsing,
    ///   a mast is only negative on a nonzero-angle tier the manual does not
    ///   describe (no real catalogue file has one; the parser warns), and the
    ///   magnitude is still the physical depth.
    /// - **Azimuth**: `phi = 2*pi*(index + gear_reference_angle)/gear_teeth_abs()`
    ///   via [`Self::index_to_azimuth`] -- see that function's doc comment for
    ///   the reference-angle convention (tooth-denominated, additive, derived
    ///   from real corpus data) and why it never flips the existing handedness
    ///   of index growth. A tier with no listed index (rare -- a bare `angle
    ///   mast` with only a name) still produces one plane, with normal
    ///   `(0, +-cos(theta), sin(theta))` (the reference angle is not applied to an
    ///   absent index). That normal points at +Z, i.e. azimuth `phi = 90` degrees,
    ///   not at index 0 (+X, `phi = 0`); for a table or culet (`theta = 0`) the
    ///   horizontal component vanishes, so the azimuth is immaterial there.
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

        for tier in &schedule.tiers {
            let is_crown = tier_is_crown_side(tier.angle_deg);

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
/// outright (two coincident half-spaces would claim the same facet twice). Removing
/// the redundant copy here does not change the
/// resulting solid at all, only avoids handing `from_planes` a construction it cannot
/// use.
///
/// A genuine `O(P^2)` tolerance compare against every already-kept plane, not a
/// quantized-bin lookup: `P` is small (real schedules top out in the low hundreds of
/// planes), so the quadratic cost is negligible, and it avoids the quantized
/// approach's own failure mode -- two planes whose true values are close but land in
/// ADJACENT bins (e.g. offsets half a quantum apart, straddling a bin edge) hash to
/// different keys and are kept as spurious distinct entries, which then makes
/// `GemPolyhedron::from_planes` reject the schedule outright as coincident planes.
///
/// The **first** occurrence of a recognized duplicate survives; a later one is
/// dropped in place, never substituted in. This is not a claim that the result is
/// independent of input order in general -- an interior member of a chain of
/// several mutually-within-tolerance planes can still end up merged into whichever
/// neighbour is visited first, so an adversarial permutation (e.g. visiting the
/// chain's middle element before its ends) can change which planes survive. What
/// this dedup does guarantee is that the same real half-space is picked as *the*
/// representative from either end of a schedule's own plane list -- i.e. reading a
/// tier's plane run forwards or reading the same run backwards keeps the same set
/// of planes, only the arrival order of two truly-distinct planes is reversed --
/// because real `.asc` schedules list a duplicate revision row physically adjacent
/// to the row it duplicates, never interleaved with an unrelated distinct facet.
/// Keeping the first occurrence (rather than the smaller-`|d|` one, as an earlier
/// version of this function did) also matters beyond dedup itself: callers such as
/// `facet_plane_boundaries`/`apply_cheater_offsets`-style per-tier attribution walk
/// `planes` positionally against the tier that produced them, and substituting a
/// later tier's plane into an earlier tier's slot would silently reattribute it.
pub(super) fn dedup_planes(planes: Vec<GpuFacetPlane>) -> Vec<GpuFacetPlane> {
    // Relative to the larger of the two offsets (floored at `1.0`, the common
    // sub-unit mast range every real schedule up to now has used): a schedule
    // authored at a larger `ScaleReference` has proportionally larger masts, and
    // an absolute quantum sized for masts around `1.0` would either fail to
    // merge genuine duplicate revision rows (too tight) or start merging
    // genuinely distinct facets (too loose) once every offset is scaled up.
    // Below the `1.0` floor this is exactly the old ~5e-4 absolute quantum.
    const OFFSET_REL_EPSILON: f32 = 1.0 / 2048.0;

    let mut kept: Vec<GpuFacetPlane> = Vec::with_capacity(planes.len());
    for plane in planes {
        let pn = plane_normal_f64(&plane);
        let is_duplicate = kept.iter().any(|k| {
            let offset_scale = k.d.abs().max(plane.d.abs()).max(1.0);
            normals_coincide(plane_normal_f64(k), pn)
                && (k.d - plane.d).abs() <= OFFSET_REL_EPSILON * offset_scale
        });
        if !is_duplicate {
            kept.push(plane);
        }
    }
    kept
}

/// The plane's stored `f32` normal widened to `f64`.
fn plane_normal_f64(plane: &GpuFacetPlane) -> DVec3 {
    DVec3::new(
        f64::from(plane.normal[0]),
        f64::from(plane.normal[1]),
        f64::from(plane.normal[2]),
    )
}

/// Whether two facet normals point the same way, to within `1 - cos(angle) <= 2e-7`
/// (about 0.036 degrees).
///
/// The one normal-equality rule shared by every plane dedup that has to agree with
/// the `.asc` plane builder: this module's `dedup_planes`, the solid measurer's
/// plane filter and the facet-provenance map. Both inputs are normalised in `f64`
/// first, because a normal that was unit-normalised in `f32` has a squared length
/// of `1 +- ~1.5e-7`, so the raw dot product of two bit-identical copies can fall
/// short of `1` by more than any bound tight enough to tell real neighbours apart.
/// After the `f64` normalisation, rounding noise from independent `sin`/`cos`/
/// `normalize` calls in `f32` (well under `1e-12` in `1 - cos`) is far below the
/// bound, while the closest facets real schedules keep distinct, one degree apart
/// (`1 - cos` of about `1.5e-4`), are far above it.
///
/// A zero or non-finite normal never coincides with anything, itself included.
#[must_use]
pub fn normals_coincide(a: DVec3, b: DVec3) -> bool {
    const NORMAL_COINCIDE_EPSILON: f64 = 2e-7;
    let (a_hat, b_hat) = (a.normalize_or_zero(), b.normalize_or_zero());
    1.0 - a_hat.dot(b_hat) <= NORMAL_COINCIDE_EPSILON
}
