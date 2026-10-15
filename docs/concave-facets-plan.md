# Concave and fantasy facets: implementation plan

Status: proposal, reviewed; open questions provisionally decided in §11a.
Nothing in this document is implemented yet. Work packages: `docs/concave-facets-workpackages.md`.

This plan adds concave (curved-tool) tiers to Indicatrix end to end: authoring,
storage, cutting sheets, solid and diagram preview, path tracing (CPU and GPU),
metrics, the remote render protocol and the web app. It was put together from a
read-only survey of every crate and app in the workspace. File references are
`path:line` as of the commit this document was written against.

## 1. The notation standard we follow

Concave and fantasy cuts already have a standard way of being written in faceting
diagrams. We use it exactly as described. We do not invent a format of our own,
or a variation on one.

- A concave cut is **another tier**, written in the same table as flat tiers
  (P2, G4, C1, ...).
- It takes **two lines** instead of one:

  | Column            | Line 1 (as for any flat tier)   | Line 2 (concave only)                                           |
  |-------------------|---------------------------------|-----------------------------------------------------------------|
  | Tier              | tier label (e.g. `P8`)          | tool type: `CYL`, `CON`, `CIR`, `DSC`, `SPH`                    |
  | Angle             | zenith / altitude angle φ (deg) | tool / azimuthal angle θ (deg)                                  |
  | Indices           | index wheel positions           | X-, Y-, Z-displacement, each as a ratio of stone width          |
  | Instructions      | meet-point / cutting text       | tool details: diameter (ratio of stone width), cone or disc angle, reciprocating or plunge |

- Concave tiers come **at the end of the pavilion/girdle section** and **at the end
  of the crown section, before the table**. They are almost always cut after all
  flat faceting, and before transfer and tabling.

Every user-visible output that prints a schedule follows this layout: the text
cutting sheet, the HTML cutting sheet, the Edit-tab schedule and the catalogue
cutting table.

### 1a. Reference template (supplied by the user)

The standard's original fantasy-facet template (a two-column diagram sheet:
title, facet/size/design data, comments, four views down +Z, +X, −Y and −Z, then
a **Pavilion** and a **Crown** schedule) fixes the exact rendering of line 2.
Its schedule rows read:

```
P4    39.00°   01-23-25-47-49-71-73-95             Meet P1, P2, G1, G2, P3
CYL  +10.00°   X = 0.000, Y = 0.000, Z = 0.000     D/W = 0.400
...
C4    27.58°   04-20-28-44-52-68-76-92             Meet C2, C3
CYL    0.00°   X = 0.000, Y = 0.000, Z = 0.000     D/W = 0.600
T      0.00°   Table                               Meet C3, C4
```

Rules taken from it (binding for every output):

- The two lines of one concave tier form **one row band**: the zebra striping
  alternates per tier, not per line (P4 and its `CYL` line share a band).
- Line 2, column by column, produced by one formatter
  `ConcaveTier::second_line_fields() -> [String; 4]` in cut-core
  (`design/concave.rs`) so every output agrees:
  1. tool code: `CYL`, `CON`, `CIR`, `DSC`, `SPH`;
  2. θ with two decimals and a degree sign, explicit `+` for positive values,
     plain for zero: `+10.00°`, `0.00°`, `-15.00°`;
  3. `X = 0.000, Y = 0.000, Z = 0.000` (three decimals, signed when negative);
  4. `D/W = 0.400`, then for `CON`/`DSC` `, angle = 60.00°`, then the motion
     `, reciprocating` or `, plunge`. The template shows only `D/W` for a
     cylinder; the motion is always printed because the standard lists it as a
     tool detail (decision §11a; drop it if Arya says it is implied).
- The concave row comes last in its section; in the crown it sits directly
  above `T`.
- Out of scope for now (noted for later): the template's four-view diagram
  (we draw three panels), its Facet Data/Size Data blocks, and its unsigned
  per-section angles and dash-joined two-digit index lists. Flat-tier rows keep
  today's formatting so planar sheets stay byte-identical.

## 2. What the codebase assumes today

Everything downstream of the editor treats the stone as a **convex polyhedron**,
the intersection of half-spaces `n·x <= m`. The assumptions that matter:

| Area | Assumption | Where |
|---|---|---|
| Ray intersection | Slab method, giving one entry/exit interval per ray. Constant normal per facet. Three WGSL copies plus a bit-identical SIMD twin. | `crates/indicatrix/src/optics/raytracer/intersect.rs:10`, `src/simd/slab.rs`, `shaders/intersect_polyhedron.wgsl`, `shaders/transport_bounce/05_*.wgsl`, `shaders/transport_functions/08_*.wgsl` |
| Transport shortcuts | Re-entry after exit is dropped as impossible. NEE and frosted-exterior shortcuts assume "a convex stone can never re-intersect". | `refraction/exit_split.rs`, `scattering/hg.rs`, `scattering/frosted.rs`, WGSL `06_nee_sampling.wgsl`, `docs/physics.md:213` |
| Edge rounding | Uses the distance to neighbouring *planes*. | `intersect.rs` `shading_normal_near_edge`, plus 2 WGSL copies |
| Mesh and volume | Plane-arrangement vertex enumeration, angular ring sort, `V = Σ m·A/3`. | `crates/indicatrix/src/geometry/stone_metrics/mesh.rs:49`, `measure.rs` |
| Solid raster | Back-face culling used as visibility. Convex scanline fill. One flat normal and one `1/a` plane per facet. | `crates/indicatrix-solid/src/raster/render.rs:58,226` |
| Diagram | Panel visibility decided by the sign of a single facet normal. | `crates/indicatrix-solid/src/diagram2d/layout.rs:13-31` |
| Facet identity | Facet id equals the plane index. `FacetMap` mirrors the plane order, including dedup. | `crates/indicatrix-solid/src/facet_map/mod.rs`, `build.rs` |
| Solver | Every tier is a plane with one unknown mast. | `crates/indicatrix/src/geometry/meet_solver/` |
| Wire and scene | `planes: Vec<GpuFacetPlane>` is the whole geometry. | `indicatrix-net/src/scene/mod.rs`, `indicatrix-web-core/src/scene/mod.rs:311` |
| `.asc` | No curved-facet concept. An unknown line is dropped with a warning. | `crates/indicatrix-formats/src/asc/parse.rs:279` |

Several conventions shape the plan:

- **Planar output is pinned bit for bit.** This covers FNV pins in
  `indicatrix-solid/src/preview/tests.rs`,
  `apps/indicatrix-cut/tests/solid_identity_pins.rs`, the golden `to_bits()`
  tables in `crates/indicatrix/tests/raytracer_tests/golden_regression.rs`, and
  the SIMD twins checked against `glam =0.34.1`.
- **Determinism contract** (`indicatrix-cut-core/src/lib.rs:62`): no
  `HashMap`/`HashSet` in any decision path, and no clocks or thread-count
  dependence.
- **`ConstraintTier` is deliberately not widened.** It has about 130 struct-literal
  sites. New per-tier data goes on `Design` instead, as `tier_targets`,
  `cheater_offsets_deg` and `tier_notes` do (`design/mod.rs:129-153`).
- **Native `.indicatrix` (TOML)** is forward compatible. It uses
  `#[serde(default, skip_serializing_if)]` and a `flatten`ed `unknown` table at
  every level.
- **No async runtime.** The workspace uses std threads and channels on desktop
  and `wasm_bindgen_futures` plus Web Workers on the web. Nothing in this plan
  needs an executor. If one is ever needed, it is `smol`, not `tokio`.
- Clippy `pedantic` and `nursery` are on. Exceptions use `#[expect(..., reason)]`.
  Rust 2024, MSRV 1.95.

## 3. Guiding decisions

1. **Additive, never perturbing.** A design with no concave tiers must produce
   byte-identical output everywhere it does today: every pin, golden and hash.
   Concave support is a parallel path that is only entered when concave tiers
   exist. This is the acceptance gate for every phase.
2. **Concave tiers live beside flat tiers, not inside them.** `Design` gains
   `concave_tiers: Vec<ConcaveTier>` (§4). The meet solver, `planes_from_solved`,
   retarget, orbit tools and manipulation handles never see a concave tier, and
   Optimize never varies one (it does evaluate the full geometry, §9.1), so no
   planar algorithm has to learn about curves. The two existing edits that touch
   *every* tier's indices (`RemapIndices`, §4.3) are the one place where the
   two lists must be handled together.
3. **Constructive solid geometry over convex pieces.** The stone becomes
   `P \ (T₁ ∪ … ∪ Tₙ)`, where `P` is the existing convex polyhedron and each `Tᵢ`
   is a convex tool volume (one per tier *placement*, i.e. per index). The tool
   bodies we model are convex: a ball, a capped cylinder, a cone frustum and a
   bicone (disc). The sweep of a convex body along a segment (reciprocating) is
   its Minkowski sum with the segment, which is again convex. `CIR` is a short
   cylinder (§11a). That gives:
   - **Ray tracing:** exact interval arithmetic. The slab interval of `P` minus
     the analytic entry/exit intervals of each `Tᵢ` (§5.2 lists the closed
     forms per tool and motion).
   - **Mesh and preview:** convex clipping only. Every polygon stays a convex
     ring, so the existing ring fill is reused. The tessellated tool is an
     approximation of the exact tool the tracer sees; §5.4 bounds the gap.
   - **Non-convexity is confined to the union.** `P \ ⋃Tᵢ` is in general not
     convex, so nothing downstream may assume one entry/exit interval, one
     visible side per normal sign, or "no re-intersection after exit". §5.3 and
     §8 list every such site found.
4. **Geometry is resolved once, then passed around in absolute units.** Authoring
   parameters (ratios of stone width, φ, index, θ) are resolved after the flat
   solve into a `ToolPrimitive`: an axis, a position, a radius profile and a sweep
   vector. Renderers, wire messages and workers only ever see resolved
   primitives, just as they only see planes today, never tiers.
5. **`.asc` stays flat.** GemCAD would read an `a` record as a flat plane and
   silently cut the wrong stone. The native `.indicatrix` file is the lossless
   carrier (§6).
6. **Desktop first; the web app is out of scope.** `apps/indicatrix-web` and
   `apps/indicatrix-web-compute` get no UI or protocol work. `indicatrix-web-core`
   keeps compiling and its tests keep passing, receiving only the changes forced
   by shared types; its `PROTOCOL_VERSION` (8) is not bumped because its wire
   bytes do not change. A concave design loaded on the web renders and previews
   as its flat stone `P`, which is wrong but honest once the desktop is the only
   place concave tiers can be authored. Every "web" item in §8 and §9 is
   therefore deferred.
7. **The GPU backend falls back to the CPU tracer whenever `tools` is
   non-empty.** The development container has no GPU adapter, so the GPU
   equivalence harness cannot run; WGSL work is deferred to a hardware session
   (§7a). The gate is one predicate beside the material's `gpu_supported()`
   check (`optics/materials/optics.rs:77`), and `HybridSplit::cpu_only` already
   exists (`renderer/gpu/hybrid.rs:76`). No WGSL file changes in this round, so
   the existing naga `shader_validation_tests.rs` stays green unchanged.

## 4. Data model

### 4.1 Authoring types (`crates/indicatrix-cut-core/src/design/concave.rs`, new)

```rust
/// A tool's three-letter code in the faceting-diagram standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConcaveTool {
    /// `CYL`
    Cylinder,
    /// `CON`
    Cone,
    /// `CIR`
    Circle,
    /// `DSC`
    Disc,
    /// `SPH`
    Sphere,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolMotion {
    Reciprocating,
    Plunge,
}

/// One concave tier exactly as the two-line standard writes it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveTier {
    // Line 1: identical meaning to a flat tier.
    pub name: String,
    /// φ, signed like `ConstraintTier::angle_deg` (negative = pavilion).
    pub angle_deg: f64,
    pub indices: Vec<f64>,
    /// Free-text meet/cutting instruction, printed verbatim.
    pub instructions: String,
    // Line 2: the tool.
    pub tool: ConcaveTool,
    /// θ, tool azimuth in degrees.
    pub tool_azimuth_deg: f64,
    /// X, Y, Z displacement as ratios of stone width.
    pub displacement: [f64; 3],
    /// Tool diameter as a ratio of stone width.
    pub diameter_ratio: f64,
    /// Cone or disc included angle. Required for CON and DSC, rejected otherwise.
    pub tool_angle_deg: Option<f64>,
    pub motion: ToolMotion,
}
```

- `ConcaveTool` has `code() -> &'static str`, `FromStr` (case-insensitive) for
  the three-letter codes with its own error type, and `Display` printing the
  code. It derives `Ord` and `Hash` so it can key `BTreeMap`s. `ToolMotion`
  derives the same set. Neither is `#[non_exhaustive]`: they are matched
  exhaustively inside the workspace and are not wire types (the wire carries
  `ToolPrimitive`); a sixth code is a change to the standard and should break
  every `match`.
- Angles and ratios stay plain `f64` with `_deg` / `_ratio` suffixes. The
  workspace has no `Degrees`/`Ratio` newtypes (`ConstraintTier::angle_deg`,
  `cheater_offsets_deg`), so introducing one here would be inconsistent.
- `ConcaveTier::validate(&self, gear_teeth: u32) -> Result<(), ConcaveTierError>`:
  - every value must be finite;
  - `0 < |φ| < 90` (no table or girdle concave tiers in v1, §11a);
  - indices must be in range and non-empty (a tier with no placement cuts
    nothing; `facet_plane_count` treats an empty flat index list as one plane,
    and we must not copy that quirk);
  - `diameter_ratio > 0`;
  - the tool-angle rule above, with `0 < angle < 180` for CON/DSC;
  - `tool_azimuth_deg` finite; it is normalised to `[0, 360)` at resolution,
    not at validation, so the stored value is what the author typed.
  
  `ConcaveTierError` names the offending field, in the existing
  "name the offending value" style, and is `#[non_exhaustive]` like the other
  cut-core error enums that are matched by the editor only through
  `tier_form_error_field`'s prefix classification.
- `ConcaveTier` is not `Copy` (it owns strings), derives `Clone, Debug,
  PartialEq` only; no `Eq`/`Hash` because of the `f64` fields, matching
  `ConstraintTier`.
- Name uniqueness: a concave tier may not share a name with a flat tier, since
  `MeetNamed`, labels, hover text and the `.asc` footnotes address tiers by
  name.

### 4.2 Storage on `Design`

- Add `concave_tiers: Vec<ConcaveTier>` and `concave_tier_ids: Vec<TierId>`.
  The ids are drawn from the **same** `next_tier_id` counter, so notes, selection
  and diffs can address either kind. `TierId` stays a plain `u64` newtype; a
  `TierRef { Flat(usize), Concave(usize) }` index type is added for APIs that
  take a position rather than an id.
- Update every `Design { .. }` literal in `design/construct.rs` (`new`,
  `fresh_from_spec`, `fresh`, `from_asc_schedule` all start with an empty list),
  the manual `PartialEq` (`design/mod.rs:330`; compares `concave_tiers`,
  excludes `concave_tier_ids` like `tier_ids`) and the docs. `templates.rs`
  builders return flat tiers only and need no change.
- Per-tier maps keyed by flat position (`cheater_offsets_deg`, `tier_notes`,
  `tier_targets`) are **not** extended to concave tiers: a concave tier has no
  mast, so a cheater offset or target is meaningless for it. Notes for concave
  tiers go in `ConcaveTier::instructions` (the standard's own free-text column).
- Section order is a derived view, not stored state. `Design::cutting_order()
  -> impl Iterator<Item = TierRef>` yields:
  1. flat pavilion and girdle tiers, in stored order;
  2. concave pavilion-side tiers, in `concave_tiers` order;
  3. flat crown tiers except the table;
  4. concave crown-side tiers, in `concave_tiers` order;
  5. the table.
  
  A concave tier's block comes from the sign of φ, using
  `indicatrix::geometry::plane::tier_is_crown_side` (which reads the sign of
  zero, so `−0` stays pavilion-side). The standard fixes only *where the group
  goes*; the order **within** a concave group is user-controlled through
  `MoveConcaveTier`. Whether a concave tier may sit on the girdle (φ = ±90) is
  Q2.
- "Table" above means the flat tier(s) the existing code already classifies as
  table; if a design has no table tier, group 4 is simply last.

### 4.3 Undo/redo (`edit/edit_type.rs`, `edit/apply.rs`)

- New `Edit` variants follow the existing `Add/Remove/Modify/Move` and `Restore*`
  idiom:
  - `AddConcaveTier { index, tier }`, inverse `RemoveConcaveTier { index }`;
  - `RemoveConcaveTier { index }`, whose inverse is
    `Batch[AddConcaveTier, RestoreConcaveTierId]` (the existing `RestoreTierId`
    writes `tier_ids[index]`, so a concave twin is needed rather than a reuse);
  - `ModifyConcaveTier { index, tier }`, inverse `ModifyConcaveTier` with the
    previous tier;
  - `MoveConcaveTier { from, to }`, inverse with the positions swapped;
  - `RestoreConcaveIndices { tiers: Vec<(usize, Vec<f64>)> }`, the concave half
    of `RemapIndices`'s inverse (see below).
- Each new variant needs:
  - an inverse in `apply_edit`, validated before any mutation (the "without
    modifying `self` on error" contract);
  - a `describe` arm;
  - a `resolve_after_edit` arm. These edits are mast-inert, so the arm returns the
    previous solve unchanged. (The exhaustive `match` there is what makes the
    compiler list every site.)
  - a `TierIndexMap::of` arm in `indicatrix-editor/src/session/selection.rs`.
    Concave edits never shift flat positions, so the arm returns the identity
    map; the selection model itself gains a concave slot (§9.1).
  - a `History` coalescing rule: consecutive `ModifyConcaveTier` on the same
    index coalesce like `ModifyTier` does.
  - coverage in the seeded property test `tests/edit_roundtrip.rs`, which must
    generate every variant.
- **Existing variants that must learn about concave tiers:**
  - `RemapIndices` (`edit/apply.rs:608`) remaps `tiers[*].indices` and
    `detached` only. It must also remap `concave_tiers[*].indices`, and its
    inverse becomes `Batch[RestoreIndices, RestoreConcaveIndices]` so the
    verbatim-snapshot undo contract is kept. Without this, a gear change
    silently moves every concave placement.
  - `SetSchedule` only rewrites `meta` (gear, symmetry, mirror); index values are
    untouched for flat tiers, so concave tiers need nothing extra. Symmetry
    *display* (which placements are implied by symmetry) is a view-model
    question and follows whatever the flat rows do.
  - `RemoveTier`/`MoveTier` of a flat tier do not affect concave tiers, which
    are stored separately and reference nothing by position.
- Validation: a flat tier's `MeetNamed` may not name a concave tier. Concave tiers
  have no mast, so the meet solver cannot use them; `missing_anchor_blocks` and
  the `Mismatch` error path are untouched.
- `Edit` is public and not `#[non_exhaustive]` (`edit/edit_type.rs:22`); it is
  matched exhaustively in `resolve_after_edit`, `describe` and
  `TierIndexMap::of`. Keep it that way so the compiler finds every arm.
- `History::apply_coalescing` keys on `(key, timestamp)` with a caller-supplied
  `Duration`; the concave coalescing key must be distinct from the flat one for
  the same position (`ModifyTier{0}` and `ModifyConcaveTier{0}` must never merge).

### 4.4 Resolving to geometry (`design/export/concave.rs`, new)

`Design::concave_tools_from_solved(&self, solved) -> Result<Vec<ToolPrimitive>, ConcaveResolveError>`
works as follows:

1. Build the flat solid and measure the stone width `W`, using
   `SolidMetrics::width_axis` from `measure_solid` on the flat planes (after
   cheater offsets and preform planes, i.e. exactly the planes the renderer
   sees). `W` is the width of the *flat* stone; concave cuts never add material,
   so this is stable under edits to concave tiers (Q3 confirms the definition).
2. For each concave tier and each index, build the dop frame `(u, v, n)` from φ
   and the index. This is the same normal math as `StandardGemCuts`, kept
   bit-compatible with `manipulate/frame.rs` and `facet_map/build.rs`.
   `n` is the would-be flat facet normal at (φ, index); `u` points down the
   facet toward the girdle, `v = n × u` (§11a, `concave_frame.rs`).
   - The tool axis is `cos θ·u + sin θ·v` (θ = 0 meridional, θ = 90 along the
     girdle); for a plunged `CON` the axis is `−n`.
   - The tool centre is `C₀ + W·(X·u + Y·v − Z·n)`, where `C₀` is the support
     point of `P` in direction `n` (the tool's first contact) and `Z > 0` is
     depth into the stone.
   - The radius is `W·diameter/2`; `CIR` gets length `CIR_RIM_WIDTH_RATIO·D`.
   - The sweep half-stroke is along the tool axis for `Reciprocating`
     (`RECIP_HALF_STROKE_RATIO·W`; `SPH_RECIP_HALF_STROKE_RATIO·D` for a ball)
     and zero for `Plunge`.
3. Emit one `ToolPrimitive` (§5.1) per placement, in tier order then index order,
   so tool index `k` is a stable, deterministic function of the design.

Resolution is `f64` throughout and converts to `f32` once at the end, like
`to_halfspace_f64`. The `(tier, placement) -> tool index` map is returned next
to the primitives as `Vec<(usize, usize)>` for `FacetMap` and hover text, so the
GPU struct does not need to carry it.

**The frame convention is provisional (§11a, Q1–Q2).** Everything
convention-dependent lives in `concave_frame.rs`, read only by this function,
and the native file records `CONCAVE_FRAME_VERSION`.

`Design::concave_tools_through_tier(solved, through: TierRef)` is the concave
twin of `try_planes_through_tier`: the tier-cutoff slider walks
`cutting_order()`, so the preview of "the stone after step k" includes exactly
the concave tiers that precede `k` in cutting order.

## 5. Geometry kernel (`crates/indicatrix`)

### 5.1 Resolved primitive (`src/geometry/tool.rs`, new)

```rust
/// A convex tool volume subtracted from the stone, in model units.
///
/// One primitive per placement. Its facet id is `planes.len() + k` where `k`
/// is its position in the slice, so the struct carries no facet bookkeeping.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolPrimitive {
    /// `ToolKind as u32`: 0 ball, 1 capped cylinder, 2 cone frustum, 3 bicone.
    pub kind: u32,
    /// `ToolSweep as u32`: 0 none, 1 along `axis`, 2 across `axis` (see §5.2).
    pub sweep_kind: u32,
    pub _pad: [u32; 2],
    /// Centre `xyz`; `w` = radius at the centre (ball, cylinder, bicone rim).
    pub origin: [f32; 4],
    /// Unit axis `xyz`; `w` = half-length along the axis (0 for a ball).
    pub axis: [f32; 4],
    /// `x` = radius at `−half_length`, `y` = radius at `+half_length` (frustum
    /// and bicone; equal to `origin.w` for a cylinder), `z` = sweep
    /// half-stroke length, `w` = 0.
    pub profile: [f32; 4],
    /// Unit sweep direction `xyz` (only read when `sweep_kind == 2`), `w` = 0.
    pub sweep_dir: [f32; 4],
}
```

- 80 bytes, 16-byte aligned, every field a `u32`/`f32`: valid as `bytemuck::Pod`
  and layout-identical to the WGSL
  `struct ToolPrimitive { kind: u32, sweep_kind: u32, pad: vec2<u32>, origin: vec4<f32>, axis: vec4<f32>, profile: vec4<f32>, sweep_dir: vec4<f32> }`
  (WGSL: `vec4<f32>` aligns to 16, struct size rounds to a multiple of 16; the
  `u32` quartet fills the first 16 bytes exactly). The layout echo in the GPU
  harness (Tier 0) pins this, as it does for `FacetPlane`.
- `kind` and `sweep_kind` are `u32`, not Rust enums, because `Pod` forbids enums;
  `ToolKind` and `ToolSweep` are `#[repr(u32)]` enums with `TryFrom<u32>` and
  `validate_scene` rejects unknown values. `PartialEq` is derived (plain
  `f32` comparison, like `GpuFacetPlane`); no `Eq`/`Hash` because of the floats.
  The cache hash (§5.5) uses `bytemuck::bytes_of`, so `−0.0` and `+0.0` hash
  differently, exactly as `hash_planes` already behaves for planes.
- `MAX_TOOL_PRIMITIVES = 128` is one `pub const` in this module; every validator
  (§7) and the WGSL loop bound refer to it. Each index is one primitive, so this
  is 128 placements, not 128 tiers (§11a, Q9).
- Add `StoneGeometry<'a> { planes: &'a [GpuFacetPlane], tools: &'a [ToolPrimitive] }`
  with `#[derive(Clone, Copy, Debug)]` and `#[must_use] fn is_convex(&self) -> bool`
  (`tools.is_empty()`), which is the single gate every "convex shortcut" tests.
- Every existing `&[GpuFacetPlane]` API keeps its signature. New `*_geom`
  entry points take `StoneGeometry`, and the plane-only functions stay as thin
  wrappers. `HitRecord` gains nothing in v1: the transport loop keeps its
  `inside_gem` flag (§5.3).

### 5.2 Ray–stone intersection (CPU)

`intersect_stone(ray, geom) -> Option<HitRecord>`:

1. `[a, b]` comes from the existing slab loop, with the code unchanged. Note
   the slab loop already decides "entry vs exit" by `t_near > 1e-4`
   (`intersect.rs:56-75`), so a ray whose origin is inside `P` gets `[a, b]`
   with `a <= 1e-4 < b`.
2. For each tool, compute the analytic interval `[cᵢ, dᵢ]` of the ray inside the
   (convex) tool, or none. Per shape:
   - **ball** (`SPH`, plunge): one quadratic.
   - **capped cylinder** (`CYL`): quadratic in the plane normal to the axis,
     intersected with the slab of the two caps.
   - **cone frustum** (`CON`): one quadratic for the nappe (clamped to the
     half-length so the mirror nappe is excluded) intersected with the cap slab.
     The included angle from line 2 gives the half-angle `α`; the two radii in
     `profile` are derived from `α` and the half-length at resolution time.
   - **bicone** (`DSC`, a disc wheel with a bevelled rim): two frusta sharing the
     rim plane; the interval is the union of the two, which is one interval
     because the bicone is convex.
   - **sweep along the axis** (`sweep_kind == 1`): for a cylinder or frustum the
     swept body is the same shape with `half_length + stroke`; for a ball it is a
     capsule = ball ∪ ball ∪ cylinder, whose ray interval is
     `[min cᵢ, max dᵢ]` over the three pieces, valid because the union is convex.
   - **sweep across the axis** (`sweep_kind == 2`, a Minkowski sum with a segment
     perpendicular to the axis): every cross-section circle of radius `r(z)`
     becomes a stadium of the same radius, so the body is
     `T(C − s) ∪ T(C + s) ∪ B`, where `B` is a prism (cylinder: a box) or a
     wedge (frustum/bicone: two planar faces tangent to both copies plus the
     caps). Again `[min cᵢ, max dᵢ]` over convex pieces whose union is convex.
     This is closed-form for every shape in the table, so **no GJK or
     iterative fallback is needed**; one is mentioned only in case Q2 names a
     motion that is not a translation (e.g. a rocking arc).
   - `CIR` is a short cylinder (the rim of a flat wheel; §11a), so it needs no
     primitive of its own.
   
   Near-tangent rays (discriminant within `1e-7` of zero, the same guard class
   as the slab loop's `denom`) report no interval; the boundary they miss is a
   measure-zero set and the choice is deterministic.
3. Inside set = `[a, b] \ ⋃[cᵢ, dᵢ]`, computed by sorting the ≤ 2n+2 boundaries
   (a fixed-size stack array; `MAX_TOOL_PRIMITIVES` bounds it) and walking them
   with a depth counter. The hit is the smallest boundary `t > 1e-4` of the
   inside set. The normal is the plane normal, or the **negated** outward normal
   of the tool at the hit point (the tool's outward normal points into the stone
   material; the stone's outward normal points into the cavity). For the union
   pieces above, the normal comes from whichever piece owns the boundary `t`.
   A boundary can be an entry or an exit; v1 does not report which, matching
   `HitRecord` today, and the transport loop keeps toggling `inside_gem`
   (§5.3). Ties (a plane and a tool boundary at the same `t`) take the plane,
   deterministically. Self-intersection: a ray leaving a tool surface has that
   tool's own boundary at `t ≈ 0`; the `t > 1e-4` floor handles it for the same
   reason it handles planes, but a quadratic is worse conditioned than a plane,
   so the case bank must include grazing restarts on a cylinder of radius
   `0.05·W` and the floor may need to scale with the radius (measured in
   Phase 3, not guessed).
4. `facet_idx`:
   - for a plane: the plane index, as today;
   - for a tool: `planes.len() + k` for the `k`-th primitive.
   
   `facet_finish_for` (`transport/inner.rs:56`) already returns `Polished` for
   any index past the slice, so the CPU needs no `facet_finishes` change; the
   WGSL already guards the same way
   (`hit.facet_idx < arrayLength(&facet_finishes)` in
   `transport_bounce/06_nee_sampling.wgsl:155` and `08_bounce_step.wgsl:437-443`),
   so tool facets read as polished on both. `girdle_facet_finishes` (`geometry/girdle.rs:76`) runs
   over planes only and never frosts a tool facet, which is correct: a girdle
   groove is still polished by its tool.

When `tools.is_empty()`, step 2 is skipped and the result is bit-identical to
`intersect_polyhedron`, including the `planes.is_empty()` sentinel hit at
`t = 1e30` that the furnace kernel relies on (`intersect.rs:303`,
`gpu/furnace_check.rs:15`). A unit test asserts this across the existing case
bank. The SIMD `slab_scan` twin is reused for step 1 untouched. Tools are few
(≤ `MAX_TOOL_PRIMITIVES`) and scanned scalar. A SIMD twin for tools is deferred
until profiling asks for one; when it comes it needs the same bit-identical
Scalar/AVX2/AVX-512 treatment as `simd/slab.rs`.

### 5.3 Convexity shortcuts in transport

These are gated on `!geom.is_convex()`. With no tools, the old code runs, so the
goldens hold. Every call to `intersect_polyhedron_soa` in the crate was
listed; the table is the complete set as of this commit
(`rg intersect_polyhedron_soa crates/indicatrix/src`). To keep it so,
`intersect_polyhedron_soa` becomes `pub(crate)`-restricted to the `intersect`
module once Phase 5 lands and every caller goes through `intersect_stone`.

| Site | Today | Concave behaviour |
|---|---|---|
| Bounce loop (`transport/inner.rs:397`) | `intersect_polyhedron_soa`, then toggles `inside_gem` per crossing. A miss after an exit means "escaped". | `intersect_stone`. The toggle stays correct because every boundary of the inside set is a crossing. A hit after an exit is now a legitimate re-entry (the ray crossed the cavity), not an error. |
| `try_split_exit_channel` (`refraction/exit_split.rs:107`) | Probes the exit direction; any hit means "re-enters the convex gem" and the channel is dropped (documented energy loss). | On a concave stone the probe hits the far wall of the cavity for a large fraction of exits, so the truncation is no longer a corner case. v1 keeps the truncation **but records the dropped energy** in the furnace test; v2 continues the split channel through the re-entry (a bounded mini-loop, cap 4 crossings) if the furnace shows a loss above the gate. This is the single biggest energy risk in the table. |
| HG-scatter NEE (`scattering/hg.rs:456`) | Shadow probe takes the first exit facet and applies one Fresnel transmittance. | Loop through crossings (cap 4), multiplying transmittance at each exit and ignoring entries into the cavity's far wall only if the probe then exits again; above the cap, drop the sample. |
| Frosted-exterior NEE (`scattering/frosted.rs:48-60, 215-250`) | Deposits directly: "a convex solid's own surface point moving outward cannot re-intersect". | Cast a real shadow probe against `StoneGeometry`; deposit only on a miss. |
| Environment NEE pdf (`environment/rig.rs:114`) | Pure pdf lookup, no geometry. | Unchanged. There is **no** separate "light NEE" site: NEE happens only at HG-scatter and frosted-exterior events. (Earlier drafts listed one; removed.) |
| `inside_gem` flag | Toggled per crossing. | Kept. In debug builds assert it against a point classification of `origin + dir·(t − 1e-4)` (inside `P` and outside every tool). |
| `shading_normal_near_edge` (`intersect.rs:145-205`, 2 WGSL copies) | Rounds using signed distances to neighbouring planes. | No rounding for a hit on a tool surface, or for a plane hit whose nearest neighbour is a tool, in v1. Documented in `docs/physics.md` next to the "no shadow ray is needed for a convex stone" sentence (`physics.md:213`), which is rewritten. |
| Denoiser guide capture (`capture_primary_hit`, `inner.rs:401`) | Stores first-hit depth, normal, facet index. | Facet ids above `planes.len()` must fit the guide buffer's index encoding (check `renderer/buffers/camera.rs:140`); the denoise guide pass clones `planes` and must clone `tools` too. |
| Metrics tracers: `color/metrics/ray_trace.rs:116`, `classify.rs:79`, `scintillation.rs:71`, tilt sweeps `sweep.rs`/`profile.rs` | All call `intersect_polyhedron_soa`. | All switch to `intersect_stone`; see §5.5. |
| `pgo_train/stages.rs`, `gpu_equivalence_harness` | Plane scenes only. | Unchanged in v1; a concave fixture is added to the harness in Phase 7. |

A white-furnace energy-conservation test on a concave stone becomes the main
correctness check for this table, because any missed re-entry shows up as lost
energy. Its gate is the same bound the existing furnace tests use
(`tests/raytracer_tests/white_furnace_energy_conservation.rs`), minus the
measured exit-split truncation, which is reported separately so that the v1/v2
decision above is data-driven.

### 5.4 Mesh, volume and measures (`stone_metrics`)

`build_solid_mesh_geom(&StoneGeometry) -> SolidStatus` keeps `build_solid_mesh`
for `P` unchanged. When there are tools, it then:

1. **Tessellates each tool as a convex polytope** `T̃ᵢ` with a fixed segment
   count `N` around the axis (default `N = 48`; `N = 24` on `wasm32`, see §8)
   and an icosphere of level 2 for balls. Deterministic, no randomness, `f64`.
   The polygon radius is the **equal-area** radius
   `r̃ = r·sqrt(2π / (N·sin(2π/N)))`, so a cylinder's cross-section area, and
   hence its removed volume, is exact; the surface then lies alternately
   outside the true tool (vertices, by `r̃ − r`) and inside it (edge midpoints,
   by `r − r̃·cos(π/N)`): +0.14 % / −0.07 % of `r` at `N = 48`, +0.57 % /
   −0.29 % at `N = 24`. (An inscribed polygon instead loses
   `1 − (N/2π)·sin(2π/N)` of the area: 0.29 % at `N = 48`, 1.1 % at `N = 24`,
   which is why equal-area is used.) For the ball the same scaling is applied to
   the icosphere so its volume matches `4πr³/3` to the icosphere's own
   constant (computed once in a test, not hard-coded).
2. **Clips the faces of `P`.** For each convex ring `A` of `P` and each tool
   polytope `T̃` with half-spaces `h₁…hₘ`, the ring is split into the convex
   pieces `A ∩ h₁ ∩ … ∩ hₖ₋₁ ∩ ¬hₖ`, `k = 1…m`, each by Sutherland–Hodgman
   (which is exact for convex clip regions). These pieces are disjoint and
   their union is `A ∖ T̃`. A tool whose bounding sphere does not reach the ring
   is skipped, which also keeps `f64` round-off away from untouched rings. A
   ring that is wholly outside every tool is **kept as the original ring
   object**, so the planar pieces of a concave stone are the same `DVec3`s the
   planar mesh produces. Applied tool by tool: each piece from tool `i` is
   then split by tool `i+1`.
3. **Clips the tool faces.** Each face of `T̃ᵢ` is clipped to the inside of `P`
   (one convex clip), then split against every other `T̃ⱼ` by the same
   decomposition. The face is reversed so its winding points out of the stone
   (into the cavity).
4. **Watertightness.** Step 2 cuts a ring of `P` along a plane of `T̃`, while
   step 3 cuts a face of `T̃` along the planes of `P`; the shared edge is
   computed twice, from different clip orders, and its endpoints differ by
   round-off. Also, the `N` tessellation planes meet a flat facet in a
   polyline, whose vertices appear on the flat side as piece corners and on the
   tool side as face corners, so **T-junctions are inherent** to the
   decomposition. Consequences and handling:
   - Volume by the divergence formula depends only on the polygon geometry, so
     T-junctions cost nothing and the round-off gap is `O(ε·area)`; the test
     in §10 pins the bound.
   - The raster can show hairline pixel gaps along such seams. The fix is to
     snap all vertices of the clipped set to a `1e-9·W` grid **after** clipping
     (deterministic, applied only on the concave path) and to rely on the
     existing fill rule; §8 measures whether any gap survives.
   - `GemPolyhedron::from_planes` (Euler check) is not run on the concave mesh.
5. **Records edge provenance** for every ring segment: the facet id of the
   piece across the segment, found by a `BTreeMap` keyed on snapped endpoint
   bits (no `HashMap`, per the determinism contract). An edge is drawn only when
   the two sides belong to different facet ids. This hides the decomposition's
   internal split edges and the tool's tessellation seams; the facet-vs-tool
   boundary (the polyline from step 4) is drawn, as it should be.
6. **Attaches analytic normals** for tool pieces, evaluated at each piece's
   centroid from the exact tool (not the polytope). Plane pieces keep their flat
   normal. Per-vertex normals are not needed in v1 (§8 shades flat per piece).

`SolidMesh` gains `piece_normals: Option<Vec<DVec3>>` (one per ring, `None`
on the planar path) and `edge_visible: Option<Vec<Vec<bool>>>`. The existing
fields keep their meaning; `rings` is still "one convex ring per entry", only
now several entries may share a `facet_id`. `normals` keeps its "never
averaged" promise. Both new fields are `None` on the planar path, so every
existing consumer and pin is untouched.

Volume uses the same divergence formula `Σ m·A/3` over all pieces, valid for
any closed oriented surface. `yield_metrics` (carat) therefore drops by the
removed volume. Width, length, the caliper hull and the `width_axis` still come
from `P`'s outline: a convex-hull measure of `P ∖ ⋃T` can only shrink, and
only when a tool removes a hull vertex, which is a manufacturability warning
(§9.1) rather than a legitimate design. Bump `SOLID_EXTENTS_VERSION` and
`SOLID_HULL_VERSION` in `indicatrix-vault` only if their stored values change
for concave designs (volume yes, hull no), i.e. `SOLID_EXTENTS_VERSION` 2 → 3,
`SOLID_HULL_VERSION` unchanged.

**Tessellation versus the exact tracer.** The preview and the volume use `T̃`;
the tracer and metrics use the exact `T`. The two surfaces differ by the
≤ 0.15 % of `r` above (`N = 48`). This is visible only as a different silhouette on a
`SolidMesh`-based picture, never in a rendered image, and is the same class
of mismatch the icosphere already is for every smooth preview in the trade.
Documented, not fixed.

### 5.5 Metrics

`trace_wavelength` (`ray_trace.rs:116`), `classify.rs:79`, `scintillation.rs:71`
and the tilt sweeps (`sweep.rs`, `profile.rs`, which call the same tracer at
each pose) switch to `intersect_stone`. Notes:

- The windowing test `n_out.y < -0.05` uses the hit normal, which is now
  position-dependent. This is still correct.
- The fire gate compares `facet_idx` between the two ends of the spectral
  fan; tool facet ids are stable per primitive, so a fan leaving through the
  same groove counts as one facet, which is the intended meaning.
- `FanGeometry` origins sit at 2.5 half-widths from `measure_model_width`,
  which is measured on `P` (§5.4), so they stay outside the stone.
- The 18×18 fan is a fixed sample set; a concave stone with narrow grooves
  will be under-sampled in exactly the way a flat stone with narrow facets is.
  No change in v1, noted for the metrics documentation.

`hash_planes` (`render_setup/plane_hash.rs:16`, `DefaultHasher` over plane
bits) becomes `hash_geometry(geom)`, which feeds the tool bytes after the plane
bytes **only when tools are present**, so a planar key is bit-for-bit the
current one. `color/metrics/cache.rs:140` and `fan.rs:46` take the new key.
`DefaultHasher` is `SipHash-1-3` with fixed keys when built with `new()`, which
is why the current code may use it despite the determinism contract; the new
code uses it the same way and does not introduce `RandomState`.

## 6. Formats and storage

### 6.1 Native `.indicatrix` is the lossless carrier

- Add `ConcaveTierTable` in `crates/indicatrix-formats/src/native/schema.rs`, and
  `DesignFile.concave_tiers: Vec<ConcaveTierTable>` with
  `#[serde(default, skip_serializing_if = "Vec::is_empty")]`, plus `unknown`
  flatten.
  - Tool codes are stored as the standard strings `"CYL"` and so on.
- **Version bump is decision Q5.** The recommendation is to bump `DESIGN_VERSION`
  1 → 2 **only when concave tiers are present**:
  - an old build refuses the file with `UnsupportedVersion` instead of silently
    rendering the stone without its concave cuts;
  - files without concave tiers stay version 1 and byte-identical.
  - The writer sets `version = 2` iff `concave_tiers` is non-empty; the reader
    accepts 1 and 2. `codec.rs`'s `TierMissingGeometry` check gets a concave
    twin (a `ConcaveTierTable` with a missing tool code or diameter is an
    error, not a default). `MAX_DESIGN_TIERS` (10 000) bounds the sum of both
    lists.
- Converters in cut-core: `native/convert.rs`, `design_file.rs`, `load.rs` and
  `save.rs`, including draft saves. `concave_tier_ids` are regenerated on load
  like `tier_ids` are, never stored.
- `web-core`'s `Solve` carries the design as native TOML
  (`solve::design_to_toml`), so a concave design still parses there; the web
  worker ignores `concave_tiers` and returns planes only (§3.6).

### 6.2 `.asc`, `.gcs` and `.gem` export

- These formats cannot represent concave cuts, so concave tiers are **omitted**
  from the tier records.
- The export emits a `ManufacturabilityWarning`-style notice, and the UI says so
  before saving. There is precedent for this in the web app's degenerate-design
  refusal.
- The standard two-line text of each concave tier is appended as `F` footnote
  lines, **two `F` records per tier** (the writer rejects a line break inside
  one footnote, `asc/write.rs:35 FootnoteContainsNewline`), each prefixed with
  the tier label so the pair is readable on its own, e.g.
  `F P8  -62.00  3 11 19 ...  cut to depth` / `F CYL  +90.00°  X = 0.000, Y = 0.120, Z = 0.050  D/W = 0.250, reciprocating`
  (line-2 fields from `ConcaveTier::second_line_fields()`, §1a).
  The writer emits all `F` lines after the last tier (`write.rs:235`), so the
  footnotes cannot sit at the end of their section; the label carries the
  position. GemCAD's handling of `F` lines (kept on load, shown on the printed
  diagram) is **not verified in this workspace** and is Q8.
- **Round-trip hazard.** `.asc` footnotes are imported into
  `Design.meta.footnotes` (`design/construct.rs:194`) and written back on every
  export (`native/save.rs:564`). A design exported with concave footnotes,
  re-imported, and exported again would duplicate them. Rule: the exporter
  writes `meta.footnotes` first, then the generated concave lines, and strips
  from `meta.footnotes` any line that is byte-equal to a line it is about to
  generate. This is idempotent and never touches a user's own footnotes.
- `save_paired`'s byte-for-byte preservation of the original `.asc` text
  applies only when the design is semantically unchanged; adding or editing a
  concave tier changes the footnote text and so counts as a change. The
  `asc_sha256` fingerprint that gates overlay restore is computed over the
  bytes actually written, so it stays consistent.
- The parser leaves those footnotes as footnotes. Re-importing them as tiers is
  out of scope (Q6).
- `crates/indicatrix-formats/src/asc/` needs no grammar change. `.gem` and
  `.gcs` writers have no footnote channel, so the notice is the only trace.

### 6.3 Vault (`crates/indicatrix-vault`)

- Add a `migrate_concave_columns` migration at the end of the chain, gated on
  `column_exists`. It adds:
  - `diagram_details.concave_tiers INTEGER NOT NULL DEFAULT 0`;
  - `angle_settings.tool TEXT` and `angle_settings.tool_line TEXT`, holding the
    formatted second line.
- Add the same columns to the fresh-table SQL in `base_schema.rs`.
- Search: `RangeFilter.has_concave: Option<bool>`, plus a clause in
  `search/predicate.rs`.
- `facet_plane_count` stays flat-only. The `facets_count` TEXT column keeps its
  `"55+6"` form **unchanged**, because `model/facets.rs` parses it into
  `facets`/`girdle_facets` and a third component would break every stored row.
  The concave placement count goes in its own column,
  `diagram_details.concave_facets INTEGER NOT NULL DEFAULT 0` (next to
  `concave_tiers` above). How the library *displays* the pair is Q7; storage
  does not depend on the answer.
- `DesignSummary.version` (SHA-256 over summary fields,
  `serve/library/version_hash.rs`) length-prefixes every field, so appending a
  concave field changes the stamp of **every** design, planar ones included.
  That is acceptable (it is a revision stamp that makes clients refetch, not a
  pin), but it must be stated in the protocol-19 README entry, and any test that
  pins a summary hash is updated in the same change.
- The native file is already attached whole, so no geometry columns are needed.

## 7. Wire protocols

| Protocol | Change | Version |
|---|---|---|
| `indicatrix-net` (postcard) | Append `tools: Vec<ToolPrimitive>` to `SceneState` after `surface_glare`. Append `concave_tiers` to `DesignSummary` and update the `version_hash.rs` field order. Append `LibraryRequest::FetchDesignNative` so remote clients can load concave designs. | `PROTOCOL_VERSION` 18 → 19; update the README history and the pin test |
| Worker `scene.json` | `#[serde(default)]` on `tools`. | n/a |
| `validate_scene` (`validate/mod.rs:81`) | Validates tools as well: every float finite, `origin.w > 0`, `profile.x/y >= 0` with at least one `> 0`, `axis.xyz` within `1e-4` of unit length, `kind`/`sweep_kind` known, `_pad` and the `w` lanes zero (so the hash is canonical), and `tools.len() <= MAX_TOOL_PRIMITIVES` (128). "Empty planes" stays an error. | n/a |
| `render_core` (`build_plane_soa`, `GpuSceneRef`, `girdle_facet_finishes`, denoise guide buffers, `serve/tilt.rs`) | Each takes a `StoneGeometry`; the denoise guide pass clones `tools` next to `planes`. | n/a |
| `indicatrix-web-core` | **Out of scope (§3.6).** No `tools` field; `SceneSpec`, `MetricsParams`, `TiltParams` and `SolveOutcome` stay plane-only, so the web worker renders `P`. Only compile fixes forced by shared types (e.g. a `Design` literal, a renamed cut-core function). | `PROTOCOL_VERSION` 8 unchanged; `wire_format_is_pinned` unchanged |

The tools are resolved primitives, so payloads stay tiny: 80 bytes per placement,
at most 5 KiB, against the 1 MiB control-frame cap. Postcard is not
self-describing, so the appended field is a hard break for old peers, which is
the accepted pre-release policy; `SOURCE_HASH` changes anyway once the tracer
changes, and the version bump makes the pairing refusal readable. `ToolPrimitive`
is `serde`-derived under the `serde` feature exactly like `GpuFacetPlane`, so
`SceneState` and `scene.json` need no custom codec.

Until Phase 6 lands, the desktop refuses to submit a remote render for a design
with concave tiers (a clear message, not a silently flat stone), and the worker
would in any case reject the unknown field.

### 7a. GPU scope in this round

- `GpuBackend` routes a scene to the GPU only when the material's
  `gpu_supported()` **and** `geom.is_convex()` hold; otherwise the frame is
  `HybridSplit::cpu_only(total_spp)`. The gate is unit-tested without an adapter
  (it is a pure function of the scene), and `HybridStats::forced_cpu_only`
  (`hybrid.rs:29`, "unreachable today") becomes reachable and is asserted in
  that test.
- No `.wgsl` file changes. The three WGSL intersection copies, the NEE probes,
  the two `shading_normal_near_edge` copies, the `tools` binding, the layout
  echo and the Tier-2 `tool_check.rs` are one later work package that must run
  on a machine with an adapter (`cargo test -p indicatrix --features gpu` and
  the `gpu_equivalence_harness` example, regenerating
  `docs/gpu_harness_last_run.json`). Until then the fallback is the shipped
  behaviour and `docs/gpu.md` says so in one line.
- `SOURCE_HASH` still changes (CPU sources change), so a remote pairing refusal
  between mismatched builds is already readable without a WGSL change.

## 8. Preview: solid, diagram and picking (`crates/indicatrix-solid`)

- **Raster** (`raster/render.rs`): both passes build a *per-facet* normal table
  (`facet_normal[facet_id]`, first ring wins, `render.rs:34-41,140-145`). On a
  concave mesh several rings share a facet id with different normals, so that
  table is wrong for tool facets. When `piece_normals` is `Some`, the passes
  read the normal **per ring** from it instead; on the planar path the table is
  untouched, so the pins hold.
  - **Back-face culling stays on for every ring, tool pieces included.** For a
    closed, consistently outward-oriented surface the cull plus the strict `<`
    depth test is exactly right, convex or not: the near wall of a cavity faces
    away and is culled, the far wall faces the camera and wins the depth test
    against the flat facets behind it. Disabling the cull for tool pieces (an
    earlier draft) would only draw back walls that the depth test then
    discards. The only requirement is the correct winding from §5.4 step 3,
    which a test checks by asserting every piece's area-weighted normal has a
    positive dot with its stored `piece_normals` entry.
  - Depth: one affine `1/a` plane per ring is exact, because every piece is a
    planar convex polygon. `fill_convex_polygon` already fits one per call.
  - Shading: **flat per piece** from the piece's analytic normal (§5.4 step 6),
    through the existing `shade`. `fill_convex_polygon` takes one color
    (`render.rs:226-232`), so Gouraud would need a new fill routine; at `N = 48`
    a flat-shaded groove shows 48 bands around the axis, which is acceptable
    for a cutter's preview. Per-vertex interpolation is a follow-up, not v1.
  - `pick` is written with `facet_id + 1` as today; tool facet ids are above
    the plane count.
  - Seams: after the §5.4 vertex snap, render the concave fixture at every
    camera in `preview/tests.rs` and assert no pixel in the pick buffer is
    unset while its 4-neighbours all belong to the stone ("hairline gap"
    detector). If gaps survive, widen the fill's edge inclusion by half a pixel
    on the concave path only.
- **Edges:** honour `edge_visible`. The Bresenham edge pass keeps
  `EDGE_DEPTH_BIAS`; split edges that are flagged invisible are skipped
  before projection, so the cost is lower, not higher.
- **Labels:** `overlays.rs:129-168` stamps at the projected-ring bbox midpoint
  and requires a pick match there. For a facet with several pieces, use the
  area-weighted centroid of its largest *visible* piece, still requiring the
  pick match; the planar path is unchanged.
- **Diagram** (`diagram2d`): `crown_visible`/`pavilion_visible`/`profile_visible`
  (`layout.rs:13-31`) take one normal. For a non-convex stone a normal-sign test
  is not a visibility test: a groove cut into a steep pavilion facet has wall
  pieces whose normal points *upward* (undercut), which a crown panel must not
  show. Rule for v1:
  - flat pieces: the existing predicates, with the flat normal (unchanged);
  - tool pieces: shown in the panel of their tier's **block** (sign of φ; profile
    uses `profile_visible` with the piece normal) and, within that panel, only
    pieces whose normal passes the panel predicate. This hides undercut walls
    from the wrong panel and is exact for every groove that does not undercut.
  - The diagram projection already carries a depth coordinate
    (`layout.rs:65-81`); if the diagram rasteriser depth-tests rings, the block
    filter is belt and braces; if it only paints in ring order, the block
    filter is what makes the picture right. Settle in Phase 4.
  Concave tier labels use the tier name, as flat ones do, and the 5×7 font
  is untouched.
- **`FacetMap`:** add `FacetInfo.kind: FacetKind { Flat, Concave { tier, placement } }`
  and an id range above the plane count, built from the `(tier, placement)`
  list that `concave_tools_from_solved` returns (§4.4), so `build.rs`'s
  dedup-mirroring logic is not touched. Hover text shows the tool code, θ and
  diameter. `overlay_flags` covers the new ids.
- **Caches:** the `MeshCache` key (FNV-1a over plane `f32` bits) extends to
  the tool bytes **only when tools are present**, so a planar key is today's.
  `RedrawRequest::Reproject` and `RenderedFrame` carry an owning
  `StoneGeometryBuf { planes: Vec<GpuFacetPlane>, tools: Vec<ToolPrimitive> }`
  in place of `planes`; `WorkerMemory`'s `last_closed` fallback keeps working
  because the concave mesh is a `SolidStatus::Closed` like any other.
  `simplify_ring` assumes a convex ring, which every piece still is.
- **`FrameGeometry`** (centroids and corners) gets one centroid per concave
  facet, from the largest piece, so `manipulate/*` can at least hit-test it
  (no handles in v1).
- **Performance budget** (desktop; the web is out of scope but the same
  single-threaded code runs there later): a solid frame must stay under 16 ms
  for 8 tools × 12 placements (96 primitives, within `MAX_TOOL_PRIMITIVES`).
  Estimate: 96 primitives × ~50 faces
  clipped against ~100 half-spaces of `P`, plus ~100 rings of `P` each split by
  the few primitives whose bounding sphere reaches them, is on the order of
  10⁶ clip steps, i.e. several ms in `f64` on wasm. The bounding-sphere
  rejection in §5.4 step 2 is what keeps it there. Benchmark on the concave
  fixture (same harness as the 4-8 ms Solid / 8-23 ms Diagram numbers in the
  crate docs), and drop `N` to 24 on `wasm32` if the budget is missed.

## 9. Editor and UI

### 9.1 GUI-free layer (`crates/indicatrix-editor`)

- `loading/concave_form.rs`:
  - `ConcaveTierFormFields` (string fields) and `parse_concave_tier_form`;
  - error prefixes registered in `tier_form_error_field` (`loading/tier_form/mod.rs:493`);
  - index parsing reused from `parse_index_list`.
- `tier_save.rs`: `concave_tier_save_edit`, in the GUI-free crate so a later
  web port does not duplicate it (the web app currently duplicates the flat
  tier save in `apps/indicatrix-web/src/editor/inspector/form.rs`; that stays
  as is).
- `session/tier_ops.rs`: duplicate, remove and move for concave tiers.
  `selection.rs` gains a concave selection slot (`Option<TierRef>` or a
  `TierId`), and `TierIndexMap::of` returns the identity for concave edits.
- View models:
  - `TierRow` gains `kind` and `tool_line: String`;
  - `CuttingRow` gains `second_line: Option<[String; 4]>`, one string per column.
  - `rows.rs` and `yield_report.rs:497` emit rows in `cutting_order()`.
- `retarget/mod.rs` (`retarget_scope` currently skips the girdle only),
  `optimize_view.rs` and `manipulate/target.rs` exclude concave tiers from the
  set of *variables*. They have no critical-angle margin and no drag handles in
  v1. Optimize's objective, however, is evaluated on the full geometry
  (`free_tier_indices` runs solve + mesh + metrics per evaluation): the tools are
  re-resolved from the candidate flat solve each time, so the optimum is for the
  stone the cutter will actually cut. This roughly doubles mesh cost per
  evaluation; acceptable, measured in Phase 5.
- **Manufacturability** (`manufacturability/warning.rs`): new
  `ManufacturabilityWarning` variants, all computed from the resolved tools and
  the flat mesh:
  - `ToolMissesStone { tier, placement }`: `T ∩ P` is empty (no piece in §5.4
    step 3 survives), or the removed volume is below `1e-6·V`;
  - `ToolBreaksThrough { tier, placement, facet }`: the tool surface meets a
    facet that is not the tier's own facet nor one of its neighbours (a cut
    through the girdle or into the opposite side);
  - `ToolRemovesMeet { tier, placement, vertex }`: a vertex of `P`, which is a
    meet point, lies inside `T`;
  - `ToolsOverlap { a, b }`: two tools intersect each other inside `P` (allowed
    geometrically, but almost always a mistake);
  - `ConcaveSliver { facet }`: a flat facet is left with a piece whose width is
    under the existing sliver floor;
  - `ToolRemovesHullVertex`: see §5.4 (width and hull would change).
  The `.asc`/`.gem`/`.gcs` omission notice (§6.2) is a warning of the same
  type so the UI needs one channel.
- **Snapshot and compare** (`snapshot.rs`, cut-core `cutting_sheet/diff.rs`
  `diff_tiers`): `diff_tiers` compares flat tiers by position and reports
  `TierDelta`s. Add `diff_concave_tiers` with its own delta (tool code, θ,
  displacement, diameter, angle, motion) and append its rows to
  `compare_rows`, flagged with the `CYL`/… badge. Without this a compare
  between two snapshots that differ only in a concave tier shows "no change".
- **Orbit** (`orbit/`): the mirror descriptor applies to flat tiers. Mirroring a
  concave placement needs θ ↦ −θ and X ↦ −X under the stone's mirror plane,
  which depends on Q1; until then orbit tools skip concave tiers and say so.
- **Cheater offsets** (`apply_cheater_offsets`, keyed by flat tier position):
  not applicable to concave tiers; the form hides the field.
- **Yield** (`yield_metrics`, `fit.rs:39`): carat comes from the concave volume
  (§5.4); the rough-fit hull stays `P`'s.

### 9.2 Cutting sheets

- **Text** (`cut-core/src/cutting_sheet/sheet.rs`):
  - hoist the column widths into consts;
  - emit the second line under the same columns.
  
  Example (layout only; the values are made up):

  ```
   12. P8               angle  -62.00 deg  indices [3 11 19 27 35 43 51 59 67 75 83 91]  meet: cut to depth
                        CYL               +90.00°           X = 0.000, Y = 0.120, Z = 0.050   D/W = 0.250, reciprocating
  ```

- **HTML** (`indicatrix-editor/src/cut_sheet/html.rs`):
  - a second `<tr class="tool">` per concave tier;
  - each pair wrapped in its own `<tbody>`, so `break-inside: avoid` keeps the
    two lines together;
  - shown only when the design has concave tiers, following the
    `TierTableColumns` optional-column pattern;
  - mm values next to the ratios when `girdle_diameter_mm` is set.
- `CutSheetRow` gains `concave: Option<ConcaveRowInfo>`, and the test literals in
  `cutting_sheet/tests.rs` are updated. The existing tests assert line counts
  of `to_text`; a concave row adds exactly one line, so the planar count
  assertions stay and a new one pins `flat_rows + 2·concave_rows`.
- `sequence` numbers one per tier in `cutting_order()`; the second line carries
  no number, as in the standard.
- Ratios print with three decimals and θ with the same `{:>7.2}` as φ, so the
  two lines align by construction (the `consts` above are the only widths).

### 9.3 Desktop (`apps/indicatrix-cut`); web deferred (§3.6)

- **Tier table:** a `CON`/`CYL`/… badge in the C/P/G column. The angle, mast,
  margin and orbit cells are read-only or blank for concave rows. Rows are
  ordered by `cutting_order()`.
- **Adding a concave tier:**
  - an "Add concave tier" entry in `tier_table_toolbar.slint` and
    `editor_command_bar.slint`;
  - it opens the inspector in concave mode.
- **Inspector form:** a Flat/Concave segmented switch at the top of
  `tier_form_tab.slint`. Concave mode shows:
  - line 1: name, φ, indices, instructions;
  - line 2: tool `ComboBox` with the five codes, θ, X/Y/Z, diameter, tool angle
    (enabled for CON/DSC only), and Reciprocating/Plunge.
  
  It reuses the existing row layout, the error-border pattern and the
  `loaded_*_snapshot` dirty tracking.
- **Saving:** a new `EditorModel.save_concave_tier(ConcaveFormData)` callback that
  takes a struct. `save_tier`'s positional six-argument signature is not widened.
  `EditorTierItem` must stay `Send`, so no `ModelRc` fields.
- **Schedule tab and catalogue `cutting_table.slint`:** two-line rows.
- **Guide:** a new `GuideAllow` group so the worked-example guide can lock the
  controls.
- **Rough planner:** uses the flat hull. That is conservative, because concave
  cuts only remove material. It shows a "concave cuts not modelled" note,
  replacing the current "cannot model a concave notch" limitation text in
  `docs/manual/15-planning-a-rough.md:741` with a precise one.
- **Manual:** a new chapter in `apps/indicatrix-cut/docs/manual/` on concave
  tiers and the notation.

## 10. Phases

Each phase is independently shippable. Each must pass
`cargo clippy --workspace --all-targets` and `cargo test --workspace` with **every
existing pin unchanged**.

| # | Phase | Delivers | Main crates |
|---|---|---|---|
| 0 | Spec | Answers to §11, as an ADR in `docs/`. Two or three reference concave designs from published diagrams as fixtures. | — |
| 1 | Model and files | `ConcaveTier`, `Edit` variants, native round trip, `.asc` omission and footnotes, text and HTML two-line cut sheet. | formats, cut-core, editor |
| 2 | Authoring UI | Form, table, schedule (desktop only). The preview shows the flat stone with a "concave cuts not yet previewed" banner. | editor, cut |
| 3 | Kernel | `ToolPrimitive`, `concave_tools_from_solved`, `intersect_stone`, `build_solid_mesh_geom`, volume. | indicatrix, cut-core |
| 4 | Preview | Raster, diagram, picking, `FacetMap`, caches. The banner is removed. | solid, editor, apps |
| 5 | CPU tracer and metrics | Transport gating (§5.3), metrics, furnace test, `docs/physics.md`. | indicatrix |
| 6 | Wire and storage | net v19, worker validation, vault migration and filter. web-core: compile-only sweep, no version bump. | net, worker, vault, (web-core) |
| 7 | GPU (deferred to a hardware session, §7a) | `tools` storage buffer (new binding; `planes_shared[128]` untouched), intersection in the three WGSL copies (`intersect_polyhedron.wgsl`, `transport_bounce/05_*.wgsl` `intersect_ray`, `transport_functions/08_*.wgsl` `tf_intersect_ray`) plus the NEE probes in `06_nee_sampling.wgsl` and the two `shading_normal_near_edge` copies, layout echo, a `tool_check` Tier-2 harness case, regenerated `gpu_harness_last_run.json`. The CPU and GPU tool code must agree to the same ULP budget as `polyhedron_check.rs`; a quadratic's discriminant is the sensitive step, so both sides use the same `mul_add` form. | indicatrix (`gpu`) |

Phases 1–2 already give cutters real value: they can author, save and print
standard concave instructions. Phases 3–5 make the stone look right. Phase 6 must
land with or before remote rendering of concave designs is enabled. Until then,
the desktop renders concave designs locally only, and the remote path refuses
them with a clear message. Phase 7 can trail; the GPU backend already falls back
to CPU per frame, and it must also fall back whenever `tools` is non-empty until
then.

Dependencies: Phase 1 needs only the *shape* of the notation (§1) and can store
raw numbers before Q1–Q3 are answered; Phase 3 is blocked on Q1–Q3 because
`concave_tools_from_solved` bakes them in; Phase 4 needs Phase 3's mesh; Phase 5
needs Phase 3's tracer; Phases 6 and 7 need Phase 3's `ToolPrimitive` to be
frozen (a layout change after Phase 6 is another protocol bump). Phase 2 can run
in parallel with 3. Phases 1–2 should not ship a release before Phase 3 settles
Q1, or saved files could carry numbers whose meaning later changes; if they do
ship, the native file records the convention it was written under (a
`concave_frame = "v0"` key, Q1).

### Test plan per phase

Test names follow the repo's long descriptive `snake_case` sentences; the
planar-unchanged tests are the acceptance gate of every phase and are listed
first.

- **1** (`indicatrix-formats/src/native/tests`, `cut-core/tests`,
  `cut-core/src/cutting_sheet/tests.rs`, `indicatrix-editor/src/loading`):
  - `native_round_trip_without_concave_tiers_is_byte_identical_to_version_1`
    (the existing fixtures, compared as bytes) and
    `native_round_trip_with_concave_tiers_preserves_every_field`.
  - `asc_export_omits_concave_tiers_and_appends_two_footnotes_per_tier`, and
    `asc_export_then_import_then_export_does_not_duplicate_concave_footnotes`
    (the idempotence rule in §6.2).
  - `tests/edit_roundtrip.rs`: the seeded generator emits every new variant;
    the existing `apply(inverse(apply(e))) == design` property covers them, and
    a new assertion checks `RemapIndices` undo restores concave indices
    verbatim.
  - `cutting_sheet_text_for_concave_fixture_matches_expected_string` (string
    compare) and `cutting_sheet_text_for_planar_fixture_is_unchanged` (pin).
  - Hostile form inputs: NaN, missing tool angle on CON, tool angle on SPH,
    unknown code, empty index list, `|φ| > 90`; each must map through
    `tier_form_error_field` to the right field.
- **3** (`crates/indicatrix/src/geometry/tool.rs` tests,
  `src/geometry/stone_metrics/tests`, `src/optics/raytracer/intersect.rs`
  tests):
  - `intersect_stone_with_no_tools_is_bit_identical_to_intersect_polyhedron`:
    `to_bits()` equality of `t`, normal and `facet_idx` over the existing case
    bank, including `planes.is_empty()`.
  - `tool_interval_agrees_with_point_classification_on_seeded_rays`: for each
    `ToolKind × sweep_kind`, 10 000 rays from a seeded xorshift; sample 64
    points per ray and assert that every point farther than `1e-4·r` from any
    boundary is classified identically by the interval set and by the
    analytic inside test. Grazing restarts from a cylinder of radius `0.05·W`
    are a named sub-case.
  - Volume, with `N = 48`, equal-area radius:
    - `tessellated_cylinder_groove_through_slab_matches_closed_form`: a slab
      minus a full-depth through-cylinder has volume `V_slab − π r² h`, and the
      equal-area polygon makes this exact up to clipping round-off:
      relative error `< 1e-9`.
    - `tessellated_partial_cylinder_groove_is_within_polygon_bound`: a groove
      of depth `0.6·r` (the axis outside the slab, so the chord does not pass
      through a vertex) is not polygon-exact; its removed area differs from the
      circular segment by at most the band between the inscribed and
      circumscribed radii times the arc length, which the test evaluates
      analytically for `N = 48` (about 0.5 % of the segment area) and asserts.
    - `cube_minus_interior_ball_matches_icosphere_volume_constant`: the removed
      volume equals the scaled icosphere's own analytic volume to `1e-9`
      relative, and that constant is within 1 % of `4πr³/3` (the icosphere
      scale in §5.4 step 1 is chosen so this holds; the test prints the
      constant so the number is recorded, not guessed).
  - `concave_mesh_is_closed`: Σ of signed edge vectors per facet-id boundary
    loop is zero to `1e-9·W` after snapping (the T-junction check), and the
    divergence volume is independent of the tool application order
    (`concave_mesh_volume_is_independent_of_tool_order`, determinism).
  - `concave_mesh_piece_windings_point_out_of_the_stone` (§8 cull precondition).
- **4** (`indicatrix-solid/src/preview/tests.rs`,
  `apps/indicatrix-cut/tests/solid_identity_pins.rs`):
  - New FNV pins for one concave fixture, identical on web and desktop.
  - The existing pins are untouched.
  - `pick_buffer_ids_on_concave_fixture_resolve_to_tier_and_placement`.
  - The hairline-gap detector from §8.
  - A `wasm32` timing note in the crate docs, measured, not estimated.
- **5** (`tests/raytracer_tests/white_furnace_energy_conservation.rs`,
  `golden_regression.rs`):
  `concave_fixture_white_furnace_energy_conservation_holds` with the existing
  furnace bound, reporting the exit-split truncation separately; new `to_bits`
  goldens for the concave fixture only; the existing golden tables are
  unchanged.
- **6:**
  - postcard round trips for `SceneState` with and without tools;
  - `wire_format_is_pinned` updated; `PROTOCOL_VERSION` pin tests updated;
  - the vault "fresh DB has X" and "old DB migrates idempotently" tests, plus
    `has_concave_filter_matches_only_concave_designs`.
- **7:**
  - layout echo for `ToolPrimitive` (Tier 0);
  - `tool_check.rs`, a Tier-2 ULP check of tool intersection beside
    `polyhedron_check.rs`;
  - a Tier-3 statistical image comparison on the concave fixture;
  - regenerated `docs/gpu_harness_last_run.json`.

## 11. Open questions (to settle in Phase 0)

The notation fixes *what is written*, but not every geometric convention needed
to compute the cut. These need a confirmed answer from the standard's source
material or from an experienced concave cutter. Ideally ask Arya Akhavan
directly, since they supplied the notation.

Where common concave-faceting practice suggests an answer it is noted as a
*hypothesis to confirm*, not as the answer: the standard is the authority.

1. **Frame.**
   - What are X, Y and Z measured relative to: the stone centre, or the facet
     contact point? In the dop frame `(u, v, n)` at φ/index, or in stone
     coordinates? Hypothesis: the dop frame at (φ, index), since the stone is
     on the dop at that setting while the tool is applied, and Z along the
     facet normal is then the plunge depth.
   - Where is θ = 0, and in which direction does it increase? Hypothesis: θ is
     measured in the facet plane from the direction of the index (the
     "down-slope" line of the facet), so a vertical groove on a crown break
     is θ = 0 and a horizontal one is θ = 90.
   - If Phases 1–2 ship before this is settled, the native file must carry a
     `concave_frame` version key so stored numbers keep their meaning.
2. **Tool geometry.**
   - What exactly is `CIR` (a ring or toroidal lap? a circular-edge wheel? the
     plain rim of a flat disc, i.e. the `DSC` with 0° edge angle?) compared
     with `DSC`? Until answered, `CIR` has no `ToolKind` and is rejected at
     validation with "not yet modelled".
   - For CON and DSC, is the "angle" the included angle or the half-angle?
   - What is the reciprocation direction and stroke? Hypothesis from
     cylinder-mandrel concave machines: the rotating mandrel (or the stone)
     reciprocates **along the mandrel axis**, so for `CYL` the stroke only
     lengthens the cylinder and its exact length is immaterial (any value past
     the stone is the same cut); for `SPH`/`CON` on a mandrel the same axis
     applies. Is a stroke across the axis ever used, and if so is its length
     written anywhere? The notation lists only "reciprocating vs plunge", so
     the stroke length may be implicitly "past the stone".
   - Is "plunge" a single tool placement with the depth given by Z, i.e. the
     tool is pressed in without lateral motion (a dimple for `SPH`, a conical
     pit for `CON`, a straight groove for `CYL` pressed sideways)?
   - May a concave tier be a girdle tier (φ = ±90), e.g. a scalloped girdle,
     and is it then listed with the pavilion/girdle group?
3. **Width.** Is the "stone width" the girdle width of the flat design before
   concave cuts, as assumed in §4.4? And for a non-round outline, is it the
   `width_axis` (minor) extent or the length?
4. **Line 1 instructions.** Is a concave tier's meet column always free text
   (e.g. "to depth"), or are there standard keywords worth parsing?
5. **File compatibility.** Should old builds refuse concave `.indicatrix` files
   (version bump; recommended) or load them as flat-only?
6. **`.asc` import.** Should footnotes written in the two-line notation be parsed
   back into concave tiers on import, or is that out of scope?
7. **Facet count notation** in the library: is there a convention for counting
   concave facets alongside flat ones? (Storage is settled in §6.3; only the
   display string depends on this.)
8. **GemCAD and `F` lines.** Does GemCAD keep `F` footnotes on load and print
   them on the diagram? §6.2 assumes yes; it is not verified here.
9. **Placement ceiling.** `MAX_TOOL_PRIMITIVES = 64` placements covers a
   12-fold design with up to five concave tiers. Are there published designs
   with more (e.g. 96 placements across 8 tiers)? The ceiling is one constant,
   but the WGSL loop bound and the wasm budget (§8) depend on it.
10. **Order among concave tiers.** The standard fixes the group position; does
    it also prescribe an order inside the group (e.g. by tool, by depth), or
    is the author's listed order the cutting order, as §4.2 assumes?

## 11a. Provisional decisions (pending confirmation by Arya Akhavan)

Every question in §11 is decided here so that implementation can start. Each
decision is the simplest reading consistent with common concave-faceting
practice, and each is **reversible**: every convention-dependent constant and
rule lives in one file, `crates/indicatrix-cut-core/src/design/export/concave_frame.rs`,
exported as `pub mod concave_frame` with a `pub const CONCAVE_FRAME_VERSION: &str = "v0"`.
`concave_tools_from_solved` is the only function that reads that module, and the
native file records `concave_frame = "v0"` next to its concave tiers, so a
later change is one file plus a reader shim keyed on that string.

| Q | Decision | Rationale (one line) |
|---|---|---|
| Q1 frame origin | X, Y, Z are measured from the **contact point** `C₀`: the support point of the flat stone `P` in direction `n`, the would-be facet normal at (φ, index); ties broken by the lexicographically smallest vertex. | A concave tool is applied with the dop set to φ/index exactly as for a flat facet, and first touches the stone at that point; it needs no mast or meet. |
| Q1 frame axes | `n` = `StandardGemCuts` normal at (φ, index); `u` = unit projection of the girdle-ward direction (`−sign(φ)·Y`) onto the facet plane (points "down the slope" toward the girdle); `v = n × u`. `Z > 0` moves the tool **into** the stone (`C = C₀ + W·(X·u + Y·v − Z·n)`). | Up/down the facet and along the girdle are the two directions a cutter sees on the stone; "depth is positive" matches how a plunge is spoken of. |
| Q1 θ | θ measured in the facet plane from `u` toward `v`; the tool **axis** is `cos θ·u + sin θ·v`. θ = 0 is a meridional groove, θ = 90 runs parallel to the girdle. | Cylinder-mandrel grooves are described by the direction of the groove, which is the mandrel axis. |
| Q1 φ range | `0 < |φ| < 90`; table and girdle concave tiers are rejected at validation in v1. | `u` is undefined at φ = 0 (no slope) and ambiguous at ±90 (the plane contains `Y`); the standard places concave tiers before the table anyway. |
| Q2 `CIR` | Modelled as `ToolKind::Cylinder` with the **rim width** `w = CIR_RIM_WIDTH_RATIO·D` (`0.1`) as its full length, i.e. the rim of a flat wheel. `DSC` is the bicone with the given rim angle; `CYL` is the long cylinder. | A "circle" tool in fantasy cutting is the plain rim of a disc wheel; a `DSC` is the same wheel with a bevelled (V) rim. |
| Q2 cone/disc angle | **Included** angle. The frustum/bicone half-angle is `angle/2`. | Lap and wheel angles are quoted as included angles in cutting practice; a half-angle would be an unusual reading of "angle of the cone". |
| Q2 reciprocation | **Along the tool axis** for `CYL`, `CON`, `CIR`, `DSC`, with a half-stroke of `RECIP_HALF_STROKE_RATIO·W` (`2.0`, i.e. "past the stone"); `SPH` reciprocates along `cos θ·u + sin θ·v` with half-stroke `SPH_RECIP_HALF_STROKE_RATIO·D` (`0.5`). `sweep_kind == 2` (across the axis) is implemented in the kernel but never produced by `v0`. | On mandrel-type concave machines the mandrel (or stone) reciprocates along the mandrel axis; the stroke length is never written, so it is "enough to clear the stone". |
| Q2 plunge | `sweep_kind == 0`; the tool is pressed in along `−n` to depth `Z` with no lateral motion. For `CYL`/`CIR`/`DSC` the axis lies in the facet plane (direction θ); for `CON` the axis is `−n` (a conical pit, θ ignored and printed as given); for `SPH` a dimple. | Matches how "plunge" is used for dimples and pits; a cylinder plunged sideways is a straight groove with no stroke. |
| Q2 girdle tiers | Rejected in v1 (`|φ| < 90`). | Scalloped girdles need their own frame; nothing in the notation covers them and the two-section placement rule does not mention the girdle group separately. |
| Q3 width | `W` is the **minor** girdle extent of the flat stone `P` (`SolidMetrics::width_axis` from `measure_solid`), after cheater offsets and preform planes. | "Width" in a cutting diagram is the across-stone size; `L/W` ratios in the library use the same axis. |
| Q4 instructions | Free text, printed verbatim, never parsed. | The standard calls it "cutting/meetpoint instructions" with no keyword list. |
| Q5 compatibility | `DESIGN_VERSION` 2 is written iff concave tiers are present; old builds refuse with `UnsupportedVersion`. | Silently rendering the flat stone would be the one failure a cutter cannot see. |
| Q6 `.asc` import | Footnotes stay footnotes; no parsing. | No machine notation of ours exists to parse, and the native file is the carrier. |
| Q7 facet count display | The library shows `facets_count` unchanged and a separate "concave" column/badge with `concave_facets` placements, e.g. `55+6` and `16`. | Keeps the stored `"55+6"` parser intact; the pair is clearer than an invented suffix. |
| Q8 GemCAD footnotes | Write the `F` lines regardless. | Worst case GemCAD ignores them; nothing else in the file changes, so the cost of being wrong is zero. |
| Q9 placement ceiling | `MAX_TOOL_PRIMITIVES = 128`. | 8 tiers × 16-fold symmetry fits; the loop is scalar on the CPU, so the ceiling only bounds a stack array and the validator. |
| Q10 order | The author's listed order within each concave group is the cutting order; `MoveConcaveTier` reorders it. | The standard fixes only the group's position; the author knows which tool follows which. |

Consequences folded into the rest of this plan:

- §4.1 `validate` gains `0 < |φ| < 90` and the `CIR` acceptance (it is a tool,
  not "not yet modelled").
- §4.4 is fully specified by the table above; `concave_frame.rs` holds `C₀`,
  `u`/`v`, the sign of `Z`, the stroke ratios and `CIR_RIM_WIDTH_RATIO`.
- §5.1 keeps `sweep_kind == 2` in the kernel for a future `v1` frame.
- §6.1 `ConcaveTierTable` is accompanied by `concave_frame: String` on
  `DesignFile` (`#[serde(default)]`, empty when there are no concave tiers;
  a reader that meets an unknown frame string refuses the file).
- §6.3 Q7 is settled; the vault stores `concave_facets`.

## 12. Risks

| Risk | Mitigation |
|---|---|
| Breaking planar bit-identity | Every new path is gated on non-empty tools. Pins are run in CI for each phase. Plane-only wrappers stay. |
| Clipping robustness (slivers, near-tangent tools) | Convex-only clipping with the existing `EPS_FACE` tolerances. A sliver-area floor. A new manufacturability warning when a tool barely touches the stone or leaves a sliver (§9.1). |
| Seams and T-junctions in the concave mesh (§5.4 step 4) | Vertex snap after clipping; closedness test; hairline-gap detector in the raster pins. Volume is immune by construction. |
| Exit-split truncation losing visible energy on concave stones (§5.3) | Measured by the furnace test before deciding between v1 (keep, report) and v2 (trace through the re-entry). |
| wasm main-thread budget | Bounding-sphere rejection before clipping, tessellation count per target, single-slot mesh cache, benchmark in Phase 4. |
| GPU register pressure from the tool loop | Tools go in a separate small buffer and loop, behind a kernel specialisation constant like the existing ones; the `planes_shared[128]` workgroup cache is not widened. |
| Wrong conventions baked in | Phase 0 blocks Phase 3. All convention-dependent math is isolated in `concave_tools_from_solved`; a `concave_frame` key in the native file names the convention a file was written under (Q1). |
| `RemapIndices` silently moving concave placements on a gear change | Covered in §4.3 and by the `edit_roundtrip` assertion; the fix is part of Phase 1, not a follow-up. |
| Duplicated `.asc` footnotes on repeated export/import | The byte-equal strip rule in §6.2 and its idempotence test. |
