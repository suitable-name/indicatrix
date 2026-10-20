# L4 Desktop Views

Implemented the desktop mode boundary and all owned §6.1 consumers. No builds, tests, Clippy, cargo or git commands were run. Source inspection found no AGENTS.md through `rg --files -g AGENTS.md`.

## Behavior

- Edit modes retain 0 Solid, 1 Path-Traced, 2 Both and 3 Diagram; 4 adds Ray-Traced. Live modes retain 0 Solid and 1 Path-Traced; 2 adds Ray-Traced.
- New typed enums expose `from_raw(i32)`, `raw()`, `is_traced()` and `engine() -> RenderEngine`. Unknown raw values explicitly become Solid.
- Ray-Traced pills appear immediately after Path-Traced. Key 5 and palette commands `view.ray_traced` / `view.live_ray_traced` select them.
- Dirty/restart, letterboxing, fit camera and pick geometry treat Ray-Traced like Path-Traced. Shared solid raster mode 4 is explicit and uses the unchanged raster passes.
- Analytic metrics HUD, optical image/panel/button, stale badge, pause and GPU status pill remain Path-Traced-only. The shared traced image, refusal banner and initial placeholder accept both engines.
- Ray-Traced presents an approximation note in each viewport and on its choice's tooltip. Existing defaults are unchanged.
- Tight mode-pill widths and reflow thresholds account for the fifth pill.
- Main palette `COMMANDS` remains a static slice with its existing order. Its view entries moved to const constructors in `commands/table_view.rs`, reducing the original over-limit table below 800 lines.

## §6.1 audit checklist

- `ui/app.slint`: digit 5, existing digits keep their mappings and focus/modifier gates.
- `solid_viewport/toolbar.slint`: fifth pill, widths/reflow, cut-slider eligibility, approximation status; Diagram-only controls/help branching remain Diagram-only.
- `solid_viewport.slint`: mode 4 shows traced image. Existing non-Diagram gesture/handle/slice/hover branches already include 4; Both-only edge/cut handling and Diagram layers remain unchanged. Existing fallback predicates include 4 correctly.
- `gem_viewport.slint`: common image/refusal/placeholder broadened, new pill and caveat; analytic HUD/overlay/panel explicitly exclude Ray-Traced.
- `gem_viewport/toolbar_actions.slint`: inspected; its three `live_view_mode == 1` gates correctly hide stale, pause and GPU controls for 2, so no edit needed.
- `main_window/callbacks.rs`: typed persistence normalization and traced entry invalidation for both viewports.
- `camera_lighting.rs`: typed normalization, Live Solid-only redraw, shared contained sizing recognizes 4. Existing callers in `editor/view/viewport.rs`, `view/viewport/replan.rs`, `auto_solve/apply.rs` and `view/optimize_apply.rs` flow through these helpers; no duplicate logic added.
- `fit_view.rs`, `commands/state.rs`, `manipulate/diagram.rs`, `guide/progress.rs`, `cutting_mode/viewport.rs`: typed interpretation replaces raw checks.
- `callbacks/tier_actions/solid_picking.rs`: uses shared normalization/contained sizing. `view/viewport/replan.rs`: typed Diagram exclusion for cut steps.
- `indicatrix-solid/src/preview/render.rs`: accepted modes 0–4 explicit; 4 shares the existing raster and pick-geometry path, without building Both edges or Diagram output.
- Library redraw sites remain Solid-only (`== 0`), as required. Startup mode initialization belongs to L3.
- Shortcuts table, key-combination test source, generated appendix B row and surrounding explanatory text updated together.
- Harness digit loop extends to 5; shortcut case for 5 added. No new callbacks, so `rows_editor.rs` needs no row.

## Tests added or extended (unrun)

- Typed mode round trips, engine selection and unknown-value fallback.
- Existing contained-sizing regression additionally asserts Ray-Traced gets the same letterboxed rectangle.
- Cutting-mode regression asserts mode 4 survives and unknown -1 normalizes to Solid.
- Existing shortcut source test includes key 5; harness exercises key 5.

## File counts before lead formatting

All paths below are relative to `apps/indicatrix-cut/`, except the shared raster file.

| File | Lines |
| --- | ---: |
| src/gui/view_modes.rs | 100 |
| src/gui/main_window/callbacks.rs | 502 |
| src/gui/render/camera_lighting.rs | 786 |
| src/gui/render/fit_view.rs | 216 |
| src/gui/commands/table.rs | 649 |
| src/gui/commands/table_view.rs | 255 |
| src/gui/commands/run.rs | 227 |
| src/gui/commands/state.rs | 244 |
| src/gui/editor/manipulate/diagram.rs | 481 |
| src/gui/editor/guide/progress.rs | 117 |
| src/gui/editor/cutting_mode/viewport.rs | 196 |
| src/gui/editor/view/viewport/replan.rs | 416 |
| src/gui/editor/callbacks/tier_actions/solid_picking.rs | 469 |
| src/gui/editor/shortcuts.rs | 545 |
| ui/components/solid_viewport.slint | 733 |
| ui/components/solid_viewport/toolbar.slint | 583 |
| ui/components/gem_viewport.slint | 585 |
| ui/models/solid_preview.slint | 169 |
| ui/models/viewport.slint | 282 |
| ui/app.slint | 1543 |
| docs/manual/appendix-b-keyboard-shortcuts.md | 196 |
| tests/ui_harness/scenarios/main_window.rs | 381 |
| tests/ui_harness/scenarios/shortcuts.rs | 583 |
| crates/indicatrix-solid/src/preview/render.rs | 287 |

`ui/app.slint` was already over 800; its only functional additions are the existing digit gate and two-line mode-4 branch. Every touched Rust file is below 800. Lead formatting should recheck `camera_lighting.rs`, which is close to the limit.

## Integration and limits of source verification

- Root owns registration `pub(crate) mod view_modes;` in `gui/mod.rs` and L3's engine/visibility wiring. Root was notified of the exact enum API.
- Root must ensure `bridge/optical_overlay/mod.rs` rejects Ray-Traced; Slint hides it, but backend analytic requests should also remain excluded.
- Existing toolbar Help maps non-Diagram modes to `HelpTopics.solid-view`; coordinate with L5 if adding a dedicated Ray-Traced anchor.
- No contract deviation. No re-pinning or weakened assertions.
- UI layout, toolkit parsing, callback compilation and click-through behavior require owner validation; none was executed here.
