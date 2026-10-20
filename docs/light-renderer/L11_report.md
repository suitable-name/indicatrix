# L11: phone GPU routing for Ray-Traced

Status: implemented and source reviewed; nothing built or tested.

## Delivered

- `crates/indicatrix-mobile-core/src/render/backend_whitted.rs`: Ray-Traced admission, independent cached small-kernel probe, status strings, bounded dispatch, cancellation, and exact CPU fallback.
- `render/backend.rs`: separate spectral and Ray-Traced probe slots in one cloneable `GpuProbe`, shared unchanged through `BackendManager::probe`/`sharing`. A live render and all its exports reuse both successful and failed probes. Choosing CPU does not acquire either GPU backend.
- `TraceBackend::RayGpu` plus `is_gpu`: the pool and export planner use one whole-frame partition for either GPU, while the spectral execution branch retains its existing behavior.
- `crates/indicatrix/src/renderer/gpu_backend/limits.rs`: `GpuWorkLimits::WHITTED` is exactly 1024x1024 pixels, sixteen taps per call, 4 MiB chunk budget, one chunk per FIFO turn. The underlying kernel additionally limits a dispatch to 4096 pixels.
- The small kernel admits isotropic, uniaxial and biaxial materials; it does not call `GemMaterial::gpu_supported` or compile/acquire the spectral megakernel. UV/fluorescence are explicit engine refusals. Excess tools, unsupported HDR storage, memory limits and unavailable GPU use the same Ray-Traced CPU core.
- Cancellation includes both the supplied cancel flag and thermal/background/epoch abort closure. GPU sums remain private until the complete absolute tap range succeeds. A GPU decline discards all partial work and retraces the whole original range on CPU; it cannot double count earlier taps.

## Regression source added

Material-class admission, exact pixel-budget boundary, excess-tool decline, UV refusal before probing, live/export probe sharing, CPU choice avoiding probes, and disabled-GPU full-range fallback/cancellation. These tests are unrun. Existing GPU-frame/analytic/probe regression sources are covered by L9.

## Verification

Verified `OwnedScene::whitted_prep`, `frame_scene`, `SceneSpec`, `MaterialSpec::catalogue`, `LightingSpec`, `GeometrySceneSpec::build`, shared chunk-range/abort APIs, compute-policy call sites, probe-sharing/export wiring, and the existing enum consumers by reading/searching source. Standalone rustfmt parsed/formatted owned files. No builds, Cargo, tests, Clippy, naga execution, GPU execution, or git commands ran.

Line counts: backend 362; new Ray-Traced backend including tests 290; pool 749; export 406; GPU limits 111. Every edited Rust file remains under 800 lines.

Remote routing remains the L12 integration task; this lane preserves the temporary local fallback for a Remote selection until that wave replaces it. Physical phone behavior, adapter fallback/recovery, thermal cancellation, image parity, and timing budgets require the owner's final validation. No L11 implementation blocker remains.
