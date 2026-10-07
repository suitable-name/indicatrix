//! Inclusions: closed shells inside a rough where no stone may go, but which are material
//! you hold and pay for.
//!
//! An inclusion behaves like a cavity for placement. The rough's mesh already treats a second
//! closed shell inside the outer one as a hollow: its triangles block placements and give
//! the fit its rows, and its volume is left out of [`RoughMesh::volume`]. What differs is the
//! weight. A hollow is air; an inclusion is stone, so the weighed carat, Fit to weight and the
//! yield read the GROSS volume ([`RoughMesh::gross_volume`]), the rough as bought.
//!
//! The mesh keeps each inclusion as a solid of its own beside the combined shells, so the
//! planner window can list and remove them and a saved plan can write them back.
//!
//! Everything is deterministic: inclusions in the order given, triangles in index order.

use glam::{DMat3, DVec3};

use super::{
    MAX_MESH_TRIANGLES, MeshError, RoughMesh,
    geometry::bounds_of,
    intersect::{TOLERANCE_FRACTION, triangles_cross},
};

/// The farthest a vertex moves when a margin is applied, in margins. A sharp spike would need
/// a longer mitre to hold the margin on both faces; it is cut off here instead.
const MAX_MITRE: f64 = 4.0;

/// The ridge added to the vertex offset system, per incident face, so a vertex whose faces
/// do not span space (a flat patch, an edge) still has a unique, short answer.
const OFFSET_RIDGE: f64 = 1e-10;

impl RoughMesh {
    /// How many inclusions the mesh has.
    #[must_use]
    pub const fn inclusion_count(&self) -> usize {
        self.inclusions.len()
    }

    /// The inclusions, each as a closed solid wound outward, in the rough's frame, in the
    /// order they were added.
    #[must_use]
    pub fn inclusions(&self) -> &[Self] {
        &self.inclusions
    }

    /// The index of the first triangle of every inclusion, ascending (empty without
    /// inclusions). The rough's registry id hashes it, so an inclusion and a hollow of the
    /// same shape are two roughs.
    #[must_use]
    pub fn inclusion_shells(&self) -> &[u32] {
        &self.inclusion_shells
    }

    /// The volume of the inclusions together in mm^3 (0 without any).
    #[must_use]
    pub const fn inclusion_volume(&self) -> f64 {
        self.inclusion_volume_mm3
    }

    /// The volume of the rough as it is held, in mm^3: [`volume`](Self::volume) plus the
    /// inclusions. This is what the weight and the yield are measured against. Without
    /// inclusions it is exactly [`volume`](Self::volume).
    #[must_use]
    pub fn gross_volume(&self) -> f64 {
        if self.inclusions.is_empty() {
            self.volume
        } else {
            self.volume + self.inclusion_volume_mm3
        }
    }

    /// The volume of the inclusions that satisfies `n . p <= m` for every `(n, m)` of `cuts`.
    #[must_use]
    pub fn inclusion_volume_within(&self, cuts: &[(DVec3, f64)]) -> f64 {
        self.inclusions
            .iter()
            .map(|body| body.volume_within(cuts))
            .sum()
    }

    /// [`volume_within`](Self::volume_within) plus the inclusions inside the same cuts: the
    /// material held within them. Without inclusions it is exactly `volume_within`.
    #[must_use]
    pub fn gross_volume_within(&self, cuts: &[(DVec3, f64)]) -> f64 {
        let usable = self.volume_within(cuts);
        if self.inclusions.is_empty() {
            usable
        } else {
            usable + self.inclusion_volume_within(cuts)
        }
    }

    /// The triangles of the rough's own surface (and its hollows), without the inclusions:
    /// what a saved plan stores as the mesh. The whole mesh when it has no inclusions.
    #[must_use]
    pub fn outer_triangles(&self) -> &[[u32; 3]] {
        match self.inclusion_shells.first() {
            Some(&first) => &self.tris[..first as usize],
            None => &self.tris,
        }
    }

    /// The vertices of [`outer_triangles`](Self::outer_triangles): the inclusions' vertices
    /// come after them.
    #[must_use]
    pub fn outer_vertices(&self) -> &[DVec3] {
        let used = self.outer_triangles().len();
        let count = self.tris[used..]
            .iter()
            .flatten()
            .map(|&v| v as usize)
            .min()
            .unwrap_or(self.verts.len());
        &self.verts[..count]
    }

    /// Gives `moved` (this mesh moved or scaled) this mesh's inclusions, each moved the same
    /// way by `map`. `None` when one of them cannot be.
    pub(super) fn carry_inclusions(
        &self,
        mut moved: Self,
        map: impl Fn(&Self) -> Option<Self>,
    ) -> Option<Self> {
        if self.inclusions.is_empty() {
            return Some(moved);
        }
        let bodies = self
            .inclusions
            .iter()
            .map(map)
            .collect::<Option<Vec<Self>>>()?;
        moved.inclusion_shells.clone_from(&self.inclusion_shells);
        moved.inclusion_volume_mm3 = bodies.iter().map(Self::volume).sum();
        moved.inclusions = bodies;
        Some(moved)
    }

    /// Whether the surface of `other` crosses this mesh's surface, with every cavity and
    /// inclusion shell it has.
    fn crossed_by(&self, other: &Self, tolerance: f64) -> bool {
        for tri in &other.tris {
            let corners = tri.map(|v| other.verts[v as usize]);
            let (lo, hi) = bounds_of(corners.into_iter());
            let (lo, hi) = (lo - DVec3::splat(tolerance), hi + DVec3::splat(tolerance));
            let mut crossed = false;
            self.walk(
                |node| node.min.cmple(hi).all() && node.max.cmpge(lo).all(),
                |t| {
                    crossed = triangles_cross(corners, self.corners(t), tolerance);
                    !crossed
                },
            );
            if crossed {
                return true;
            }
        }
        false
    }

    /// Checks that `inclusion` (a closed solid wound outward) can be added to this mesh as
    /// inclusion number `index` of a call: its surface must not cross the rough's, and all
    /// of it must lie in the rough's material.
    ///
    /// An inclusion that wraps a hollow or another inclusion of the rough is not detected.
    ///
    /// # Errors
    ///
    /// [`MeshError::InclusionReachesSurface`] when the two surfaces cross (that is a notch,
    /// to be modelled in the rough's own mesh), or [`MeshError::InclusionOutside`] when they
    /// do not cross but a vertex is not in the material.
    pub fn check_inclusion(&self, inclusion: &Self, index: usize) -> Result<(), MeshError> {
        let tolerance = TOLERANCE_FRACTION * (self.hi - self.lo).max_element();
        if self.crossed_by(inclusion, tolerance) {
            return Err(MeshError::InclusionReachesSurface(index));
        }
        if !inclusion.verts.iter().all(|&p| self.contains_point(p)) {
            return Err(MeshError::InclusionOutside(index));
        }
        Ok(())
    }

    /// The mesh `outer` with `inclusions` added as extra closed shells inside it.
    ///
    /// Each inclusion is a closed solid wound outward (a mesh from [`new`](Self::new)). The
    /// combined mesh holds the outer triangles first and then each inclusion's, turned to face
    /// into the void like a cavity, so stones keep out of it; the mesh remembers which shells
    /// are inclusions. Inclusions the outer mesh already has stay. The result's
    /// [`volume`](Self::volume) is the outer volume less the inclusions, and
    /// [`gross_volume`](Self::gross_volume) is the outer volume.
    ///
    /// An inclusion must lie wholly inside the material and must not cross the rough's
    /// surface or another inclusion; one that touches the surface within the crossing
    /// tolerance is accepted.
    ///
    /// # Errors
    ///
    /// [`MeshError::InclusionReachesSurface`], [`MeshError::InclusionOutside`] or
    /// [`MeshError::InclusionsOverlap`] for an inclusion that does not fit (the number is its
    /// place in `inclusions`), [`MeshError::TooManyTriangles`] when the combined mesh is over
    /// the limit, or [`MeshError::Degenerate`] when it would enclose no volume.
    pub fn with_inclusions(outer: &Self, inclusions: &[Self]) -> Result<Self, MeshError> {
        if inclusions.is_empty() {
            return Ok(outer.clone());
        }
        let added: usize = inclusions.iter().map(|body| body.tris.len()).sum();
        let total = outer.tris.len() + added;
        if total > MAX_MESH_TRIANGLES {
            return Err(MeshError::TooManyTriangles(total));
        }
        let tolerance = TOLERANCE_FRACTION * (outer.hi - outer.lo).max_element();
        for (k, body) in inclusions.iter().enumerate() {
            outer.check_inclusion(body, k)?;
            for earlier in &inclusions[..k] {
                let holds =
                    |a: &Self, b: &Self| b.verts.first().is_some_and(|&p| a.contains_point(p));
                if earlier.crossed_by(body, tolerance)
                    || holds(earlier, body)
                    || holds(body, earlier)
                {
                    return Err(MeshError::InclusionsOverlap);
                }
            }
        }

        let mut verts = outer.verts.clone();
        let mut tris = outer.tris.clone();
        let mut shells = outer.inclusion_shells.clone();
        let mut bodies = outer.inclusions.clone();
        for body in inclusions {
            shells.push(tris.len() as u32);
            let offset = verts.len() as u32;
            verts.extend_from_slice(&body.verts);
            // Wound the other way: the material is outside an inclusion's shell.
            tris.extend(
                body.tris
                    .iter()
                    .map(|t| [t[0] + offset, t[2] + offset, t[1] + offset]),
            );
            bodies.push(body.clone());
        }
        let mut mesh = Self::from_parts(verts, tris).ok_or(MeshError::Degenerate)?;
        mesh.repair_note.clone_from(&outer.repair_note);
        mesh.inclusion_volume_mm3 = bodies.iter().map(Self::volume).sum();
        mesh.inclusion_shells = shells;
        mesh.inclusions = bodies;
        Ok(mesh)
    }

    /// The mesh without inclusion `index` (0-based, in the order added); `None` when there is
    /// no such inclusion. Removing the last one gives back exactly the mesh it was added to.
    #[must_use]
    pub fn without_inclusion(&self, index: usize) -> Option<Self> {
        if index >= self.inclusions.len() {
            return None;
        }
        let mut outer = Self::from_parts(
            self.outer_vertices().to_vec(),
            self.outer_triangles().to_vec(),
        )?;
        outer.repair_note.clone_from(&self.repair_note);
        let rest: Vec<Self> = self
            .inclusions
            .iter()
            .enumerate()
            .filter(|&(k, _)| k != index)
            .map(|(_, body)| body.clone())
            .collect();
        Self::with_inclusions(&outer, &rest).ok()
    }

    /// This solid grown outward by `margin_mm`: every vertex moves so that each face it
    /// belongs to moves out by the margin (the mitre, cut off at four margins), the shape of
    /// an inclusion plus the room real inclusion boundaries need, since they are uncertain.
    ///
    /// The offset is exact for a convex solid whose vertices meet three faces, as a cube's
    /// do, and approximate for a smooth or a non-convex one. A margin that is not positive
    /// (or not finite) returns the solid unchanged. It is meant for a solid wound outward,
    /// without hollows or inclusions of its own.
    ///
    /// # Errors
    ///
    /// Returns [`MeshError`] when the grown solid is not a usable mesh (for example, a
    /// non-convex one that now crosses itself).
    pub fn grown(&self, margin_mm: f64) -> Result<Self, MeshError> {
        if !(margin_mm.is_finite() && margin_mm > 0.0) {
            return Ok(self.clone());
        }
        let n = self.verts.len();
        let mut normals = vec![DVec3::ZERO; n];
        let mut gram = vec![DMat3::ZERO; n];
        let mut count = vec![0.0_f64; n];
        for (t, tri) in self.tris.iter().enumerate() {
            let normal = self.planes[t].0;
            let outer = DMat3::from_cols(normal * normal.x, normal * normal.y, normal * normal.z);
            for &v in tri {
                let v = v as usize;
                normals[v] += normal;
                gram[v] += outer;
                count[v] += 1.0;
            }
        }
        let longest = MAX_MITRE * margin_mm;
        let verts: Vec<DVec3> = (0..n)
            .map(|v| {
                let ridge = OFFSET_RIDGE * count[v].max(1.0);
                let system = gram[v] + DMat3::from_diagonal(DVec3::splat(ridge));
                let mut shift = system.inverse() * (normals[v] * margin_mm);
                if !shift.is_finite() {
                    shift = normals[v].normalize_or_zero() * margin_mm;
                }
                if shift.length() > longest {
                    shift = shift.normalize_or_zero() * longest;
                }
                self.verts[v] + shift
            })
            .collect();
        Self::new(&verts, &self.tris)
    }
}
