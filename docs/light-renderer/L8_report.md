# L8 Android CPU And UI Integration

Implemented persisted numeric renderer selection (`render.engine`, unknown values fall back to Path-Traced without discarding other settings), scene identity, fixed quality-to-tap mapping (Draft 1, Normal 4, Fine 8), sixteen-tap still exports and Ray-Traced progress text. CPU chunks reuse the shared browser-core OwnedScene preparation. Engine changes cancel old accumulation and restart the selected view. Battery/thermal policy still controls worker availability; the Ray target is its fixed tap count rather than the spectral sample clamp.

The Android render controls offer Path-Traced/Ray-Traced, hide denoising for Ray-Traced, show the approximation caveat, and explain which material controls are omitted or averaged. The export sheet shows sixteen fixed AA taps. Desktop-preview CLI accepts `--engine path|ray`; phone/tablet Ray screenshot entries were added. Settings tests cover old/default files, future numeric engine values and the quality mapping; tests and screenshots were not run.

L11 subsequently adds the independent small GPU backend, bounded phone GPU work and complete CPU fallback. L12 adds authenticated remote engine propagation, HDR assets, live DisplayOnly without denoise, exports and durable jobs; the temporary CPU-only remote exclusion is removed. Those lanes have separate reports.

Source review and standalone rustfmt only. No builds, cargo, tests, Clippy or git commands were run.

## Source Line Counts

| File | Lines |
|---|---:|
| `crates/indicatrix-mobile-core/src/settings/model.rs` | 377 |
| `crates/indicatrix-mobile-core/src/settings/tests.rs` | 343 |
| `crates/indicatrix-mobile-core/src/render.rs` | 596 |
| `crates/indicatrix-mobile-core/src/render/pool.rs` | 749 |
| `crates/indicatrix-mobile-core/src/render/export.rs` | 409 |
