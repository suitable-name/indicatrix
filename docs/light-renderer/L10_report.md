# L10: Browser GPU

Implemented the small Ray-Traced kernel in the GPU Worker alongside the event-wavefront renderer. The Worker prepares uploads once per scene, traces whole-frame tap ranges in bounded 4096-pixel chunks, and commits a pass only after every chunk succeeds and the device remains healthy. A decline hands the complete range back to CPU workers; a lost device never publishes partial sums.

The `whitted` compile request asynchronously builds the small renderer and performs an actual 16 × 16, four-tap GPU render against the CPU twin under the shared G-D10 rule before reporting success. It uses the existing soft/hard compile deadlines and its own persisted per-adapter qualification record, including a crash marker written before the request. On normal Path-Traced startup this qualification follows the event-wavefront test; when Ray-Traced is selected it can qualify independently. Explicit Test GPU and Forget GPU Test Result act on the selected renderer.

Every resolvable material class is eligible, including uniaxial, biaxial and zoned materials (zoning remains omitted by the reduced rendering model). UV illumination, fluorescence, oversized HDR maps and tool counts beyond 64 decline. Numeric tap targets are capped before planning and GPU tap passes stop at the requested target. Switching engines disables the old admission before the new accumulation is scheduled.

The page reports “GPU · Ray-Traced” after qualification and keeps the CPU route/reason otherwise. The Render panel, guide and README explain the two engines' different material eligibility. No EW shader unit, `EW_KERNELS`, `KERNEL_SET_HASH` or existing adapter-key function changed. The whitted key uses the separately generated hash with a `whitted:` domain prefix.

## Files And Pre-Format Line Counts

| File | Lines |
|---|---:|
| `apps/indicatrix-web-gpu/src/lib.rs` | 384 |
| `apps/indicatrix-web-gpu/src/entry/init.rs` | 145 |
| `apps/indicatrix-web-gpu/src/entry/compile.rs` | 295 |
| `apps/indicatrix-web-gpu/src/entry/ray.rs` | 152 |
| `apps/indicatrix-web/src/render/gpu_gate.rs` | 740 |
| `apps/indicatrix-web/src/render/gpu_gate/ray.rs` | 175 |
| `apps/indicatrix-web/src/render/gpu_gate/status.rs` | 118 |
| `apps/indicatrix-web/src/render/live.rs` | 742 |
| `apps/indicatrix-web/src/render/export.rs` | 410 |
| `crates/indicatrix-web-core/src/gpu_admission.rs` | 337 |
| `crates/indicatrix-web-core/src/gpu_admission/whitted.rs` | 63 |
| `crates/indicatrix-web-core/src/host/engine.rs` | 17 |
| `crates/indicatrix-web-core/src/host/gpu_slot.rs` | 380 |
| `crates/indicatrix-web-core/src/host/render_pool.rs` | 658 |
| `crates/indicatrix-web-core/src/render/engine_tests.rs` | 95 |
| `crates/indicatrix/src/renderer/whitted/prep.rs` | 166 |
| `crates/indicatrix-web-core/src/scene/engine.rs` | 38 |

Also updated text in the two browser READMEs and the existing Render panel/user-guide Slint files. The L2 cache helper gained `into_owned_with_hdr`; `OwnedScene` now shares the exact scene's HDR Arc rather than cloning its large decoded pixel allocation. The environment helper itself was added by L9 in coordination.

## Added Tests And Qualification (Unrun)

- Every built-in material is eligible for Ray-Traced GPU; limits and fluorescence decline.
- Whitted admission keys differ from unchanged event-wavefront keys even with identical supplied hash text.
- Whole-frame GPU passes cover each 1/4/8/16 tap target exactly and never exceed it.
- Runtime qualification code compares the actual four-tap GPU/CPU frame with `compare_frames` and `FrameDiff::tol_passed` before admission.

No build, cargo, test, Clippy, browser GPU probe or git command was run. Owner validation must include the wasm build and Test GPU in a spare browser profile. GPU-driver compilation, timing and images remain unverified here. No existing pin was weakened or replaced.

## Final Source Review Corrections

The worker now tries the unchanged eight-binding `GpuContext::acquire_async`
first, then the additive `acquire_whitted_context` fallback. A genuine four-binding
adapter keeps a usable context and compiles only Ray-Traced; it never constructs
the event-wavefront renderer. The page remembers this Ray-only device class and
does not attempt Path-Traced qualification. Existing eight-binding behavior stays
on the original acquisition path. This was checked by reading the core GPU
context helper and browser init/gate branches, not by executing WebGPU.

Standalone rustfmt was applied with lead authorization. GPU admission was split
into `gpu_gate/admit.rs` to keep the parent below 800 lines. Final source counts,
superseding the earlier pre-format counts, are in `final-line-counts.md`.
