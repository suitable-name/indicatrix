# L12 mobile: remote engine propagation

Status: implemented and source reviewed, without builds or test execution.

`EngineSpec` now follows the phone's selected renderer through live reconnection, FullData rendering, coordinator DisplayOnly rendering, final PNG requests, and durable job submission. `open_session` installs the engine on the locally resolved scene before connecting, preserving UV/fluorescence refusals and preparing the same tint/lighting used locally. `SceneState.engine` copies that effective engine and AA/Fire options exactly.

All remote output modes validate a positive sample count and refuse a Ray-Traced count beyond the chosen AA taps. Ray-Traced DisplayOnly no longer requires denoising and every received picture is marked undenoised for that engine. Path-Traced retains its established DisplayOnly/denoise policy. Material zoning metadata remains intact; the Ray-Traced base-color approximation does not require spectral zoning capability.

Removed temporary phone-local substitution for Remote plus Ray-Traced. Live/export sessions now open the authenticated endpoint for either engine. Remote cancellation and Send To Server controls are available for Ray-Traced. The Remote page names both supported renderers. Desktop `engine::remote_allowed` now permits both engines under protocol 28.

Existing HDR asset bytes/hashes, TLS/certificate pinning, metered-network policy, capability checks, payload budgets, durable idempotent submit tokens, reconnect limits, and background/epoch cancellation are unchanged. New unrun regression sources verify exact engine/AA/Fire propagation and remote tap bounds.

The existing summed-f32 XYZ wire contract is retained. Pixel partitions and CPU thread counts preserve per-pixel operation order. Regrouping tap sums from separate remote lanes can change the final low bits through floating-point associativity; arbitrary tap-range regrouping is therefore checked with a numerical tolerance, not claimed bit-exact. Absolute tap identities and non-overlapping ranges remain fixed. No physics quantization or wire expansion was introduced.

Changed Rust files and line counts: mobile `remote/rendering.rs` 697; `remote/final_image.rs` 216; `remote/jobs.rs` 488; `remote/rendering/tests.rs` 183; `render/remote_session.rs` 229; `render.rs` 596; `render/export.rs` 409; `render/backend_whitted.rs` 289; Android `design/render.rs` 199; desktop `bridge/render_thread/engine.rs` 140. Three small Android Slint predicate/text edits expose Remote actions for both engines. Every changed Rust file remains below 800 lines.

Verified every changed call site by source search and inspected the request chain in both directions. Standalone rustfmt parsed/formatted these files. No Cargo, builds, tests, Clippy, shader validation, network sessions, GPU execution, or git commands ran. Root owns worker/coordinator protocol-28 routing and job-format persistence; end-to-end checks remain for the owner's final validation.

Independent source review of root's worker/net integration checked stream/coordinator request limits, explicit CPU/GPU intent, durable engine and format-2 persistence, standalone rendering, DisplayOnly denoise bypass, HDR pinning, and cancellation. Fixed the worker child tracer to reject malformed output dimensions and tap ranges without panicking or committing partial sums; unsupported preparation returns failure, and cached preparation reuses the held HDR Arc instead of copying its pixels. No additional expected fixture type/API mismatch was found in those reviewed paths. The existing generic wrapper conservatively uses spectral availability, while actual Ray-Traced stream, coordinator, batch, durable and CLI entry points carry explicit compute intent.
