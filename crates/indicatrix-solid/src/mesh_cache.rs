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
//!
//! A concave stone ([`StoneGeometryBuf`] with tools) keys on
//! [`StoneGeometryBuf::cache_key`], which continues the same FNV stream over the tool
//! bytes and is *equal* to the planar key when there are none, so a tool-free stone
//! behaves exactly as before.

use super::{preview::StoneGeometryBuf, raster};
use glam::{DVec3, Vec3};
use indicatrix::geometry::{
    ToolPrimitive,
    stone_metrics::{SolidMesh, SolidStatus, build_solid_mesh_geom},
};
use std::sync::Arc;

/// The 64-bit FNV-1a hash of `bytes` (offset basis `0xcbf29ce484222325`, prime
/// `0x100000001b3`).
///
/// A fixed, fully-specified algorithm, so a hash is reproducible across runs and
/// machines. It consumes the bytes as a stream, so a caller hashing several fields
/// builds no intermediate buffer; a slice is passed as `bytes.iter().copied()`.
#[must_use]
pub fn fnv1a_64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0100_0000_01b3;
    bytes.into_iter().fold(FNV_OFFSET, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(FNV_PRIME)
    })
}

/// FNV-1a over every plane's `f32` bit pattern, in slice order -- see this module's
/// doc comment for why a bitwise hash rather than `std::hash::Hash`.
fn hash_planes(planes: &[(Vec3, f32)]) -> u64 {
    fnv1a_64(planes.iter().flat_map(|&(normal, offset)| {
        [normal.x, normal.y, normal.z, offset]
            .into_iter()
            .flat_map(|component| component.to_bits().to_le_bytes())
    }))
}

/// One facet's ring after [`raster::simplify_ring`] has collapsed it to its true
/// corners -- built once per cache miss, consumed every frame by `render_prepared`.
type PreparedRing = (usize, Vec<DVec3>);

/// Two simplified-ring corners closer than this (world units) are one corner in
/// [`CachedMesh::corner_points`].
const CORNER_MERGE_EPS: f64 = 1e-6;

/// Every distinct corner of the simplified `rings`, merged within
/// [`CORNER_MERGE_EPS`]. Deterministic: the points are sorted by `(x, y, z)` and
/// each is compared only against the already-accepted corners whose `x` lies within
/// the merge distance (no hashing, so no iteration-order dependence).
fn distinct_corners(rings: &[PreparedRing]) -> Vec<Vec3> {
    let mut points: Vec<DVec3> = rings
        .iter()
        .flat_map(|(_, ring)| ring.iter().copied())
        .collect();
    points.sort_by(|a, b| {
        a.x.total_cmp(&b.x)
            .then(a.y.total_cmp(&b.y))
            .then(a.z.total_cmp(&b.z))
    });
    let mut corners: Vec<DVec3> = Vec::new();
    for point in points {
        let duplicate = corners
            .iter()
            .rev()
            .take_while(|corner| point.x - corner.x <= CORNER_MERGE_EPS)
            .any(|corner| corner.distance_squared(point) <= CORNER_MERGE_EPS * CORNER_MERGE_EPS);
        if !duplicate {
            corners.push(point);
        }
    }
    corners.into_iter().map(DVec3::as_vec3).collect()
}

/// `facet_centroids[facet_id]` for every one of `plane_count` planes: the arithmetic
/// mean of that facet's simplified ring, `None` for a plane with no ring (cut away
/// entirely, or collapsed below three corners).
fn facet_centroids_of(rings: &[PreparedRing], plane_count: usize) -> Vec<Option<Vec3>> {
    let mut centroids = vec![None; plane_count];
    for (facet_id, ring) in rings {
        if let Some(slot) = centroids.get_mut(*facet_id) {
            let sum: DVec3 = ring.iter().copied().sum();
            *slot = Some((sum / ring.len() as f64).as_vec3());
        }
    }
    centroids
}

/// `centroids[facet_id]` for a concave stone: the mean of the simplified ring of that
/// facet's *largest* piece (3-D area, first piece wins a tie).
///
/// A facet cut by a tool has several convex pieces, and the last-ring-wins rule of
/// [`facet_centroids_of`] would hand back an arbitrary small one; the largest piece
/// is where a hit-test or a label is most likely to land on the facet.
fn largest_piece_centroids(rings: &[PreparedRing], facet_count: usize) -> Vec<Option<Vec3>> {
    let mut best: Vec<Option<(f64, Vec3)>> = vec![None; facet_count];
    for (facet_id, ring) in rings {
        let Some(slot) = best.get_mut(*facet_id) else {
            continue;
        };
        let origin = ring[0];
        let doubled_area = ring
            .windows(2)
            .map(|pair| (pair[0] - origin).cross(pair[1] - origin))
            .sum::<DVec3>()
            .length();
        if slot.is_none_or(|(area, _)| doubled_area > area) {
            let sum: DVec3 = ring.iter().copied().sum();
            *slot = Some((doubled_area, (sum / ring.len() as f64).as_vec3()));
        }
    }
    best.into_iter()
        .map(|entry| entry.map(|(_, centroid)| centroid))
        .collect()
}

/// A [`SolidMesh`] plus every facet's pre-simplified ring, ready for
/// [`SolidRasterizer::render_prepared`]. Only [`MeshCache::get_or_build`] builds one.
#[derive(Debug)]
pub struct CachedMesh {
    /// Mesh built from the plane arrangement.
    pub mesh: SolidMesh,
    /// `(facet_id, simplified ring)`, skipping any facet whose ring has fewer than 3
    /// points after simplification, in [`SolidMesh::rings`]'s own order.
    pub(super) rings: Vec<PreparedRing>,
    /// For each entry of `rings`, its index in [`SolidMesh::rings`] (and so in
    /// `piece_normals` / `edge_visible`), since a ring that collapses below three
    /// corners is skipped and the two lists drift apart.
    pub(super) ring_source: Vec<usize>,
    /// Per entry of `rings`, per simplified edge `i -> i + 1`: whether it is drawn
    /// (see [`SolidMesh::edge_visible`]). `None` on the planar path.
    pub(super) edge_visible: Option<Vec<Vec<bool>>>,
    /// Every distinct corner of the simplified rings (merged within 1e-6 world
    /// units), built once with the mesh and shared by `Arc` with every frame drawn
    /// from it.
    pub corner_points: Arc<Vec<Vec3>>,
    /// Facet id -> mean of that facet's simplified ring (`None` for a facet with no
    /// ring), one entry per facet (plane or tool) the mesh was built from; built
    /// once, `Arc`-shared. On a concave stone the ring is the facet's largest piece.
    pub facet_centroids: Arc<Vec<Option<Vec3>>>,
}

impl CachedMesh {
    pub(crate) fn build(mesh: SolidMesh, facet_count: usize) -> Self {
        let mut dedup_scratch = Vec::new();
        let mut rings = Vec::with_capacity(mesh.rings.len());
        let mut ring_source = Vec::with_capacity(mesh.rings.len());
        let mut edge_visible = mesh
            .edge_visible
            .as_ref()
            .map(|_| Vec::with_capacity(mesh.rings.len()));
        for (source, (facet_id, ring)) in mesh.rings.iter().enumerate() {
            let mut simplified = Vec::new();
            raster::simplify_ring(ring, &mut dedup_scratch, &mut simplified);
            if simplified.len() >= 3 {
                if let (Some(flags), Some(visible)) = (&mut edge_visible, &mesh.edge_visible) {
                    flags.push(raster::simplified_edge_flags(
                        ring,
                        &simplified,
                        visible.get(source).map_or(&[], Vec::as_slice),
                    ));
                }
                ring_source.push(source);
                rings.push((*facet_id, simplified));
            }
        }
        let corner_points = Arc::new(distinct_corners(&rings));
        let facet_centroids = Arc::new(if mesh.piece_normals.is_some() {
            largest_piece_centroids(&rings, facet_count)
        } else {
            facet_centroids_of(&rings, facet_count)
        });
        Self {
            mesh,
            rings,
            ring_source,
            edge_visible,
            corner_points,
            facet_centroids,
        }
    }

    /// The largest distance from the origin to any of this mesh's own vertices,
    /// in the same model units [`SolidMesh::positions`] uses.
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
/// still hand back a real solid instead of blanking the viewport and discarding
/// the last mesh this cache already built.
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
        self.get_or_build_keyed(key, planes.len(), &[], || {
            planes
                .iter()
                .map(|&(n, m)| {
                    (
                        DVec3::new(f64::from(n.x), f64::from(n.y), f64::from(n.z)),
                        f64::from(m),
                    )
                })
                .collect()
        })
    }

    /// [`Self::get_or_build`] for a stone that may carry concave tools.
    ///
    /// Keyed on [`StoneGeometryBuf::cache_key`], which equals the planar key when
    /// `geometry.tools` is empty, so a tool-free `geometry` shares its cache slot
    /// with the same planes passed to [`Self::get_or_build`] and builds the
    /// identical mesh. Tool `k` is facet id `planes.len() + k`.
    pub fn get_or_build_geometry(&mut self, geometry: &StoneGeometryBuf) -> Option<&CachedMesh> {
        let key = geometry.cache_key();
        self.get_or_build_keyed(
            key,
            geometry.as_geometry().facet_count(),
            &geometry.tools,
            || {
                geometry
                    .planes
                    .iter()
                    .map(|plane| plane.to_halfspace_f64())
                    .collect()
            },
        )
    }

    /// The shared body of both `get_or_build*` entry points: rebuild only when
    /// `key` changed, keeping the outgoing `Closed` entry as `last_closed`.
    fn get_or_build_keyed(
        &mut self,
        key: u64,
        facet_count: usize,
        tools: &[ToolPrimitive],
        widened_planes: impl FnOnce() -> Vec<(DVec3, f64)>,
    ) -> Option<&CachedMesh> {
        if self.key != Some(key) {
            self.key = Some(key);
            let new_entry = match build_solid_mesh_geom(&widened_planes(), tools) {
                SolidStatus::Closed(mesh) => {
                    CacheEntry::Closed(CachedMesh::build(mesh, facet_count))
                }
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
    use indicatrix::{geometry::stone_metrics::build_solid_mesh, optics::raytracer::Camera};
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

    /// A cylinder along `z` lying across the top of the unit box of [`box_planes`]:
    /// a groove, so the stone is genuinely concave.
    fn groove() -> ToolPrimitive {
        ToolPrimitive::cylinder(Vec3::new(0.0, 0.6, 0.0), Vec3::Z, 0.3, 2.0)
    }

    fn grooved_box() -> StoneGeometryBuf {
        StoneGeometryBuf {
            tools: vec![groove()],
            placements: vec![(0, 0)],
            ..StoneGeometryBuf::from_halfspaces(&box_planes(0.6))
        }
    }

    #[test]
    fn cache_key_equals_the_planar_key_when_tools_are_empty() {
        let planes = box_planes(0.6);
        let planar = StoneGeometryBuf::from_halfspaces(&planes);
        assert_eq!(planar.cache_key(), hash_planes(&planes));
        assert_eq!(planar.halfspaces(), planes, "the plane conversion is exact");

        let grooved = grooved_box();
        assert_ne!(grooved.cache_key(), planar.cache_key());
        let mut other_tool = grooved_box();
        other_tool.tools[0].origin[1] = 0.5;
        assert_ne!(other_tool.cache_key(), grooved.cache_key());
        let mut renamed = grooved_box();
        renamed.placements = vec![(3, 4)];
        assert_eq!(
            renamed.cache_key(),
            grooved.cache_key(),
            "placements are bookkeeping, not geometry"
        );
    }

    #[test]
    fn a_tool_free_geometry_shares_the_planar_cache_slot() {
        let mut cache = MeshCache::default();
        let planes = box_planes(0.6);
        let planar = cache.get_or_build(&planes).unwrap().mesh.positions.clone();
        let geometry = StoneGeometryBuf::from_halfspaces(&planes);
        let via_geometry = cache.get_or_build_geometry(&geometry).unwrap();
        assert_eq!(via_geometry.mesh.positions, planar);
        assert!(via_geometry.mesh.piece_normals.is_none());
        assert!(
            cache.last_closed().is_none(),
            "the same key is a hit, not a rebuild"
        );
    }

    #[test]
    fn a_grooved_box_builds_a_concave_mesh_with_one_centroid_per_facet() {
        let mut cache = MeshCache::default();
        let stone = grooved_box();
        let cached = cache.get_or_build_geometry(&stone).expect("closes");
        assert!(cached.mesh.piece_normals.is_some());
        assert!(cached.edge_visible.is_some());
        assert_eq!(cached.ring_source.len(), cached.rings.len());
        assert_eq!(cached.facet_centroids.len(), 7, "six planes plus one tool");
        assert!(
            cached.facet_centroids[6].is_some(),
            "the tool surface has a centroid"
        );
        let planar_top = cached.facet_centroids[2].expect("the +Y facet survives");
        assert!(
            planar_top.y > 0.5,
            "taken from a piece lying on the top plane"
        );
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

    /// A build that stops closing must not lose the last real solid this cache
    /// already had -- `last_closed` should keep handing it back across any number
    /// of consecutive failing rebuilds, until a NEW closed build replaces it.
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

    /// A unit cube's own farthest vertex is at `(1,1,1)`, radius `sqrt(3)` --
    /// confirms `bounding_radius` measures from the origin against every vertex,
    /// not just one axis's own half-extent.
    #[test]
    fn bounding_radius_is_the_farthest_vertex_from_the_origin() {
        let mut cache = MeshCache::default();
        let cached = cache.get_or_build(&box_planes(1.0)).unwrap();
        assert!((cached.bounding_radius() - 3.0_f64.sqrt()).abs() < 1e-9);
    }

    /// A unit cube's eight corners are each shared by three facets, so the merged
    /// corner list must hold exactly eight points; every face centre sits on its axis.
    #[test]
    fn a_cube_has_eight_corner_points_and_six_face_centroids() {
        let mut cache = MeshCache::default();
        let planes = box_planes(1.0);
        let cached = cache.get_or_build(&planes).unwrap();
        assert_eq!(cached.corner_points.len(), 8);
        for corner in cached.corner_points.iter() {
            assert!(
                (corner.abs() - Vec3::ONE).abs().max_element() < 1e-5,
                "every cube corner is (+-1, +-1, +-1), got {corner:?}"
            );
        }
        assert_eq!(cached.facet_centroids.iter().flatten().count(), 6);
        for (facet_id, &(normal, offset)) in planes.iter().enumerate() {
            let centroid = cached.facet_centroids[facet_id].expect("every cube face has a ring");
            assert!(
                (centroid - normal * offset).length() < 1e-5,
                "facet {facet_id} centroid {centroid:?}"
            );
        }
    }

    /// `facet_centroids` is indexed by plane, so its length is the plane count; a
    /// plane that never touches the solid has no ring and no centroid.
    #[test]
    fn a_plane_cut_away_entirely_has_no_centroid() {
        let mut cache = MeshCache::default();
        let mut planes = box_planes(1.0);
        planes.push((Vec3::X, 5.0));
        let cached = cache.get_or_build(&planes).unwrap();
        assert_eq!(cached.facet_centroids.len(), planes.len());
        assert_eq!(cached.facet_centroids.len(), 7);
        assert!(cached.facet_centroids[6].is_none());
        assert!(cached.facet_centroids[..6].iter().all(Option::is_some));
        assert_eq!(cached.corner_points.len(), 8);
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
        let prepared = CachedMesh::build(mesh.clone(), planes.len());
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
    ///
    /// `#[cfg(not(target_arch = "wasm32"))]`: its only caller
    /// (`timing_cache_build_and_render_prepared_103_tier`) is gated the same way.
    #[cfg(not(target_arch = "wasm32"))]
    fn crackotto_step_planes() -> Vec<(DVec3, f64)> {
        use indicatrix::geometry::meet_solver;
        const TEXT: &str =
            include_str!("../../indicatrix-cut-core/src/optimize_cost_probe_crackotto_step.asc");
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

    /// `std::time::Instant`-based perf measurement, not a correctness check --
    /// `#[cfg(not(target_arch = "wasm32"))]` (on top of `#[ignore]`) since this
    /// crate must contain no `Instant::now` at all, even in a test that never runs
    /// on `wasm32-unknown-unknown` -- see the crate README.
    #[cfg(not(target_arch = "wasm32"))]
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
        let prepared = CachedMesh::build(mesh.clone(), planes.len());
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
