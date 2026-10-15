# Non-convex `.obj` roughs in the rough planner — design

Status: implemented (2026-10); this document is the design it was built from, kept for
the reasoning. Companion to `concave-facets-plan.md`
(which covers concave *design* tiers). This document covers concave *roughs*.

## Goal

A notch, hollow or re-entrant part of a scanned rough is respected: no planned
stone extends into material that is not there. Rough volume (yield, fill %)
is the mesh volume. Convex roughs (box, cylinder, pebble, hull, and `.obj`
files whose mesh is convex) plan **bit-identically** to before: same layouts,
scores and saved-plan bytes.

## Current flow (verified)

- The parsers live in `apps/indicatrix-cut/src/mesh_io/` (`obj.rs`, `stl.rs`, `ply.rs`);
  `obj_import.rs` only drives the dialog. The first parse (`parse_obj_vertices`) read `v`
  lines only; `base_from_file`
  → `shape/hull.rs:236 import_hull` → `register_planes` stores planes and
  corners in the global `REGISTRY: Mutex<BTreeMap<u64, Arc<HullShape>>>` and
  returns a `Copy` `RoughBase::Hull { id, x, y, z }` keyed by a content hash.
- `base.rs:160` `Hull => hull::scaled(id, factor)` re-registers scaled planes.
- `plan.rs:61 path()`: only a `Block` with no cuts is `PlainBlock`, so a Hull
  base is always Shaped; mesh roughs never reach the block DP.
- Shaped stones are box-model: caliper box inside the piece's stone box and
  violated `non_box` planes, solved by `PartialSolver::solve` (`rows.rs:64`).
  Pieces are classified by `classify_box_into` (`clip.rs:184`) at clip.rs:392,
  tree.rs:60 (`StoneFitter::fit`, final authority), refine.rs:80, uniform.rs:94;
  app display in metrics/group.rs:317.
- Single-stone fit uses the rotated design hull via `SupportWorkspace`
  (`fit/support.rs`), call sites fit/mod.rs:266, :275, :439, :596.
- The app re-drives the stages across lanes in `run/stages.rs` and
  `run/drive.rs`; a core-only change is not enough.
- Volume: `RoughModel::measure` → `solid_of_canonical` (shape/mod.rs:399).
- View: `shape_worker.rs:131 mesh_of_planes` → `view/scene.rs ModelScene`.
- Save: `saved/convert.rs:745 rough_to_dto`; `dto.rs:13 CURRENT_SCHEMA_VERSION = 1`.

## Design

**The mesh is attached through the hull registry**, not a new `RoughModel`
field: `RoughBase` stays `Copy`, `RoughModel` unchanged, and a convex input
registers exactly as today (same id), so bit-identity is structural. Every new
branch is `if let Some(mesh)`.

### New module `rough_plan/shape/mesh.rs`

- `pub struct RoughMesh { verts: Vec<DVec3>, tris: Vec<[u32; 3]>, volume: f64, bvh: Vec<BvhNode> }`
  in the same bbox-at-origin rough frame as `register_planes`. BVH: median
  split, `total_cmp`, ties by triangle index, flat arrays.
- `pub enum MeshError { NoFaces, BadFace(usize), Open, NonManifold, Inconsistent, TooManyTriangles(usize), Degenerate }`.
- `RoughMesh::new(points, tris) -> Result<Self, MeshError>`: weld by quantised
  coordinates (BTreeMap); drop zero-area triangles; every undirected edge used
  exactly twice with opposite directions (no per-component flip, so cavities
  keep inward normals); flip all if signed volume < 0; require volume > 0;
  `MAX_MESH_TRIANGLES` (also on load); it is 200 000 in
  `crates/indicatrix-cut-core/src/rough_plan/shape/mesh.rs` (the first design said 50 000).
- Queries (mm, rough frame; `inset` = skin + allowance):
  - `contains_point(p)`: ray parity along 3 fixed irrational directions via
    BVH, majority vote.
  - `box_state(min, max, inset) -> BoxState { Clear, Air, Crossing }`:
    triangle-vs-AABB (Akenine-Möller) on the box inflated by `inset` minus
    `eps = EPS_FRACTION·scale` (`EPS_FRACTION = 1e-7`); with no hit, `contains_point(centre)` decides.
  - `box_blockers(min, max, inset, &mut Vec<u32>)`.
  - `polytope_blockers(planes_world: &[(DVec3, f64)], inset, &mut Vec<u32>)`:
    BVH-surviving triangles are Sutherland–Hodgman clipped by P's planes
    (shifted outward by `inset`, inward by eps); a blocker if anything survives.
  - `volume_within(cuts: &[(DVec3, f64)]) -> f64` (exact, with cap polygons).
  - `clipped_surface(cuts)` for the view.

**Exact predicate.** A convex P lies in M iff no triangle of M meets int(P)
and one interior point of P (its centroid) is inside M.

**Fit with a mesh — cutting-plane loop.** Solve the LP; if the pose has
blockers, add the plane of the least-violated blocking triangle (offset by
inset) plus every other blocker plane the stone's centre is on the material
side of (`n . c <= d - inset`) and the planes that face the same way within 5°
of the chosen one (one wall of a noisy scan), at most 64 rows per round, and
re-solve, at most 12 rounds, else `None`. A plane the centre satisfies cannot
make the LP infeasible together with the other centre-satisfied ones (the
centre at scale 0 is feasible for all of them), so a smooth cap's whole
contact set (a box in a round pebble touches at eight corners) goes in
together; a facing wall of a notch has the centre on its air side, so it is
never added except as the single least-violated choice, which is what keeps
the rows feasible. The one-wall rule alone could not clear eight independent
contacts in six rounds and planned no stone in any smooth scan. Re-solves are
done in the frame of the current centre (offsets minus `n . c`): every row the
centre satisfies then has a non-negative offset and the simplex needs no
artificial variables for it. This only shrinks the feasible set,
so every accepted fit is verified valid; it may be conservative near notches.
`PartialSolver::solve_in_mesh(region, extents, mesh, inset)` (rows.rs) and
`SupportWorkspace::evaluate_in_mesh` (support.rs).

**Plumbing** (`mesh: Option<&RoughMesh>`): `ShapedCtx { mesh: Option<Arc<RoughMesh>>, inset_mm }`
from new `RoughModel::mesh()` (→ `hull::mesh(id)`); `ClipRegion`,
`BuildClipParams`, `StoneFitter::new`, `LeafEval`, `grid_geometry`/`DesignEval`.
Old public signatures stay as wrappers passing `None`; new
`screen_designs_with`, `fit_shortlisted_with`, `fit_single_stones_with` take
`FitMesh { mesh, inset }`; the app's stages.rs/drive.rs call the `_with` forms.

**Registry and import.** `hull::import_mesh(points, tris) -> Result<(RoughBase, Option<String>), HullError>`
keeps the mesh only if `mesh.volume < hull_volume·(1 − 1e-6)`, where the hull
is that of the mesh's own welded vertices, and then builds the registered hull
and frame from those vertices (what a saved plan stores, so a stray `v` line
changes nothing after a save); otherwise it registers exactly as `import_hull`
of ALL points. A saved v2 mesh that import does not keep is refused on load. A kept mesh's id hash also covers the mesh.
`HullShape.mesh: Option<Arc<RoughMesh>>`; `hull::mesh(id)`; `hull::scaled`
scales the mesh. `obj_import.rs parse_obj_mesh(text)` parses `f` with
`v`, `v/vt`, `v//vn`, `v/vt/vn`, negative indices; a convex polygon is fanned, a
non-convex or skew one (mixed turn signs about its Newell normal) is ear-clipped
in its plane (failure: hull fallback with a note); on a
`MeshError` the import falls back to the hull with a visible note.

**Guarded acceptance sites.** clip.rs:392 `PieceWork::fill` (Air → EXTERIOR,
Crossing → PARTIAL, never the size-table INTERIOR shortcut), tree.rs:60
`StoneFitter::fit`, refine.rs:80 `leaf_val_at`, uniform.rs:94/146; fit/mod.rs
:275 (screening), :439 (only when entering basins), :596 (only on
improvement), :472–493 polish. Rejection = the existing "does not fit" path.

**Volume.** In `solid_of_canonical`, `volume_mm3 = mesh.volume_within(&cut_planes)`
when a mesh exists. bbox stays the hull's. Containment with user cuts = inside
M and inside all cut half-spaces (the LP already carries the cut planes).

**View.** `shape_worker.rs mesh_for(model)`: with a mesh, build the `SolidMesh`
from `clipped_surface` (triangles facet id 0, caps `hull_plane_count + k` so
cut picking still works; `piece_normals`/`edge_visible` set, edges visible on
sharp creases > 30° and cap boundaries).

**Persistence.** `RoughDto.mesh: Option<MeshDto { vertices, triangles }>`
(`serde(default, skip_serializing_if = "Option::is_none")`). A plan with a
mesh writes schema version 2; convex plans still write 1 (identical bytes). As built, the
saved-plan schema is version 3 (`apps/indicatrix-cut/src/gui/rough_plan/saved/dto.rs`,
`CURRENT_SCHEMA_VERSION`): inclusions are the version-3 addition, a plan with a mesh and no
inclusions still writes 2, and versions 1 to 3 are read.

**Determinism.** BTreeMap / sorted Vec only, `total_cmp`, fixed ray
directions, triangle order = file order.

## Risks

- Performance: per-piece BVH queries; mitigate with a per-grid cell-state
  prefix sum if needed. Triangle cap bounds BVH depth.
- Robustness: self-intersecting scans give wrong parity (documented
  limitation); near-coincident seams fall back to the hull with a note.
- Optimality: the cutting-plane rows are conservative near concavities.

The planner hands a stone's sawn piece to the editor as a convex plane-set preform
(`PreformShape::Planes`, 2026-10-12); the true mesh surface does not reach the editor.
