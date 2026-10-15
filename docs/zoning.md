# Rough Color and Zoning: Developer Notes

Zoning adds the "Rough Color" wizard, a zoned absorption model for the tracers, and everything
between them. It is always built and always shown: there is no `zoning` Cargo feature any more.
The protocol constants are net 24 and web-core 15.

The user chapter is `apps/indicatrix-cut/docs/manual/24-rough-colour-and-zoning.md`. The
user-facing spectral model is the smooth basis (model B). The chromophore model (A) stays in the
solver and is shown only with the `physical-color` feature (`PHYSICS_COLOR_UI`).

## Where It Lives

| Crate | What |
|---|---|
| `indicatrix` | `optics::zoning`, `optics::fresnel`, `color::led`, the zoned branch of the CPU segment absorption, the zone table in the GPU material layout and the zoned WGSL |
| `indicatrix-cut-core` | `rough_plan::{photometry, camera_spectral, colour_fit, zoned_plan}`; `rawler` and `image` are plain dependencies |
| `indicatrix-net` | the `ZoningPayload` wire extension (with the `render` feature, which brings the zone types) |
| `indicatrix-worker` | accepting zoned scenes (with the `worker` feature) |
| `indicatrix-vault` | the side tables |
| `indicatrix-cut` | the wizard, storage, planner and adopt integration |

`indicatrix-web-core` has no zoning payload path: it decodes `SceneState` only, so zoned rendering
on the web workers does not exist yet.

`rawler` 0.8.0 (pure Rust, LGPL-2.1) is the RAW decoder and a dependency of `indicatrix-cut-core`
only. LGPL-2.1 linked statically into a distributed binary has relinking obligations: decide that
before a release.

The Slint global `Zoning` (`ui/models/zoning.slint`, named in `app.slint`'s export block) carries
the `Zoning.enabled` flag the Rust side sets per window (`apply_to_main`, `apply_to_planner`, ...);
every zoning element sits under `if Zoning.enabled`. A new window with zoning elements needs its
own setter.

## Crate Map

| Where | What |
|---|---|
| `crates/indicatrix/src/optics/zoning/` | `ZonedAbsorption`, `Zone`, `ZoneShape`, `ZoneFrame`, validation, optical depth; `kernels.rs` (path lengths, one macro source instantiated as f64 `k64` and f32 `k32`); `mesh.rs` (closed mesh shells) |
| `crates/indicatrix/src/optics/fresnel.rs` | `fresnel_dielectric(cos_i, n_from, n_to)` |
| `crates/indicatrix/src/color/led.rs`, `data/cie_illum_leds_5nm.csv` | the nine CIE 15:2018 LED illuminants (`Illuminant::Led(LedKind)`) |
| `crates/indicatrix/src/optics/raytracer/{zoned.rs,absorption.rs,scattering/hg.rs}` | the zoned main CPU tracer; the field is `GemMaterial::zoning` (`serde(skip)`) |
| `crates/indicatrix/src/optics/materials/zoned.rs` | `GemMaterial::with_zoning`, `zone_swatches` |
| `crates/indicatrix/src/renderer/buffers/zoning.rs`, `renderer/shaders/zoning/` | the GPU zone table (`zones` at offset 576 of `GpuGemMaterial`), `01_zone_table.wgsl`, `02_zone_kernel.wgsl`, `03_zone_absorption.wgsl` |
| `crates/indicatrix-cut-core/src/rough_plan/photometry/` | decode (RAW/TIFF/PNG/JPEG), dark and white frames, noise model, HDR merge, masks, working-resolution resample |
| `.../rough_plan/camera_spectral/` | camera sensitivity tiers (measured, filter calibration, matrix fallback), backlight spectrum and tilt |
| `.../rough_plan/colour_fit/forward/` | the rig path tracer, record compression, disk cache, evaluation and Jacobians |
| `.../rough_plan/colour_fit/solve/` | models B and A, Levenberg-Marquardt, selection, leave-one-view-out, predictions with uncertainty and metamer spread |
| `.../rough_plan/colour_fit/zones/` | zone fits from marks, the editing backend with locks, overlay projection, suggestions, joint refinement |
| `.../rough_plan/zoned_plan/` | zones into a stone's frame, pose choice, adopt, codecs |
| `crates/indicatrix-net/src/messages/zoning.rs` | `ZoningPayload` and the handshake marker |
| `crates/indicatrix-vault/src/db/sqlite/zoning_store.rs` | the side tables |
| `apps/indicatrix-cut/src/gui/rough_colour/` | `store` (the storage API), `wizard/`, `handles`, `mesh_paint`, `preview`, `material_zones`, `adopt_link`, `zone_link` |
| `apps/indicatrix-cut/src/gui/rough_plan/{colour_link,zoning_hooks}.rs`, `view/zoning_view.rs` | the planner side |

## Data Flow

```
photos (RAW/TIFF/PNG/JPEG) + white/dark frames
  -> photometry: linear light, transmittance = (stone - dark)/(white - dark), noise, masks,
     working grid (about 384 px)
  -> forward: per pixel, trace the rig once (camera ray, Fresnel or GGX events, zone lengths per
     inside leg), compress to path records (lengths per zone, weight); cached on disk
  -> solve: any candidate absorption is a sum of exponentials over the records, with analytic
     Jacobians; LM, priors, leave-one-view-out, predictions
  -> zones: marks, suggestions, refinement (geometry changes re-trace)
  -> storage: vault side tables, keyed by plan id, material name or job id
  -> render: GemMaterial::with_zoning, the CPU tracer or the GPU zone table, or a ZoningPayload
     to a worker
```

The path geometry does not depend on the absorption being fitted. That is the central design
choice: paths are traced once, and every evaluation of a candidate spectrum is exact.

Units: zone geometry is in millimetres. The tracer works in model units and multiplies points by
`GemMaterial::absorption_path_scale` before calling the kernel. The kernel's per-zone lengths are
used as fractions of the segment, so a segment inside one zone gets exactly the unzoned product.
Zoned materials are always `AbsorptionUnit::PerMm`.

## Twins That Must Stay in Step

- `k64` and `k32` of `optics/zoning/kernels.rs` (one macro source) and the WGSL port
  `renderer/shaders/zoning/02_zone_kernel.wgsl`, function by function. The export of the f32
  kernel's numbers to the GPU is `ZoneKernelF32::export`.
- The forward tracer (`colour_fit/forward`) and the main tracer on the same scene: the white
  furnace cross-check in `tests/zoning_validation/cross_check.rs`.
- `fresnel_dielectric` and the tracer's inline reflectance (the tests transcribe it).
- The mode selection in `raytracer/zoned.rs` and `channel_absorption_alphas_assigned`.
- `GpuZoneTable` in Rust and WGSL (`renderer/gpu/layout_check.rs`, the echo shader
  `zone_table_echo.wgsl`).

After any `.wgsl` edit, validate the concatenated units with naga-cli 30.0.1. `build.rs` appends
the zoning units and applies two anchor patches (`zoned_bounce_shared`) to the shared bounce
text. The anchors panic when they do not occur exactly once, so a later edit of the bounce text
fails the build loudly.
The same units are parsed and validated by `renderer/gpu/shader_validation_tests.rs`.

## Storage: Side Tables

Created by `Database::new` (idempotent `CREATE TABLE IF NOT EXISTS`, no foreign keys, so an
older build deleting a plan, material or job never cascades into them):

| Table | Key | Holds |
|---|---|---|
| `rough_colour` | plan id | version, created, zones JSON, fit JSON |
| `rough_colour_photo` | plan id, view, kind | cached working-resolution photos (f16 transmittance, f32 variance, f16 coverage, u8 mask, grid) |
| `stone_pose_choice` | plan id, layout index, stone index | the chosen pose of a placed stone |
| `material_zoning` | material name (NOCASE) | zones of a custom material; `relative_to_stone` |
| `render_job_zoning` | job id | zones of a queued render job |

`Database::open_read_only` is unchanged: readers answer "nothing" when the tables are missing.
Orphans are removed at startup (`prune_zoning_orphans`). Saved-plan files and material
rows keep their bytes. The API is `gui::rough_colour::store`.

A plan file carries a plan's `rough_colour` row and its `stone_pose_choice` rows as the `[colour]`
table (`version`, `zoned_json`, `fit_json`, `created`) and the `[[pose_choices]]` tables (`layout`,
`stone`, `pose`), plan file schema version 4 (`saved/sections.rs`). Export reads the two tables and
writes the document with them; import takes them out of the document before it is stored and writes
them to the new plan's rows, deleting the plan again if that fails. The cached photos are not
carried. A plan with neither is written and exported unchanged (version 1 to 3).

## Wire: `ZoningPayload`

The protocol stays at 24. `Hello` and `Welcome` have no flags field and postcard is not
self-describing, so the capability is a three-byte marker `ZONING_TAIL = [0x5A, 0x4E, 0x01]`
appended to the postcard `HELLO` by a viewer. Every server ignores trailing bytes on
`HELLO`; a worker answers `WELCOME` plus the marker only when it saw one. A peer that does not
know the extension never sends the marker, so it never receives one. `Hello::zoning` and `Welcome::zoning` are
`serde(skip)`. A zoned scene travels as the unchanged scene plus `ClientMessage::ZoningPayload`
(enum index 9, appended last), keyed by the request id. A client never sends it to a peer
without the bit; it renders locally. The worker routes zoned scenes like the desktop does: to the GPU or hybrid path when the scene is GPU-supported, otherwise to the CPU. A coordinator is
treated as not capable. The test `hello_and_welcome_bytes_are_the_pre_zoning_layout` pins the old
bytes.

## Pins

`GemMaterial`'s derived Debug text has a trailing `, zoning: None`. Two pins
hash that text: `resolved_material_dumps_are_pinned` (`bridge/export_thread/render_setup_pins.rs`)
and `retarget_output_is_pinned` (`gui/editor/identity_pins.rs`). Each hashes a projection of the
dump (`without_zoning_field` removes exactly `, zoning: None`), so the constants stay those of
before the field existed. No re-pin and no `ignore` is needed. A material that really carries zones
prints `zoning: Some(..)`, is not projected away and fails the pin.

Zoned materials in the library: the material editor has a "Zone Geometry Relative to Stone Size"
checkbox (shown only when `Zoning.enabled`). With it on, the zone geometry is stored as fractions
of the stone width and is re-scaled whenever the stone width changes; with it off the zones keep
their millimetre values. The stone width is the design's girdle diameter (the read-only
`SettingsModel.stone_width_mm` mirror shows it; a library row has no design and uses the 7 mm
default). `material_zones::stone_width_applied` re-scales the relative rows on every change of
that width.

Material flow: "Use Color" first opens the design through Load Selected (the unsaved-changes
guard asks first), so the zones always land on the design they were computed for. Once the design is
on the bench it adopts the rough's color as ONE undoable design edit (the design's material and its
girdle diameter, `Edit::Batch([SetMaterial, SetGirdleDiameterMm])`), ends a running "Preview As..."
and clears every body-color override. "Open in Editor" (the pill beside it on the same design row)
opens the design the same way and applies the same batch with two more edits added: `SetPreform`
(a `PreformShape::Planes` piece from `rough_plan::stone_preform_planes`) and `SetPreformYOffset(0)`;
`SetMaterial` is in that batch only when the plan has a stored rough color. The
material is named `"<plan> - <design> color"` (`adopted_material_name(rough_name, design_title)`; it
used to be one name per plan, which a second design of the same plan overwrote). The width is the
design's `width_axis` times the pose's mm per unit, the quantity the girdle diameter measures, so the
editor's millimetre scale equals the planner's. There is no "Linked to Design" switch; "Preview As..."
is explicit. A zoned material cannot be recolored: its color is its zones, so the design's body-color
controls are greyed for it and a body-color override is not applied to it (the base zone is edited in
the material editor). Only the refractive-index override still applies.

Reopening a Rough Planner plan that has a rough color restores its cached working-resolution
photos into the wizard (`load_view_photos` / `cached_views`), so Compare and Accept work without
the original photos. A re-Accept replaces the stored photo rows, deleting the color deletes its
photos, and loading the original photo of a view again prefers the original.

## Tests and the Benchmark

```
# libraries
cargo test -p indicatrix -p indicatrix-cut-core

# the synthetic validation suite: recovery, forward-vs-main-tracer cross-check, determinism, boundary
cargo test -p indicatrix-cut-core --test zoning_validation

# a whole run
cargo test --workspace --all-features --all-targets

# GPU: zone table echo, six zoned Tier 3 comparisons, then the gpu_backend tests with
# INDICATRIX_REQUIRE_GPU=1
cargo run -p indicatrix --example gpu_equivalence_harness --features gpu

# clippy
cargo clippy --all-targets --all-features --workspace --tests
```

Since 2026-10-12 the recovery tests assert the metamer-limited criterion (the error of every prediction within `1.25 x verified metamer spread x sqrt(delta chi2 of the truth)`, plus three chi-square health checks) and count the plan's 1.5 / 3.0 dE2000 figure in the output: with one backlight and a three-channel camera the colour under D65 and A is determined only up to the camera's metamers (see `solve/models.rs` module notes).

The benchmark (the performance budget: trace, solve, record memory) is an example of
`indicatrix-cut-core`:

```
cargo run --release -p indicatrix-cut-core --example zoning_bench
  [--views 8 --samples 256 --px 384 --threads 0 --stone-mm 30 --configs frosted,polished,immersion --skip-solve]
```

It prints the trace time, the record memory (exact), the solve time, the worst color difference,
the leave-one-view-out figure and the share of lost depth. The process peak is not measured by the
example; read it from the system.

## Determinism

No hashed collections in results, no unseeded randomness, fixed reduction orders. The forward
tracer uses a counter-based RNG keyed by (seed, view, pixel, sample, bin). Thread counts
(1, 4, 16) must give bitwise identical records and fits; `determinism.rs` checks it.

## Open Items

- LGPL-2.1 of `rawler` (above).
- Record the GPU goldens of `zoned_absorption`; decide
  `GPU_SOFT_SUBDIV` (16 now) from the harness.
- The chapter's real-world validation section has no measured figures until the owner has done the
  procedure.
