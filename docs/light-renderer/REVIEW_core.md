# Lightweight Renderer Core Review

Status: independent source review completed; concrete fixes applied. No Cargo, build, test, Clippy, git, shader validator, or GPU command was run. Standalone rustfmt parsed/formatted the edited Rust files (only existing stable-toolchain/nightly-option warnings).

## Fixed Findings

| Severity | Current location | Defect and fix |
|---|---|---|
| P2 | `crates/indicatrix/src/renderer/shaders/whitted/60_frame.wgsl:12`; `gpu/whitted/buffers.rs:91` | Raster width was converted from u32 to f32 and back before integer pixel division/modulo. Accepted widths above 2^24 can round, causing a last-column pixel to become a first-column ray on the GPU. Upload exact integer framebuffer dimensions in the unused z/w lanes of the dimensions uniform; use the integer width for raster indexing. Camera arithmetic retains the CPU's existing f32 convention. |
| P2 | `crates/indicatrix/src/renderer/gpu/whitted/buffers.rs:50` | Oversized plane/tool scenes were packed into an owned allocation before the GPU storage-budget rejection. A tool-bearing input could exhaust CPU memory while attempting a GPU path that must decline. Check the packed record count and frost storage size before packing or allocating the HDR upload. |
| P2 | `crates/indicatrix/src/renderer/buffers/whitted.rs:53` | WhittedPrep fields are public. CPU sampling sanitizes taps and caps chain_count at three, while GPU upload forwarded both unchecked; invalid values could divide by zero or index outside the AA/mask/index arrays and disagree with CPU fallback. GPU uniforms now use the same sanitized tap count and bounded chain count; WhittedGpuScene reports that prepared count. |
| P2 | `crates/indicatrix/src/renderer/gpu_backend/whitted.rs:19`; `whitted_native.rs:137`; `apps/indicatrix-cut/src/bridge/render_thread/engine.rs:24` | Native requests converted/uploaded geometry and HDR again for every tap, and successful GPU guide filling called the allocating scalar wrapper for every pixel. Added opaque frozen WhittedGpuPrepared uploads, a prepared-execution API, and a desktop generation/dimensions cache containing GPU uploads and CPU PlanesSoA32. Picking guides reuse the shared SoA kernel through a new whitted_primary_hit wrapper. Declines clear the cached upload and take transactional CPU fallback; returning to Path drops the scene cache. |
| P2 | `crates/indicatrix/src/renderer/gpu_backend/whitted_native.rs:65`, `:115` | The new facade recovered poisoned renderer locks with into_inner and could reuse GPU state after a panic, despite adapter_label treating poison as unavailable. Preparation/execution now decline poisoned renderer locks, and recovery never reuses them. |
| P1 | `apps/indicatrix-cut/src/bridge/render_thread/context/frame.rs:99`, `:172`; `gui/remote/orchestrator/tick/poll.rs:262`, `:358` | Pausing Path-Traced and then selecting Ray-Traced left local and remote Ray rendering paused, while Ray intentionally hides Pause/Resume. A shared effective_paused accessor ignores the manual pause for Ray and preserves the stored Path preference. Both local frame admission and remote suspension/redispatch use it. Visibility and export suspension still apply. |
| P2 | `apps/indicatrix-cut/ui/components/solid_viewport.slint:518`, `:532`, `:535`, `:641` | Edit Ray displayed a previous render unchanged when UV/fluorescence preparation refused the new scene, and an empty frame only showed the generic placeholder. Ray refusal now dims the old image, displays the actual trace_refusal, and replaces the generic empty state. The behavior is scoped to Edit Ray and a real design. |

The lead found the two desktop issues, flagged poisoned-renderer consistency during follow-up review, and explicitly expanded this review's ownership to implement the desktop fixes. The raster indexing, count sanitization, allocation and repeated-upload findings were independently identified from source.

## Scope Reviewed

Read IMPLEMENTATION_STATUS, L1, L2, L9 and the plan's core/preparation/GPU/numerics contracts. Independently traced engine defaults/sanitization, fixed AA offsets, Fresnel/Snell arithmetic, branch ordering, bounded cavity worklist, absorption, frost, primary hit/miss behavior, Studio/ASET/HDR sampling, white balance, buffer layout, acquisition, upload, dispatch/readback, native facade concurrency/recovery/cancellation and feature gates. Compared shared camera, SIMD slab/CSG, tool packing, HDR interpolation, lighting and Bradford dependencies without modifying them.

The existing spectral tracer, event-wavefront sources/generation, EW_KERNELS and KERNEL_SET_HASH inputs were not edited. No tests were weakened and no pins were changed.

## Added Regression Sources (Unrun)

- Actual GPU kernel versus CPU for one pixel in an accepted >2^24-wide frame, with zero and oversized public prep tap counts and oversized chain_count. The probe does not allocate the huge framebuffer.
- Local frame snapshots for paused Path -> Ray -> Path, asserting that the stored manual pause survives.
- Remote live admission for the same engine transition, including a hidden-tab refusal.
- Frozen-upload native regression verifies that subsequent public CPU prep mutations cannot change the snapshot, repeated taps preserve the image, per-call range/shape/work limits still apply, cancellation after an earlier chunk leaves the accumulator untouched, replacement devices reject old uploads, and poisoned renderer state stays declined.
- The temporary Edit Ray exact-string source assertion was removed because it mirrored implementation. Actual Slint layout/refusal interaction remains an owner UI check.

## Edited Files

| File | Lines | Endings |
|---|---:|---|
| `crates/indicatrix/src/renderer/gpu/whitted/buffers.rs` | 162 | LF |
| `crates/indicatrix/src/renderer/gpu/whitted/tests.rs` | 318 | LF |
| `crates/indicatrix/src/renderer/buffers/whitted.rs` | 140 | LF |
| `crates/indicatrix/src/renderer/shaders/whitted/60_frame.wgsl` | 40 | LF |
| `crates/indicatrix/src/renderer/gpu_backend/whitted.rs` | 110 | LF |
| `crates/indicatrix/src/renderer/gpu_backend/whitted_native.rs` | 273 | LF |
| `crates/indicatrix/src/renderer/gpu_backend/whitted_native_tests.rs` | 136 | LF |
| `crates/indicatrix/src/renderer/whitted/frame.rs` | 133 | LF |
| `crates/indicatrix/src/renderer/whitted/mod.rs` | 54 | LF |
| `apps/indicatrix-cut/ui/components/solid_viewport.slint` | 767 | LF |
| `apps/indicatrix-cut/src/bridge/render_thread/context/frame.rs` | 216 | LF |
| `apps/indicatrix-cut/src/gui/remote/orchestrator/tick/poll.rs` | 708 | LF |
| `apps/indicatrix-cut/src/bridge/render_thread/engine.rs` | 161 | LF |
| `apps/indicatrix-cut/src/bridge/render_thread/whitted_scanline.rs` | 98 | LF |
| `docs/light-renderer/REVIEW_core.md` | this report | LF |

Original file endings were preserved. All edited source files are below 800 lines. The report is new.

## Remaining Verification

Compilation across feature/target combinations, naga validation, CPU analytics, GPU tolerance/ULP/image tests, actual small-adapter acquisition, desktop Slint rendering, cancellation/device-loss/concurrency behavior and performance remain unverified. The owner must run the requested acceptance checks. Source review does not establish successful execution.

The prepared API accepts only an owned frozen upload plus tap range/output/cancellation; it does not retain references to mutable public CPU prep or infer validity from addresses. The existing one-shot API remains compatible and delegates through the new API. Every prepared call checks current frame/sample limits, output length, tap range and cancellation, and the existing device ID check rejects stale uploads after replacement. Other lanes wired their worker/export/mobile consumers separately; their edits are outside this report's file list. Performance gains are not measured.
