# L12 Network And Worker Integration

Implemented protocol 28, with `SceneState.engine` appended after frosting and defaulted only for legacy JSON. Browser protocol 23 is handled by L7. Updated HELLO/WELCOME pins, version assertions, tail-layout tests and every discovered complete SceneState initializer; existing test fixtures explicitly remain Path-Traced. Version mismatch still refuses old network peers.

The worker caches deterministic tint/light/frost preparation and plane SoA per tracer thread and scene, shares pinned HDR memory, and traces absolute fixed tap ranges. It independently acquires the four-storage-buffer GPU when compute intent permits; CPU-only never probes it. GPU cancellation or decline commits nothing; the CPU retry covers the same full range under the existing global CPU permit pool. Ray skips spectral hybrid calibration and sizing probes. Batch previews and the coordinator own lane use the same branch. Invalid/overflowing tap ranges, unsupported tap counts, UV and fluorescence are refused; Ray stream refusals use UNSUPPORTED_REQUEST. DisplayOnly creates neither guides nor a denoiser and uses the existing plain tone-map path.

Durable jobs retain the wire engine, write format 2 for Ray (format 1 for Path), and reapply the authoritative job engine before tracing. Path durable jobs retain their existing larger budget validation. The worker CLI supports `--engine path|ray`; Ray accepts 1/4/8/16 taps. Desktop live dispatch copies the actual engine, caps the epoch before range allocation and disables remote guide/denoise work for Ray. Shared zoning payload generation skips Ray, whose documented model uses the already-carried base color. Mobile/client/export propagation is detailed in L6 and L12_mobile_report.md.

Added unrun regressions for worker/CPU-twin equality and thread-count identity, absolute tap partitions, invalid/cancelled transactional output, Ray request validation, no-denoise DisplayOnly, format-2 durable storage, network tail bytes, old-layout rejection and legacy JSON defaults. Existing Path assertions were retained. The oversized batch module was split into tracing and test children; owned implementation Rust files stay below 800 lines. Previously oversized network fixture/settings files received only necessary field additions.

Important numerical limit: per-pixel/thread partitioning is bit exact. Regrouping already-summed f32 tap ranges across machines is mathematically additive but can change last bits through floating-point associativity, just as on the existing transport. The unrun range regression uses a stated 1e-6 relative/absolute scale; CPU/GPU images use the planned G-D10 comparison. Exact bits under arbitrary tap regrouping would require per-tap ordered merge or a different accumulator wire format; this implementation preserves the plan's summed-XYZ wire contract rather than pretending that guarantee holds.

Verification was source inspection, API/call-site searches, initializer inventory and standalone rustfmt only. No builds, cargo, tests, Clippy, GPU probes or git commands were run. Owner runtime checks remain necessary on native GPU, browser WebGPU and Android.

## Source Line Counts

| File | Lines |
|---|---:|
| `apps/indicatrix-worker/src/render_core/mod.rs` | 758 |
| `apps/indicatrix-worker/src/render_core/whitted_tests.rs` | 81 |
| `apps/indicatrix-worker/src/stream_emit/tracer.rs` | 434 |
| `apps/indicatrix-worker/src/stream_emit/emitter/display.rs` | 501 |
| `apps/indicatrix-worker/src/validate/mod.rs` | 430 |
| `apps/indicatrix-worker/src/validate/tests.rs` | 457 |
| `apps/indicatrix-worker/src/jobs/mod.rs` | 380 |
| `apps/indicatrix-worker/src/jobs/store.rs` | 794 |
| `apps/indicatrix-worker/src/jobs/store_tests.rs` | 456 |
| `apps/indicatrix-worker/src/jobs/run.rs` | 353 |
| `apps/indicatrix-worker/src/jobs/tests.rs` | 518 |
| `apps/indicatrix-worker/src/serve/mod.rs` | 608 |
| `apps/indicatrix-worker/src/serve/connection/batch.rs` | 650 |
| `apps/indicatrix-worker/src/serve/connection/batch_trace.rs` | 185 |
| `apps/indicatrix-worker/src/serve/connection/batch_tests.rs` | 147 |
| `apps/indicatrix-worker/src/serve/connection/requests.rs` | 640 |
| `apps/indicatrix-worker/src/coordinator/job/mod.rs` | 769 |
| `apps/indicatrix-worker/src/cli/args.rs` | 299 |
| `apps/indicatrix-worker/src/cli/parse.rs` | 652 |
| `apps/indicatrix-worker/src/cli/tests/render.rs` | 220 |
| `apps/indicatrix-worker/src/render_cmd/mod.rs` | 473 |
| `crates/indicatrix-net/tests/scene_roundtrip.rs` | 401 |
| `crates/indicatrix-net/src/scene/mod.rs` | 182 |
| `crates/indicatrix-net/src/messages/mod.rs` | 289 |
| `crates/indicatrix-net/src/messages/hello.rs` | 420 |
| `crates/indicatrix-net/src/messages/zoning.rs` | 722 |
| `apps/indicatrix-cut/src/gui/remote/orchestrator/tick/dispatch.rs` | 403 |
| `apps/indicatrix-cut/src/gui/remote/orchestrator/tick/poll.rs` | 683 |
| `apps/indicatrix-cut/src/gui/remote/orchestrator/tick/update.rs` | 708 |
| `apps/indicatrix-cut/src/bridge/remote/remote_render/connection/persistent.rs` | 333 |
| `apps/indicatrix-cut/src/bridge/remote/remote_render/connection/one_shot.rs` | 427 |
| `apps/indicatrix-cut/src/bridge/render_thread/whitted_scanline.rs` | 98 |
| `apps/indicatrix-worker/src/stream_emit/tests/fixtures.rs` | 158 |
| `apps/indicatrix-worker/src/stream_emit/sizing.rs` | 163 |
| `apps/indicatrix-worker/src/serve/zoning.rs` | 322 |
| `apps/indicatrix-worker/src/serve/tilt.rs` | 657 |
| `apps/indicatrix-worker/src/serve/tests/fixtures.rs` | 328 |
| `apps/indicatrix-worker/src/render_core/hybrid/tests.rs` | 609 |
| `crates/indicatrix-dispatch/src/pool/tests/fake.rs` | 318 |
| `apps/indicatrix-worker/src/assets/tests.rs` | 445 |
| `apps/indicatrix-worker/src/join/mod.rs` | 314 |
| `apps/indicatrix-worker/examples/write_pgo_scene.rs` | 70 |
| `apps/indicatrix-worker/src/enroll/tests.rs` | 588 |
| `apps/indicatrix-worker/examples/capture_frame_payloads.rs` | 191 |
| `apps/indicatrix-worker/src/coordinator/tests/fixtures.rs` | 219 |
| `crates/indicatrix-net/src/messages/tilt.rs` | 223 |
| `crates/indicatrix-net/src/messages/stream/tests.rs` | 831 |
| `crates/indicatrix-net/src/messages/render.rs` | 251 |
| `crates/indicatrix-net/src/messages/job.rs` | 619 |
| `crates/indicatrix-net/src/messages/final_image.rs` | 278 |
| `crates/indicatrix-net/src/messages/batch.rs` | 190 |
| `apps/indicatrix-cut/src/gui/rough_colour/store_tests.rs` | 629 |
| `crates/indicatrix-net/src/client/session.rs` | 788 |
| `apps/indicatrix-cut/src/gui/batch/tilt/engine/sweep.rs` | 261 |
