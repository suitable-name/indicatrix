# L9: standalone desktop GPU renderer

Status: implemented and source reviewed; no shader validation, build, test, or hardware measurement has been run.

## Delivered

- `crates/indicatrix/src/renderer/shaders/whitted/`: independent bindings, the existing scalar exponential polynomial, prefixed studio-lighting twins, prepared ASET/HDR lookup, exact concave-tool intersection, Fresnel/Snell/absorption/frost chains, and summed-XYZ frame output.
- Exactly four storage bindings: planes/tools, HDR-or-ASET radiance, frost radiance/flags, and XYZ output. Camera, tracer parameters, and HDR dimensions are uniforms.
- `renderer/buffers/whitted.rs`: 864-byte explicit uniform layout, AA taps, indices/masks, tint, matrix/white balance, environment parameters, and CPU-prepared ring directions. Uploading ring directions avoids duplicating CPU/GPU rig trigonometry.
- `renderer/gpu/whitted/`: separate four-storage-buffer adapter/device acquisition, shader pipeline, scene uploads, bounded 4096-pixel dispatches, and native/asynchronous APIs. Browser initialization never calls a blocking native initializer. Device health and upload/device identity are checked around dispatches.
- `renderer/gpu_backend/whitted{,_native}.rs`: signature-compatible always-available facade, no-GPU stub, transactional frame accumulation, FIFO chunk turns, and the established loss-recovery policy. A decline or cancellation leaves the caller's accumulator untouched, including failure after earlier chunks succeeded.
- `apps/indicatrix-cut/src/bridge/render_thread/engine.rs`: GPU and CPU+GPU route whole Ray-Traced tap batches through the new kernel; CPU and GPU declines use the existing deterministic CPU path. Primary-hit buffers are still populated for picking/diagnostics. The spectral dispatch remains unchanged.
- `indicatrix/build.rs`: independent `whitted.generated.wgsl` and `WHITTED_KERNEL_HASH`. Existing event-wavefront units and their admission hash are unchanged. The small-kernel hash is also available without GPU features through `renderer::buffers::whitted`.
- `renderer/whitted/env.rs`: borrowed GPU preparation accessor. Added shared-HDR ownership transfer so browser workers can reuse their already-admitted map allocation; an unrelated Arc cannot silently replace the source map.

## Public integration contract

`gpu_backend::whitted::WhittedGpuBackend` exposes `acquire`, `disabled`, `acquire_with_diagnostics`, `adapter_label`, `try_recover`, `set_work_limits`, `try_accumulate`, and `try_accumulate_cancellable`. The latter accepts a shared `FrameScene`, tools, cached `WhittedPrep`, absolute tap offset/count, mutable XYZ accumulator, and cancellation closure. It returns `GpuAccumulate`.

`gpu::whitted::WhittedGpuRenderer` exposes native `new` and `trace_chunk`, plus `new_async`, `from_context_async`, `prepare`, and `trace_chunk_async`. `prepare` returns owned `WhittedGpuScene` uploads. Chunk tracing returns detached XYZ sums; each chunk has at most 4096 pixels, and its absolute tap range must fit the prepared AA count. The facade commits only after the complete request succeeds.

## Added regression source

- Generated shader entry in the existing validation registry.
- Adapter-free naga validation of the actual production unit and appended probe; exact Rust/WGSL field offsets/size; reachable storage binding list fixed to four.
- Shared `gpu::test_support::require_gpu` gate, respecting `INDICATRIX_REQUIRE_GPU=1`.
- Actual-kernel Fresnel/refraction/exponential probes and 4096-direction studio probes over every non-UV/non-ASET preset. ASET is exercised through its prepared lookup/frame path.
- Real-adapter CPU/GPU frame comparison with the shared G-D10 rule, concave cases, two identical dispatches, and an analytic white-furnace anchor.

## Verification and remaining owner checks

Read the existing wgpu context, acquisition limits, buffer/upload/readback API, tool-packing layout and CSG shader, lighting rig/shader twins, error-scope API usage, comparison rule, and renderer consumers. Standalone rustfmt parsed/formatted the new Rust files. New files remain below 400 lines; modified existing files remain below 800. No Cargo, builds, tests, Clippy, naga execution, GPU execution, or git commands were run.

Runtime shader validation, Windows adapter behavior, G-D10/ULP results, device loss, browser admission and performance budgets remain for owner validation after all waves are integrated. L10 and L11 consume this backend separately; remote routing is L12. No L9 implementation blocker remains.
