# L7: Browser CPU Integration

Implemented renderer metadata beside the unchanged `SceneSpec` payload, per-scene cached preparation, the CPU chunk branch, protocol 23 and appended `SetEngine` variant 12, numeric persisted settings, atomic host scene/engine replacement, and browser live/export UI.

`OwnedScene::engine() -> EngineSpec`, `set_engine(EngineSpec) -> Result<(), WhittedDecline>` and `whitted_prep()` are available for Android. Selection validates Ray-Traced before any chunk is counted. The cache resets with engine or fluorescence changes. Host scene messages precede engine metadata and trace assignments; replacement and Worker respawn replay the choice. The target is capped before scene restart as well as on start, preventing old Path-Traced targets from scheduling extra Ray-Traced taps.

The browser exposes Path-Traced/Ray-Traced, four AA choices and Fire. Live status reports taps and completion, and Ray-Traced bypasses denoising. PNG exports use the selected engine and tap count, hide samples/denoise for Ray-Traced and suppress the filter even if a caller retained the old denoise setting. User guide, walkthrough browser wording and READMEs describe the approximation. The temporary CPU GPU refusal is present for the subsequent L10 replacement; EW shader units and hashes remain unchanged.

## Files And Pre-Format Line Counts

| File | Lines |
|---|---:|
| `crates/indicatrix-web-core/src/scene/mod.rs` | 779 |
| `crates/indicatrix-web-core/src/scene/engine.rs` | 35 |
| `crates/indicatrix-web-core/src/render/mod.rs` | 195 |
| `crates/indicatrix-web-core/src/render/engine_tests.rs` | 79 |
| `crates/indicatrix-web-core/src/protocol.rs` | 574 |
| `crates/indicatrix-web-core/src/protocol/tests.rs` | 496 |
| `crates/indicatrix-web-core/src/protocol/geometry_tests.rs` | 146 |
| `crates/indicatrix-web-core/src/worker.rs` | 745 |
| `crates/indicatrix-web-core/src/worker/engine.rs` | 24 |
| `crates/indicatrix-web-core/src/settings.rs` | 642 |
| `crates/indicatrix-web-core/src/settings/engine.rs` | 31 |
| `crates/indicatrix-web-core/src/host/mod.rs` | 222 |
| `crates/indicatrix-web-core/src/host/engine.rs` | 11 |
| `crates/indicatrix-web-core/src/host/render_pool.rs` | 657 |
| `crates/indicatrix-web-core/src/host/render_events.rs` | 312 |
| `crates/indicatrix-web-core/src/host/gpu_slot.rs` | 374 |
| `crates/indicatrix-web-core/src/gpu_admission.rs` | 337 |
| `crates/indicatrix-web-core/src/guide.rs` | 211 |
| `crates/indicatrix-web-core/README.md` | 82 |
| `apps/indicatrix-web/src/render/mod.rs` | 66 |
| `apps/indicatrix-web/src/render/live_ray.rs` | 20 |
| `apps/indicatrix-web/src/render/live.rs` | 739 |
| `apps/indicatrix-web/src/render/panel.rs` | 151 |
| `apps/indicatrix-web/src/render/export.rs` | 409 |
| `apps/indicatrix-web/src/render/gpu_gate/status.rs` | 102 |
| `apps/indicatrix-web/ui/components/render_model.slint` | 105 |
| `apps/indicatrix-web/ui/components/render_settings_panel.slint` | 423 |
| `apps/indicatrix-web/ui/components/export_dialog.slint` | 172 |
| `apps/indicatrix-web/ui/components/user_guide_dialog.slint` | 140 |
| `apps/indicatrix-web-gpu/src/lib.rs` | 378 |
| `apps/indicatrix-web/README.md` | 386 |

## Tests Added Or Extended (Unrun)

- Protocol Init/Loaded version bytes are explicitly updated from 22 to 23; all other old message bytes remain pinned.
- `SetGeometryScene` retains index 11; `SetEngine` explicitly pins `[12,9,1,8,1]` and round-trips.
- Numeric engine downgrade compatibility, tap sanitization and targets below Path-Traced minima.
- Cached prep reuse; absolute tap-range and pixel-partition output against the shared core.
- Worker engine scene-id isolation and UV decline followed by a dropped chunk.

## Notes And Limits

Crystal Axis Override is labelled “tint only in Ray-Traced” rather than claiming the whole control has no effect: the prescribed uniaxial absorption model really does follow the axis. No build, test, Clippy, cargo or git command was run. Browser wasm, Slint and graphics-device behavior require owner validation. L10 will replace temporary GPU refusal, and an additive Arc-aware owned-cache path is being coordinated with the GPU lane to avoid doubling admitted HDR memory.
