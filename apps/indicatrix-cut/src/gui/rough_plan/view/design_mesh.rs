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
    stone_metrics::{SolidStatus, build_solid_mesh_geom},
    tool::ToolPrimitive,
};
use indicatrix_cut_core::rough_plan::shape::hull::design_outline;
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

/// One face of a design: a flat facet, or on a concave design one piece of a facet or of
/// a tool's curved surface (a polygon with its own normal).
#[derive(Debug, Clone)]
pub(super) struct DesignFacet {
    /// The outward unit normal.
    pub(super) normal: DVec3,
    /// The face polygon's corners, in order.
    pub(super) ring: Vec<DVec3>,
    /// Whether the edge from `ring[i]` to `ring[i + 1]` (cyclic) is drawn. `None` draws
    /// every edge, as a flat design does; a concave design hides the seams where one
    /// facet or tool surface is cut into several pieces.
    pub(super) edge_drawn: Option<Vec<bool>>,
}

impl DesignFacet {
    /// A facet whose every edge is drawn.
    #[cfg(test)]
    #[must_use]
    pub(super) const fn new(normal: DVec3, ring: Vec<DVec3>) -> Self {
        Self {
            normal,
            ring,
            edge_drawn: None,
        }
    }
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

/// The drawn flags of the corners `simplified` kept of `ring`, given the flag of each
/// original segment `s -> s + 1` (cyclic; a missing flag counts as drawn). A kept edge is
/// drawn when any original segment it spans is, so a facet boundary stays drawn when
/// collinear seam points were removed from its middle. [`simplify_ring`] only drops
/// points, so each kept corner is found again by exact equality; if one is not, every
/// edge is drawn rather than losing a boundary.
pub(super) fn simplified_flags(
    ring: &[DVec3],
    simplified: &[DVec3],
    visible: &[bool],
) -> Vec<bool> {
    let mut source = Vec::with_capacity(simplified.len());
    let mut cursor = 0;
    for corner in simplified {
        let Some(offset) = ring[cursor..].iter().position(|p| p == corner) else {
            return vec![true; simplified.len()];
        };
        source.push(cursor + offset);
        cursor += offset + 1;
    }
    let count = ring.len();
    (0..source.len())
        .map(|i| {
            let (from, to) = (source[i], source[(i + 1) % source.len()]);
            let mut segment = from;
            loop {
                if visible.get(segment).copied().unwrap_or(true) {
                    return true;
                }
                segment = (segment + 1) % count;
                if segment == to {
                    return false;
                }
            }
        })
        .collect()
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

/// Builds the mesh of the stone `planes` (`n . p <= m`) bound, minus the concave `tools`,
/// in the caliper frame. `None` when the planes do not close into a solid or the outline
/// has no width.
///
/// The frame comes from the planner's own hull preparation, applied to the stone's hull
/// corners rounded to `f32` like the cached hull the planner works on: an outline with
/// tied caliper widths then picks the same direction as the plan did, and a stone is drawn
/// turned the way it was planned. Those corners are the ring corners of a planar design and
/// the convex hull of the CARVED mesh for a design with tools (the hull the plan fitted,
/// see `run::scan`). The centre is that of the exact corners in that frame.
///
/// A concave mesh has several polygons per facet and rings that belong to a tool, so each
/// ring takes its own normal from `piece_normals`, and the seam edges `edge_visible`
/// hides are not drawn. Without tools this is the planar mesh exactly.
#[must_use]
pub(super) fn design_mesh_from_planes(
    planes: &[(DVec3, f64)],
    tools: &[ToolPrimitive],
) -> Option<DesignMesh> {
    let SolidStatus::Closed(mesh) = build_solid_mesh_geom(planes, tools) else {
        return None;
    };
    let corners_of = |mesh: &indicatrix::geometry::stone_metrics::SolidMesh| -> Vec<[f64; 3]> {
        mesh.rings
            .iter()
            .flat_map(|(_, ring)| ring.iter().map(DVec3::to_array))
            .collect()
    };
    let mesh_corners = corners_of(&mesh);
    // The planner measures a design with tools from the convex hull of its carved mesh
    // (`run::scan`), so the frame and the centre come from the same corners. Without
    // tools every ring corner is used, exactly as before.
    let corners = if tools.is_empty() {
        mesh_corners
    } else {
        let points: Vec<DVec3> = mesh_corners
            .iter()
            .copied()
            .map(DVec3::from_array)
            .collect();
        design_outline(&points).map_or(mesh_corners, |outline| {
            outline.corners.iter().map(DVec3::to_array).collect()
        })
    };
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
        .enumerate()
        .filter_map(|(index, (id, ring))| {
            let normal = match &mesh.piece_normals {
                // Tool-piece normals come from the kernel's f32 `outward_normal`, so
                // they are unit only to ~1e-7; renormalise in f64. Flat designs
                // (`None`) keep their plane normals bit for bit.
                Some(normals) => normals.get(index)?.normalize(),
                None => planes.get(*id)?.0,
            };
            let ring: Vec<DVec3> = ring.iter().map(|&p| to_caliper(p) - centre).collect();
            let corners = simplify_ring(&ring);
            if corners.len() < 3 {
                return None;
            }
            let edge_drawn = mesh.edge_visible.as_ref().map(|visible| {
                simplified_flags(
                    &ring,
                    &corners,
                    visible.get(index).map_or(&[], Vec::as_slice),
                )
            });
            Some(DesignFacet {
                normal: to_caliper(normal),
                ring: corners,
                edge_drawn,
            })
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
    /// preform's planes come first in a design file's list and are skipped), minus the
    /// design's concave tools.
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
        if let Some(reason) = &resolved.concave_error {
            // Drawing it flat would show a stone the plan did not measure.
            warn!(
                "Rough planner: design #{entry_id} has concave tiers that do not resolve: {reason}"
            );
            return Err(());
        }
        let halfspaces: Vec<(DVec3, f64)> = resolved
            .planes
            .iter()
            .copied()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect();
        let start = resolved.preform_plane_count.min(halfspaces.len());
        let Some(mesh) = design_mesh_from_planes(&halfspaces[start..], &resolved.tools) else {
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

/// How a design's model frame relates to the planner's caliper frame, for the corners the planner
/// measures the design from (`zoning` builds only): the caliper turn of their outline and the
/// centre of their bounding box in the turned frame. These are the same helpers
/// [`design_mesh_from_planes`] applies (`rotate_into_caliper_frame`, `caliper_centre`), called in
/// the same order on the same `f32`-rounded corners, so the placement and the mesh agree.
#[cfg(feature = "zoning")]
#[must_use]
pub(in crate::gui::rough_plan) fn placement_from_corners(
    corners: &[[f64; 3]],
) -> Option<indicatrix_cut_core::rough_plan::zoned_plan::DesignPlacement> {
    let stored: Vec<[f64; 3]> = corners
        .iter()
        .map(|corner| corner.map(|c| f64::from(c as f32)))
        .collect();
    let width_dir = rotate_into_caliper_frame(&stored)?.width_dir;
    let centre = caliper_centre(corners, width_dir);
    Some(
        indicatrix_cut_core::rough_plan::zoned_plan::DesignPlacement {
            width_dir,
            centre_units: centre.to_array(),
        },
    )
}

/// The placement of the stone `planes` bound minus the concave `tools` (`zoning` builds only): the
/// corners are chosen exactly as [`design_mesh_from_planes`] chooses them (the ring corners of a
/// planar design, the convex hull of the carved mesh for a design with tools), then
/// [`placement_from_corners`]. `None` when the planes do not close into a solid or the outline has
/// no width.
#[cfg(feature = "zoning")]
#[must_use]
pub(in crate::gui::rough_plan) fn design_placement_from_planes(
    planes: &[(DVec3, f64)],
    tools: &[ToolPrimitive],
) -> Option<indicatrix_cut_core::rough_plan::zoned_plan::DesignPlacement> {
    let SolidStatus::Closed(mesh) = build_solid_mesh_geom(planes, tools) else {
        return None;
    };
    let mesh_corners: Vec<[f64; 3]> = mesh
        .rings
        .iter()
        .flat_map(|(_, ring)| ring.iter().map(DVec3::to_array))
        .collect();
    let corners = if tools.is_empty() {
        mesh_corners
    } else {
        let points: Vec<DVec3> = mesh_corners
            .iter()
            .copied()
            .map(DVec3::from_array)
            .collect();
        design_outline(&points).map_or(mesh_corners, |outline| {
            outline.corners.iter().map(DVec3::to_array).collect()
        })
    };
    placement_from_corners(&corners)
}

#[cfg(feature = "zoning")]
impl MeshLibrary {
    /// The placement of design `entry_id` (`None` for a design that is gone, unreadable or has no
    /// solid). Read from the library each time; the caller asks once per design.
    pub(in crate::gui::rough_plan) fn placement(
        &self,
        entry_id: i64,
    ) -> Option<indicatrix_cut_core::rough_plan::zoned_plan::DesignPlacement> {
        let full = self
            .db
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get_diagram_full(entry_id)
            .ok()??;
        let resolved = crate::gui::editor::resolve_catalogue_planes(&full);
        if resolved.concave_error.is_some() {
            return None;
        }
        let halfspaces: Vec<(DVec3, f64)> = resolved
            .planes
            .iter()
            .copied()
            .map(GpuFacetPlane::to_halfspace_f64)
            .collect();
        let start = resolved.preform_plane_count.min(halfspaces.len());
        design_placement_from_planes(&halfspaces[start..], &resolved.tools)
    }
}

impl MeshSource for MeshLibrary {
    fn mesh(&self, entry_id: i64) -> Result<Arc<DesignMesh>, MeshMiss> {
        self.cache.get_or_load(entry_id, || self.load(entry_id))
    }
}

#[cfg(test)]
mod tests;
