# indicatrix-cut — the `gpu` feature and its fallback rules

What the `gpu` feature routes through the GPU megakernel, the measured speedup,
and exactly when and why a frame or batch falls back to the CPU tracer instead.
For everything else see [the README](../README.md).

```
cargo build -p indicatrix-cut --features gpu
```

Routes both the viewport's progressive accumulation *and* the high-resolution
export worker through `indicatrix`'s verified GPU megakernel
(`indicatrix::renderer::gpu::frame`) instead of the multithreaded CPU tracer. **Off
by default**, so an ordinary build pulls in neither `wgpu` nor `pollster` and
behaves exactly as it did before the feature existed.

Export is where it matters most — a 4K render at 1024 spp is ~8.5 billion
spectral paths. Measured on this project's integrated AMD Radeon (Vulkan), at
960x540 / 64 spp / 12 bounces on Emerald: **1.44 s on GPU vs 10.66 s across 16
CPU threads, a 7.4x speedup**. A discrete GPU widens that considerably.

The fallback is per frame (per batch, for an export), not per session, and is a
normal outcome rather than an error — each frame or batch is offered to the GPU
and falls through to the CPU tracer whenever it declines:

| The GPU declines when | Because |
| --- | --- |
| No usable adapter on this machine | Logged once at startup; every frame then uses the CPU |
| The GPU device was lost mid-run | Logged at `warn`; the backend stays disabled for the rest of the process |
| The environment is an HDR map too large for the device | Its texel buffer would exceed the adapter's storage-buffer limit. Ordinary HDR maps render on the GPU — the megakernel has its own environment mode for them |

All materials support the GPU path: the `BiaxialIndicatrix` machinery is ported to
WGSL and verified at the same Tier 2 / Tier 3 bar as every other material, so
`GemMaterial::gpu_supported` returns `true` for every material. See
`indicatrix::renderer::gpu_backend`'s module doc comment for the authoritative
decline list (which covers GPU adapter support, not materials).

Both the viewport (`bridge::render_thread`, through its `ViewportGpu` wrapper in
`bridge::render_thread::gpu_backend`) and the export worker (`bridge::export_thread`)
go through the same `indicatrix::renderer::gpu_backend::GpuBackend` — the one
`indicatrix-worker` uses too — so there is one decline-and-fall-back rule rather than
several that could drift apart. Each acquires its own instance.
Both backends *add* into the same accumulation buffer with the same meaning for
the sample counter, so a render that switches between them mid-flight continues a
correct running average rather than restarting.

One thing the GPU cannot supply is the denoiser's first-hit depth/normal/facet-id
guide buffers — the megakernel returns radiance only. That is the same gap a
remote worker's `FRAME` payload has, and it takes the same answer: the local
primary-ray prepass (`indicatrix::renderer::guide_pass`), cached on pose plus geometry
by `bridge::frame_cache::guide_pass`, reused unchanged here.
