# Lightweight Renderer Remote Review

Source review of the new Ray-Traced network, worker, coordinator, durable-job and CLI paths. Read IMPLEMENTATION_STATUS, L12/L6 reports, L12_root_files and the plan's engine, output, protocol, worker and validation contracts. No Cargo, builds, tests, Clippy, Git, GPU probes or SQLite access were used. Standalone rustfmt parsed the edited Rust files; its usual stable/nightly configuration warnings remain.

## Findings And Fixes

- **[P2] Renderer refusal could be reported as successful output or cancellation.** `apps/indicatrix-worker/src/stream_emit/tracer.rs:173` now records an explicit failure when a Ray producer stops before its fixed range finishes without cancellation/panic. The existing emitter failure path sends ERROR and suppresses the final successful picture/DONE. `TracedRange.failed` carries the reason to `apps/indicatrix-worker/src/coordinator/lanes/mod.rs:562`, allowing normal bounded lane failure/retry behavior. `apps/indicatrix-worker/src/jobs/run.rs:161` distinguishes Ray failure from cancellation; `apps/indicatrix-worker/src/serve/connection/batch_trace.rs:177` fails the affected Ray item and continues the batch; `apps/indicatrix-worker/src/render_cmd/mod.rs:146` checks the trace outcome before writing a PNG. The renderer's bool means both cancellation and refusal, so blindly breaking/ignoring it was unsafe even though ingress validation catches the presently known unsupported scenes. Existing Path outcomes are unchanged. Added unrun decline-versus-cancel source regressions.
- **[P2] Durable Ray tap equality was checked too late.** `crates/indicatrix-net/src/messages/job.rs:160` now requires exactly the supported AA tap set in `JobSubmit::check_shape`. Previously the worker's range check accepted a prefix, could request an HDR upload, and only the stored job-file validator refused the incomplete set later. This now fails before asset negotiation or storage; normal RenderRequest/BatchItem tap subsets remain valid. Added unrun coverage for all four supported counts, short/long submissions, and an unsupported count.
- **[P2] Ray beta absorption preparation lacked a work bound.** `apps/indicatrix-worker/src/validate/mod.rs:252` now applies the existing absorption-band cap to `beta_ray` when rendering Ray-Traced. The new preparation explicitly evaluates all three biaxial absorption lists, while the old guard only bounded o/e. The check is scoped to Ray and leaves Path admission unchanged. Added an unrun exact-cap/over-cap regression. The UV guard now selects the matching UV decline reason explicitly.

## Reviewed Contracts

- Protocol 28 is pinned in HELLO/WELCOME and compatibility is checked before decoding scene-bearing requests. SceneState appends EngineSpec; legacy JSON defaults to Path-Traced, while protocol mismatch/old binary layout is refused. No physical wire fields changed during this review.
- Worker-only renderer dependencies remain optional; CLI engine parsing stores a string so library-only builds do not name optional renderer types.
- Stream and batch Ray branches bypass spectral calibration. Absolute tap ranges are bounded at ingress and the coordinator passes/clamps its existing finite sample ranges without restarting tap numbering. Ray GPU use follows explicit compute intent and its independent small-kernel admission; CPU permits bound fallback threads.
- Ray DisplayOnly creates neither guides nor a denoising thread. UV/fluorescence are refused, with HDR correctly overriding an otherwise inactive studio lamp selection.
- Durable storage copies the wire engine into the authoritative job engine and uses format 2 for Ray. Execution reapplies that job engine. Added an unrun PNG equality regression with an intentionally stale Path scene copy to guard engine authority. CLI override updates engine, format and Ray target count together.
- Analytic tilt requests use their independent evaluator. The parent verified that the desktop metric scene builder uses the default Path engine, so image-engine validation does not interfere with the current metric client.

## Edited Files

| File | Lines |
|---|---:|
| apps/indicatrix-worker/src/stream_emit/tracer.rs | 453 |
| apps/indicatrix-worker/src/stream_emit/tests/ray_failures.rs (new) | 58 |
| apps/indicatrix-worker/src/stream_emit/tests/mod.rs | 19 |
| apps/indicatrix-worker/src/coordinator/lanes/mod.rs | 646 |
| apps/indicatrix-worker/src/jobs/run.rs | 381 |
| apps/indicatrix-worker/src/serve/connection/batch_trace.rs | 190 |
| apps/indicatrix-worker/src/render_cmd/mod.rs | 475 |
| apps/indicatrix-worker/src/validate/mod.rs | 443 |
| apps/indicatrix-worker/src/validate/tests.rs | 477 |
| crates/indicatrix-net/src/messages/job.rs | 646 |
| docs/light-renderer/REVIEW_remote.md | this report |

No render-jobs schema or desktop CLI edits were needed. No existing assertions were weakened or image pins changed. All edited Rust files remain below 800 lines.

## Residual Validation

Owner validation must compile default worker/library-only, worker CPU, worker GPU and desktop CLI configurations; run existing protocol/layout/format and renderer tests plus the added regressions; and exercise Ray Local/Remote/Both, DisplayOnly, batch item failures, final-image viewer contribution/reclaim, durable finish/cancel/restart, and GPU decline/cancellation on a real adapter. These have not been executed. Generic terminal-state persistence and transport/runtime races remain governed by the existing job/stream implementations and need the existing integration suite.

The documented f32 grouped-tap-sum associativity limitation remains: arbitrary regrouping can change final bits. This review preserves the summed-XYZ protocol and its stated numerical tolerance; it does not claim arbitrary partition bit identity. No additional source blocker identified in the reviewed Ray integration.

## Follow-Up: Frozen GPU Upload Reuse

- **[P2] Every worker/export tap claim re-uploaded the complete HDR and geometry.** The core review added the owned opaque `WhittedGpuPrepared` API. `apps/indicatrix-worker/src/render_core/whitted.rs:31` now stores that snapshot beside the immutable full-SceneState CPU preparation cache. `trace_prepared_gpu` at line 198 uploads only when this scene lacks a snapshot, then dispatches subsequent absolute tap batches against the frozen uploads. A device-generation/dispatch rejection drops the cached snapshot and the full unchanged range retries on CPU; the next worker call may prepare after recovery. A poisoned cache lock permanently declines GPU access for that cache entry rather than reusing resources after a panic. There is no address-only identity cache.
- `apps/indicatrix-cut/src/bridge/export_thread/worker/core.rs:133` prepares owned GPU uploads once per frozen export frame. `batch.rs:53` carries scoped references to that backend and snapshot in ExportCtx. `worker/export_ray.rs:59` executes every local claim against the snapshot. A rejected snapshot retires GPU execution for the remaining frame and retries the complete claim on CPU; the next tilt frame creates a fresh context and can recover. CPU/RemoteOnly paths acquire no additional small GPU. Ray cancellation is checked before acquisition and after preparation, before entering remote/local claim work; a cancelled upload never starts a CPU retry.
- Added unrun source regressions in worker whitted.rs for full-content scene cache identity and poisoned-cache whole-range CPU fallback without touching the lazy GPU. Added an export core regression using explicitly disabled adapters: upload decline produces the exact CPU tap claim, and cancellation leaves the accumulator untouched. Actual upload-count/performance and stale-device execution still require native GPU owner validation; no GPU was acquired by this review.

Additional edited files (standalone rustfmt only; all under 800 lines):

| File | Lines |
|---|---:|
| apps/indicatrix-worker/src/render_core/whitted.rs | 276 |
| apps/indicatrix-cut/src/bridge/export_thread/batch.rs | 534 |
| apps/indicatrix-cut/src/bridge/export_thread/worker/core.rs | 477 |
| apps/indicatrix-cut/src/bridge/export_thread/worker/export_ray.rs | 94 |

This follow-up adds no protocol or persisted-format changes. The core agent owns the facade and low-level prepared-buffer API and its validation; this report records only worker/export integration. Existing convenience API remains available for one-shot callers.
