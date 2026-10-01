//! The mesh of one catalogue design as the planner places it: in the caliper frame,
//! centred on its bounding box, in the design's own model units.
//!
//! The rotation and the centring are exactly those the planner applies to a design's
//! cached hull (the same `run` helpers), so a stone drawn from this mesh at a plan's pose
//! is the stone the plan measured.

use super::super::run::{rotate_into_caliper_frame, to_caliper_frame};
use glam::DVec3;
use indicatrix::geometry::{
    GpuFacetPlane,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use indicatrix_vault::db::sqlite::Database;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};
use tracing::warn;

/// Two consecutive ring points closer than this share of the ring's extent are one point.
const MERGE_FRACTION: f64 = 1e-7;

/// Sine tolerance under which three consecutive ring points count as collinear.
const COLLINEAR_SIN: f64 = 1e-6;

/// One flat face of a design.
#[derive(Debug, Clone)]
pub(super) struct DesignFacet {
    /// The outward unit normal.
    pub(super) normal: DVec3,
    /// The face polygon's corners, in order.
    pub(super) ring: Vec<DVec3>,
}

/// A design's faces in the caliper frame (x = caliper width, y = up, z = caliper
/// length), centred on the bounding box.
#[derive(Debug, Clone)]
pub(super) struct DesignMesh {
    /// The faces that have a polygon.
    pub(super) facets: Vec<DesignFacet>,
}

impl DesignMesh {
    /// The mesh's extent along x, the caliper width, in model units (`0` without faces).
    #[must_use]
    pub(super) fn caliper_width(&self) -> f64 {
        let (mut low, mut high) = (f64::INFINITY, f64::NEG_INFINITY);
        for facet in &self.facets {
            for point in &facet.ring {
                low = low.min(point.x);
                high = high.max(point.x);
            }
        }
        if high >= low { high - low } else { 0.0 }
    }
}

/// Why a design has no mesh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MeshMiss {
    /// The library has no such design any more.
    Gone,
    /// The design could not be read, or its planes do not close into a solid.
    Unreadable,
}

/// Where a stone's design mesh comes from.
pub(super) trait MeshSource {
    /// The mesh of design `entry_id`, or why there is none.
    ///
    /// # Errors
    ///
    /// Returns [`MeshMiss::Gone`] for a design the library no longer holds and
    /// [`MeshMiss::Unreadable`] for one that cannot be read or solved.
    fn mesh(&self, entry_id: i64) -> Result<Arc<DesignMesh>, MeshMiss>;
}

/// Collapses a face polygon to its true corners: consecutive points that coincide are
/// merged, then points on the line between their neighbours are dropped.
#[must_use]
pub(super) fn simplify_ring(ring: &[DVec3]) -> Vec<DVec3> {
    let Some(&first) = ring.first() else {
        return Vec::new();
    };
    let (mut low, mut high) = (first, first);
    for &point in ring {
        low = low.min(point);
        high = high.max(point);
    }
    let merge = (high - low).length().max(1e-12) * MERGE_FRACTION;
    let mut distinct: Vec<DVec3> = Vec::with_capacity(ring.len());
    for &point in ring {
        if distinct
            .last()
            .is_none_or(|&last| (point - last).length() > merge)
        {
            distinct.push(point);
        }
    }
    while distinct.len() > 1 && (distinct[0] - distinct[distinct.len() - 1]).length() <= merge {
        distinct.pop();
    }
    if distinct.len() < 3 {
        return distinct;
    }
    let count = distinct.len();
    let mut corners = Vec::with_capacity(count);
    let mut previous = distinct[count - 1];
    for (i, &current) in distinct.iter().enumerate() {
        let next = distinct[(i + 1) % count];
        let before = current - previous;
        let after = next - current;
        let collinear =
            before.cross(after).length() <= COLLINEAR_SIN * before.length() * after.length();
        if !collinear {
            corners.push(current);
            previous = current;
        }
    }
    corners
}

/// The centre of the bounding box of `corners` turned into the caliper frame of
/// `width_dir`.
fn caliper_centre(corners: &[[f64; 3]], width_dir: [f64; 2]) -> DVec3 {
    let mut low = DVec3::splat(f64::INFINITY);
    let mut high = DVec3::splat(f64::NEG_INFINITY);
    for &corner in corners {
        let turned = DVec3::from_array(to_caliper_frame(corner, width_dir));
        low = low.min(turned);
        high = high.max(turned);
    }
    (low + high) * 0.5
}

/// Builds the mesh of the solid `planes` (`n . p <= m`) bound, in the caliper frame.
/// `None` when the planes do not close into a solid or the outline has no width.
///
/// The frame comes from the planner's own hull preparation, applied to the solid's corners
/// rounded to `f32` like the cached hull the planner works on: an outline with tied
/// caliper widths then picks the same direction as the plan did, and a stone is drawn
/// turned the way it was planned. The centre is that of the exact corners in that frame.
#[must_use]
pub(super) fn design_mesh_from_planes(planes: &[(DVec3, f64)]) -> Option<DesignMesh> {
    let SolidStatus::Closed(mesh) = build_solid_mesh(planes) else {
        return None;
    };
    let corners: Vec<[f64; 3]> = mesh
        .rings
        .iter()
        .flat_map(|(_, ring)| ring.iter().map(DVec3::to_array))
        .collect();
    let stored: Vec<[f64; 3]> = corners
        .iter()
        .map(|corner| corner.map(|c| f64::from(c as f32)))
        .collect();
    let width_dir = rotate_into_caliper_frame(&stored)?.width_dir;
    let centre = caliper_centre(&corners, width_dir);
    let to_caliper = |v: DVec3| DVec3::from_array(to_caliper_frame(v.to_array(), width_dir));

    let facets: Vec<DesignFacet> = mesh
        .rings
        .iter()
        .filter_map(|(id, ring)| {
            let normal = to_caliper(planes.get(*id)?.0);
            let ring: Vec<DVec3> = ring.iter().map(|&p| to_caliper(p) - centre).collect();
            let ring = simplify_ring(&ring);
            (ring.len() >= 3).then_some(DesignFacet { normal, ring })
        })
        .collect();
    (!facets.is_empty()).then_some(DesignMesh { facets })
}

/// What one design's slot holds: nothing until it is loaded, then the mesh (or `None` for
/// a design that is gone or has no solid).
type Slot = Arc<Mutex<Option<Option<Arc<DesignMesh>>>>>;

/// A cache of design meshes with one slot per design, so a design is loaded once even
/// when two threads ask for it at the same time, and single designs can be dropped.
#[derive(Default)]
struct MeshCache {
    slots: Mutex<HashMap<i64, Slot>>,
}

impl MeshCache {
    /// The mesh of design `id`: the cached one, or what `load` returns (which runs once
    /// per design; a second asker waits for it). `Err` from `load` is not remembered, and
    /// the design reads as unreadable this time; `Ok(None)` is remembered as gone.
    fn get_or_load<E>(
        &self,
        id: i64,
        load: impl FnOnce() -> Result<Option<DesignMesh>, E>,
    ) -> Result<Arc<DesignMesh>, MeshMiss> {
        let slot = Arc::clone(
            self.slots
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .entry(id)
                .or_default(),
        );
        let mut held = slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(known) = held.as_ref() {
            return known.clone().ok_or(MeshMiss::Gone);
        }
        let Ok(loaded) = load() else {
            return Err(MeshMiss::Unreadable);
        };
        let built = loaded.map(Arc::new);
        *held = Some(built.clone());
        drop(held);
        built.ok_or(MeshMiss::Gone)
    }

    /// The designs that have a slot.
    fn ids(&self) -> Vec<i64> {
        self.slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .copied()
            .collect()
    }

    /// Forgets design `id`. A load that is under way finishes into a slot nobody reads
    /// any more, so it cannot bring an old design back.
    fn remove(&self, id: i64) {
        self.slots
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
    }
}

/// The design meshes of the session, built on first use and kept across plans until the
/// design's library row changes: a design is read from the catalogue and solved once,
/// whichever thread asks first.
pub(super) struct MeshLibrary {
    db: Arc<Mutex<Database>>,
    cache: MeshCache,
    /// The revision stamp (`diagram_entries.updated_at`) each cached design was read at.
    stamps: Mutex<HashMap<i64, Option<i64>>>,
    /// The newest plan epoch whose designs were checked against the library.
    checked: Mutex<u64>,
}

impl MeshLibrary {
    /// An empty library reading designs from `db`.
    #[must_use]
    pub(super) fn new(db: Arc<Mutex<Database>>) -> Self {
        Self {
            db,
            cache: MeshCache::default(),
            stamps: Mutex::default(),
            checked: Mutex::default(),
        }
    }

    /// Drops the meshes of designs whose library row changed (or is gone) since they were
    /// read, so the plan of `epoch` draws them as they are in the catalogue now. Runs once
    /// per epoch: the first thread to ask checks, the others wait for it and return.
    pub(super) fn refresh(&self, epoch: u64) {
        let mut checked = self.checked.lock().unwrap_or_else(PoisonError::into_inner);
        if *checked >= epoch {
            return;
        }
        self.drop_stale_meshes();
        *checked = epoch;
    }

    /// Removes every cached mesh whose library row changed or vanished since it was read.
    fn drop_stale_meshes(&self) {
        let ids = self.cache.ids();
        if ids.is_empty() {
            return;
        }
        let stale: Vec<i64> = {
            let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            let stamps = self.stamps.lock().unwrap_or_else(PoisonError::into_inner);
            ids.into_iter()
                .filter(|id| {
                    stamps.get(id).is_none_or(|known| {
                        !db.entry_updated_at(*id).is_ok_and(|now| now == *known)
                    })
                })
                .collect()
        };
        let mut stamps = self.stamps.lock().unwrap_or_else(PoisonError::into_inner);
        for id in stale {
            self.cache.remove(id);
            stamps.remove(&id);
        }
    }

    /// Reads design `entry_id` and builds its mesh from the facet planes alone (the
    /// preform's planes come first in a design file's list and are skipped).
    /// `Ok(None)` means the library has no such design. `Err` means the read failed or the
    /// planes do not close, so the answer must not be remembered.
    fn load(&self, entry_id: i64) -> Result<Option<DesignMesh>, ()> {
        // The stamp is read in the same locked section as the record it describes.
        let (stamp, full) = {
            let db = self.db.lock().unwrap_or_else(PoisonError::into_inner);
            (
                db.entry_updated_at(entry_id).ok().flatten(),
                db.get_diagram_full(entry_id),
            )
        };
        let full = match full {
            Ok(Some(full)) => full,
            Ok(None) => return Ok(None),
            Err(error) => {
                warn!("Rough planner: could not read design #{entry_id}: {error}");
                return Err(());
            }
        };
        let resolved = crate::gui::editor::resolve_catalogue_planes(&full);
        let halfspaces: Vec<(DVec3, f64)> = resolved
            .planes
            .iter()
            .copied()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect();
        let start = resolved.preform_plane_count.min(halfspaces.len());
        let Some(mesh) = design_mesh_from_planes(&halfspaces[start..]) else {
            warn!("Rough planner: design #{entry_id} has no closed solid to draw");
            return Err(());
        };
        self.stamps
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(entry_id, stamp);
        Ok(Some(mesh))
    }
}

impl MeshSource for MeshLibrary {
    fn mesh(&self, entry_id: i64) -> Result<Arc<DesignMesh>, MeshMiss> {
        self.cache.get_or_load(entry_id, || self.load(entry_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The six planes of an axis-aligned box `[-hx, hx] x [-hy, hy] x [-hz, hz]`.
    fn box_planes(half: DVec3) -> Vec<(DVec3, f64)> {
        vec![
            (DVec3::X, half.x),
            (DVec3::NEG_X, half.x),
            (DVec3::Y, half.y),
            (DVec3::NEG_Y, half.y),
            (DVec3::Z, half.z),
            (DVec3::NEG_Z, half.z),
        ]
    }

    /// The same box turned by `degrees` about y and moved by `shift`.
    fn turned_box(half: DVec3, degrees: f64, shift: DVec3) -> Vec<(DVec3, f64)> {
        let (sin, cos) = degrees.to_radians().sin_cos();
        let turn = |v: DVec3| {
            DVec3::new(
                v.x.mul_add(cos, v.z * sin),
                v.y,
                (-v.x).mul_add(sin, v.z * cos),
            )
        };
        box_planes(half)
            .into_iter()
            .map(|(n, m)| {
                let turned = turn(n);
                (turned, m + turned.dot(shift))
            })
            .collect()
    }

    fn bounds(mesh: &DesignMesh) -> (DVec3, DVec3) {
        let (mut low, mut high) = (DVec3::splat(f64::INFINITY), DVec3::splat(f64::NEG_INFINITY));
        for facet in &mesh.facets {
            for &p in &facet.ring {
                low = low.min(p);
                high = high.max(p);
            }
        }
        (low, high)
    }

    #[test]
    fn a_turned_box_gets_its_width_on_x_and_its_length_on_z_and_is_centred() {
        // 2 wide, 3 tall, 6 long, turned 30 degrees and moved off the origin.
        let planes = turned_box(DVec3::new(1.0, 1.5, 3.0), 30.0, DVec3::new(0.7, 0.2, -0.4));
        let mesh = design_mesh_from_planes(&planes).expect("a box closes");
        let (low, high) = bounds(&mesh);
        assert!((low + high).length() < 1e-9, "bounding box is centred");
        // The frame comes from f32-rounded corners (like the planner's cached hull), so the
        // box is axis-aligned to about 1e-7 rad.
        assert!((high - low - DVec3::new(2.0, 3.0, 6.0)).length() < 1e-5);
        assert_eq!(mesh.facets.len(), 6);
    }

    #[test]
    fn the_caliper_width_is_the_extent_along_x() {
        // The box is 2 wide (x), 3 tall, 6 long, so the width is its 2 units.
        let planes = turned_box(DVec3::new(1.0, 1.5, 3.0), 30.0, DVec3::ZERO);
        let mesh = design_mesh_from_planes(&planes).expect("a box closes");
        assert!((mesh.caliper_width() - 2.0).abs() < 1e-5);
        assert!(DesignMesh { facets: Vec::new() }.caliper_width().abs() < f64::EPSILON);
    }

    #[test]
    fn facet_normals_stay_unit_and_point_away_from_the_centre() {
        let planes = turned_box(DVec3::new(1.0, 1.5, 3.0), 30.0, DVec3::ZERO);
        let mesh = design_mesh_from_planes(&planes).expect("a box closes");
        for facet in &mesh.facets {
            assert!((facet.normal.length() - 1.0).abs() < 1e-9);
            let mid = facet.ring.iter().copied().sum::<DVec3>() / facet.ring.len() as f64;
            assert!(facet.normal.dot(mid) > 0.0, "outward normal");
        }
    }

    #[test]
    fn the_caliper_frame_agrees_with_the_hull_pipeline_on_a_turned_box() {
        // The planner's own hull rotation (`run::rotate_into_caliper_frame`) applied to
        // the box's vertices as the vault stores them (f32) must give the same bounding
        // box, and the same centre to subtract, as this mesh (to the f32 rounding).
        let planes = turned_box(DVec3::new(1.0, 1.5, 3.0), 30.0, DVec3::new(0.7, 0.2, -0.4));
        let mesh = design_mesh_from_planes(&planes).expect("a box closes");
        let SolidStatus::Closed(raw) = build_solid_mesh(&planes) else {
            panic!("a box closes");
        };
        let corners: Vec<[f64; 3]> = raw
            .rings
            .iter()
            .flat_map(|(_, r)| r.iter().map(|p| p.to_array().map(|c| f64::from(c as f32))))
            .collect();
        let hull = rotate_into_caliper_frame(&corners).expect("outline");
        let hull_size = hull.size();
        let (mesh_low, mesh_high) = bounds(&mesh);
        let mesh_size = (mesh_high - mesh_low).to_array();
        for axis in 0..3 {
            assert!(
                (hull_size[axis] - mesh_size[axis]).abs() < 1e-5,
                "axis {axis}: hull {} vs mesh {}",
                hull_size[axis],
                mesh_size[axis]
            );
        }
        // Every mesh corner is a hull corner minus the hull's centre.
        let centre = DVec3::from_array(hull.centre());
        let hull_corners: Vec<DVec3> = hull
            .vertices
            .iter()
            .map(|&v| DVec3::from_array(v) - centre)
            .collect();
        for facet in &mesh.facets {
            for &corner in &facet.ring {
                assert!(
                    hull_corners.iter().any(|&h| (h - corner).length() < 1e-5),
                    "mesh corner {corner:?} is not a centred hull corner"
                );
            }
        }
        // And it is the design's own shape: 2 wide (x), 3 tall (y), 6 long (z).
        assert!((mesh_size[0] - 2.0).abs() < 1e-5 && (mesh_size[1] - 3.0).abs() < 1e-5);
        assert!((mesh_size[2] - 6.0).abs() < 1e-5);
    }

    #[test]
    fn a_tied_outline_is_turned_the_way_the_stored_f32_hull_is() {
        // A square outline has four equal caliper widths, so which edge the frame follows
        // is decided by rounding noise. The mesh must follow the edge the f32 hull picks.
        let planes = turned_box(DVec3::new(1.5, 1.0, 1.5), 17.0, DVec3::new(9.3, 0.1, -4.7));
        let mesh = design_mesh_from_planes(&planes).expect("a box closes");
        let SolidStatus::Closed(raw) = build_solid_mesh(&planes) else {
            panic!("a box closes");
        };
        let stored: Vec<[f64; 3]> = raw
            .rings
            .iter()
            .flat_map(|(_, r)| r.iter().map(|p| p.to_array().map(|c| f64::from(c as f32))))
            .collect();
        let hull = rotate_into_caliper_frame(&stored).expect("outline");
        // The facet whose outward normal points along the caliper width direction in the
        // design frame is the +x facet of the mesh.
        let [wx, wz] = hull.width_dir;
        let along_width = mesh
            .facets
            .iter()
            .filter(|facet| facet.normal.x > 0.999)
            .count();
        assert_eq!(along_width, 1, "one facet faces +x of the caliper frame");
        // Turning the +x mesh normal back into the design frame gives the hull's width
        // direction (a proper rotation about y: x -> (wx, 0, wz)).
        let facet = mesh
            .facets
            .iter()
            .find(|facet| facet.normal.x > 0.999)
            .expect("a +x facet");
        let back = DVec3::new(
            facet.normal.x.mul_add(wx, -facet.normal.z * wz),
            facet.normal.y,
            facet.normal.x.mul_add(wz, facet.normal.z * wx),
        );
        assert!(
            (back - DVec3::new(wx, 0.0, wz)).length() < 1e-5,
            "the mesh's width axis {back:?} is the hull's {wx}, {wz}"
        );
    }

    #[test]
    fn a_cached_mesh_is_loaded_once_and_dropped_by_remove() {
        let cache = MeshCache::default();
        let loads = std::sync::atomic::AtomicUsize::new(0);
        let load = || -> Result<Option<DesignMesh>, ()> {
            loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(DesignMesh { facets: Vec::new() }))
        };
        let first = cache.get_or_load(5, load).expect("loaded");
        let second = cache.get_or_load(5, load).expect("cached");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
        // Another design is another slot.
        cache.get_or_load(6, load).expect("loaded");
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 2);
        // After a remove the design is read again, and the old mesh is not handed out.
        cache.remove(5);
        let third = cache.get_or_load(5, load).expect("reloaded");
        assert!(!Arc::ptr_eq(&first, &third));
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 3);
        // The other design was not touched, and both have a slot.
        let mut ids = cache.ids();
        ids.sort_unstable();
        assert_eq!(ids, vec![5, 6]);
        cache.get_or_load(6, load).expect("still cached");
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 3);
    }

    #[test]
    fn an_absent_design_is_remembered_as_gone_but_a_failed_read_is_not_and_reads_unreadable() {
        let cache = MeshCache::default();
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let absent = || -> Result<Option<DesignMesh>, ()> {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(None)
        };
        assert_eq!(cache.get_or_load(1, absent).err(), Some(MeshMiss::Gone));
        assert_eq!(cache.get_or_load(1, absent).err(), Some(MeshMiss::Gone));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);

        let failing = || -> Result<Option<DesignMesh>, ()> {
            calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Err(())
        };
        assert_eq!(
            cache.get_or_load(2, failing).err(),
            Some(MeshMiss::Unreadable)
        );
        assert_eq!(
            cache.get_or_load(2, failing).err(),
            Some(MeshMiss::Unreadable)
        );
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "the failed read was tried twice"
        );
        // A later successful read fills the slot.
        let ok =
            || -> Result<Option<DesignMesh>, ()> { Ok(Some(DesignMesh { facets: Vec::new() })) };
        assert!(cache.get_or_load(2, ok).is_ok());
    }

    #[test]
    fn two_threads_asking_for_one_design_load_it_once() {
        let cache = MeshCache::default();
        let loads = std::sync::atomic::AtomicUsize::new(0);
        let meshes: Vec<Arc<DesignMesh>> = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..4)
                .map(|_| {
                    scope.spawn(|| {
                        cache
                            .get_or_load(9, || -> Result<Option<DesignMesh>, ()> {
                                loads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                std::thread::sleep(std::time::Duration::from_millis(20));
                                Ok(Some(DesignMesh { facets: Vec::new() }))
                            })
                            .expect("loaded")
                    })
                })
                .collect();
            handles
                .into_iter()
                .map(|h| h.join().expect("no panic"))
                .collect()
        });
        assert_eq!(loads.load(std::sync::atomic::Ordering::SeqCst), 1);
        assert!(meshes.iter().all(|m| Arc::ptr_eq(m, &meshes[0])));
    }

    #[test]
    fn a_load_under_way_during_a_remove_cannot_refill_the_cache() {
        let cache = MeshCache::default();
        let stale = cache
            .get_or_load(3, || -> Result<Option<DesignMesh>, ()> {
                // The design is edited (and its slot dropped) while it loads.
                cache.remove(3);
                Ok(Some(DesignMesh { facets: Vec::new() }))
            })
            .expect("the old asker still gets its mesh");
        let fresh = cache
            .get_or_load(3, || -> Result<Option<DesignMesh>, ()> {
                Ok(Some(DesignMesh { facets: Vec::new() }))
            })
            .expect("reloaded");
        assert!(!Arc::ptr_eq(&stale, &fresh), "the stale mesh was not kept");
    }

    #[test]
    fn open_planes_give_no_mesh() {
        assert!(design_mesh_from_planes(&[(DVec3::X, 1.0), (DVec3::NEG_X, 1.0)]).is_none());
        assert!(design_mesh_from_planes(&[]).is_none());
    }

    #[test]
    fn simplifying_a_ring_drops_repeats_and_midpoints() {
        let ring = [
            DVec3::new(0.0, 0.0, 0.0),
            DVec3::new(0.5, 0.0, 0.0),
            DVec3::new(0.5, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 0.0),
            DVec3::new(1.0, 0.0, 1.0),
            DVec3::new(0.0, 0.0, 1.0),
        ];
        let corners = simplify_ring(&ring);
        assert_eq!(corners.len(), 4);
        assert_eq!(simplify_ring(&[]), Vec::<DVec3>::new());
    }
}
