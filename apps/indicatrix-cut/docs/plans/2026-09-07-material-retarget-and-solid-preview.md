# Implementation plan: material-aware editing and a solid inspection preview

Date: 2026-09-07. Scope: `apps/indicatrix-cut`, `crates/indicatrix-cut-core`, and small additive changes in `crates/indicatrix` and `crates/indicatrix-formats`. Written after the 2026-09-06 review findings were closed; it assumes today's landed work (Adopt re-solves, tilt-curve cache keyed on the full material, the latest-wins redraw gate on the display path, 18 new render materials, wavelength-dependent birefringence on CPU and GPU).

The plan has two parts that share one dependency, the design's material, and one bottleneck, the meet-point solve.

- Part A gives a design a single, editable notion of material, refractive index, index gear and symmetry, and a guided way to retarget an existing cut for another stone.
- Part B adds a flat-shaded solid preview in edit mode that updates on every edit, with facet picking and a critical-angle overlay, so a cutter sees the stone they are building without waiting for the path tracer.

---

## Part A. Material, refractive index, gear and symmetry

### A.1 Current state (verified in code)

Three unrelated notions of "material" exist today.

| Notion | Where it lives | What reads it | How it is set |
|---|---|---|---|
| Schedule refractive index | `indicatrix_cut_core::design::ScheduleMeta::refractive_index` | `.asc` export header only | Copied from the imported `.asc`; hard-coded 1.54 by `Design::fresh` |
| Yield material | `indicatrix_cut_core::material::MaterialSelection` on `Design.material` | Carat-weight yield report; `optimize_solve` resolves it to a `GemMaterial` by name (falls back to diamond) for the optimizer objective | Edit tab "Material" combo plus SG override |
| Render material | `RenderContext.material_name` (viewport "Material" dropdown) | Path tracer, tilt-curve window | Viewport dropdown, independent of the design |

Consequences:

- The optimizer scores windowing, extinction and tilt brilliance for whichever yield material happens to be selected, or for diamond when none is, regardless of what the cutter renders or exports.
- `Design::fresh` hard-codes 96 teeth, 8-fold symmetry, RI 1.54, and no `Edit` variant can change gear, symmetry, mirror or RI afterwards, so undo/redo cannot cover them either.
- Adjusting a loaded design for another stone means retyping every angle by hand and pressing Solve.

The meet-point solver itself is purely geometric. RI matters for: the critical angle (a pavilion facet below it windows), the optimizer objective, the tilt curves, and the `.asc` header. That makes the retarget problem tractable: geometry stays solvable, only the angle targets move.

### A.2 Goals and non-goals

Goals:

1. One source of truth for a design's material: a `MaterialSelection` that resolves to a `GemMaterial` (built-in or custom catalogue material) and to a schedule RI; the viewport, optimizer, tilt curves, yield report and `.asc` header all derive from it.
2. Gear, symmetry order and mirror become editable, undoable design properties for both new and loaded designs.
3. A guided "Retarget for material" action that proposes new angles for another RI, shows the diff with critical-angle margins, and applies it as one undoable edit followed by a Solve.
4. Catalogue loads suggest a material from the schedule RI.

Non-goals: per-facet materials, coatings, the Berreman anisotropic Fresnel (review finding P2 roadmap), and any change to solver decisions.

### A.3 Data model changes (`indicatrix-cut-core`)

1. `MaterialSelection` gains `refractive_index_override: Option<f64>` next to the existing SG override, and a resolver `MaterialSelection::resolve(&self, catalogue: &dyn MaterialLookup) -> ResolvedMaterial { gem: GemMaterial, n_d: f64, critical_angle_deg: f64 }`. The trait keeps `indicatrix-cut-core` free of `indicatrix-vault`; the editor implements it over built-ins plus `custom_gem_materials` (which now carries per-axis dispersion, migration Q9).
2. `ScheduleMeta::refractive_index` becomes derived: `Design::effective_refractive_index()` returns the override, else the resolved material's n_D, else the legacy schedule value. `to_asc_schedule` writes the effective value. The legacy field stays for round-trip fidelity of untouched imports.
3. New `Edit` variants, all validated and inverted by the existing `edit` module so `History` covers them:
   - `SetSchedule { gear_teeth: i32, symmetry_order: u32, mirror: bool }` (wholesale, like `SetMaterial`).
   - `RemapIndices { from_gear: i32, to_gear: i32, rounding: RemapRounding }` applied to every tier's `indices` and `detached` lists; the inverse restores the original vectors verbatim (store them in the edit), so undo is exact even when rounding was lossy.
   - `RetargetAngles { changes: Vec<(usize, f64 /* old */, f64 /* new */)> }` (one undoable step for a whole retarget).
   `SetMaterial` keeps its shape; the RI override rides inside `MaterialSelection`.
4. `Design::fresh` takes a `FreshDesignSpec { gear_teeth, symmetry_order, mirror, material: MaterialSelection, preform }` instead of four positional values; the editor's "New" dialog fills it.
5. Critical-angle helpers in `indicatrix_cut_core::optics_hints` (new, thin): `critical_angle_deg(n)`, `tier_margin_deg(tier_angle, n)`, `windowing_risk(tier, n) -> Risk { Safe, Marginal(<2°), Windows }`. Pure functions, unit tested, shared by Part B's overlay.

### A.4 Retarget algorithm

Two modes, both producing a `RetargetAngles` proposal the user reviews before applying.

**Critical-angle shift (deterministic, default).** For each pavilion tier with angle `θ` under the old index `n0`, keep its margin above the critical angle: `θ' = θc(n1) + (θ − θc(n0))`, where `θc(n) = asin(1/n)` in degrees. Crown tiers are shifted by a user-selectable fraction of the same delta (default 0, matching common practice of leaving the crown alone), with a "scale crown by ratio" option `θ' = θ · θc(n1)/θc(n0)`. Girdle tiers are never touched. Angles are clamped to the optimizer's safety bound (89.5°) and tiers that would fall below the new critical angle are flagged, not moved past it.

**Optimize for material.** Runs the existing coordinate-search optimizer with the resolved target material as the objective, seeded from the critical-angle shift, over the free (non-anchored) tiers only. This reuses `optimize_solve` unchanged except for the material source; anchored tiers must be adopted first, and the dialog says so.

The proposal dialog shows one row per tier: block, old angle, new angle, margin over the new critical angle, and the risk badge. Apply pushes one `RetargetAngles` edit, then calls the same `refresh_all` Solve path the Solve button uses. Undo reverts the angles in one step.

### A.5 Editor UI

- **Design settings panel** in the Edit tab (above the tier list): material combo (all built-ins, then custom catalogue materials, then "custom RI…"), RI readout with override field, critical angle readout, gear combo (96, 80, 77, 72, 64, 120, custom), symmetry order, mirror toggle. Changing gear opens the index remap confirmation (shows non-integral remaps in red).
- **Viewport material follows the design** while the Edit tab is active; the viewport dropdown becomes a display override with a "linked to design" checkbox (default on), so the render, the tilt curve and the optimizer agree.
- **Tier list** gains a "margin" column and the risk badge from A.3(5).
- **"Retarget for material…"** button next to Optimize, opening the proposal dialog from A.4.
- **New design dialog** replacing the bare "New" action: preform, gear, symmetry, mirror, material.
- **Load from catalogue**: after loading, suggest the built-in material whose n_D is nearest the schedule RI (within 0.01) with a toast "Set material to Quartz (RI 1.544)?"; never silently change the schedule RI of an import.

### A.6 Persistence

- `.asc` export writes the effective RI, gear, symmetry and mirror from the design (already does for the legacy fields; the change is only where the value comes from).
- `.gemcut.toml` gains `[material] name, sg_override, ri_override` and `[schedule] gear_teeth, symmetry_order, mirror`; missing keys load as today (backwards compatible).
- The catalogue's `custom_gem_materials` needs no further schema change; per-axis dispersion (Q9) already exists.

### A.7 Tests

- `indicatrix-cut-core`: each new `Edit` applies and inverts exactly (property test over random tier lists); `RemapIndices` undo restores lossy remaps verbatim; retarget math on a synthetic pavilion (margin preserved to 1e-9); risk classification at the boundaries; `Design::fresh` spec round-trips through `.gemcut.toml`.
- Editor state tests (no Slint): retarget proposal for RBC-445 from diamond (2.417) to quartz (1.544) lists every pavilion tier with the expected shift; applying then undoing leaves the design byte-identical; material change re-solves nothing (geometry unaffected) but invalidates the tilt-curve cache key.
- Golden `.asc` export for a retargeted design.

### A.8 Milestones

| # | Deliverable | Files (main) | Effort |
|---|---|---|---|
| A1 | `MaterialSelection` resolver, effective RI, `optics_hints` | `indicatrix-cut-core/src/material.rs`, `design/export.rs`, new `optics_hints.rs` | 1.5 days |
| A2 | New `Edit` variants + `History` coverage + `FreshDesignSpec` | `indicatrix-cut-core/src/edit/*`, `design/construct.rs` | 2 days |
| A3 | Design settings panel, viewport link, tier-list margin column | `ui/components/editor_view.slint`, `src/gui/editor/{state,view,callbacks}` | 2 days |
| A4 | Retarget dialog (both modes) | new `ui/components/retarget_dialog.slint`, `src/gui/editor/retarget.rs` | 2.5 days |
| A5 | New-design dialog, catalogue material suggestion, persistence | `ui/components/new_design_dialog.slint`, `src/gui/editor/{loading,native_io}.rs` | 1.5 days |
| A6 | Manual chapter 6 and 7 rewrite, appendix update | `docs/manual/06-*.md`, `07-*.md` | 0.5 day |

Order: A1 → A2 → A3 → A4 → A5 → A6. A3 is the first user-visible step and can ship alone.

### A.9 Risks

- Optimizer objective changes when the material becomes design-driven: existing optimize tests pin diamond; add the material explicitly in those tests before A3 lands.
- Index remap on non-multiple gears (96 → 80) is lossy; the confirmation dialog and exact undo are the mitigation, not a cleverer rounding.
- Custom catalogue materials with only a single RI and no dispersion render as non-dispersive; state it in the material combo tooltip.

---

## Part B. Solid inspection preview in edit mode

### B.1 Current state (verified in code)

- The Edit tab shows the same path-traced `GemViewportView` as the Live Render tab. First useful frames take seconds on CPU and are noisy; there is no wireframe or shaded mode anywhere in the UI.
- `indicatrix::geometry::stone_metrics::build_solid_mesh(&[(normal, offset)]) -> SolidStatus` already produces, for a closed solid, a `SolidMesh` with per-vertex positions, flat per-facet normals, `facet_id` per vertex, triangle indices (centroid fans) and each face's ordered polygon ring. The editor calls it only to compute the Closed / Degenerate / Unbounded status; the mesh is thrown away.
- The editor converts a design to planes with `design_to_gpu_planes` after a Solve. Any edit other than Solve marks the panel stale and does not re-solve (0.4 to 1.7 s on real designs since the refinement-sweep cap of 2026-09-07; 5.9 s before). `indicatrix-cut-core` has `resolve_dirty`, which re-solves only the edited tier and non-anchor tiers, and is benchmarked on the 103-tier design.
- The display path now has `RedrawGate` (latest-wins generation counter, one pending UI closure) and converts frames off the UI thread.

### B.2 Goals

1. A flat-shaded, edge-outlined view of the current design that redraws within one display interval (33 ms) after any edit, without a GPU.
2. Facet hover and click map to tiers (via `facet_id`), highlighting the tier and its orbit in the list, and the reverse (selecting a tier highlights its facets).
3. Overlays: critical-angle risk (from A.3) hatched on facets, preform ghost, block colouring, angle labels on hover.
4. View modes: Solid, Path-traced, Both (solid edges over the path-traced image), remembered in settings. The solid view shares the camera with the path tracer so switching does not move the stone.

### B.3 Approach chosen: CPU software rasterizer

Options considered:

| Option | Pros | Cons |
|---|---|---|
| **CPU rasterizer into a `SharedPixelBuffer`** (chosen) | No GPU dependency; works with remote/headless and the web build; a 100-facet stone is about 1–2 k triangles, well under 5 ms at 800×600 single-threaded; reuses the existing `Image::from_rgba8` path and `RedrawGate` | Must write a small z-buffered triangle rasterizer (roughly 300 lines) |
| wgpu raster pipeline on the existing GPU context | Fastest; could later draw the path trace and solid in one pass | Needs the `gpu` feature; readback or Slint texture import; another shader to keep in parity; GPU mutex is shared with the tracer |
| Slint `Path` polygons | No pixels to manage | No depth test (painter's order only), no lighting control, hundreds of elements re-laid out per edit |

The rasterizer takes `&SolidMesh`, a `Camera` (the same yaw/pitch/distance/FOV the tracer uses) and a `SolidStyle`, and returns an RGBA buffer plus a per-pixel `facet_id` buffer for picking. The mesh input is the seam that lets a wgpu backend replace it later without touching the editor.

### B.4 Architecture

New module `src/gui/solid_preview/`:

- `raster.rs`: `SolidRasterizer { width, height, depth: Vec<f32>, color: Vec<u8>, pick: Vec<u32> }` with `render(&mut self, mesh: &SolidMesh, camera: &Camera, style: &SolidStyle)`. Perspective projection with the tracer's camera, back-face culling, edge-function triangle fill with a depth test, Lambert shading from `normals` with a fixed key light plus a small rim term so near-vertical facets stay readable, edges drawn from `rings` after the fill (1 px, darker), and hatching for flagged facets. Pure Rust, no Slint types, fully unit-testable.
- `mesh_cache.rs`: `SolidMeshCache { key: PlanesHash, mesh: Arc<SolidMesh> }`. Keyed by the hash the tilt-curve cache already uses for the plane set. Rebuilt when the planes change; `build_solid_mesh` itself is cheap compared to the solve.
- `preview_state.rs`: the editor-side controller. Decides which geometry to draw (see B.5), drives the rasterizer on the display thread through `RedrawGate<SolidFrame>`, and pushes the finished `Image` to a new `solid_image` property on the viewport.
- `pick.rs`: maps a viewport coordinate to `facet_id` → plane index → tier index and orbit member, using the pick buffer of the last frame.

UI: `gem_viewport.slint` gets a second `Image` layer bound to `solid_image`, a `view_mode` enum property (Solid / Traced / Both), hover/click callbacks reporting pixel coordinates, and a small legend. In Both mode the solid layer is rendered with a transparent fill and opaque edges.

### B.5 What geometry to draw after an edit

The preview needs planes, and planes need masts. Three tiers of freshness, chosen automatically:

1. **Pinned tiers (ScaleReference)** carry their mast directly; a design where every tier is pinned (every catalogue import) previews instantly with no solver call.
2. **Free tiers** after an edit go through `resolve_dirty` with a time budget (default 50 ms, measured on the 103-tier design before choosing). If it finishes, the preview shows the freshly resolved solid.
3. **Budget exceeded**: draw the last solved solid, with the edited tier's facets outlined in the "pending" colour and the panel's existing "Not solved" banner. Pressing Solve refreshes everything, as today.

Every edit callback that currently calls `refresh_editor_panel_stale` also calls `preview_state.request_redraw(design)`; the gate coalesces bursts (typing in the angle field) to the latest state.

### B.6 Interaction details

- Hover: facet under the cursor lights up, tooltip shows tier name, angle, index, block and margin over the critical angle.
- Click: selects the tier in the list (existing selection mechanism) and scrolls it into view; the whole orbit is tinted.
- Selecting a tier in the list tints its facets in the preview (reverse link).
- Critical-angle overlay uses `optics_hints::windowing_risk` with the design's effective RI from Part A; before A1 lands it uses the viewport material's n_D, which keeps B independent of A for the first release.
- Camera: shared `yaw/pitch/distance` with the path-traced view; dragging in Solid mode moves both.

### B.7 Performance budget and verification

- Rasterizer: under 5 ms for 2 k triangles at 800×600 on one core (benchmark in `benches/` or an `#[ignore]`d timing test on the 103-tier design), under 1 ms for RBC-445.
- Mesh build: measure `build_solid_mesh` on the 103-tier design; if it exceeds 10 ms, move it to the display thread with the frame.
- UI thread: only the image swap runs there (same rule as today's `RedrawGate`).
- Tests: a unit cube renders the expected coverage and depth ordering; a stone with a facet behind another never paints the hidden one; pick buffer returns the right `facet_id` at known pixels; the preview state chooses tiers 1/2/3 of B.5 correctly for pinned-only, cheap-free and over-budget designs (inject a fake solver); the coalescing test from today's redraw gate is reused.

### B.8 Milestones

| # | Deliverable | Files (main) | Effort |
|---|---|---|---|
| B1 | Rasterizer with tests, renders a `SolidMesh` to a PNG in a test | `src/gui/solid_preview/raster.rs` | 2 days |
| B2 | Viewport layer, view-mode toggle, static preview after Solve | `ui/components/gem_viewport.slint`, `preview_state.rs`, `mesh_cache.rs` | 1.5 days |
| B3 | Live update on edit via `resolve_dirty` budget and `RedrawGate` | `preview_state.rs`, `src/gui/editor/callbacks/*` | 2 days |
| B4 | Picking, hover, selection links, overlays | `pick.rs`, `editor_view.slint`, tier-list wiring | 2 days |
| B5 | Both mode, settings persistence, manual chapter | `settings/model/*.rs`, `docs/manual/03-*.md` | 1 day |

Order: B1 → B2 → B3 → B4 → B5. B2 alone already gives a useful "solid after Solve" view.

### B.9 Risks

- `resolve_dirty` may exceed the budget on large designs with many free tiers; the fallback in B.5(3) keeps the UI responsive, and the measured numbers decide the default budget.
- The solid status can be Degenerate or Unbounded mid-edit; draw whatever `build_solid_mesh` returns for the closed subset and show the status banner, never an empty viewport.
- Slint image updates at 30 Hz with two layers: keep the solid image at the viewport's logical size, not the render resolution.

---

## Shared dependencies and follow-ups

- **Solve time (review finding G3)**: today's candidate cache was measured and rejected (27 % slower, nearly every tier moves on every one of the 16 sweeps). Both parts benefit from fewer sweeps rather than cheaper sweeps: the next attempt should measure convergence per sweep on the 103-tier design and try damping or ordering changes only if outputs stay byte-identical, which the domain rules require.
- **Catalogue re-sync**: 214 designs imported from facetdiagrams.org carry leaked page text or a single stored tier; run the private loader once after the parser fix so Part A's "suggest material from RI" and Part B's instant pinned preview see real schedules.
- **Spectral splitting (P6)**: a design note exists in `refraction.rs`; independent of this plan.
- **Benchmark checksum**: `examples/simd_bench.rs` carries a stale pinned checksum after today's physics changes; re-pin it in the next perf pass.
