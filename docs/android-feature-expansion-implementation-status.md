# Android feature expansion: implementation handoff

Status recorded 2026-10-19, updated the same day with the C, E and I completion work
(in source, uncompiled). This accompanies
[the accepted plan](android-feature-expansion-implementation-plan.md); it does not
replace its acceptance criteria. No Cargo command, build, test, Clippy command or
Git command was run by the implementing agent. Rustfmt and direct source review
were used. The primary agent completed the source review and applied the corrections
listed below. No builds, tests or Clippy were run during that review either.
Device, compiler, numerical and interoperability validation remain open.

## Milestones

| Milestone | Implemented behavior | Remaining condition |
|---|---|---|
| A / WP0–1 | Document/run/input provenance; cancellation and stale completion checks; one navigation contract; pinned Slint ownership opt-out; app-owned API 33+ Back and API 34+ predictive callbacks; legacy route below API 33. | Android navigation/device matrix. The patch has not been submitted upstream. |
| B / WP2 | Shared cancellable compute permits; thermal hysteresis; app/system power saver; foreground handling; governed rendering, export, operation workers and analysis. Backend selection remains explicit; Automatic stays local. | Owner-run contention, lifecycle, heat, battery and memory measurements. |
| C / WP3 | Existing CA-pinned enrollment and mutual TLS; encrypted Android credential vault; protocol/build compatibility; endpoint/certificate/capability reporting; explicit analytic-scene interactive DisplayOnly or FullData rendering; FullData PNG export; bounded decoding; source-stamped frames; cancel and explicit retry. **Completed in source:** exact effective HDR asset interchange (versioned float `IXENVMAP` asset, SHA-256 content hash, endpoint `hdr` capability, bounded on-disk server cache and bounded upload); coordinator DisplayOnly FinalImage export (the endpoint's finished PNG, ICC profile attached on the phone); metered-network policy (`Platform::network_metered`, Android `ConnectivityManager`, setting Allow Metered Networks, desktop/unknown default unmetered); reconnection (bounded automatic restart from sample zero after a lost transport, wait for the foreground after a stay in the background) and a write path that survives full send buffers during a large upload. | Nothing compiled or run. Device validation of the upload over real Wi-Fi/mobile data, of the background/resume reconnect, and an end-to-end HDR render compared with the device render. |
| D / WP4 | Existing GPU backend connected to mobile; worker-side adapter probing; bounded memory, samples and dispatch turns; genuine adapter labels; transactional cancellation/CPU fallback; live and export routing; default Android GPU feature and CPU-only build option. | Representative Adreno/Mali validation; sustained thermal/memory measurements; reduced-binding kernels if required by the intended device set. Current unsupported adapters/scenes decline to exact CPU. Concave stones now route to the GPU (see below). |
| E / WP5 | History listing/jump; immutable comparison; current/historical/candidate variant capture; worker-owned vault CRUD; dirty replacement confirmation; shared atomic labeled replacement. **Completed in source:** a variant whose custom material recipe differs from the catalog opens, and the recipe is installed as part of the same undoable step (`recipe_steps`): Undo, Redo and History jumps bring the old or new recipe with the design, and a new edit after Undo drops the step with its redo branch. | A same-design variant that differs only in the recipe is still refused with a sentence (there is no step to attach the change to). Tests were written, not run. |
| F / WP6–7 | Deep Solve with external printed targets, report and explicit Pin; Retarget Shift using the shared preservation baseline, checked complete authored candidate, comparison and one-step Apply. | Owner's shared preservation regression gate and device validation. External target entry is supported; richer durable source labels/import provenance remain limited. |
| G / WP7–8 | Retarget search and Optimize forms, bounded governed workers, real progress/cancellation, immutable candidate provenance, comparison and Save Variant, shared validation and atomic Apply. Candidate reproduction/solves are prepared off the UI thread. | Done 2026-10-19: the shared regressions pass (owner's run 5), and `validated-design-search` is a default feature, so release builds include Optimize and Retarget. |
| H / WP9 | Typed concave tool/motion forms, stale-safe Save/Delete, independent concave selection, actual resolved tools in CPU scenes, denoise guides and remote SceneState; GPU scenes beyond the GPU's own limits (more than 64 tools, fluorescence or UV lighting, unsupported materials) use exact CPU, and the render report says why. | End-to-end solve/solid/diagram/render/export/undo device validation and every tool/motion combination. No automatic commit on form typing. |
| I | Session interruption/restart behavior is stated honestly. **Completed in source:** durable authenticated server jobs (protocol v25 `ClientMessage::Job`): server-derived durable id from the client certificate and a client token, persisted ownership, status, result retrieval, cancel, delete, list, retention (default 7 days, `INDICATRIX_JOB_RETENTION_HOURS`), restart recovery, file-backed storage in the `indicatrix-render-jobs` file shape; the phone submits from the export sheet (Send To Server) and lists, saves, cancels and deletes jobs under Remote > Server Jobs. | Nothing compiled or run. A server job is rendered on the worker's CPU lane (the GPU path is limited to 512 x 512 on the shared viewer lane); color-zoned materials are refused. Owner validation against a real worker. |

## GPU routing of concave stones

The mobile backend (`render/backend.rs`) no longer sends a stone with concave tools to
the CPU just because it has tools. It asks `scene_decline_with_tools` (the tool-aware
routing of the shared GPU backend) and traces with `GpuBackend::try_accumulate_geom_cancellable`,
passing the stone's tools beside the plane scene, with the mobile work limits unchanged
(`GpuWorkLimits::MOBILE`). The megakernel traces up to 64 tools
(`GPU_MAX_TOOL_PRIMITIVES`); above that, with fluorescence or a UV lamp, or with a
material the kernels cannot render, the exact CPU tracer takes over and the backend
report states the reason (for example "The stone has 65 concave tools; the GPU traces at
most 64; exact CPU tracing is used"). The legacy plane-only `scene_routes_to_gpu` is not
used for mobile any more and was not changed (it still declines tools, which is what
keeps callers without a tool list from drawing a stone flat). The tool kernels are
unmeasured on Adreno/Mali; run the probe test on a device before trusting them.

## Remote boundaries

Enrollment claims are validated on a worker and accepted on the UI thread only
while the same request and page remain current. Cancellation or leaving Remote
before acceptance drops the uninstalled credentials. After acceptance, the short
atomic encrypted vault write completes on its worker; it is no longer cancellable.
Enrollment network redemption and DNS resolution are not promptly interruptible:
the shared enrollment helper uses its existing socket timeouts, and DNS follows
the system resolver. Cancellation prevents installation of unaccepted results,
but a token may already have been redeemed and require replacement.

No scene is uploaded by Automatic, enrollment or Test Connection. Selecting Remote
in the Render view is the explicit scene-upload action. Only resolved supported
scenes are sent: material, planes, tools, fluorescence, camera and lighting. Zoned
materials require the endpoint's advertised zoning extension. Coordinator
DisplayOnly images never enter a radiance accumulator. Standalone workers require
an explicit FullData choice.

Phone-side remote output is capped at 1,048,576 pixels and checked against the
endpoint limit. Payloads are bounded before allocation. FullData sample ranges and
counts use the shared protocol accumulator. Final PNG export works in both
transfer modes: FullData traces on the endpoint and the phone tone-maps and
encodes the PNG locally; DisplayOnly asks a coordinator for the finished picture
(`FinalImageRequest`) and only attaches the ICC profile of the chosen color space.
Standalone workers cannot deliver a finished picture and are refused with a
sentence.

An HDR scene sends the device's effective environment map, exactly as the device
renders with it (rotation, exposure and size reduction already applied), as a
versioned floating-point asset: `IXENVMAP` v1, the same bytes as the private
`current.envmap` file. The scene names it by its SHA-256; the endpoint asks for it
(`NEED_ASSET`) only when its bounded cache does not hold it, verifies the hash and
decodes it bit for bit. It is sent only to an endpoint that advertises HDR support,
never approximated as `.hdr`, and never substituted. A map over the 256 MiB asset
limit is refused before connecting.

The metered-network policy: when Android reports the active network as metered
(`ConnectivityManager`, permission `ACCESS_NETWORK_STATE`) and Allow Metered Networks
is off in Remote settings, a session that would move many megabytes (a lighting-map
upload, FullData frames, a job result download) is refused before any connection
with a sentence. DisplayOnly pictures without a lighting map and job management
requests are not blocked. A platform that cannot tell (and the desktop preview)
counts as unmetered.

A live interactive session is one worker owning the complete TLS connection and cannot
be resumed. When it is interrupted the epoch starts a new connection from sample zero:
after a lost transport up to three times with a growing pause, and after the app
returned from the background without counting an attempt. A refusal or invalid data
is not retried. Activity recreation that keeps the process uses the same path; a
process that was killed loses the live session, and the picture is rendered again
when the user asks, or fetched if it was sent as a server job.

Durable server jobs outlive all of that. The server derives the job id from the
client certificate fingerprint and the client's random token, stores the job as a
`*.job.json` file (the `indicatrix-render-jobs` file shape) beside its result PNG,
answers only the owner, and keeps a finished job for its retention. The phone keeps
nothing: "my jobs" is the endpoint's list for the enrolled certificate. A Submit
whose answer was lost is repeated with the same token and finds the same job.

The Android vault encrypts the exportable TLS credential bundle using AES-GCM and
an Android Keystore wrapping key in no-backup app storage. This does **not** claim
that the TLS private key is non-exportable or hardware backed. Secrets are not
written into ordinary settings, shared files or logs.

## Review corrections applied by the primary agent

- Back now enters the toolkit through Escape so dropdowns close before their
  enclosing page. Versioned commits reject duplicate/stale native callbacks,
  including a duplicate after popup dismissal. Legacy Back consumes the whole
  popup-dismissal key sequence; root navigation remains Android-owned. Navigation
  priority follows visible overlays and identifies different More/dialog pages.
- The enlarged Design overflow menu is height-bounded and scrollable. Remote has
  a separate labeled upload action instead of being reached by cycling through
  device backends. Switching to Remote clears a previous local pause.
- Optimizer and Retarget inputs preserve imported refractive indices and reject
  unresolved or unrepresentable materials. Certified candidate edits, reproduction
  and solves are prepared on workers; Apply uses the accepted edit and original
  identity. Matching failed/cancelled completions clear busy state without letting
  older callbacks change a newer run.
- History comparison includes solved masts and complete authoring metadata;
  historical variant previews survive closing History. Family identity is assigned
  before capture. Current name-keyed custom recipes take precedence over stale
  opened-file recipes during saving and variant capture.
- Deep Solve resolves printed dimensional targets before solving. Analysis uses
  exact concave geometry, document/material identity and shared resource permits.
  Result publication checks request and busy-slot identity under the same mutex.
- Scene preparation, denoise guides/filtering and export conversion obey the shared
  compute budget. Denoise guides include exact concave tools. GPU recovery is
  attempted through the existing bounded backend path.
- Export callbacks are stamped with run/document identity and recheck cancellation
  before publishing or sharing. Background/thermal cancellation refreshes operation
  screens. Remote failures cannot overwrite a newer render epoch.
- Remote replies must match the negotiated transfer mode and exact requested sample
  count; DisplayOnly pixels cannot complete a FullData radiance export. Credential
  claim acceptance and atomic installation are separate. Leaving the Remote page,
  cancellation and backgrounding discard unaccepted claims and clear busy state;
  endpoint edits invalidate displayed certificate identity. Vault reads recover
  interrupted AtomicFile replacements before checking encrypted size.
- Optimize and Retarget accept designs with concave tiers: the shared engines now
  score every candidate on the stone with its tools (cut-core `ScoredStone` and the
  `*_geom_under` objectives, editor `measure_analysis_column`), so the mobile
  declines were removed (`optimization.rs`, `retarget.rs`; tests in
  `concave_scoring_tests.rs`). Deep Solve still declines concave designs: it verifies the
  planar solve against printed flat proportions, and the shared `availability` says so.
- Leaving the foreground (file picker, share sheet) stops work in flight but no longer
  discards a finished Optimize or Retarget result (`OperationState::interrupt`).
- The four unused `handle-back` functions were removed from the Slint models; the
  navigation owner is `navigation.slint` and the Android dispatcher. The Back dispatch
  failure is logged with `tracing`, not `eprintln!`. New and changed labels use Title
  Case. The per-progress-tick fingerprint recomputation on the UI thread (Optimize,
  Retarget, Deep Solve) was left as it is: it is the identity check by design and costs
  one design serialization per tick; the owner measures before it is changed.
- Changed oversized Rust files were mechanically split without executing their
  tests. All Android/mobile-core/vendor Rust files are below 800 lines after review.

Source inspection and formatting do not establish compiler, numerical or device
correctness. The owner can run the validation commands below now. The partial
milestones above remain incomplete and must not be described as finished features.

## Source inventory

Paths below are relative to `C:/Users/ngussek/Dev/Rust/diagram_loader`. This lists
the implementation and integration surface; reviewer changes to these files are
part of the same working copy. Existing unrelated workspace edits were retained.

Workspace/dependencies:

```text
Cargo.toml
Cargo.lock
crates/indicatrix-mobile-core/Cargo.toml
apps/indicatrix-mobile-android/Cargo.toml
```

Mobile core:

```text
crates/indicatrix-mobile-core/src/lib.rs
crates/indicatrix-mobile-core/src/app_state.rs
crates/indicatrix-mobile-core/src/design_state.rs
crates/indicatrix-mobile-core/src/document.rs
crates/indicatrix-mobile-core/src/platform.rs
crates/indicatrix-mobile-core/src/operations.rs
crates/indicatrix-mobile-core/src/operation_material.rs
crates/indicatrix-mobile-core/src/analysis.rs
crates/indicatrix-mobile-core/src/analysis/inputs.rs
crates/indicatrix-mobile-core/src/analysis/worker.rs
crates/indicatrix-mobile-core/src/compute_policy.rs
crates/indicatrix-mobile-core/src/concave.rs
crates/indicatrix-mobile-core/src/deep_solve.rs
crates/indicatrix-mobile-core/src/history.rs
crates/indicatrix-mobile-core/src/variants.rs
crates/indicatrix-mobile-core/src/optimization.rs
crates/indicatrix-mobile-core/src/retarget.rs
crates/indicatrix-mobile-core/src/library.rs
crates/indicatrix-mobile-core/src/library/variants.rs
crates/indicatrix-mobile-core/src/settings.rs
crates/indicatrix-mobile-core/src/settings/model.rs
crates/indicatrix-mobile-core/src/render.rs
crates/indicatrix-mobile-core/src/render/backend.rs
crates/indicatrix-mobile-core/src/render/pool.rs
crates/indicatrix-mobile-core/src/render/governor.rs
crates/indicatrix-mobile-core/src/render/scene.rs
crates/indicatrix-mobile-core/src/render/export.rs
crates/indicatrix-mobile-core/src/render/geometry_tests.rs
crates/indicatrix-mobile-core/src/render/tests.rs
crates/indicatrix-mobile-core/src/remote.rs
crates/indicatrix-mobile-core/src/remote/rendering.rs
crates/indicatrix-mobile-core/src/remote/final_image.rs
crates/indicatrix-mobile-core/src/remote/jobs.rs
crates/indicatrix-mobile-core/src/render/remote_session.rs
crates/indicatrix-mobile-core/src/recipe_steps.rs
crates/indicatrix-mobile-core/src/concave_scoring_tests.rs
crates/indicatrix-mobile-core/src/viewport.rs
crates/indicatrix-mobile-core/src/viewport/state.rs
crates/indicatrix-mobile-core/src/viewport/render_view.rs
crates/indicatrix-mobile-core/src/viewport/render_view_tests.rs
```

Shared helpers and rendering:

```text
crates/indicatrix-editor/src/lib.rs
crates/indicatrix-editor/src/deep_solve.rs
crates/indicatrix-editor/src/variants.rs
crates/indicatrix-editor/src/optimize_view.rs
apps/indicatrix-cut/src/gui/editor/deep_solve.rs
apps/indicatrix-cut/src/gui/editor/view/solve_results.rs
apps/indicatrix-cut/src/gui/editor/variants/apply.rs
crates/indicatrix-web-core/src/scene/mod.rs
crates/indicatrix-web-core/src/scene/geometry.rs
crates/indicatrix-web-core/src/render/mod.rs
crates/indicatrix-web-core/src/display.rs
crates/indicatrix-web-core/src/display/tests.rs
crates/indicatrix-web-core/src/solve/mod.rs
crates/indicatrix-web-core/src/solve/optical.rs
crates/indicatrix-web-core/src/worker.rs
crates/indicatrix-web-core/src/worker/tests.rs
crates/indicatrix/src/renderer/guide_pass.rs
crates/indicatrix/src/renderer/guide_pass/tests.rs
crates/indicatrix/src/renderer/gpu_backend/mod.rs
crates/indicatrix/src/renderer/gpu_backend/backend.rs
crates/indicatrix/src/renderer/gpu_backend/limits.rs
```

Android application:

```text
apps/indicatrix-mobile-android/src/lib.rs
apps/indicatrix-mobile-android/src/run.rs
apps/indicatrix-mobile-android/src/run/ui_state.rs
apps/indicatrix-mobile-android/src/navigation.rs
apps/indicatrix-mobile-android/src/remote.rs
apps/indicatrix-mobile-android/src/remote_jobs.rs
apps/indicatrix-mobile-android/src/commands/mod.rs
apps/indicatrix-mobile-android/src/commands/shell.rs
apps/indicatrix-mobile-android/src/platform_android/mod.rs
apps/indicatrix-mobile-android/src/platform_android/jvm.rs
apps/indicatrix-mobile-android/src/platform_android/back.rs
apps/indicatrix-mobile-android/src/design/mod.rs
apps/indicatrix-mobile-android/src/design/concave.rs
apps/indicatrix-mobile-android/src/design/deep_solve.rs
apps/indicatrix-mobile-android/src/design/history.rs
apps/indicatrix-mobile-android/src/design/history_compare.rs
apps/indicatrix-mobile-android/src/design/variants.rs
apps/indicatrix-mobile-android/src/design/optimization.rs
apps/indicatrix-mobile-android/src/design/retarget.rs
apps/indicatrix-mobile-android/src/design/tiers.rs
apps/indicatrix-mobile-android/src/design/tiers/rows.rs
apps/indicatrix-mobile-android/src/design/render.rs
apps/indicatrix-mobile-android/src/design/render/model.rs
apps/indicatrix-mobile-android/src/design/render/runtime.rs
apps/indicatrix-mobile-android/src/design/render/export.rs
```

Android UI, host and documentation:

```text
apps/indicatrix-mobile-android/ui/app.slint
apps/indicatrix-mobile-android/ui/navigation.slint
apps/indicatrix-mobile-android/ui/pages/design.slint
apps/indicatrix-mobile-android/ui/pages/more.slint
apps/indicatrix-mobile-android/ui/pages/preferences.slint
apps/indicatrix-mobile-android/ui/pages/about.slint
apps/indicatrix-mobile-android/ui/pages/licenses.slint
apps/indicatrix-mobile-android/ui/pages/remote.slint
apps/indicatrix-mobile-android/ui/design/concave.slint
apps/indicatrix-mobile-android/ui/design/deep_solve.slint
apps/indicatrix-mobile-android/ui/design/history.slint
apps/indicatrix-mobile-android/ui/design/variants.slint
apps/indicatrix-mobile-android/ui/design/optimization.slint
apps/indicatrix-mobile-android/ui/design/retarget.slint
apps/indicatrix-mobile-android/ui/design/tiers/tiers_model.slint
apps/indicatrix-mobile-android/ui/design/render/render_model.slint
apps/indicatrix-mobile-android/ui/design/render/render_view.slint
apps/indicatrix-mobile-android/ui/design/render/export_sheet.slint
apps/indicatrix-mobile-android/android/app/src/main/AndroidManifest.xml
apps/indicatrix-mobile-android/android/app/src/main/java/com/indicatrixsuite/mobile/MainActivity.kt
apps/indicatrix-mobile-android/android/app/src/main/java/com/indicatrixsuite/mobile/BackNavigationController.kt
apps/indicatrix-mobile-android/android/app/src/main/java/com/indicatrixsuite/mobile/RemoteCredentialVault.kt
apps/indicatrix-mobile-android/README.md
docs/android-feature-expansion-implementation-status.md
```

The pinned backend is `vendor/slint-android-activity-1.18.1/`, with upstream
Cargo.toml, lib.rs, build.rs, vsync.rs, Java helper, README and all three license
files. `PATCH.md` and `application-owned-back.patch` document the ownership change.
androidwindowadapter.rs/androidwindowadapter_input.rs and
javahelper.rs/javahelper_callbacks.rs are mechanically split in the same lexical
modules to retain the changed-Rust-file size limit. No registry file was edited.

Shared crates changed for C, E and I (all additive; desktop and web behavior is
unchanged, see `C:/temp/mcp2/review_2026-10-19/ANDROID_FINISH_report.md`):

```text
crates/indicatrix/src/renderer/env_map.rs                 EnvironmentMap::texels
crates/indicatrix-net/src/messages/envmap_asset.rs        new: IXENVMAP v1 codec
crates/indicatrix-net/src/messages/job.rs                 new: durable job messages
crates/indicatrix-net/src/messages/mod.rs                 PROTOCOL_VERSION 25
crates/indicatrix-net/src/messages/stream/types.rs        ClientMessage::Job (index 10)
crates/indicatrix-net/src/messages/error_codes.rs         JOB_UNKNOWN, JOB_LIMIT, JOB_STATE
apps/indicatrix-worker/src/assets/decoded.rs              float asset accepted beside .hdr
apps/indicatrix-worker/src/jobs/                          new: job service, store, executor
```

The wire protocol moved from 24 to 25 (postcard is not self-describing, so an old
peer is refused at the handshake). The desktop app and the web app speak the same
`indicatrix-net` and move together with their workers.

Cargo.lock package dependency lists were reconciled as text. Newly used external
dependencies already had resolved lock entries: zeroize 1.9.1, rustls-pemfile 2.2.0
and rustls 0.23.46. The local pinned Slint package retains version 1.18.1. Cargo has
not verified this lock reconciliation. What cargo will do on its first run (a
`--locked` run fails when it would rewrite anything):

- The vendored `i-slint-backend-android-activity` entry lists both `ndk 0.8.0` and
  `ndk 0.9.0`, and `android-activity 0.5.2` and `0.6.x`, because the vendored
  manifest has the optional `aa-05` and `aa-06` features and the lock records every
  optional dependency. If cargo agrees, the entry is final.
- If cargo drops the optional `aa-05` branch (the app selects only
  `backend-android-activity-06`), it removes `android-activity 0.5.2` and `ndk 0.8.0`
  from that entry and prunes the packages only they used (`ndk-sys 0.5.0`,
  `jni 0.21.1`, `windows-sys 0.45.0` and their exclusive transitive dependencies).
  This is a plain rewrite, not a conflict.
- The root `Cargo.toml` now has `exclude = ["vendor"]`, so the vendored crate is a
  plain path dependency and not a workspace member.

## Lane A2 (compile sweep and finish items)

Nobody has compiled this code. A2 read every Rust, Slint and Kotlin file of
`apps/indicatrix-mobile-android` and `crates/indicatrix-mobile-core` against the
shared APIs, and made these changes:

- Exports share the live render's GPU probe (`BackendManager::sharing`,
  `render/export.rs`) instead of probing again.
- Fluorescence: `GeometrySceneSpec::build_with_customs` (web-core
  `scene/geometry.rs`) fills the scene's fluorescence from the traced material's
  physics recipe. The mobile render path carries that recipe (`SceneKey.customs`) to
  the local live trace, the local export and the remote session and job; the remote
  wire already sent `scene.fluorescence()`. `build` delegates with an empty slice, so
  the desktop and the web app are unchanged.
- The `wire` functions of the optimization, retarget, Deep Solve, variants and render
  screens are split to satisfy `too_many_lines`; every `#[allow]` is now
  `#[expect(..., reason)]` or removed.
- The vendored Slint Android backend reports `Scroll` (and hover) motion events as
  unhandled instead of `todo!()` (see `vendor/slint-android-activity-1.18.1/PATCH.md`).
- Root `Cargo.toml` typo (`--profile`) and the `vendor` exclusion; Cargo.lock
  reconciled as described above.
- Deep Solve host tests in `crates/indicatrix-mobile-core/src/deep_solve.rs`.
- Stale documentation corrected (`platform.rs`, `run.rs`, the Android README).

## Owner-run validation

Run the shared preservation/optimizer regressions before enabling release search.
Suggested entry points, to be run by the owner, are:

```powershell
cargo test -p indicatrix-cut-core meet_preservation
cargo test -p indicatrix-cut-core optimize
cargo test -p indicatrix-editor
cargo test -p indicatrix-web-core
cargo test -p indicatrix-mobile-core
cargo test -p indicatrix-net --features render
cargo test -p indicatrix-worker --features worker
cargo test -p indicatrix --features gpu renderer::gpu_backend
cargo check -p indicatrix-mobile-android --features desktop-preview
cargo check -p indicatrix-mobile-android --no-default-features --features desktop-preview
```

Use the Android README's NDK/Gradle workflow for actual APKs. Review the local
backend patch before updating Slint. Release search is on: `validated-design-search`
is a default feature since 2026-10-19, after the shared tests passed.

Device acceptance remains the plan's matrix: API 28/32, 33, 34/35 and 36; Pixel and
Samsung; phone/tablet; gesture and three-button Back; IME and popup-first handling;
cancelled and rapid predictive gestures; rotation and lifecycle. Exercise one-step
undo, stale candidates after edits/Undo/document replacement, signed angles,
relation followers, all concave tools/motions, geometry-aware CPU output, GPU
declines/device loss, concurrent export/search/analysis, thermal hysteresis and
system power saver.

Remote acceptance needs real coordinator and standalone-worker builds with
matching engine/source identities: revoked/expired credentials, library-only/no
capacity, unsupported DisplayOnly, zoning, cancellation, delayed/out-of-order
frames, disconnection during each transfer phase, memory limits, backgrounding
and exact analytic scene comparisons. HDR acceptance: render one HDR scene on the
device and on a worker and compare (the asset is bit-identical, so the CPU renders
should agree to rounding). Durable-job acceptance: submit, kill the app, reopen,
list and save the picture; restart the worker mid-job and see it re-queue; let a
job expire; use a second enrolled phone and confirm it cannot see the first
phone's jobs.
