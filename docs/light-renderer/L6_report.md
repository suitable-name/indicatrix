# L6 Desktop output implementation report

DONE. Source implementation and review complete; no builds, tests, Clippy, Cargo or Git were run. Standalone rustfmt parsed/formatted the owned Rust sources (stable rustfmt warns that the repository's nightly import-granularity options are unavailable).

## Implemented

- EngineSpec carried independently in ExportParams, SceneSnapshot, frozen jobs and each render boundary; Path-Traced algorithms and fingerprint strings retained.
- Shared export SampleCursor distributes absolute Ray AA taps to Local/Remote/Both. WhittedPrep and SoA are cached per frame. GPU/CpuGpu use the small deterministic GPU backend, cached across tilt frames; a transactional decline retries the entire exact claim on CPU. No hybrid calibration or denoising for Ray.
- Final-picture transfers and local contributions use the same Ray engine and absolute tap ranges. UV/fluorescence and diagnostic conflicts fail before rendering or network dispatch.
- Independent persisted still/tilt Renderer, AA and Fire controls; defaults remain Path-Traced, output AA starts at16. Advanced Ray settings moved below all Path live-quality controls.
- Frozen job writers retain format1 for Path, format2 for Ray; readers accept through2. Ray count must equal taps, and diagnostics are refused. Queue metadata records actual format. CLI --engine path|ray updates engine, transport scene and fixed tap target, with help/README/parser coverage.
- Persisted Thumbnail Renderer defaults Path-Traced. Settings, startup/reset and offer quality/recount all use it. RayPreview and RayDraft fingerprints are distinct and versioned; quick Ray drafts stay local CPU and cannot satisfy current persistent previews. Optional remembered import choice uses string ray/int4.
- Compare carries active fluorescence refusal by side before calling the RGB helper; catalogue Ray dispatch checks stored physics fluorescence before either lane. Approximation copy clarifies one index per color chain.
- Added source regressions for legacy/Ray job round trips and strict counts, future-format refusal, CLI engine parsing, exact RayDraft fingerprint/quality invalidation, persistent preview scans, absolute-tap assembly, diagnostic and Compare fluorescence refusal. None executed.

## API verification

- EngineSpec/RenderEngine/RayOptions: crates/indicatrix/src/renderer/engine.rs.
- WhittedPrep and WHITTED_VERSION: crates/indicatrix/src/renderer/whitted/{mod,prep}.rs.
- try_accumulate_cancellable/GpuAccumulate transactional behavior: renderer/gpu_backend/whitted.rs and native backend implementation; live desktop engine.rs uses the same interface.
- SceneState.engine and protocol28: indicatrix-net scene/messages; every owned constructor checked. Worker production job propagation is root's L12 work and now uses request.scene.engine.
- snapshot_color/SnapshotColor and ColorMode::fluorescence: cut-core native/color modules; existing restore_material.rs pattern reused.
- custom_material_fluorescence_from_rows and RenderContext custom-material/physics/fluorescence sidecars: crystal_optics.rs and render_thread/context/mod.rs.
- Existing shared SampleCursor, remote final-image contribution, picker/export callbacks and Slint models reused; 6-int queue callback unchanged.

## Requests and limits

- No remaining cross-file implementation requests. Root lifted remote_allowed for protocol28 and owns NET zoning payload omission for Ray.
- New/modified owned Rust sources remain below800 lines, except the already oversized AppSettings file where this lane adds only the thumbnail renderer field/default. Worker store remains794 lines (its final propagation is root-owned).
- Runtime numerical/device/remote/UI validation is intentionally deferred to the owner. Source formatting is not a build or a proof that the renderer executes correctly.
- Named custom catalogue materials re-read their small physics table only when a Ray item needs that fluorescence guard; built-ins use no additional read.

## Rust line counts

- 654: `apps/indicatrix-cli/src/args/parsers.rs`
- 671: `apps/indicatrix-cli/src/args/tests.rs`
- 561: `apps/indicatrix-cli/src/args/types.rs`
- 535: `apps/indicatrix-cli/src/commands/render_job.rs`
- 689: `apps/indicatrix-cli/src/help.rs`
- 530: `apps/indicatrix-cut/src/bridge/export_thread/batch.rs`
- 208: `apps/indicatrix-cut/src/bridge/export_thread/hybrid_export_tests.rs`
- 250: `apps/indicatrix-cut/src/bridge/export_thread/params.rs`
- 169: `apps/indicatrix-cut/src/bridge/export_thread/remote/dispatch/scene_state.rs`
- 266: `apps/indicatrix-cut/src/bridge/export_thread/remote/dispatch/tests.rs`
- 637: `apps/indicatrix-cut/src/bridge/export_thread/scene_snapshot.rs`
- 342: `apps/indicatrix-cut/src/bridge/export_thread/tests.rs`
- 374: `apps/indicatrix-cut/src/bridge/export_thread/worker/core.rs`
- 672: `apps/indicatrix-cut/src/bridge/export_thread/worker/core/lanes.rs`
- 161: `apps/indicatrix-cut/src/bridge/export_thread/worker/export.rs`
- 91: `apps/indicatrix-cut/src/bridge/export_thread/worker/export_ray.rs`
- 595: `apps/indicatrix-cut/src/bridge/export_thread/worker/final_picture.rs`
- 42: `apps/indicatrix-cut/src/bridge/export_thread/worker/mod.rs`
- 184: `apps/indicatrix-cut/src/bridge/export_thread/worker/tests.rs`
- 74: `apps/indicatrix-cut/src/bridge/export_thread/worker/types.rs`
- 712: `apps/indicatrix-cut/src/bridge/preview_render.rs`
- 326: `apps/indicatrix-cut/src/bridge/preview_render/fingerprint.rs`
- 506: `apps/indicatrix-cut/src/bridge/preview_render/fingerprint_tests.rs`
- 83: `apps/indicatrix-cut/src/bridge/preview_render/ray.rs`
- 251: `apps/indicatrix-cut/src/bridge/preview_render_tests.rs`
- 582: `apps/indicatrix-cut/src/bridge/remote/remote_render/connection/batch_client.rs`
- 777: `apps/indicatrix-cut/src/gui/batch/preview/engine.rs`
- 76: `apps/indicatrix-cut/src/gui/batch/preview/engine/ray.rs`
- 121: `apps/indicatrix-cut/src/gui/batch/preview/frosting_tests.rs`
- 84: `apps/indicatrix-cut/src/gui/batch/preview/import_choice.rs`
- 201: `apps/indicatrix-cut/src/gui/batch/preview/mod.rs`
- 278: `apps/indicatrix-cut/src/gui/batch/preview/offer_quality.rs`
- 355: `apps/indicatrix-cut/src/gui/batch/preview/remote_lane.rs`
- 684: `apps/indicatrix-cut/src/gui/batch/preview/remote_lane/remote_batch.rs`
- 242: `apps/indicatrix-cut/src/gui/batch/preview/scan.rs`
- 788: `apps/indicatrix-cut/src/gui/batch/preview/wiring.rs`
- 699: `apps/indicatrix-cut/src/gui/editor/compare/metrics.rs`
- 330: `apps/indicatrix-cut/src/gui/editor/compare/origins.rs`
- 671: `apps/indicatrix-cut/src/gui/editor/compare/render.rs`
- 546: `apps/indicatrix-cut/src/gui/editor/compare/session.rs`
- 772: `apps/indicatrix-cut/src/gui/editor/compare/tests.rs`
- 586: `apps/indicatrix-cut/src/gui/render/output_defaults.rs`
- 737: `apps/indicatrix-cut/src/gui/render/render_export/wiring.rs`
- 391: `apps/indicatrix-cut/src/gui/render_jobs/capture_still.rs`
- 202: `apps/indicatrix-cut/src/gui/render_jobs/capture_video.rs`
- 455: `apps/indicatrix-cut/src/gui/render_jobs/convert.rs`
- 386: `apps/indicatrix-cut/src/gui/render_jobs/execute.rs`
- 642: `apps/indicatrix-cut/src/gui/render_jobs/tests.rs`
- 647: `apps/indicatrix-cut/src/gui/render_jobs/ui_tests.rs`
- 269: `apps/indicatrix-cut/src/gui/render_jobs/ui_tests_bundle.rs`
- 789: `apps/indicatrix-cut/src/gui/startup_settings.rs`
- 760: `apps/indicatrix-cut/src/gui/tilt/video_export/frames.rs`
- 743: `apps/indicatrix-cut/src/gui/tilt/video_export/mod.rs`
- 301: `apps/indicatrix-cut/src/gui/tilt/video_export/render.rs`
- 895: `apps/indicatrix-cut/src/settings/model/app_settings.rs`
- 48: `apps/indicatrix-cut/src/settings/model/import_preview.rs`
- 354: `apps/indicatrix-cut/src/settings/model/output_settings.rs`
- 371: `apps/indicatrix-cut/src/settings/model/parameter_defaults.rs`
- 299: `apps/indicatrix-cut/tests/ui_harness/coverage_allow.rs`
- 128: `apps/indicatrix-cut/tests/ui_harness/scenarios/coverage/rows_startup.rs`
- 353: `apps/indicatrix-worker/src/jobs/run.rs`
- 794: `apps/indicatrix-worker/src/jobs/store.rs`
- 291: `crates/indicatrix-render-jobs/src/codec.rs`
- 711: `crates/indicatrix-render-jobs/src/job.rs`
- 358: `crates/indicatrix-render-jobs/src/job_tests.rs`
- 37: `crates/indicatrix-render-jobs/src/lib.rs`
