//! Girdle finish (bruted/frosted facets): `FacetFinish` is looked up per-hit via
//! `HitRecord::facet_idx` into a `&[FacetFinish]` slice PARALLEL to `&[GpuFacetPlane]` on
//! the CPU -- extending `GpuFacetPlane` itself would disturb that struct's layout, echo
//! test, and the `intersect_polyhedron` kernel, none of which need to know about finish.
//! The GPU encoding mirrors that with a SEPARATE storage buffer (one u32 per facet).
//! Still gets its own struct-echo test (`layout_check::run_facet_finish`).

use crate::optics::raytracer::FacetFinish;

pub mod facet_finish {
    pub const POLISHED: u32 = 0;
    pub const FROSTED: u32 = 1;
}

/// Encodes a `&[FacetFinish]` slice into a `facet_finish::{POLISHED,FROSTED}`-valued
/// `Vec<u32>` of exactly `num_planes` entries.
///
/// Mirrors `trace_spectral_ray_with_finish`'s per-facet lookup semantics
/// (`facet_finishes.get(i).copied().unwrap_or_default()`): an index past the end of a
/// shorter slice, or an empty slice, defaults to `FacetFinish::Polished`, matching the
/// CPU's "no explicit finish means polished" default.
#[must_use]
pub fn encode_facet_finishes(finishes: &[FacetFinish], num_planes: usize) -> Vec<u32> {
    (0..num_planes)
        .map(|i| match finishes.get(i).copied().unwrap_or_default() {
            FacetFinish::Polished => facet_finish::POLISHED,
            FacetFinish::Frosted => facet_finish::FROSTED,
        })
        .collect()
}
