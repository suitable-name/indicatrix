//! [`check_manufacturability`], the orchestrator, plus checks 1 and 2 (the
//! two that need a real solved mesh): [`check_vanishing_facets`] and
//! [`check_undersized_facets`], and the plane-boundary/area machinery they
//! share. See the parent module's doc comment for why these run off an
//! already-solved design rather than forcing a second solve.

use super::warning::ManufacturabilityWarning;
use crate::design::Design;
use indicatrix::geometry::{
    meet_solver::SolvedTier,
    stone_metrics::{SolidStatus, build_solid_mesh, measure_solid},
};
use std::collections::BTreeMap;

/// Default minimum facet area for [`check_undersized_facets`].
///
/// Expressed as a fraction of the stone's own measured width squared rather than
/// an absolute number -- masts (and areas) are in an arbitrary per-design "mast
/// unit" scale, so an absolute threshold would mean a different real fraction of
/// the stone on every design.
///
/// **A reasoned default, not a corpus-measured one**: `1e-4` is `(1%)^2`, i.e. it
/// flags a facet whose *linear* extent is below roughly 1% of the stone's own
/// width -- smaller than that and a standard faceting lap has no real margin to
/// polish the facet flat without rounding its edges into its neighbors. Exposed
/// as a parameter so a caller who disagrees can override it.
pub const DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2: f64 = 1e-4;

/// Runs all four checks over `design`'s current authored state.
///
/// Uses `solved` (an already-[`Design::solve`]'d, or [`Design::resolve_dirty`]'d,
/// mast list -- see this module's doc comment for why a second solve is not
/// forced here) for the two checks that need real geometry.
///
/// `min_facet_area_fraction_of_w2` is [`check_undersized_facets`]'s threshold,
/// expressed as a fraction of the solid's measured width squared -- pass
/// [`DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2`] for the reasoned default.
///
/// Checks 3/4 always run (they need no mast at all). Checks 1/2 run only when
/// `solved` actually closes into a real solid ([`SolidStatus::Closed`]) -- an
/// unbounded or degenerate arrangement has no well-defined facet set to check.
///
/// # Panics
///
/// Same alignment contract as [`Design::planes_from_solved`]: `solved` must have
/// one entry per tier `design` currently has, in the same order.
#[must_use]
pub fn check_manufacturability(
    design: &Design,
    solved: &[SolvedTier],
    min_facet_area_fraction_of_w2: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = super::authored_checks::check_gear_quantization(design);
    warnings.extend(super::authored_checks::check_cut_order(design));

    let planes = design.planes_from_solved(solved);
    if let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) {
        let schedule = design.to_asc_schedule_from_solved(solved);
        let boundaries = facet_plane_boundaries(&schedule);
        let preform_len = design.preform.planes().len();
        let ring_by_index: BTreeMap<usize, &Vec<glam::DVec3>> =
            mesh.rings.iter().map(|(i, ring)| (*i, ring)).collect();

        warnings.extend(check_vanishing_facets(
            design,
            preform_len,
            &boundaries,
            &ring_by_index,
        ));

        if let Some(metrics) = measure_solid(&planes) {
            let threshold = min_facet_area_fraction_of_w2 * metrics.width_axis * metrics.width_axis;
            warnings.extend(check_undersized_facets(
                design,
                preform_len,
                &boundaries,
                &ring_by_index,
                threshold,
            ));
        }
    }

    warnings
}

/// Cumulative facet-plane count contributed by `schedule.tiers[0..=i]`, for every
/// `i` -- i.e. `boundaries[i]` is how many facet planes
/// `StandardGemCuts::from_asc_schedule(schedule)` places at or before tier `i`'s
/// own contribution, in [`Design::planes`]'s combined list (offset by the
/// preform's own plane count, which the caller adds).
///
/// # Why this calls the real production function repeatedly instead of re-deriving plane counts
///
/// `StandardGemCuts::from_asc_schedule` deduplicates near-identical planes (two
/// `.asc` tier rows occasionally produce the exact same half-space), so a tier's
/// contribution is not always exactly `indices.len().max(1)`. Dedup only ever
/// *drops* an element based on what came before it, so calling
/// [`StandardGemCuts::from_asc_schedule`] once per prefix and taking the length is
/// byte-for-byte consistent with a single call over the whole schedule, avoiding a
/// second copy of the dedup rule that could drift out of sync. This costs O(n^2)
/// tier-generation passes rather than O(n), but `n` is capped at
/// `meet_solver::MAX_PLANES = 400` and each pass is well under a millisecond,
/// negligible next to the solve this module's caller already paid for.
///
/// [`StandardGemCuts::from_asc_schedule`]: indicatrix::geometry::cuts::StandardGemCuts::from_asc_schedule
fn facet_plane_boundaries(schedule: &indicatrix_formats::asc::AscSchedule) -> Vec<usize> {
    (0..schedule.tiers.len())
        .map(|i| {
            let prefix = indicatrix_formats::asc::AscSchedule {
                gemcad_version: String::new(),
                gear_teeth: schedule.gear_teeth,
                gear_reference_angle: 0.0,
                symmetry_order: 0,
                mirror: false,
                refractive_index: 0.0,
                headers: Vec::new(),
                footnotes: Vec::new(),
                tiers: schedule.tiers[..=i].to_vec(),
            };
            indicatrix::geometry::cuts::StandardGemCuts::from_asc_schedule(&prefix).len()
        })
        .collect()
}

/// Check 1: a facet plane this tier describes never reaches the solid's surface --
/// see the module docs.
///
/// `preform_len` and `boundaries` (from [`facet_plane_boundaries`]) locate each
/// tier's own slice of [`Design::planes`]'s combined list; `ring_by_index` (keyed
/// by that same combined-list index) is present for exactly the planes
/// `SolidMesh::rings` kept -- so a plane index in a tier's range absent from
/// `ring_by_index` is exactly a vanished facet.
///
/// A tier whose own facet-plane count falls short of its authored
/// `indices.len().max(1)` had one of its planes deduplicated against an earlier
/// tier's identical plane (see [`facet_plane_boundaries`]) -- a data redundancy,
/// not a vanished facet, and not attributable to one specific index without
/// re-deriving which one collided. `vanished`/`total` are always exact counts, but
/// the exact index value is not necessarily reliable in that rare case.
fn check_vanishing_facets(
    design: &Design,
    preform_len: usize,
    boundaries: &[usize],
    ring_by_index: &BTreeMap<usize, &Vec<glam::DVec3>>,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let mut prev = 0usize;
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let end = boundaries[tier_index];
        let total = end - prev;
        let vanished = (preform_len + prev..preform_len + end)
            .filter(|i| !ring_by_index.contains_key(i))
            .count();
        if vanished > 0 {
            warnings.push(ManufacturabilityWarning::VanishingFacet {
                tier_index,
                tier_name: tier.name.clone(),
                vanished,
                total,
            });
        }
        prev = end;
    }
    warnings
}

/// Check 2: a facet survives but its polygon area is below `threshold` --
/// see the module docs and [`DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2`].
fn check_undersized_facets(
    design: &Design,
    preform_len: usize,
    boundaries: &[usize],
    ring_by_index: &BTreeMap<usize, &Vec<glam::DVec3>>,
    threshold: f64,
) -> Vec<ManufacturabilityWarning> {
    let mut warnings = Vec::new();
    let mut prev = 0usize;
    for (tier_index, tier) in design.tiers.iter().enumerate() {
        let end = boundaries[tier_index];
        for plane_index in preform_len + prev..preform_len + end {
            if let Some(ring) = ring_by_index.get(&plane_index) {
                let area = polygon_area(ring);
                if area < threshold {
                    warnings.push(ManufacturabilityWarning::UndersizedFacet {
                        tier_index,
                        tier_name: tier.name.clone(),
                        facet_plane_index: plane_index,
                        area,
                        threshold,
                    });
                }
            }
        }
        prev = end;
    }
    warnings
}

/// Area of a planar polygon given as an ordered ring of 3D vertices: the standard
/// vector-area shoelace formula, generalized off the ring's own centroid so it
/// needs no assumption about which 2D plane the polygon lies in (`0.5 * |sum of
/// (v_i - c) x (v_{i+1} - c)|`, exact for a planar, non-self-intersecting polygon
/// regardless of its normal's orientation).
fn polygon_area(ring: &[glam::DVec3]) -> f64 {
    if ring.len() < 3 {
        return 0.0;
    }
    let centroid = ring.iter().copied().sum::<glam::DVec3>() / ring.len() as f64;
    let mut cross_sum = glam::DVec3::ZERO;
    for i in 0..ring.len() {
        let a = ring[i] - centroid;
        let b = ring[(i + 1) % ring.len()] - centroid;
        cross_sum += a.cross(b);
    }
    0.5 * cross_sum.length()
}
