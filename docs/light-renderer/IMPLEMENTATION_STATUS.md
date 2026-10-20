# Lightweight Renderer Implementation Status

Implementation is integrated across all five planned waves. No build, test, Clippy, GPU probe or performance command was run. Standalone rustfmt and source/API/call-site review were used. Existing spectral image hashes were not repinned; new image pins remain ignored owner-capture probes.

| Area | Implementation | Review record |
|---|---|---|
| Shared options, deterministic chain, tint/environment/frost | CPU core and portable preparation | L1_report.md, L2_report.md |
| Desktop image loop/settings, five Edit modes, three Live modes | Opt-in, fixed taps, cached preparation, no Ray denoise | L3_report.md, L4_report.md |
| Guides/help/Compare | Mode validators, commands, Ray Compare and refusal states | L5_report.md |
| Still/tilt exports, jobs, CLI, thumbnails | Independent choices, persisted fingerprints and format 2 | L6_report.md |
| Browser CPU and message protocol | Engine metadata, worker replay, bounded tap planner | L7_report.md |
| Android CPU/UI/settings | Quality mapping, 16-tap export, numeric persistence | L8_report.md |
| Independent native GPU | Four storage buffers, transactional CPU fallback | L9_report.md |
| Browser GPU | Independent hash/qualification, four-binding fallback acquisition | L10_report.md |
| Android GPU | Small-kernel probe/cache, work limits, cancellation | L11_report.md |
| Network/workers/remote jobs | Protocol 28, engine-preserving dispatch, no Ray denoise | L12_report.md, L12_mobile_report.md |
| Documentation/site | Updated manuals/READMEs and staged site diff | L13_L14_report.md |

Remote rendering (lane L12) shipped as part of this campaign, although decision D7 asked for a separate owner go after the performance check. It stays behind the documented `remote_allowed` switch in `apps/indicatrix-cut/src/bridge/render_thread/engine.rs`, which currently returns `true` for both engines.

## Owner Validation

Run the repository review checks after syncing all changed files; step 4 includes the new CPU renderer tests and optional step 15 exercises the actual small GPU renderer when RUN_GPU=1. Browser wasm and Android checks remain in their existing steps. Do not treat source review as a successful build or device test. Review the new screenshots/gallery and capture the Windows-only tiny-frame pin before release.

The summed-XYZ protocol preserves the existing f32 accumulator. Thread and pixel partitions are exact, while regrouping summed tap ranges can differ in their last bits. The new range regression documents a small floating-point tolerance; see L12_report.md for why arbitrary sample regrouping cannot promise bit identity under this wire contract.

Website changes were applied to the separate C:/temp/mcp2/ghp checkout after filesystem approval; copied bytes match the reviewed staging manifest. The staging copy (`site-update/`) was removed on 2026-10-20. Nothing was published.

## Final Source Review Fixes

The lead corrected independent four-binding browser admission, worker CLI feature gating (library-only builds keep renderer dependencies optional), cancellation during phone GPU fallback, shared HDR ownership in desktop preparation, remote fixed-tap epochs and denoise suppression, and explicit fluorescence/diagnostic refusals at output boundaries. An independent agent also reviewed the desktop and mobile state/target/cancellation paths. No additional source-level blocker remained after review; runtime acceptance is still unverified.

## Second Source Review (2026-10-20)

A further review of only the new implementation found and corrected desktop pause/refusal handling, browser material admission and qualification lifecycle, independent browser AA defaults, repeated native GPU scene uploads, GPU boundary/resource handling, worker failure propagation and remote admission bounds. Three Sol 6.1 agents implemented the corrections and the lead reviewed their integration. See [REVIEW_SUMMARY.md](REVIEW_SUMMARY.md) and its linked lane records. The code is ready for owner validation; production readiness still depends on the unrun build, regression, GPU/device, image and performance gates above.
