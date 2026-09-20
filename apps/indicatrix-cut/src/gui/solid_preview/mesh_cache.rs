//! Caches one [`SolidMesh`] build against the plane arrangement it was built from.
//!
//! It also keeps a one-time [`raster::simplify_ring`] pass over every facet, so a burst of
//! redraws carrying the SAME planes (e.g. an orbit drag) never re-solves the mesh or
//! re-simplifies its rings more than once. Re-simplifying every frame was expensive
//! enough to matter -- see [`super::raster`]'s doc comment.
//!
//! Deliberately independent of every editor type: [`MeshCache::get_or_build`] takes a
//! plain `&[(Vec3, f32)]` plane slice -- normal plus signed offset, `n . x <= m` -- the
//! same half-space convention [`build_solid_mesh`] itself takes, just narrowed to
//! `f32`. `preview_state::SolidPreviewState` is the only caller and is the boundary
//! that converts from editor types.
//!
//! # Cache key
//!
//! Exactly one cached entry (a redraw always wants the current design, never a
//! historical one). The key is a plain FNV-1a hash of every plane's `f32` bit
//! pattern, in slice order: [`Vec3`]/`f32` do not implement `Hash`, so [`hash_planes`]
//! hashes `to_bits()` of each component directly. Deterministic, which only matters
//! for reproducible tests, not correctness.

use super::raster;
use glam::{DVec3, Vec3};
use indicatrix::geometry::stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh};

/// FNV-1a over every plane's `f32` bit pattern, in slice order -- see this module's
/// doc comment for why a bitwise hash rather than `std::hash::Hash`.
fn hash_planes(planes: &[(Vec3, f32)]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = FNV_OFFSET;
    for &(normal, offset) in planes {
        for bits in [
            normal.x.to_bits(),
            normal.y.to_bits(),
            normal.z.to_bits(),
            offset.to_bits(),
        ] {
            for byte in bits.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(FNV_PRIME);
            }
        }
    }
    hash
}

/// One facet's ring after [`raster::simplify_ring`] has collapsed it to its true
/// corners -- built once per cache miss, consumed every frame by `render_prepared`.
type PreparedRing = (usize, Vec<DVec3>);

/// A [`SolidMesh`] plus every facet's pre-simplified ring, ready for
/// [`SolidRasterizer::render_prepared`]. Only [`MeshCache::get_or_build`] builds one.
#[derive(Debug)]
pub struct CachedMesh {
    pub mesh: SolidMesh,
    /// `(facet_id, simplified ring)`, skipping any facet whose ring has fewer than 3
    /// points after simplification, in [`SolidMesh::rings`]'s own order.
    pub(super) rings: Vec<PreparedRing>,
}

impl CachedMesh {
    fn build(mesh: SolidMesh) -> Self {
        let mut dedup_scratch = Vec::new();
        let mut rings = Vec::with_capacity(mesh.rings.len());
        for (facet_id, ring) in &mesh.rings {
            let mut simplified = Vec::new();
            raster::simplify_ring(ring, &mut dedup_scratch, &mut simplified);
            if simplified.len() >= 3 {
                rings.push((*facet_id, simplified));
            }
        }
        Self { mesh, rings }
    }

    /// The largest distance from the origin to any of this mesh's own vertices,
    /// in the same model units [`SolidMesh::positions`] uses (#118, `cad_todo.md`
    /// item 118).
    ///
    /// The solver's own coordinate origin is already the centre every facet
    /// plane's offset is expressed against, so this needs no separate centroid
    /// pass -- used to size the orbit camera's distance clamp
    /// (`render::camera_lighting::orbit_distance_bounds`) to the design
    /// ACTUALLY loaded instead of a fixed `[1.2, 8.0]` range that clips a large
    /// preform's facets at minimum distance and leaves a tiny one lost in empty
    /// space at maximum. `0.0` for an empty mesh (no vertices at all).
    #[must_use]
    pub fn bounding_radius(&self) -> f64 {
        self.mesh
            .positions
            .iter()
            .map(|position| position.length())
            .fold(0.0_f64, f64::max)
    }
}

/// A finished cache slot: either the last successfully built [`CachedMesh`], or the
/// [`SolidStatus`] explaining why the last attempt failed, so a caller can show a
/// real reason instead of an empty viewport.
#[derive(Debug)]
enum CacheEntry {
    Closed(CachedMesh),
    Other(SolidStatus),
}

/// Single-slot mesh cache keyed by [`hash_planes`] -- see this module's doc comment.
///
/// `last_closed` remembers the most recent [`SolidStatus::Closed`] build
/// independently of `entry`, so a later arrangement that fails to close (a
/// mid-keystroke [`SolidStatus::Unbounded`]/[`SolidStatus::Degenerate`] edit) can
/// still hand back a real solid -- CAD audit item 63: an intermediate unbounded
/// edit used to blank the viewport entirely, discarding the last mesh this cache
/// had already built, because a failing rebuild simply overwrote `entry` with
/// nothing to show.
#[derive(Debug, Default)]
pub struct MeshCache {
    key: Option<u64>,
    entry: Option<CacheEntry>,
    last_closed: Option<CachedMesh>,
}

impl MeshCache {
    /// Rebuilds only when `planes` hashes differently from the last call; otherwise
    /// returns the cached mesh. Returns `None` when the arrangement is not
    /// [`SolidStatus::Closed`], remembering the status for [`Self::status_message`]/
    /// [`Self::status`], and stashing the outgoing `Closed` entry (if any) into
    /// [`Self::last_closed`] first so it survives the failed rebuild.
    pub fn get_or_build(&mut self, planes: &[(Vec3, f32)]) -> Option<&CachedMesh> {
        let key = hash_planes(planes);
        if self.key != Some(key) {
            self.key = Some(key);
            let widened: Vec<(DVec3, f64)> = planes
                .iter()
                .map(|&(n, m)| {
                    (
                        DVec3::new(f64::from(n.x), f64::from(n.y), f64::from(n.z)),
                        f64::from(m),
                    )
                })
                .collect();
            let new_entry = match build_solid_mesh(&widened) {
                SolidStatus::Closed(mesh) => CacheEntry::Closed(CachedMesh::build(mesh)),
                other => CacheEntry::Other(other),
            };
            if let Some(CacheEntry::Closed(outgoing)) = self.entry.take() {
                self.last_closed = Some(outgoing);
            }
            self.entry = Some(new_entry);
        }
        match self.entry.as_ref() {
            Some(CacheEntry::Closed(cached)) => Some(cached),
            _ => None,
        }
    }

    /// The most recent [`Self::get_or_build`] call that actually closed, kept even
    /// while the CURRENT `planes` do not -- a caller whose own `get_or_build` just
    /// returned `None` can render this instead of blanking the viewport. `None`
    /// only before the very first successful build.
    #[must_use]
    pub const fn last_closed(&self) -> Option<&CachedMesh> {
        self.last_closed.as_ref()
    }

    /// The raw [`SolidStatus`] behind the last [`Self::get_or_build`] failure, or
    /// `None` when the last build was `Closed` (or nothing built yet). Lets a caller
    /// with enough context on hand (a `Design`/solved masts) build a better message
    /// than [`Self::status_message`]'s generic one -- e.g. naming the tier that owns
    /// an `Unbounded` arrangement's escaping plane index via
    /// `indicatrix_cut_core::Design::tier_for_plane_index` -- without this module
    /// taking on that dependency itself (see the module doc comment).
    #[must_use]
    pub const fn status(&self) -> Option<&SolidStatus> {
        match self.entry.as_ref() {
            Some(CacheEntry::Other(status)) => Some(status),
            _ => None,
        }
    }

    /// A short human-readable reason the last [`Self::get_or_build`] call returned
    /// `None`, empty when the last build was `Closed` (or nothing built yet). Wording
    /// mirrors `gui::editor::state::status_text_and_is_problem`'s, reimplemented here
    /// so `solid_preview` stays independent of `gui::editor` types. A generic
    /// fallback for a caller with no `Design` context to name the escaping planes'
    /// owning tiers with (see [`Self::status`] for that better-informed path).
    #[must_use]
    pub fn status_message(&self) -> String {
        let Some(CacheEntry::Other(status)) = self.entry.as_ref() else {
            return String::new();
        };
        match status {
            SolidStatus::Unbounded { escaping } => {
                format!("Unbounded: plane(s) {escaping:?} never close the solid.")
            }
            SolidStatus::Degenerate {
                vertex_count,
                volume,
            } => {
                let volume_text =
                    volume.map_or_else(|| "non-finite".to_string(), |v| format!("{v:.4}"));
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume {volume_text}."
                )
            }
            // Unreachable in practice; kept total rather than `unreachable!()`.
            SolidStatus::Closed(_) => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::optics::raytracer::Camera;
    use raster::{SolidRasterizer, SolidStyle};

    fn box_planes(y_half: f32) -> Vec<(Vec3, f32)> {
        vec![
            (Vec3::X, 1.0),
            (Vec3::NEG_X, 1.0),
            (Vec3::Y, y_half),
            (Vec3::NEG_Y, y_half),
            (Vec3::Z, 1.0),
            (Vec3::NEG_Z, 1.0),
        ]
    }

    /// Two parallel, opposite-facing planes never bound a solid (nothing closes off
    /// `x`/`z`); `build_solid_mesh` reports it `Unbounded`.
    fn unbounded_planes() -> Vec<(Vec3, f32)> {
        vec![(Vec3::X, 1.0), (Vec3::NEG_X, 1.0)]
    }

    #[test]
    fn identical_planes_reuse_the_cached_mesh() {
        let mut cache = MeshCache::default();
        let planes = box_planes(0.6);
        let first_vertex_count = cache.get_or_build(&planes).unwrap().mesh.positions.len();
        // A second call with bit-identical planes must be a cache hit.
        let second_vertex_count = cache.get_or_build(&planes).unwrap().mesh.positions.len();
        assert_eq!(first_vertex_count, second_vertex_count);
        assert!(first_vertex_count > 0);
    }

    #[test]
    fn changed_planes_rebuild_a_different_mesh() {
        let mut cache = MeshCache::default();
        let narrow = cache.get_or_build(&box_planes(0.6)).unwrap().mesh.clone();
        let wide = cache.get_or_build(&box_planes(0.9)).unwrap().mesh.clone();
        let narrow_y_extent: f64 = narrow
            .positions
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        let wide_y_extent: f64 = wide
            .positions
            .iter()
            .map(|p| p.y)
            .fold(f64::NEG_INFINITY, f64::max);
        assert!(
            wide_y_extent > narrow_y_extent,
            "changing the plane offset must change the cached mesh"
        );
    }

    #[test]
    fn non_closed_status_yields_none_and_is_remembered() {
        let mut cache = MeshCache::default();
        assert!(cache.get_or_build(&unbounded_planes()).is_none());
        assert!(
            cache.status_message().contains("Unbounded"),
            "got: {}",
            cache.status_message()
        );

        assert!(cache.get_or_build(&box_planes(0.6)).is_some());
        assert_eq!(cache.status_message(), "");
    }

    /// CAD audit item 63: a build that stops closing must not lose the last real
    /// solid this cache already had -- `last_closed` should keep handing it back
    /// across any number of consecutive failing rebuilds, until a NEW closed build
    /// replaces it.
    #[test]
    fn last_closed_survives_a_failing_rebuild() {
        let mut cache = MeshCache::default();
        assert!(cache.last_closed().is_none());

        let closed_vertex_count = cache
            .get_or_build(&box_planes(0.6))
            .unwrap()
            .mesh
            .positions
            .len();
        assert!(cache.get_or_build(&unbounded_planes()).is_none());
        assert!(cache.status().is_some(), "the failure must be remembered");
        let last_closed = cache
            .last_closed()
            .expect("the earlier closed build must still be available");
        assert_eq!(last_closed.mesh.positions.len(), closed_vertex_count);

        // A second, DIFFERENT failing arrangement must not clear `last_closed` --
        // only a fresh Closed build should ever replace it.
        let other_unbounded_planes = vec![(Vec3::Y, 1.0), (Vec3::NEG_Y, 1.0)];
        assert!(cache.get_or_build(&other_unbounded_planes).is_none());
        assert_eq!(
            cache
                .last_closed()
                .expect("still available after a second failure")
                .mesh
                .positions
                .len(),
            closed_vertex_count
        );
    }

    /// #118: a unit cube's own farthest vertex is at `(1,1,1)`, radius `sqrt(3)`
    /// -- confirms `bounding_radius` measures from the origin against every
    /// vertex, not just one axis's own half-extent.
    #[test]
    fn bounding_radius_is_the_farthest_vertex_from_the_origin() {
        let mut cache = MeshCache::default();
        let cached = cache.get_or_build(&box_planes(1.0)).unwrap();
        assert!((cached.bounding_radius() - 3.0_f64.sqrt()).abs() < 1e-9);
    }

    fn rbc_planes_f64() -> Vec<(DVec3, f64)> {
        use indicatrix::geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane};
        StandardGemCuts::standard_round_brilliant()
            .into_iter()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect()
    }

    #[test]
    fn render_prepared_is_pixel_identical_to_render_on_rbc_445() {
        let planes = rbc_planes_f64();
        let mesh = match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("RBC-445 must close: {other:?}"),
        };
        let prepared = CachedMesh::build(mesh.clone());
        let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
        let style = SolidStyle::default();

        let mut rasterizer_render = SolidRasterizer::new(200, 150);
        rasterizer_render.render(&mesh, &camera, &style);

        let mut rasterizer_prepared = SolidRasterizer::new(200, 150);
        rasterizer_prepared.render_prepared(&prepared, &camera, &style);

        assert_eq!(
            rasterizer_render.color, rasterizer_prepared.color,
            "render_prepared must produce the exact same pixels as render"
        );
        assert_eq!(rasterizer_render.pick, rasterizer_prepared.pick);
    }

    /// `indicatrix-cut-core`'s `optimize_cost_probe_crackotto_step.asc` fixture; mirrors
    /// `raster.rs`'s `crackotto_step_planes` helper (one `ScaleReference` bootstrap per
    /// block, since every tier in this design is meet-derived).
    fn crackotto_step_planes() -> Vec<(DVec3, f64)> {
        use indicatrix::geometry::meet_solver;
        const TEXT: &str = include_str!(
            "../../../../../crates/indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc"
        );
        let schedule = indicatrix_formats::asc::parse_asc(TEXT).expect("fixture must parse");
        let mut inputs = meet_solver::meet_tier_inputs_from_asc(&schedule);
        let blocks = meet_solver::classify_blocks(&inputs);
        for block in [
            meet_solver::Block::Crown,
            meet_solver::Block::Pavilion,
            meet_solver::Block::Girdle,
        ] {
            let anchored = inputs.iter().zip(&blocks).any(|(t, &b)| {
                b == block && matches!(t.constraint, meet_solver::MeetConstraint::ScaleReference(_))
            });
            if anchored {
                continue;
            }
            if let Some(i) = (0..inputs.len()).find(|&i| blocks[i] == block) {
                inputs[i].constraint =
                    meet_solver::MeetConstraint::ScaleReference(schedule.tiers[i].mast);
            }
        }
        let normals = meet_solver::tier_instance_normals(schedule.gear_teeth_abs(), &inputs);
        let solved = meet_solver::solve_meet_points(schedule.gear_teeth_abs(), &inputs);
        normals
            .iter()
            .zip(solved.iter().map(|s| s.mast))
            .flat_map(|(ns, m)| ns.iter().map(move |&n| (n, m)))
            .collect()
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check -- run with \
                --release --ignored --nocapture"]
    fn timing_cache_build_and_render_prepared_103_tier() {
        let planes = crackotto_step_planes();
        let mesh = match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("CrackOtto-Step must close: {other:?}"),
        };

        let build_start = std::time::Instant::now();
        let prepared = CachedMesh::build(mesh.clone());
        let build_elapsed = build_start.elapsed();
        println!(
            "CrackOtto-Step (103 tiers) cache build (simplify {} facets): {build_elapsed:?} \
             (target < 10 ms, else move to the worker thread -- which it already runs on)",
            prepared.rings.len()
        );

        let camera = Camera::new(0.6, 0.35, 3.0, 42.0);
        let style = SolidStyle::default();

        let mut plain = SolidRasterizer::new(800, 600);
        plain.render(&mesh, &camera, &style); // warm-up
        let iters = 100u32;
        let start = std::time::Instant::now();
        for _ in 0..iters {
            plain.render(&mesh, &camera, &style);
        }
        let render_per_frame = start.elapsed() / iters;

        let mut fast = SolidRasterizer::new(800, 600);
        fast.render_prepared(&prepared, &camera, &style); // warm-up
        let start = std::time::Instant::now();
        for _ in 0..iters {
            fast.render_prepared(&prepared, &camera, &style);
        }
        let prepared_per_frame = start.elapsed() / iters;

        println!(
            "CrackOtto-Step (103 tiers) 800x600: render {render_per_frame:?}/frame, \
             render_prepared {prepared_per_frame:?}/frame (must not be slower)"
        );
    }
}
