# Lightweight renderer: browser and Android review

Reviewed independently against the L7/L8/L10/L11/L12 mobile reports and plan sections 7, 8, 9, 10.2 and 11.3–11.6. Scope was the new Ray-Traced client integration and related regressions. No Cargo, build, test, Clippy, git, network-session or GPU-probe command was run. Standalone rustfmt parsed/formatted the edited Rust sources; its stable-channel notices only concern the repository's nightly formatting options.

## Findings and fixes

1. **P1 — Browser gate applied spectral material restrictions to Ray-Traced. Fixed.** `crates/indicatrix-web-core/src/host/gpu_slot.rs:184` previously called the event-wavefront classifier unconditionally. Both Ray page admission and its status text consume this public method, so independently qualified uniaxial/biaxial/zoned scenes were kept on CPU. The public gate and internal pass scheduler now use the same engine-aware classifier (`src/gpu_admission/whitted.rs:17`). The added source regression checks Sapphire, Peridot and Ametrine on both engines and retains the Ray HDR storage refusal.

2. **P2 — Intentional exit/off during Ray qualification left a false crash marker. Fixed.** `apps/indicatrix-web/src/render/gpu_gate/ray.rs:139` now starts the pure `ProbeRecord` lifecycle (`crates/indicatrix-web-core/src/gpu_admission/probe.rs:24`) before writing Probing. `ray.rs:236` restores the prior disk record on intentional exit, and `ray.rs:248` rearms Probing on a back/forward-cache return. Abandonment also restores the in-memory record, retaining the automatic-retry block after a failed explicit retry is interrupted. Two unrun pure regressions distinguish a crash from intentional release, resumed ownership, abandonment, success and failure (`probe.rs:67`, `:89`). The existing pagehide/pageshow handling calls these alongside the spectral marker lifecycle (`gpu_gate/startup.rs:272`, `:339`); GPU switch-off abandons Ray before discarding its busy state (`gpu_gate.rs:250`). A genuine unsuccessful/crashed qualification still retains its failure protection.

3. **P2 — Engine switches/explicit tests could queue a second qualification behind a busy sibling and consume its deadline while waiting. Fixed.** Both engines share one asynchronous FIFO worker. Ray qualification now waits while the spectral startup/test/admit phase owns it (`gpu_gate/ray.rs:97`), and spectral admission/manual tests wait for a busy Ray test (`gpu_gate/admit.rs:21`, `gpu_gate.rs:532`). A Ray page-side timeout now uses the shared worker give-up path (`ray.rs:168`), so losing that worker also clears the sibling's usable/admitted state and reports that reload is required.

4. **P2 — Browser AA defaults and export settings were coupled. Fixed.** Browser live Ray AA defaulted to eight instead of the planned four taps, and the PNG dialog edited the live AA property, inheriting its target instead of defaulting to sixteen. `crates/indicatrix-web-core/src/settings.rs:104`, `:199` now supply independent persisted export/live defaults. `settings/engine.rs:30` builds the export engine with its own sanitized taps. The PNG dialog reads/writes a separate Slint property and uses that engine (`apps/indicatrix-web/src/render/export.rs:73`, `:213`; `ui/components/export_dialog.slint:107`). Older settings omit the additive numeric export field and receive sixteen; Path-Traced sample and denoise behavior is unchanged. The existing explicit one-tap export regression remains, and the new regression checks defaults, sanitization, independence and JSON round trip (`src/render/engine_tests.rs:54`).

5. **P2 — Mobile repeatedly uploaded immutable HDR/scene inputs for each tap range. Fixed jointly with the core review.** The shared native facade previously exposed only an all-in-one accumulation call that prepared scene uploads again each time. The core reviewer added owned immutable `WhittedGpuPrepared` uploads. Mobile now owns one `RayGpuSession` per immutable live/export scene (`crates/indicatrix-mobile-core/src/render/backend_whitted.rs:23`), while sharing only the adapter probe. The session lazily prepares once and reuses the upload for subsequent absolute tap ranges. Cancelled preparation can retry; a decline discards cached uploads for that epoch and retraces the complete range on CPU. A later epoch prepares fresh uploads after recovery; no borrowed scene address serves as a cache identity. The unrun regression checks pending cancellation, epoch isolation, subsequent CPU fallback ranges, and the session's Send/Sync bound (`backend_whitted.rs:352`).

6. **P2 — A poisoned mobile upload cache could reuse a partially updated Ready entry. Fixed during final integration review.** `crates/indicatrix-mobile-core/src/render/backend_whitted.rs:54` now declines poisoned upload locks permanently for that scene session instead of recovering their contents. Already-cancelled work returns Cancelled, including cancellation observed after the poisoned lock is acquired. No poisoned Ready entry reaches preparation or dispatch. A new unrun panic/poison regression checks cancellation precedence, repeated permanent decline, and unchanged accumulator values (`backend_whitted.rs:319`).

## Other reviewed paths

- Browser scene replacement posts scene then engine metadata before CPU/GPU assignments; respawn replays metadata and old scene ids cannot merge into the new accumulator. Ray target capping happens before scene restart and before start. GPU declines return a complete range to the CPU planner; private GPU sums are committed only on a complete healthy pass. Four-binding acquisition remains separate from the event-wavefront renderer. PNG requests suppress Ray denoise even if stale caller settings retain it.
- Android keys include EngineSpec and backend/remote identity. Local preparation installs the engine before routing. Live Draft/Normal/Fine targets are 1/4/8, still exports use 16, and progress/denoise follow the selected engine. The independent Ray GPU probe is shared by live/exports, CPU choice does not acquire it, and tool/memory/device declines retain the same CPU physics. Cancellation reaches both GPU work and whole-range CPU retry; partial GPU sums are discarded.
- Mobile remote live/reconnect, FullData, DisplayOnly, final PNG and durable jobs carry the selected EngineSpec. Local resolution refuses UV/fluorescence before the connection, sample bounds are checked, Ray DisplayOnly does not require denoise, and Ray pictures are marked undenoised. HDR ownership/assets, endpoint authentication and epoch/background cancellation remain on their established paths.
- No additional concrete client source defect was confirmed beyond the findings above. Android UI and the browser GPU worker required no edits from this review; the shared native prepared-upload API is owned/documented by the core reviewer.

## Edited files

- `crates/indicatrix-web-core/src/gpu_admission.rs`
- `crates/indicatrix-web-core/src/gpu_admission/whitted.rs`
- `crates/indicatrix-web-core/src/gpu_admission/probe.rs`
- `crates/indicatrix-web-core/src/host/gpu_slot.rs`
- `crates/indicatrix-web-core/src/settings.rs`
- `crates/indicatrix-web-core/src/settings/engine.rs`
- `crates/indicatrix-web-core/src/render/engine_tests.rs`
- `apps/indicatrix-web/src/render/export.rs`
- `apps/indicatrix-web/src/render/gpu_gate.rs`
- `apps/indicatrix-web/src/render/gpu_gate/admit.rs`
- `apps/indicatrix-web/src/render/gpu_gate/ray.rs`
- `apps/indicatrix-web/src/render/gpu_gate/startup.rs`
- `apps/indicatrix-web/ui/components/render_model.slint`
- `apps/indicatrix-web/ui/components/export_dialog.slint`
- `crates/indicatrix-mobile-core/src/render/backend.rs`
- `crates/indicatrix-mobile-core/src/render/backend_whitted.rs`
- This review record.

Every intentionally edited Rust file remains below 800 lines (largest: gpu_gate.rs, 723). No existing image pin or tolerance was weakened or repinned.

## Residual runtime validation

The new source regressions are unrun. Browser wasm/Slint compilation, native feature combinations, actual four-binding adapters, qualification timeout/exit/back-forward-cache ordering, engine switching during tests, GPU decline/device-loss recovery, and physical Android thermal/background behavior still need owner validation. Remote acceptance requires authenticated worker/coordinator sessions for both transfer modes, still export and durable jobs. Source inspection and rustfmt do not establish runtime correctness or image/performance parity.
