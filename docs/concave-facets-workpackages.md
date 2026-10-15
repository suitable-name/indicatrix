# Concave and fantasy facets: work packages

Companion to `docs/concave-facets-plan.md` (the plan). Each work package (WP)
is sized for one implementer agent working alone, roughly 300–1500 changed
lines, and is self-contained: an agent should need to read only the files
named here plus the plan sections cited. Where a type or signature is given,
copy it **exactly**; later WPs depend on it.

Scope reminders (plan §3.6–§3.7): no web UI work (`apps/indicatrix-web`,
`apps/indicatrix-web-compute` untouched); `indicatrix-web-core` gets compile
fixes only and no protocol bump; no `.wgsl` changes; the GPU backend falls back
to the CPU tracer whenever tools are present. The container is CPU-only
(4 cores, 15 GB), so never build with `--features gpu` here.

## Conventions every WP must follow

- Rust 2024, MSRV 1.95. Clippy `pedantic` + `nursery` are on; suppress with
  `#[expect(lint, reason = "…")]`, never `#[allow]`.
- Doc comments explain **why**. Public items get `#[must_use]` where the result
  is a value.
- Determinism: `BTreeMap`/`BTreeSet`, never `HashMap`/`HashSet`, in anything
  that influences output; no clocks; no thread-count dependence.
- Tests: hand-rolled seeded generators (xorshift/LCG as in
  `crates/indicatrix-cut-core/tests/edit_roundtrip.rs`), FNV byte-hash pins,
  `f32::to_bits` goldens. No `proptest`, no `insta`.
- `glam` is pinned `=0.34.1`; do not touch `Cargo.lock` versions.
- Test names are long descriptive `snake_case` sentences, e.g.
  `intersect_stone_with_no_tools_is_bit_identical_to_intersect_polyhedron`.
- Additive only: a design with no concave tiers must produce byte-identical
  output everywhere. Each WP lists the pins it must keep green.
- Do not commit. Leave the tree compiling at every save you make: other agents
  share this checkout and the `target/` directory.

Regression commands referenced below ("the pin suite"):

```
cargo test -p indicatrix --test raytracer_tests golden_regression
cargo test -p indicatrix --test raytracer_tests white_furnace
cargo test -p indicatrix-solid preview::tests
cargo test -p indicatrix-cut --test solid_identity_pins
cargo test -p indicatrix-cut identity_pins
```

Final gate for the whole series (run once after the last WP):
`cargo clippy --workspace --all-targets -- -D warnings && cargo test --workspace`.

## Order and parallel groups

| Group | WPs (parallel within a group; disjoint crates) | After |
|---|---|---|
| A | WP1 (cut-core model), WP4 (indicatrix kernel), WP12a (vault) | — |
| B | WP2 (cut-core edits), WP5 (indicatrix mesh) | A |
| C | WP3 (formats + cut-core native/asc), WP8 (indicatrix transport/metrics/GPU gate) | B |
| D | WP6 (cut-core resolve + manufacturability), WP9 (indicatrix-solid preview) | C |
| E | WP7 (cut-core cutting sheet + editor), WP12b (net + worker) | D |
| F | WP10 (desktop UI) | E |
| G | WP13 (web-core compile sweep + workspace gate) | F |

Within a group the WPs touch different crates, so they can run concurrently.
Across groups a later WP must wait for every WP it names under
"Depends on". WP2 and WP3 both live in `indicatrix-cut-core`; they are
sequential for that reason.

Quirks every agent must know:

- `ConstraintTier` (`crates/indicatrix-cut-core/src/design/tier.rs:17`) has
  ~130 struct-literal sites. **Never add a field to it.** Per-tier data goes on
  `Design` maps.
- `Design` (`crates/indicatrix-cut-core/src/design/mod.rs`) has a manual
  `PartialEq` (around line 330) that excludes `tier_ids`/`next_tier_id`;
  ~68 `Design { .. }` literals exist in cut-core, mostly in
  `design/construct.rs` and tests.
- `Edit` (`crates/indicatrix-cut-core/src/edit/edit_type.rs:23`) is matched
  exhaustively in `edit/apply.rs` (`apply_edit` + `describe`),
  `crates/indicatrix-cut-core/src/resolve.rs:111` (`resolve_after_edit`) and
  `crates/indicatrix-editor/src/session/selection.rs` (`TierIndexMap::of`).
  Adding a variant breaks all four until each has an arm.
- `AscTier` (`crates/indicatrix-formats/src/asc/tier.rs`) has 19 literal sites;
  do not add fields to it (this plan never does).
- `SceneState` (`crates/indicatrix-net/src/scene/mod.rs:25`) has 75 literal
  sites across 35 files; a new field needs every one updated (WP12b).
- `SolidMesh` (`crates/indicatrix/src/geometry/stone_metrics/types.rs:154`)
  has 5 literal sites.
- `DesignFile` (`crates/indicatrix-formats/src/native/design/schema.rs`) has
  24 literal sites; `SceneSpec` (web-core) 23 — the latter is **not** changed.
- Postcard is not self-describing: appending a field to a wire struct changes
  the bytes of every message of that type.

---

## WP1 — Concave tier model in cut-core

**Goal.** `ConcaveTier` and friends exist, live on `Design`, validate, and
yield the cutting order. No edits, no files, no geometry yet.

**Depends on.** Nothing. **Parallel with.** WP4, WP12a.

**Files.**
- Create `crates/indicatrix-cut-core/src/design/concave.rs`.
- Edit `crates/indicatrix-cut-core/src/design/mod.rs` (fields, `PartialEq`,
  `pub mod concave`, re-exports), `design/construct.rs` (every constructor),
  `design/tier_id.rs` only if `TierId` helpers are needed.
- Every `Design { .. }` literal in cut-core (`rg "Design \{" crates/indicatrix-cut-core`)
  and in other crates that construct one (`rg "Design \{" crates apps`), adding
  `concave_tiers: Vec::new(), concave_tier_ids: Vec::new()`.

**Types to add (exact).**

```rust
// design/concave.rs
/// A tool's three-letter code in the faceting-diagram standard (plan §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ConcaveTool { Cylinder, Cone, Circle, Disc, Sphere }
impl ConcaveTool {
    pub const ALL: [Self; 5];
    #[must_use] pub const fn code(self) -> &'static str; // "CYL" "CON" "CIR" "DSC" "SPH"
}
impl core::fmt::Display for ConcaveTool {}           // prints `code()`
impl core::str::FromStr for ConcaveTool { type Err = UnknownToolCode; } // case-insensitive, trimmed
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownToolCode(pub String);                // implements Display + std::error::Error

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolMotion { Reciprocating, Plunge }
impl ToolMotion { #[must_use] pub const fn label(self) -> &'static str; } // "recip." / "plunge"

/// One concave tier exactly as the two-line standard writes it (plan §4.1).
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveTier {
    pub name: String,
    pub angle_deg: f64,            // φ, signed like ConstraintTier::angle_deg
    pub indices: Vec<f64>,
    pub instructions: String,
    pub tool: ConcaveTool,
    pub tool_azimuth_deg: f64,     // θ
    pub displacement: [f64; 3],    // X, Y, Z as ratios of stone width
    pub diameter_ratio: f64,
    pub tool_angle_deg: Option<f64>, // required for Cone and Disc, rejected otherwise
    pub motion: ToolMotion,
}

#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ConcaveTierError {
    NonFinite { field: &'static str, value: f64 },
    AngleOutOfRange { angle_deg: f64 },                 // requires 0 < |φ| < 90
    NoIndices,
    IndexOutOfRange { index: f64, gear_teeth: i32 },
    DiameterNotPositive { diameter_ratio: f64 },
    ToolAngleMissing { tool: ConcaveTool },
    ToolAngleUnexpected { tool: ConcaveTool },
    ToolAngleOutOfRange { tool_angle_deg: f64 },        // requires 0 < angle < 180
    NameClash { name: String },                          // same name as a flat tier
}
impl ConcaveTier {
    /// `gear_teeth` is `design.meta.gear_teeth` (an `i32` in the codebase; indices must lie in `[0, |gear_teeth|)`).
    pub fn validate(&self, gear_teeth: i32) -> Result<(), ConcaveTierError>;
    #[must_use] pub fn is_crown_side(&self) -> bool; // indicatrix::geometry::plane::tier_is_crown_side(self.angle_deg)
}

/// A position in either tier list (plan §4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TierRef { Flat(usize), Concave(usize) }
```

On `Design`:

```rust
pub concave_tiers: Vec<ConcaveTier>,
pub concave_tier_ids: Vec<TierId>,   // same next_tier_id counter; excluded from PartialEq
impl Design {
    /// Plan §4.2: flat pav/girdle, concave pav-side, flat crown (not table), concave crown-side, table.
    pub fn cutting_order(&self) -> Vec<TierRef>;
    pub fn validate_concave_tiers(&self) -> Result<(), (usize, ConcaveTierError)>;
    pub fn ensure_concave_tier_ids(&mut self);  // mirrors the existing ensure_tier_ids
    #[must_use] pub fn concave_placement_count(&self) -> usize; // Σ indices.len()
}
```

"The table" for `cutting_order()`: a flat tier with `angle_deg == 0.0` that is
crown-side by `indicatrix::geometry::plane::tier_is_crown_side` (i.e. `+0`,
not `−0` which is a culet). There is no existing helper; add
`ConstraintTier::is_table(&self) -> bool` as an inherent method (a method, not
a field, so the ~130 literal sites are untouched).

**Invariants.** All existing cut-core tests pass unchanged; the manual
`PartialEq` still ignores ids; `Design::new`/`fresh*`/`from_asc_schedule`
start with empty concave lists.

**Tests to add** (`design/concave.rs` `#[cfg(test)]` and
`design/tests/concave.rs` if a tests dir exists):
- `concave_tool_codes_round_trip_through_from_str_case_insensitively`
- `concave_tier_validate_rejects_each_hostile_field` (NaN, φ=0, φ=90, empty
  indices, index ≥ gear, diameter 0, CON without angle, SPH with angle, angle
  180, name clash).
- `cutting_order_places_concave_groups_at_the_end_of_each_section_before_the_table`
- `design_equality_ignores_concave_tier_ids`

**Gate.**
```
cargo clippy -p indicatrix-cut-core --all-targets -- -D warnings
cargo test -p indicatrix-cut-core
cargo check --workspace   # the Design literal sweep must leave every crate compiling
```

---

## WP2 — Edit/undo variants for concave tiers

**Goal.** Concave tiers are editable with exact inverses, and `RemapIndices`
remaps concave indices.

**Depends on.** WP1. **Parallel with.** WP5.

**Files.** `crates/indicatrix-cut-core/src/edit/edit_type.rs`, `edit/apply.rs`,
`edit/history.rs` (coalescing key), `src/resolve.rs` (`resolve_after_edit`: new
arms return the previous solve), `crates/indicatrix-editor/src/session/selection.rs`
(`TierIndexMap::of`: identity for the new variants — this is the only file
outside cut-core), `tests/edit_roundtrip.rs`.

**Variants to add (exact; plan §4.3).**

```rust
AddConcaveTier { index: usize, tier: ConcaveTier },           // inverse RemoveConcaveTier
RemoveConcaveTier { index: usize },                            // inverse Batch[AddConcaveTier, RestoreConcaveTierId]
ModifyConcaveTier { index: usize, tier: ConcaveTier },        // inverse ModifyConcaveTier(previous)
MoveConcaveTier { from: usize, to: usize },                    // inverse swapped
RestoreConcaveTierId { index: usize, id: TierId },
RestoreConcaveIndices { tiers: Vec<(usize, Vec<f64>)> },      // verbatim snapshot
```

Changes to existing behaviour:
- `apply_remap_indices` (`apply.rs:608`) also remaps `concave_tiers[*].indices`
  and returns `Edit::Batch(vec![RestoreIndices{..}, RestoreConcaveIndices{..}])`
  (keep the `RestoreIndices` payload exactly as today). Check how `Batch`
  inverses are ordered (reverse) and follow it.
- `describe` arms for each new variant, in the existing wording style.
- `resolve_after_edit`: the new variants are mast-inert; return the previous
  solve unchanged (follow what `SetTierNote` does).
- Index validation before mutation (`EditError { index, tier_count }` uses the
  **concave** list length for concave variants).
- `AddConcaveTier`/`ModifyConcaveTier` call `ConcaveTier::validate` and the name
  clash check; a failure is an error without mutation. Pick the existing error
  type that `ModifyTier` uses for invalid tiers, or add a `ConcaveTierError`
  conversion to it.
- History coalescing: the key for `ModifyConcaveTier{index}` must differ from
  `ModifyTier{index}`.

**Invariants.** `tests/edit_roundtrip.rs` existing tests unchanged; every
existing `Edit` inverse unchanged; the `RemapIndices` inverse for a design with
no concave tiers must still be *exactly* `RestoreIndices` **or** the test
`history_undo_and_redo_of_a_removal_keep_the_tier_ids`-style expectations must
be adjusted — prefer: return plain `RestoreIndices` when `concave_tiers` is
empty, the `Batch` only when it is not. Document why.

**Tests to add** (`tests/edit_roundtrip.rs`, using the existing `Lcg`):
- `add_concave_tier_round_trips`, `remove_concave_tier_round_trips`,
  `modify_concave_tier_round_trips`, `move_concave_tier_round_trips`
- `undoing_a_concave_tier_removal_restores_the_exact_tier_id`
- `remap_indices_remaps_concave_indices_and_its_undo_restores_them_verbatim`
- `remap_indices_inverse_is_unchanged_for_a_design_without_concave_tiers`
- `modify_concave_tier_rejects_an_invalid_tier_without_mutating`
- in `crates/indicatrix-editor`: `tier_index_map_is_identity_for_concave_edits`.

**Gate.**
```
cargo clippy -p indicatrix-cut-core -p indicatrix-editor --all-targets -- -D warnings
cargo test -p indicatrix-cut-core
cargo test -p indicatrix-editor
```

---

## WP3 — Native file carrier and `.asc` footnotes

**Goal.** `.indicatrix` round-trips concave tiers (version 2 only when present);
`.asc`/`.gem`/`.gcs` export omits them, writes two `F` lines per tier, and does
not duplicate them on re-export.

**Depends on.** WP1 (and WP2 merged, since both edit cut-core). **Parallel with.** WP8.

**Files.**
- `crates/indicatrix-formats/src/native/schema.rs` (add `ConcaveTierTable`),
  `native/design/schema.rs` (`DesignFile.concave_tiers`, `concave_frame`,
  version rule), `native/design/codec.rs` (missing-geometry twin check), tests
  under `native/`.
- `crates/indicatrix-cut-core/src/native/{convert.rs,design_file.rs,load.rs,save.rs}`,
  `design/export/schedule.rs` (asc export: omit + footnotes + strip rule),
  `manufacturability/warning.rs` (one new variant, see below), `native/tests`.

**Types (exact).**

```rust
// indicatrix-formats native/schema.rs
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ConcaveTierTable {
    pub name: String,
    pub angle_deg: f64,
    pub indices: Vec<f64>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instructions: String,
    pub tool: String,                 // "CYL" | "CON" | "CIR" | "DSC" | "SPH"
    pub tool_azimuth_deg: f64,
    pub displacement: [f64; 3],
    pub diameter_ratio: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_angle_deg: Option<f64>,
    pub motion: String,               // "reciprocating" | "plunge"
    #[serde(flatten)]
    pub unknown: toml::Table,
}
// DesignFile gains:
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub concave_tiers: Vec<ConcaveTierTable>,
#[serde(default, skip_serializing_if = "String::is_empty")]
pub concave_frame: String,            // "v0" when concave_tiers is non-empty, else ""
pub const DESIGN_VERSION_CONCAVE: u32 = 2;
```

Rules (plan §6.1, §11a): write `version = 2` iff `concave_tiers` is non-empty;
the reader accepts 1 and 2; `concave_frame` must equal
`indicatrix_cut_core::design::export::concave_frame::CONCAVE_FRAME_VERSION`
("v0") when tiers are present — but that constant arrives in WP6, so in this
WP define the string literal `"v0"` in the formats crate as
`pub const CONCAVE_FRAME_V0: &str = "v0";` and have WP6 assert the two are equal.
An unknown non-empty `concave_frame` is a load error (new `DesignFileError`
variant, name it `UnsupportedConcaveFrame { frame: String }`).

`.asc` export (`design/export/schedule.rs`): **careful** — `planes_from_solved`
also goes through `to_asc_schedule_from_solved` (→ `from_asc_schedule`), and
`manufacturability::facet_plane_boundaries` builds prefix schedules. Do not
change those. Add a separate `Design::to_asc_schedule_for_export(&self, solved)`
(and use it from `native/save.rs` and wherever a file is written) that calls
`to_asc_schedule_from_solved` and then appends the footnotes. Concave tiers are
never emitted as tiers. Append to `schedule.footnotes`, per concave tier in `cutting_order()`, exactly two
strings (no `F ` prefix — the writer adds it; no newlines):

```
"{name}  {angle_deg:.2}  {indices space-separated, format_indices style}  {instructions}"
"{f[0]}  {f[1]}  {f[2]}  {f[3]}"   // f = tier.second_line_fields()
```

Add `#[must_use] pub fn second_line_fields(&self) -> [String; 4]` to
`ConcaveTier` in `crates/indicatrix-cut-core/src/design/concave.rs`, formatting
exactly as plan §1a (reference template): `["CYL", "+10.00°", "X = 0.000, Y = 0.000, Z = 0.000",
"D/W = 0.400, reciprocating"]`; `CON`/`DSC` insert `, angle = 60.00°` before the
motion; θ gets an explicit `+` when positive and none at zero. Test:
`second_line_fields_match_the_reference_template` (cylinder case above, a cone
case with angle, a negative θ, a plunge case). Every later output (text and HTML
sheets, view models) uses this function.

Before appending, remove from `design.meta.footnotes` (as copied into the
schedule, never mutating the design) any entry byte-equal to a line about to be
generated (plan §6.2 idempotence). Add
`ManufacturabilityWarning::ConcaveTiersOmittedFromExport { count: usize }`
and return it from the export path that already returns warnings (find the
existing export-warning channel; if there is none, add
`Design::export_warnings(&self) -> Vec<ManufacturabilityWarning>` and have the
desktop show it in WP10).

**Invariants.** Native round trip of every existing fixture is byte-identical
(there is an existing byte-identity test in `native/tests`; find and extend).
`save_paired` byte-preservation logic unchanged for planar designs.
`.asc` writer and parser untouched (`crates/indicatrix-formats/src/asc/`).

**Tests to add.**
- formats: `design_file_without_concave_tiers_serialises_as_version_1_byte_identical`,
  `design_file_with_concave_tiers_serialises_as_version_2_and_round_trips`,
  `design_file_with_unknown_concave_frame_is_refused`.
- cut-core `native/tests`: `native_round_trip_with_concave_tiers_preserves_every_field`.
- cut-core export tests: `asc_export_omits_concave_tiers_and_appends_two_footnotes_per_tier`,
  `asc_export_then_import_then_export_does_not_duplicate_concave_footnotes`.

**Gate.**
```
cargo clippy -p indicatrix-formats -p indicatrix-cut-core --all-targets -- -D warnings
cargo test -p indicatrix-formats
cargo test -p indicatrix-cut-core
cargo check -p indicatrix-web-core -p indicatrix-editor   # DesignFile literal sweep
```

---

## WP4 — Kernel: `ToolPrimitive`, `StoneGeometry`, `intersect_stone`

**Goal.** The exact CSG ray intersection on the CPU, bit-identical to
`intersect_polyhedron` when there are no tools.

**Depends on.** Nothing. **Parallel with.** WP1, WP12a.

**Files.** Create `crates/indicatrix/src/geometry/tool.rs` and
`crates/indicatrix/src/optics/raytracer/intersect_stone.rs`; edit
`geometry/mod.rs` and `optics/raytracer/mod.rs` for `pub mod`/re-exports only.
Do **not** touch `intersect.rs`, `simd/`, or any `.wgsl`.

**Types (exact; plan §5.1).**

```rust
// geometry/tool.rs
pub const MAX_TOOL_PRIMITIVES: usize = 128;

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolKind { Ball = 0, Cylinder = 1, Frustum = 2, Bicone = 3 }
impl TryFrom<u32> for ToolKind { type Error = u32; }

#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolSweep { None = 0, AlongAxis = 1, AcrossAxis = 2 }
impl TryFrom<u32> for ToolSweep { type Error = u32; }

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ToolPrimitive {
    pub kind: u32,          // ToolKind as u32
    pub sweep_kind: u32,    // ToolSweep as u32
    pub _pad: [u32; 2],     // must be zero
    pub origin: [f32; 4],   // centre xyz; w = radius at the centre (ball, cylinder, bicone rim)
    pub axis: [f32; 4],     // unit axis xyz; w = half-length along the axis (0 for a ball)
    pub profile: [f32; 4],  // x = radius at −half_length, y = radius at +half_length, z = sweep half-stroke, w = 0
    pub sweep_dir: [f32; 4],// unit sweep direction xyz (only for AcrossAxis), w = 0
}
impl ToolPrimitive {
    pub fn ball(centre: Vec3, radius: f32) -> Self;
    pub fn cylinder(centre: Vec3, axis: Vec3, radius: f32, half_length: f32) -> Self;
    pub fn frustum(centre: Vec3, axis: Vec3, r_neg: f32, r_pos: f32, half_length: f32) -> Self;
    pub fn bicone(centre: Vec3, axis: Vec3, rim_radius: f32, half_length: f32) -> Self;  // r_neg = r_pos = 0 at the ends
    #[must_use] pub fn with_sweep(self, sweep: ToolSweep, half_stroke: f32, dir: Vec3) -> Self;
    #[must_use] pub fn kind(&self) -> Option<ToolKind>;
    #[must_use] pub fn sweep(&self) -> Option<ToolSweep>;
    /// Every float finite, radii >= 0 with one > 0, axis unit to 1e-4, pad/w lanes zero, kinds known.
    pub fn validate(&self) -> Result<(), ToolPrimitiveError>;
    /// Analytic inside test, used by tests and debug assertions.
    #[must_use] pub fn contains(&self, p: Vec3) -> bool;
    /// Outward normal of the TOOL at a surface point (callers negate it for the stone).
    #[must_use] pub fn outward_normal(&self, p: Vec3) -> Vec3;
    /// Entry/exit interval of the ray inside the tool, or None.
    #[must_use] pub fn ray_interval(&self, ray: Ray) -> Option<(f32, f32)>;
}
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolPrimitiveError { NonFinite, NonPositiveRadius, AxisNotUnit, UnknownKind(u32), UnknownSweep(u32), PaddingNotZero }

#[derive(Debug, Clone, Copy)]
pub struct StoneGeometry<'a> { pub planes: &'a [GpuFacetPlane], pub tools: &'a [ToolPrimitive] }
impl<'a> StoneGeometry<'a> {
    #[must_use] pub const fn planes_only(planes: &'a [GpuFacetPlane]) -> Self;
    #[must_use] pub const fn is_convex(&self) -> bool; // tools.is_empty()
    #[must_use] pub const fn facet_count(&self) -> usize; // planes.len() + tools.len()
}

// optics/raytracer/intersect_stone.rs
#[must_use]
pub fn intersect_stone(ray: Ray, geom: StoneGeometry<'_>) -> Option<HitRecord>;
/// SoA twin used by transport: identical result to `intersect_stone`.
pub(crate) fn intersect_stone_soa(ray: Ray, plane_soa: &PlanesSoA32, planes_len: usize, tools: &[ToolPrimitive]) -> Option<HitRecord>;
```

Algorithm: plan §5.2 verbatim. Step 1 calls `intersect_polyhedron` /
`intersect_polyhedron_soa` only when `tools.is_empty()` (return it directly);
otherwise compute `[a, b]` by the same slab loop (copy the loop into a private
`slab_interval` helper that returns both ends and both facets; do not edit
`intersect.rs`). Interval subtraction with a fixed stack array of
`2 * MAX_TOOL_PRIMITIVES + 2` boundaries sorted by `t` (ties: plane first,
then lower tool index). Hit = smallest boundary of the inside set with
`t > 1e-4`. Normal on a tool boundary = `-outward_normal(hit_point)`.
`facet_idx` for a tool = `planes.len() + k`. Across-axis sweep per plan §5.2
(union of convex pieces, `[min c, max d]`).

**Invariants.** `intersect.rs`, `simd/slab.rs`, all shaders untouched; the pin
suite passes (nothing calls the new code yet).

**Tests to add** (in `tool.rs` and `intersect_stone.rs` `#[cfg(test)]`):
- `intersect_stone_with_no_tools_is_bit_identical_to_intersect_polyhedron`
  (`to_bits` on `t` and normal, equal `facet_idx`, over the cube/RBC/empty-planes
  cases already in `intersect.rs` tests — copy the case constructors).
- `tool_interval_agrees_with_point_classification_on_seeded_rays` for each
  `ToolKind × ToolSweep`: 10 000 seeded rays, 64 sample points per ray, every
  point farther than `1e-4·r` from a boundary agrees with `contains`.
- `grazing_restart_from_a_small_cylinder_does_not_self_intersect` (radius
  `0.05`, origin on the surface, direction outward).
- `tool_primitive_layout_is_80_bytes_and_16_aligned` (`size_of`, `align_of`).
- `tool_primitive_validate_rejects_each_bad_field`.
- `stone_hit_normal_on_a_tool_points_into_the_cavity` (dot with ray dir < 0 for
  an entry from outside through a groove's far wall).

**Gate.**
```
cargo clippy -p indicatrix --all-targets -- -D warnings
cargo test -p indicatrix --lib geometry::tool
cargo test -p indicatrix --lib intersect_stone
cargo test -p indicatrix --test raytracer_tests golden_regression
```

---

## WP5 — Concave mesh and volume (`stone_metrics`)

**Goal.** `build_solid_mesh_geom` produces a closed, consistently oriented mesh
of `P ∖ ⋃T̃` with per-piece normals and edge visibility; volume is correct.

**Depends on.** WP4. **Parallel with.** WP2.

**Files.** Create `crates/indicatrix/src/geometry/stone_metrics/concave.rs`
(tessellation, clipping, provenance) and `stone_metrics/tests/concave.rs`;
edit `stone_metrics/types.rs` (two fields), `stone_metrics/mesh.rs` (new entry
point only; `build_solid_mesh` body untouched), `stone_metrics/mod.rs`. The 5
`SolidMesh { .. }` literal sites get `piece_normals: None, edge_visible: None`.

**Types (exact; plan §5.4).**

```rust
// types.rs: SolidMesh gains
pub piece_normals: Option<Vec<DVec3>>,        // one per `rings` entry; None on the planar path
pub edge_visible: Option<Vec<Vec<bool>>>,     // per ring, per segment i→i+1; None on the planar path

// mesh.rs
pub const TOOL_SEGMENTS: usize = 48;           // around the axis; 24 under cfg(target_arch = "wasm32")
pub const TOOL_ICOSPHERE_LEVEL: u32 = 2;
/// Plan §5.4. With no tools this is exactly `build_solid_mesh(planes)`.
#[must_use]
pub fn build_solid_mesh_geom(planes: &[(DVec3, f64)], tools: &[ToolPrimitive]) -> SolidStatus;
/// Convex polytope approximating one tool: outward half-spaces, equal-area radius.
#[must_use]
pub fn tessellate_tool(tool: &ToolPrimitive, segments: usize) -> Vec<(DVec3, f64)>;
/// Signed volume by the divergence formula over `rings`; identical to today's for a planar mesh.
#[must_use]
pub fn mesh_volume(mesh: &SolidMesh) -> f64;
```

Rules: equal-area radius `r̃ = r·sqrt(2π/(N·sin(2π/N)))`; bounding-sphere
rejection; untouched rings keep their original `DVec3`s; Sutherland–Hodgman
decomposition `A ∩ h₁ ∩ … ∩ hₖ₋₁ ∩ ¬hₖ`; tool faces clipped to `P` then split
against other tools and **reversed**; snap all clipped vertices to a
`1e-9·W` grid after clipping (`W` = bounding-box width of `P`); edge
provenance via `BTreeMap<(u64,u64,u64,u64,u64,u64), ...>` on snapped endpoint
bits; piece normals from the exact tool at the piece centroid (negated tool
outward normal). `facet_id` for tool pieces = `planes.len() + k`.
`SolidMesh.normals`/`positions`/`indices` are filled from the pieces the way
`build_solid_mesh` fills them (centroid fan), so existing consumers see a
valid mesh.

**Invariants.** `build_solid_mesh` output unchanged (the pin suite, especially
`cargo test -p indicatrix-cut --test solid_identity_pins` and
`cargo test -p indicatrix-solid preview::tests`); `measure_solid` untouched.

**Tests to add** (`stone_metrics/tests/concave.rs`):
- `build_solid_mesh_geom_with_no_tools_equals_build_solid_mesh` (field-by-field
  `to_bits` equality on the existing fixtures).
- `tessellated_cylinder_groove_through_slab_matches_closed_form` (relative
  error `< 1e-9`).
- `tessellated_partial_cylinder_groove_is_within_polygon_bound` (depth
  `0.6·r`, bound computed analytically in the test).
- `cube_minus_interior_ball_matches_icosphere_volume_constant` (`1e-9`
  relative to the scaled icosphere's own volume; prints the constant; asserts
  it is within 1 % of `4πr³/3`).
- `concave_mesh_is_closed` (per-facet boundary loops sum to zero within
  `1e-9·W`).
- `concave_mesh_volume_is_independent_of_tool_order`.
- `concave_mesh_piece_windings_point_out_of_the_stone` (area-weighted normal ·
  `piece_normals[i]` > 0 for every ring).
- `edge_visibility_hides_tessellation_seams_and_shows_facet_tool_boundaries`.

**Gate.**
```
cargo clippy -p indicatrix --all-targets -- -D warnings
cargo test -p indicatrix --lib stone_metrics
cargo test -p indicatrix-solid preview::tests
cargo test -p indicatrix-cut --test solid_identity_pins
```

---

## WP6 — Resolving tiers to tools; manufacturability; yield

**Goal.** `Design` turns concave tiers into `ToolPrimitive`s under the `v0`
frame, exposes the geometry to consumers, warns about bad tools, and reports
concave volume.

**Depends on.** WP1, WP3, WP4, WP5. **Parallel with.** WP9.

**Files.** Create `crates/indicatrix-cut-core/src/design/export/concave_frame.rs`
and `design/export/concave.rs`; edit `design/export/mod.rs`,
`design/export/planes.rs` (add the `_geom` twins, do not change existing
functions), `manufacturability/warning.rs`, `manufacturability/mesh_checks.rs`
(new checks), `yield_metrics/fit.rs` (around line 39: use `build_solid_mesh_geom`
when concave tiers exist), tests.

**Types (exact; plan §4.4, §11a).**

```rust
// design/export/concave_frame.rs — the ONLY convention-dependent file
pub const CONCAVE_FRAME_VERSION: &str = "v0";   // must equal indicatrix_formats::native::CONCAVE_FRAME_V0
pub const RECIP_HALF_STROKE_RATIO: f64 = 2.0;    // × W, along the axis
pub const SPH_RECIP_HALF_STROKE_RATIO: f64 = 0.5;// × D
pub const CIR_RIM_WIDTH_RATIO: f64 = 0.1;        // × D, full length of the CIR cylinder
/// Dop frame at (φ, index): n = StandardGemCuts normal, u = girdle-ward unit vector in the plane, v = n × u.
#[must_use] pub fn dop_frame(angle_deg: f64, index: f64, gear_teeth: i32) -> (DVec3, DVec3, DVec3);
/// Support point of the convex polytope `planes` in direction `n` (ties: lexicographically smallest vertex).
#[must_use] pub fn contact_point(planes: &[(DVec3, f64)], n: DVec3) -> Option<DVec3>;
/// Tool centre: c0 + W·(X·u + Y·v − Z·n).
#[must_use] pub fn tool_centre(c0: DVec3, width: f64, frame: (DVec3, DVec3, DVec3), displacement: [f64; 3]) -> DVec3;
/// Tool axis: cos θ·u + sin θ·v, or −n for a plunged cone.
#[must_use] pub fn tool_axis(tier: &ConcaveTier, frame: (DVec3, DVec3, DVec3)) -> DVec3;
/// Builds the primitive for one placement from the resolved centre/axis/width.
#[must_use] pub fn primitive_for(tier: &ConcaveTier, centre: DVec3, axis: DVec3, width: f64) -> ToolPrimitive;

// design/export/concave.rs
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ConcaveResolveError { Invalid { tier: usize, error: ConcaveTierError }, NoFlatSolid, TooManyPlacements { count: usize, max: usize } }
/// `(tier index, placement index)` for each primitive, in primitive order.
pub type ToolPlacements = Vec<(usize, usize)>;
impl Design {
    pub fn concave_tools_from_solved(&self, solved: &[SolvedTier]) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError>;
    /// Tools of the concave tiers that precede `through` in `cutting_order()` (plan §4.4).
    pub fn concave_tools_through_tier(&self, solved: &[SolvedTier], through: TierRef) -> Result<(Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError>;
    /// Planes + tools in one call; `tools` is empty for a design without concave tiers.
    pub fn geometry_from_solved(&self, solved: &[SolvedTier]) -> Result<(Vec<(DVec3, f64)>, Vec<ToolPrimitive>, ToolPlacements), ConcaveResolveError>;
}
```

`W` = `measure_solid(&planes_from_solved(..))?.width_axis`. Tools are emitted
in `cutting_order()` order of concave tiers, then index order. `f64`
throughout, converted to `f32` once in `primitive_for`.

Manufacturability (plan §9.1), computed from the tools and the flat mesh:
`ToolMissesStone { tier, placement }`, `ToolBreaksThrough { tier, placement, facet }`,
`ToolRemovesMeet { tier, placement, vertex }`, `ToolsOverlap { a, b }`,
`ConcaveSliver { facet }`, `ToolRemovesHullVertex { tier, placement }`. Add them
to `ManufacturabilityWarning` (not `#[non_exhaustive]` today; if the editor
matches it exhaustively, add arms there too — grep `ManufacturabilityWarning::`).

Yield: where `yield_metrics/fit.rs` builds the mesh for carat/volume, use
`build_solid_mesh_geom(&planes, &tools)` when `concave_tiers` is non-empty.

**Invariants.** For a design without concave tiers `geometry_from_solved`
returns `planes_from_solved` bytes and empty tools; all yield/manufacturability
outputs unchanged (`cargo test -p indicatrix-cut identity_pins` covers
`yield_and_proportion_texts_are_pinned`).

**Tests to add** (`design/export/tests.rs` or a new `design/export/concave_tests.rs`):
- `dop_frame_is_right_handed_and_u_points_toward_the_girdle` (crown and pavilion cases).
- `contact_point_is_the_support_vertex_with_deterministic_tie_break`.
- `tool_centre_moves_into_the_stone_for_positive_z`.
- `concave_tools_from_solved_emits_one_primitive_per_index_in_cutting_order`.
- `concave_frame_version_matches_the_formats_constant`.
- `a_tool_entirely_outside_the_stone_warns_tool_misses_stone`.
- `a_through_cylinder_on_the_girdle_warns_breaks_through`.
- `yield_volume_drops_by_the_groove_volume_for_the_concave_fixture` (compare
  against WP5's closed form, tolerance 1e-6 relative).

Add the **concave fixture**: `Design::concave_fixture()` under `#[cfg(test)]`
or a `pub fn` in `templates.rs` guarded by a doc note — a standard round
brilliant (reuse `round_brilliant_fixture` from `tests/edit_roundtrip.rs`)
plus one `CYL` tier (φ = −42, 8 indices, θ = 0, X = 0, Y = 0.15, Z = 0.03,
D = 0.25, reciprocating) and one `SPH` plunge tier on the crown. Later WPs pin
against it, so its numbers are frozen once this WP lands.

**Gate.**
```
cargo clippy -p indicatrix-cut-core --all-targets -- -D warnings
cargo test -p indicatrix-cut-core
cargo test -p indicatrix-cut identity_pins
```

---

## WP7 — Cutting sheets, editor view-models and forms

**Goal.** Two-line concave rows in the text and HTML cutting sheets, the
schedule rows, tier rows, concave form parsing/saving, session ops, compare.
GUI-free only.

**Depends on.** WP2, WP6. **Parallel with.** WP12b.

**Files.**
- cut-core: `cutting_sheet/sheet.rs` (column width consts + second line),
  `cutting_sheet/build.rs`, `cutting_sheet/diff.rs` (`diff_concave_tiers`),
  `cutting_sheet/tests.rs`.
- editor: create `loading/concave_form.rs`; edit `loading/tier_form/mod.rs`
  (`tier_form_error_field` prefixes), `tier_save.rs`, `session/tier_ops.rs`,
  `session/selection.rs` (concave selection slot), `view_model/mod.rs`
  (`TierRow`, `CuttingRow`), `view_model/rows.rs`, `view_model/yield_report.rs`
  (`cutting_instructions_rows`), `view_model/solid_status.rs`
  (`design_to_gpu_geometry`), `cut_sheet/html.rs`, `snapshot.rs`,
  `retarget/mod.rs`, `optimize_view.rs`, `manipulate/target.rs` (exclusions).

**Types (exact).**

```rust
// cut-core cutting_sheet/sheet.rs
pub const SHEET_NAME_WIDTH: usize = 16;   // hoisted from the existing format string; values unchanged
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveRowInfo { pub tool: ConcaveTool, pub tool_azimuth_deg: f64, pub displacement: [f64; 3], pub diameter_ratio: f64, pub tool_angle_deg: Option<f64>, pub motion: ToolMotion }
// CutSheetRow gains:
pub concave: Option<ConcaveRowInfo>,       // None for flat rows (2 literal sites)

// cut-core cutting_sheet/diff.rs
#[derive(Debug, Clone, PartialEq)]
pub struct ConcaveTierDelta { pub position: usize, pub before: Option<ConcaveTier>, pub after: Option<ConcaveTier> }
#[must_use] pub fn diff_concave_tiers(before: &[ConcaveTier], after: &[ConcaveTier]) -> Vec<ConcaveTierDelta>;

// editor view_model/mod.rs
pub enum TierRowKind { Flat, Concave }     // TierRow gains `kind: TierRowKind, tool_line: String`
// CuttingRow gains `second_line: Option<[String; 4]>`  (tool, theta, displacement, details)

// editor loading/concave_form.rs
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConcaveTierFormFields { pub name: String, pub angle_deg: String, pub indices: String, pub instructions: String, pub tool: String, pub tool_azimuth_deg: String, pub x: String, pub y: String, pub z: String, pub diameter_ratio: String, pub tool_angle_deg: String, pub reciprocating: bool }
pub fn parse_concave_tier_form(fields: &ConcaveTierFormFields, gear_teeth: i32) -> Result<ConcaveTier, String>; // error strings prefixed "name:", "angle:", "indices:", "tool:", "theta:", "x:", "y:", "z:", "diameter:", "tool angle:"
#[must_use] pub fn concave_tier_form_fields(tier: &ConcaveTier) -> ConcaveTierFormFields;

// editor tier_save.rs
pub fn concave_tier_save_edit(design: &Design, index: Option<usize>, tier: ConcaveTier) -> Edit; // Add when None, Modify otherwise

// editor view_model/solid_status.rs
#[must_use] pub fn design_to_gpu_geometry(design: &Design) -> (Vec<GpuFacetPlane>, Vec<ToolPrimitive>, ToolPlacements); // design_to_gpu_planes stays and is its first element
```

Text sheet second line (plan §9.2), under the same columns, no sequence number.
The four column strings come from `ConcaveTier::second_line_fields()` (added in
WP3, format fixed by plan §1a); the HTML sheet, `CuttingRow::second_line` and the
schedule views use the same strings. HTML: the two `<tr>`s of a tier share one
zebra band (one `<tbody>` per tier, stripe class on the `<tbody>`).

```
 12. P8               angle  -62.00 deg (elevation  28.00 deg)  indices [3 11 19 ...]  mast        -  meet: cut to depth
                      CYL               +90.00°           X = 0.000, Y = 0.120, Z = 0.050   D/W = 0.250, reciprocating
```

(`mast` is printed as a dash for a concave row, which has no mast; the row's
`mast` field is `0.0`, `angle_of_elevation_deg` computed as for a flat tier.) Rows are emitted
in `cutting_order()`; `sequence` counts tiers of both kinds.

HTML: a second `<tr class="tool">` per concave tier, each pair in its own
`<tbody>`; shown only when the design has concave tiers (follow the
`TierTableColumns` pattern at `html.rs:298`).

`retarget_scope`, optimize free-tier selection and manipulate targets skip
concave tiers (they only enumerate `design.tiers`, so mostly this is asserting
and documenting; add the `TierRef` awareness where a function returns "all
tiers").

**Invariants.** `cargo test -p indicatrix-cut identity_pins` —
`tier_rows_and_formats_are_pinned`, `cutting_sheet_and_diagram_bytes_are_pinned`,
`form_parsing_is_pinned`, `retarget_output_is_pinned`, `optimize_views_are_pinned`
all unchanged; cut-core `cutting_sheet/tests.rs` line-count assertions unchanged;
editor `cut_sheet/tests.rs` unchanged.

**Tests to add.**
- cut-core: `cutting_sheet_text_for_concave_fixture_matches_expected_string`,
  `cutting_sheet_text_for_planar_fixture_is_unchanged` (string pin of an
  existing fixture), `cutting_sheet_line_count_is_flat_rows_plus_two_per_concave_row`,
  `diff_concave_tiers_reports_added_removed_and_changed_positions`.
- editor: `parse_concave_tier_form_maps_each_hostile_input_to_its_field`
  (NaN, missing angle on CON, angle on SPH, unknown code, empty indices,
  φ = 90), `concave_form_fields_round_trip_through_parse`,
  `cutting_rows_emit_second_lines_in_cutting_order`,
  `html_sheet_groups_each_concave_pair_in_one_tbody`,
  `compare_rows_include_concave_deltas`,
  `design_to_gpu_geometry_has_empty_tools_for_a_planar_design`.

**Gate.**
```
cargo clippy -p indicatrix-cut-core -p indicatrix-editor --all-targets -- -D warnings
cargo test -p indicatrix-cut-core
cargo test -p indicatrix-editor
cargo test -p indicatrix-cut identity_pins
cargo check -p indicatrix-web   # must still compile; do not edit it
```

---

## WP8 — Transport gating, metrics, cache key, GPU fallback

**Goal.** The CPU tracer and all metrics trace `StoneGeometry`; convexity
shortcuts are gated; the GPU backend falls back to CPU for concave scenes; the
furnace test runs on the concave fixture.

**Depends on.** WP4 (and WP6 only for the fixture; if WP6 is not merged yet,
build the fixture's tools by hand from `ToolPrimitive::cylinder` in the test).
**Parallel with.** WP3.

**Files.** `crates/indicatrix/src/optics/raytracer/transport/inner.rs`,
`transport/bounce.rs` (signature threading), `refraction/exit_split.rs`,
`refraction/context.rs`, `scattering/hg.rs`, `scattering/frosted.rs`,
`color/metrics/{ray_trace.rs,classify.rs,scintillation.rs,sweep.rs,profile.rs,cache.rs,fan.rs}`,
`render_setup/plane_hash.rs`, `renderer/gpu_backend/mod.rs` (+`backend.rs`),
`renderer/gpu/hybrid.rs` (make `forced_cpu_only` reachable),
`renderer/frame_scene.rs`/`guide_pass.rs` (clone tools), the public tracer
entry points that take `&[GpuFacetPlane]` (add `*_geom` twins, keep old ones
as wrappers), `tests/raytracer_tests/white_furnace_energy_conservation.rs`,
`docs/physics.md:213`, `docs/gpu.md` (one line).

**Signatures (exact).**

```rust
// render_setup/plane_hash.rs
#[must_use] pub fn hash_geometry(geom: StoneGeometry<'_>) -> u64; // == hash_planes(planes) when tools is empty
// renderer/gpu_backend
#[must_use] pub fn scene_routes_to_gpu(material: &GemMaterial, geom: StoneGeometry<'_>) -> bool; // material.gpu_supported() && geom.is_convex()
```

Behaviour per plan §5.3 table, row by row. Specifically:
- bounce loop: `intersect_stone_soa`; `inside_gem` toggling unchanged;
  `debug_assert!` on point classification when `!is_convex()`.
- `try_split_exit_channel`: keep the truncation; add a per-trace counter of
  truncated energy that the furnace test can read (a `cfg(test)`/`debug` stat
  on the trace context, zero cost in release).
- HG NEE: loop crossings up to 4, Fresnel at each exit.
- frosted exterior NEE: real shadow probe when `!is_convex()`.
- `shading_normal_near_edge`: skipped for tool hits and for plane hits whose
  nearest neighbour would be a tool (pass `geom`; when convex, call the
  existing function unchanged).
- metrics: every `intersect_polyhedron_soa` call replaced by
  `intersect_stone_soa` with the scene's tools; cache keys via `hash_geometry`.
- Guide buffers / denoise: facet ids above `planes.len()` must encode; check
  `renderer/buffers/camera.rs:140`.

**Invariants.** With empty tools every code path is the old one: the pin suite
(`golden_regression`, both furnace tests, `studio_rig_trace_output_is_pinned_to_current_values`)
is byte-identical. No `.wgsl` edits. `cargo test -p indicatrix` must pass
without `--features gpu`.

**Tests to add.**
- `concave_fixture_white_furnace_energy_conservation_holds` (same bound as the
  existing furnace tests; prints the exit-split truncation separately).
- `hash_geometry_equals_hash_planes_when_there_are_no_tools`.
- `scene_routes_to_gpu_is_false_whenever_tools_are_present` and the
  `HybridStats::forced_cpu_only` assertion.
- `metrics_on_a_planar_design_are_unchanged_by_the_geometry_entry_point`
  (`to_bits` equality of the metrics struct between old and new entry points).
- `golden_regression.rs`: a new `to_bits` table for the concave fixture only.

**Gate.**
```
cargo clippy -p indicatrix --all-targets -- -D warnings
cargo test -p indicatrix
cargo test -p indicatrix --test raytracer_tests
```

---

## WP9 — Solid preview, diagram, facet map, caches

**Goal.** `indicatrix-solid` renders, labels, picks and diagrams a concave mesh
with planar output byte-identical.

**Depends on.** WP5 (mesh fields). **Parallel with.** WP6.

**Files.** `crates/indicatrix-solid/src/raster/render.rs` (per-ring normal
path), `raster/overlays.rs` (label placement), `edges_layer.rs`
(`edge_visible`), `diagram2d/layout.rs` + its renderer (block filter),
`facet_map/mod.rs` + `facet_map/build.rs` (`FacetKind`, id range),
`mesh_cache.rs` (key over tools), `preview/request.rs`, `preview/render.rs`,
`preview/types.rs`, `preview/state.rs` (`StoneGeometryBuf`), `preview/tests.rs`.

**Types (exact; plan §8).**

```rust
// preview/types.rs
#[derive(Debug, Clone, PartialEq)]
pub struct StoneGeometryBuf { pub planes: Vec<GpuFacetPlane>, pub tools: Vec<ToolPrimitive>, pub placements: Vec<(usize, usize)> }
impl StoneGeometryBuf {
    #[must_use] pub fn planes_only(planes: Vec<GpuFacetPlane>) -> Self;
    #[must_use] pub fn as_geometry(&self) -> StoneGeometry<'_>;
    /// FNV-1a over plane f32 bits, then over tool bytes only when tools exist (== today's key when empty).
    #[must_use] pub fn cache_key(&self) -> u64;
}
// facet_map
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FacetKind { Flat, Concave { tier: usize, placement: usize } }
// FacetInfo gains `kind: FacetKind` (3 literal sites)
```

`RedrawRequest::Reproject { planes, .. }` and `RenderedFrame.planes` become
`geometry: StoneGeometryBuf` (update every constructor; grep `Reproject {`).
Raster: when `mesh.piece_normals` is `Some`, both passes read the ring's normal
from it (cull + shade per ring); otherwise the existing per-facet table.
Diagram: tool pieces are shown only in the panel of their tier's block
(`placements` → tier → sign of φ comes through `FacetKind`/`FacetMap`), and
only when the panel predicate passes for the piece normal. Labels: largest
visible piece's centroid for multi-piece facets. Hover text for concave
facets: `"{tier name} {CODE} θ {theta:.1}° D {d:.3}"`.

**Invariants.** `cargo test -p indicatrix-solid` — every pin in
`preview/tests.rs` (`web_frames_reproduce_the_desktop_pins` and the rest) is
unchanged; `cargo test -p indicatrix-cut --test solid_identity_pins`
(`solid_identity_pin_crackotto_step_full_pipeline`, `solid_identity_pin_box`)
unchanged. `simplify_ring` untouched.

**Tests to add** (`preview/tests.rs`, `raster/tests`, `facet_map/tests`):
- `concave_fixture_frames_are_pinned` (FNV over solid RGBA, pick, diagram for
  the three standard views; fixture built from hand-made `ToolPrimitive`s so
  the crate does not depend on cut-core).
- `pick_buffer_ids_on_concave_fixture_resolve_to_tier_and_placement`.
- `concave_raster_has_no_hairline_gaps` (no unset pick pixel whose 4
  neighbours are all stone).
- `diagram_hides_undercut_tool_pieces_from_the_opposite_panel`.
- `cache_key_equals_the_planar_key_when_tools_are_empty`.

**Gate.**
```
cargo clippy -p indicatrix-solid --all-targets -- -D warnings
cargo test -p indicatrix-solid
cargo test -p indicatrix-cut --test solid_identity_pins
cargo check -p indicatrix-editor -p indicatrix-cut -p indicatrix-web
```

---

## WP10 — Desktop UI (`apps/indicatrix-cut`)

**Goal.** Author, view and print concave tiers on the desktop; preview and
render them locally; refuse remote rendering until WP12b.

**Depends on.** WP7, WP8, WP9 (WP6 transitively). **Parallel with.** nothing.

**Files.** `apps/indicatrix-cut/ui/components/editor_tier_table.slint`,
`editor_tier_table/tier_table_toolbar.slint`, `editor_command_bar.slint`,
`editor_inspector/tier_form_tab.slint`, `editor_inspector/schedule_tab.slint`,
`cutting_table.slint`, `ui/types.slint` (new `ConcaveFormData` struct),
`src/gui/editor/callbacks/tier_actions/{tier_form.rs,tier_crud.rs,mod.rs}`,
`src/gui/editor/mod.rs` (callback wiring), the preview/solid request site that
builds `RedrawRequest::Reproject` (now `StoneGeometryBuf` via
`design_to_gpu_geometry`), the local render path (pass tools), the remote
submit path (`src/gui/remote/orchestrator/tick/dispatch.rs`: refuse when tools
non-empty with a message), guide locks (`GuideModel.allows_*` + a new
`GuideAllow` entry), `docs/manual/15-planning-a-rough.md:741` (replace the
"cannot model a concave notch" sentence), new manual chapter
`docs/manual/<next number>-concave-tiers.md`.

**Slint contract (exact).**

```slint
export struct ConcaveFormData {
    name: string, angle_deg: string, indices: string, instructions: string,
    tool: string, tool_azimuth_deg: string, x: string, y: string, z: string,
    diameter_ratio: string, tool_angle_deg: string, reciprocating: bool,
}
// EditorModel gains:
callback save_concave_tier(int /*index, -1 = add*/, ConcaveFormData);
callback add_concave_tier();
in property <bool> inspector_concave_mode;
```

`EditorTierItem` gains `kind: int` (0 flat, 1 concave) and `tool_line: string`;
it must stay `Send` (no `ModelRc`). `save_tier`'s six-argument signature is
**not** changed. Rows are ordered by `cutting_order()` (already done in
WP7's view models; the UI just displays). Tool angle field enabled for
CON/DSC only. Error borders reuse `tier_form_error_field` prefixes
(`"theta:"`, `"x:"`, … from WP7).

**Invariants.** `cargo test -p indicatrix-cut` (all identity pins) unchanged;
no new i18n; Theme tokens only. The web app is not edited.

**Tests to add.** There are no UI tests; add
`apps/indicatrix-cut/tests/concave_tier_round_trip.rs`: build the concave
fixture through the public editor session API as the callbacks do (form →
`concave_tier_save_edit` → `EditorSession`), save/load the native file, and
pin the cutting-sheet text and the solid pick FNV for one view.

**Gate.**
```
cargo clippy -p indicatrix-cut --all-targets -- -D warnings
cargo test -p indicatrix-cut
```

---

## WP12a — Vault columns, migration and search filter

**Goal.** The library stores and filters by concave counts.

**Depends on.** Nothing. **Parallel with.** WP1, WP4.

**Files.** `crates/indicatrix-vault/src/db/sqlite/mod.rs` (append
`db.migrate_concave_columns()?;` after `migrate_custom_material_specific_gravity`
at `mod.rs:365`), the migration itself in `db/sqlite/migrations/` (the
existing `migrate_*` functions live in `migrations/{mod.rs,tables.rs,schema.rs}`
with helpers in `migrations/helpers.rs`; `mirror_state.rs:26` is a small
self-gating example to copy), `db/sqlite/base_schema.rs`, `model/filter.rs`
(`RangeFilter.has_concave: Option<bool>`), `db/sqlite/search/predicate.rs`,
the `angle_settings` model/row types (two nullable TEXT columns), tests.

**Schema (exact).**
```
diagram_details.concave_tiers  INTEGER NOT NULL DEFAULT 0
diagram_details.concave_facets INTEGER NOT NULL DEFAULT 0
angle_settings.tool      TEXT      -- NULL for flat rows
angle_settings.tool_line TEXT      -- the formatted second line, NULL for flat rows
```
`facets_count` TEXT and its `"55+6"` parser (`model/facets.rs`) are untouched.

**Invariants.** Existing vault tests pass; the existing "import survives a
catalogue migrated from the old column layout" test still passes; migrating an
already-migrated DB is a no-op.

**Tests to add.** `fresh_database_has_the_concave_columns`,
`migrate_concave_columns_is_idempotent_on_an_old_layout`,
`has_concave_filter_matches_only_designs_with_concave_tiers`.

**Gate.**
```
cargo clippy -p indicatrix-vault --all-targets -- -D warnings
cargo test -p indicatrix-vault
```

---

## WP12b — Wire protocol 19 and worker validation

**Goal.** Remote rendering of concave scenes: `SceneState.tools`, worker
validation and render core, library fetch of the native design.

**Depends on.** WP4, WP8, WP12a (only for `angle_settings.tool_line` in the
library record). **Parallel with.** WP7.

**Files.** `crates/indicatrix-net/src/scene/mod.rs` (+ all 75 `SceneState { .. }`
literal sites across 35 files: `rg "SceneState \{" crates apps`),
`crates/indicatrix-net/src/messages/mod.rs` (`PROTOCOL_VERSION` 18 → 19, pin
test `protocol_version_matches_constant`, README history), library messages
(`DesignSummary.concave_tiers: u32`, `AngleSettingWire.tool_line: Option<String>`,
`LibraryRequest::FetchDesignNative { .. }` + its response),
`apps/indicatrix-worker/src/validate/mod.rs`, `render_core/mod.rs`
(`StoneGeometry` through `build_plane_soa`, `GpuSceneRef`,
`girdle_facet_finishes`, denoise guide), `serve/tilt.rs`,
`serve/library/version_hash.rs` (append the new fields), the desktop remote
orchestrator (`apps/indicatrix-cut/src/gui/remote/...`: lift the WP10 refusal
and send tools).

**Wire (exact).**
```rust
// SceneState, appended LAST, after surface_glare:
#[serde(default, skip_serializing_if = "Vec::is_empty")]
pub tools: Vec<ToolPrimitive>,
pub const PROTOCOL_VERSION: u16 = 19;
```
`validate_scene`: every `ToolPrimitive::validate` ok, `tools.len() <= MAX_TOOL_PRIMITIVES`.

**Invariants.** `indicatrix-web-core` untouched (its protocol stays 8).
`scene.json` fixtures without `tools` still load (`serde(default)`).
`SOURCE_HASH`/`BUILD_ID` gating untouched.

**Tests to add.** `scene_state_with_and_without_tools_round_trips_through_postcard`,
`validate_scene_rejects_too_many_or_invalid_tools`,
`protocol_version_matches_constant` updated to 19,
`design_summary_version_hash_changes_when_concave_fields_change`.

**Gate.**
```
cargo clippy -p indicatrix-net -p indicatrix-worker -p indicatrix-cut --all-targets -- -D warnings
cargo test -p indicatrix-net
cargo test -p indicatrix-worker
cargo check --workspace
```

---

## WP13 — web-core compile sweep and workspace gate

**Goal.** `indicatrix-web-core` compiles and its tests pass with only the
changes forced by shared types; the whole workspace is green.

**Depends on.** Everything above. **Parallel with.** nothing.

**Files.** `crates/indicatrix-web-core/src/**` compile fixes only (new `Design`
fields in literals, renamed cut-core functions). **No** `tools` field in
`SceneSpec`/`MetricsParams`/`TiltParams`/`SolveOutcome`; `PROTOCOL_VERSION`
stays 8; `wire_format_is_pinned` unchanged. `apps/indicatrix-web` and
`apps/indicatrix-web-compute`: compile fixes only if the workspace build
requires them, nothing else.

**Gate.**
```
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```
plus the pin suite one last time.

---

## Deferred: WP-G (hardware session) — GPU tool intersection

Not scheduled here. Needs a machine with a GPU adapter. Scope: `tools` storage
buffer, intersection in `intersect_polyhedron.wgsl`,
`transport_bounce/05_intersect_and_edge_rounding.wgsl` (`intersect_ray`),
`transport_functions/08_nee_and_intersect.wgsl` (`tf_intersect_ray`), the NEE
probes in `06_nee_sampling.wgsl`, both `shading_normal_near_edge` copies,
`phase1_layout_echo.wgsl` for `ToolPrimitive`, `polyhedron_check.rs`-style
`tool_check.rs` (Tier 2), a Tier-3 image comparison on the concave fixture,
removal of the `is_convex()` term in `scene_routes_to_gpu`, regenerated
`docs/gpu_harness_last_run.json`. Until then the CPU fallback from WP8 is the
shipped behaviour.
