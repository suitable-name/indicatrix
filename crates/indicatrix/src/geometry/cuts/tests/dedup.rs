//! `.asc`-reconstruction plane dedup coverage: bin-edge straddling, first-
//! occurrence-wins, and the normal/offset tolerance boundaries.

use glam::Vec3;

use crate::geometry::{cuts::asc_schedule::dedup_planes, plane::GpuFacetPlane};

/// The failure mode a quantized-bin dedup has and a tolerance compare does not:
/// two offsets that are genuinely close (well within the dedup quantum) but sit
/// on opposite sides of a bin's rounding boundary. Built at the boundary between
/// bin `0` and bin `1` (`0.5 * QUANT`, i.e. `0.5/2048`) `+/- 1e-7`, so the old
/// `(v / QUANT).round()` bin lookup would put them in different bins and keep
/// both, even though they are only `2e-7` apart -- four orders of magnitude
/// smaller than the `~5e-4` quantum itself.
#[test]
fn dedup_merges_planes_straddling_a_quantization_bin_edge() {
    const QUANT: f32 = 1.0 / 2048.0;
    let bin_edge = 0.5 * QUANT;
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let planes = vec![
        GpuFacetPlane::new(normal, bin_edge - 1e-7),
        GpuFacetPlane::new(normal, bin_edge + 1e-7),
    ];
    let deduped = dedup_planes(planes);
    assert_eq!(
        deduped.len(),
        1,
        "two near-identical offsets straddling a bin edge must merge into one plane"
    );
}

/// The first occurrence is the one that survives, not the last -- callers that
/// keep the earlier metadata (name, source tier) alongside the plane depend on
/// this.
#[test]
fn dedup_keeps_the_first_occurrence() {
    const QUANT: f32 = 1.0 / 2048.0;
    let bin_edge = 0.5 * QUANT;
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let first = GpuFacetPlane::new(normal, bin_edge - 1e-7);
    let second = GpuFacetPlane::new(normal, bin_edge + 1e-7);
    let deduped = dedup_planes(vec![first, second]);
    assert_eq!(deduped, vec![first]);
}

/// Two planes with genuinely different offsets (well beyond the dedup tolerance)
/// must both survive -- the tolerance compare must not over-merge.
#[test]
fn dedup_keeps_genuinely_distinct_planes() {
    const QUANT: f32 = 1.0 / 2048.0;
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let planes = vec![
        GpuFacetPlane::new(normal, 0.0),
        GpuFacetPlane::new(normal, 10.0 * QUANT),
    ];
    let deduped = dedup_planes(planes.clone());
    assert_eq!(
        deduped, planes,
        "genuinely distinct planes must both be kept"
    );
}

/// Two planes with the same offset but different (non-parallel) normals are
/// different half-spaces, not duplicates, regardless of how close their offsets
/// are.
#[test]
fn dedup_keeps_planes_with_different_normals_even_at_the_same_offset() {
    let planes = vec![
        GpuFacetPlane::new(Vec3::new(0.0, 0.0, 1.0), 1.0),
        GpuFacetPlane::new(Vec3::new(1.0, 0.0, 0.0), 1.0),
    ];
    let deduped = dedup_planes(planes.clone());
    assert_eq!(deduped, planes);
}

/// Two facets at the same offset whose normals are genuinely ~1 degree
/// apart -- not a near-bit-identical duplicate -- must both survive.
///
/// A looser epsilon (`1 - 1/2048`, ~1.79 degrees) would silently collapse these
/// into a single half-space; `NORMAL_DOT_EPSILON` (`1 - 1e-5`, ~0.26 degrees) does
/// not.
#[test]
fn dedup_keeps_planes_one_degree_apart() {
    let normal_a = Vec3::new(0.0, 0.0, 1.0);
    let angle = 1.0f32.to_radians();
    let normal_b = Vec3::new(angle.sin(), 0.0, angle.cos());
    let planes = vec![
        GpuFacetPlane::new(normal_a, -1.0),
        GpuFacetPlane::new(normal_b, -1.0),
    ];
    let deduped = dedup_planes(planes.clone());
    assert_eq!(
        deduped, planes,
        "planes one degree apart must not be merged into a single half-space"
    );
}

/// When two near-duplicate planes are merged, the tighter-fitting one
/// (smaller `|d|`) survives, regardless of which arrived first.
#[test]
fn dedup_keeps_the_plane_with_smaller_absolute_offset() {
    let normal = Vec3::new(0.0, 0.0, 1.0);
    let tighter = GpuFacetPlane::new(normal, -1.0);
    let looser = GpuFacetPlane::new(normal, -1.0 - 1e-7);
    let deduped = dedup_planes(vec![looser, tighter]);
    assert_eq!(
        deduped,
        vec![tighter],
        "the smaller-|d| plane must survive even when it arrives second"
    );
}
