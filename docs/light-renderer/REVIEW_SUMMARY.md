# Lightweight Renderer: Second Source Review

Reviewed on 2026-10-20. Scope: the new lightweight renderer and the integration changed for it. Unrelated existing implementation, the two AI crates, optimizer changes and the earlier Android project were excluded.

## Verdict

The review found correctness and performance defects, so the original implementation was not ready for acceptance. Three Sol 6.1 agents corrected the confirmed findings in parallel; the lead reviewed their fixes and shared API integration. The corrected implementation is ready for the owner's validation run. No remaining concrete source blocker was identified within this review's scope.

Production readiness is not yet established. No build, test, Clippy, GPU probe, device session or performance benchmark was run. Source/API inspection and standalone rustfmt do not establish successful compilation, image agreement or runtime behavior.

## Corrected Findings

| Area | Problem | Correction |
|---|---|---|
| Desktop mode transitions | A paused Path view could leave Ray frozen despite hiding its pause control. | Local and remote scheduling use effective pause; the Path preference is preserved. |
| Desktop Edit | Unsupported UV/fluorescence could leave a stale Ray image with no explanation. | The Ray viewport displays the existing refusal and dims stale content. |
| Browser GPU admission | The public admission gate applied spectral material restrictions to Ray. | Page admission and pass scheduling share an engine-aware classifier. |
| Browser qualification | Intentional exit/off could look like a crash; sibling qualifications could consume deadlines while queued. | Explicit probe-record lifecycle, mutually exclusive qualification, and shared worker failure handling. |
| Browser AA settings | Live default differed from the plan and export AA changed the live setting. | Independent persisted defaults: four live taps and sixteen export taps. |
| Native GPU preparation | Every tap claim could re-upload geometry/HDR; desktop guide generation rebuilt the plane arena per pixel. | Owned immutable GPU snapshots reused by desktop, phone, worker and export consumers; desktop shares its plane arena. |
| GPU resource lifetime | Cached uploads need strict device and panic boundaries. | Replaced-device uploads decline, poisoned caches/backend state are not reused, and complete rejected claims fall back to CPU. |
| GPU boundary handling | Large raster indexing lost integer precision; public preparation counts differed from CPU sanitation; geometry checks followed packing. | Integer raster dimensions, matching defensive count handling, and allocation checks before packing. |
| Worker completion | An incomplete Ray range could be reported as success or cancellation. | Explicit failure propagation through stream, coordinator, batch, durable-job and standalone output paths. |
| Remote admission | Durable jobs accepted incomplete AA sets too early; Ray beta-absorption preparation lacked the existing work bound. | Validate complete supported tap sets before asset/storage work and bound the additional absorption list. |

The shared GPU snapshot API accepts no replacement CPU scene at execution time. Consumers own snapshots within their established scene generation, epoch, complete scene identity or frozen export-frame lifetime. Dispatch still validates tap range, output shape, device identity and work limits; cancelled or declined GPU work does not commit partial sums.

## Compatibility Review

- Path-Traced defaults, the existing spectral implementation and its admission hashes remain on their established paths.
- Ray still uses fixed absolute AA taps and bypasses denoising. UV/fluorescence is explicitly refused rather than silently changing engines.
- Render jobs retain their authoritative engine at execution/storage boundaries. This review adds no protocol or job-format change beyond the original feature's protocol 28 / Ray format 2.
- Numeric persisted choices and new browser export defaults preserve loading of older settings.
- Existing image hashes and tolerance assertions were not weakened or repinned. Regression sources were added for the corrected behavior but remain unrun.
- The final static file check found all 40 Rust files referenced by the correction reports present and below 800 lines (largest: 723).
- The existing summed-XYZ f32 protocol cannot promise bit identity for arbitrary regrouping of tap sums. Its documented small numerical tolerance remains; this review did not change that contract.

## Owner Acceptance Gates

1. Run the repository's normal build, test and Clippy checks, including the new regression sources. Cover native CPU/GPU configurations, worker library-only/CLI builds, browser wasm/Slint and Android builds.
2. Run the actual small-kernel GPU comparison suite on supported adapters, including four-storage-buffer browser admission, HDR/concave scenes, cancellation, device replacement and CPU fallback.
3. Exercise pause/mode transitions and refusal UI, browser Test GPU/off/exit/back-forward-cache behavior, independent export settings, and physical Android background/thermal behavior.
4. Exercise authenticated local/remote/both rendering, DisplayOnly and full-data paths, batch failures, still/tilt export, and durable-job completion/cancellation/restart.
5. Complete the plan's image-gallery, CPU/GPU parity and performance gates. Verify upload reuse on large HDR scenes and capture/review the new Windows image pin before release.

Detailed records: [desktop](REVIEW_desktop.md), [shared core/GPU](REVIEW_core.md), [browser/Android](REVIEW_clients.md), and [worker/remote/export](REVIEW_remote.md).
