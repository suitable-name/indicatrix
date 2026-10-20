**Android feature expansion — implementation plan**

Prepared 2026-10-19 from the current repository and Android platform documentation. Planning only: no application implementation, dependency updates, builds, tests or Clippy are part of this task. Acceptance checks below are future implementation/release work, run by the owner unless separately authorized.

**1. Intended result and scope**

Deliver an Android editor with local CPU and supported-device GPU tracing, authenticated remote rendering, reliable Back navigation, Optimize, material Retarget, Deep Solve, a history browser and saved variants, and concave-tier editing. Reuse the existing numerical engines, document formats and edit semantics. The two AI crates are out of scope.

Keep CPU rendering available offline. Preserve thermal and power protections across every local backend; GPU acceleration does not remove heat or battery limits. Remote rendering is an explicit user choice after server setup, not an automatic upload when a phone becomes hot.

Use the following product defaults unless changed during implementation review:

- Rendering offers **Automatic on device**, **Device CPU**, **Device GPU**, and **Remote**. Automatic chooses between supported local backends only. Show the backend actually running and a useful fallback explanation.
- Live rendering prioritizes interaction; an explicit final render uses the requested quality. A governor may slow or pause a final render, but must not silently lower its requested sample count and label it complete.
- Long editor operations run locally in the first release. Remote rendering is not a promise that the server can run Optimize, Retarget or Deep Solve; those would require separate authenticated compute protocols.
- Phone workflows use dedicated screens or full-height sheets. Tablets use list/detail panes where useful. Keep ordinary editing and Undo/Redo immediately accessible.
- Results are previews until explicitly applied. Applying a candidate or opening a variant produces one reversible edit and uses the shared editor's validation and dependency handling.
- Executable history covers the current editing session, matching the shared history engine. Saved variants survive restarts. Previous-session description logs remain readable, not falsely presented as replayable undo entries.

**2. Current implementation and reuse map**

| Area | Current code and behavior | Implication |
|---|---|---|
| Application state | [Mobile AppState](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/app_state.rs) owns an `EditorSession`, document, solve, render, analysis, HDR and library state. | Extend this UI-independent layer; do not create a second design model in Kotlin or Slint. |
| Android host | [MainActivity](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-mobile-android/android/app/src/main/java/com/indicatrixsuite/mobile/MainActivity.kt) extends `NativeActivity`; [Android entry](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-mobile-android/src/android_entry.rs) forwards lifecycle events. | NativeActivity is not ComponentActivity. An AndroidX Back dispatcher cannot simply be assumed available. |
| Back | [Manifest](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-mobile-android/android/app/src/main/AndroidManifest.xml) opts out of predictive Back. [App shell](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-mobile-android/ui/app.slint) handles Back/Escape through several models. Pinned Slint 1.18.1 also registers its own Back callback on API 34+. | Changing the manifest flag alone does not establish a single handler or correct root-navigation behavior. |
| Local render | [Render state](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/render.rs), [CPU pool](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/render/pool.rs), and [governor](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/render/governor.rs) provide progressive CPU tracing and lifecycle throttling. | Retain the working CPU path behind a backend abstraction. |
| Resource gaps | [ExportJob](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/render/export.rs) has a separate CPU pool. Battery saver currently comes from the app setting; the platform interface exposes thermal headroom but not Android power-save state. | Govern exports and searches too; combine system and application power policies. |
| Remote | [Settings](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/settings/model.rs) already contain address, enrollment, transfer mode and cadence. INTERNET permission and an optional Vulkan declaration already exist. Remote UI is a placeholder. | Complete existing configuration; avoid inventing a parallel settings scheme. |
| Editor operations | [Shared editor](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-editor/src/lib.rs) and cut-core already contain optimization, Retarget, history and concave edit semantics. | Mobile primarily needs orchestration, state and usable interfaces. |
| History | [Mobile edit plumbing](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/design_state/edit.rs) already supports Undo/Redo. [History view](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-editor/src/session/history_view.rs) supplies timeline snapshots and mapped jumps. | Add browsing and comparison instead of replacing undo. |
| Concave | [Mobile tier rows](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-mobile-core/src/design_state/rows.rs) recognize concave tiers but intentionally withhold editing. | Replace the explicit restriction with a distinct typed editing route. |

Shared changes should remain UI-independent. Extract reusable helpers from the desktop where necessary, retain desktop consumers, and keep each changed Rust file below 800 lines. Proposed module names below describe new responsibilities, not existing APIs.

**3. WP0 — operation identity, scheduling and result contracts**

Build this foundation before connecting long operations to buttons. Extend mobile-core with small `operations`, `compute_policy` and feature-state modules rather than expanding AppState into a large implementation file.

1. Define operation identity with document identity, session generation, run ID and an immutable input fingerprint. Include the inputs relevant to the operation: authoring state, material, constraints, selected tiers, scoring/lighting configuration, printed targets and rendering settings. A generation counter alone must not make a result from another document acceptable.
2. Model `Preparing`, `Running`, `Cancelling`, `Paused`, `Completed`, `Failed` and `Stale` explicitly. Only advertise pause/resume where the engine has a real continuation mechanism. A cancelled restart is not a resumed search.
3. Reuse `BackgroundQueue`, generation checks and cancellation primitives. Put long searches on a separate queue from ordinary solve and UI work. Limit total parallelism across searches, tracing, metrics, exports and thumbnails; do not let each independently consume all CPU cores.
4. Use immutable worker inputs and typed progress/result messages. Deliver completion to the UI thread without blocking JNI or the UI event loop. Check operation identity before publishing both progress and completion.
5. Centralize result invalidation through the existing edit/solve refresh path. Editing, Undo/Redo, history jumps, variant loads, document replacement and relevant material changes invalidate dependent results. Closing a screen does not silently apply anything.
6. Preserve exact accepted designs, provenance and preservation baselines in candidate state. Never reconstruct an Apply operation from rounded display rows. Keep a distinction between geometry validity and scoring freshness.
7. Define lifecycle policy: live rendering pauses off-screen; ordinary local long computations cancel or cooperatively pause according to real engine support. Save document recovery state and explicitly marked partial results where supported. After process death, show interrupted operations as interrupted; never invent a completed result.
8. Establish shared preview/compare support using `DesignSnapshot` and `snapshot::compare_rows`, including concave changes. The live document remains untouched while browsing candidates or historical states.

Acceptance: late messages cannot overwrite a newer run or document; no Apply succeeds with stale identity; cancellation stops useful work within bounded engine checkpoints; all edits use existing dirty-solve, autosave and history conventions. Test replacement and cancellation races, not just the happy path.

**4. WP1 — replace the Back workaround with one navigation owner**

Android 16 changes Back delivery for apps targeting API 36; opting out is a compatibility measure, not the intended long-term implementation. Use the supported callback APIs and keep exactly one consuming path. [Android 16 behavior changes](https://developer.android.com/about/versions/16/behavior-changes-16), [predictive Back migration](https://developer.android.com/guide/navigation/custom-back/predictive-back-gesture).

Implementation sequence:

1. Document every existing Back route and its state dependencies. Consolidate the shell, document, render, tiers, library and new operation screens behind a shared navigation contract: a side-effect-free `can_handle_back`/target query plus a single commit action.
2. Preserve platform ownership of system dialogs and the IME. Within the app, consume only the topmost relevant state: confirmation/info dialog, editor overlay or draft, nested workflow, expanded sheet, current subpage, destination navigation, then system root behavior. Specify dirty-form cancel/discard behavior and operation-screen behavior separately from document-save prompts.
3. Choose an app-owned Android callback bridge. First resolve Slint's competing registration: use a supported upstream switch if available in the selected version; otherwise make a narrowly scoped, pinned backend patch that allows application-owned Back and contributes that change upstream. Do not install a second callback while Slint's registration remains active, and do not edit a developer's Cargo registry in place.
4. Keep the existing single legacy key route on API 28–32. Use `OnBackInvokedCallback` on API 33+. On API 34+, use `OnBackAnimationCallback` for in-app progress/cancel/commit handling. Cancelled gestures must have no navigation or edit effects. [Android animation callback API](https://developer.android.com/reference/android/window/OnBackAnimationCallback).
5. Mirror whether the app can consume Back to the Android UI thread. Register a consuming callback only when necessary; at the root, allow Android to perform normal task/navigation behavior. Do not replace root Back unconditionally with `finish()`.
6. Send committed app Back actions asynchronously to the Slint event loop. Avoid synchronous cross-thread JNI waits. Use navigation state/version checks and one event owner, not a time-based debounce that drops legitimate consecutive gestures.
7. Route the app toolbar Back and desktop-preview Escape through the same navigation logic. Change the manifest opt-in only together with the callback ownership fix. Remove the README instruction telling users to flip a flag.

Acceptance: exactly one action per committed gesture or hardware Back; no action on gesture cancellation; keyboard closes without also exiting a form; dismissing a sheet does not also switch tabs; root behavior is platform-controlled. Validate gesture and three-button navigation on API 28/32, 33, 34/35 and 36, with Pixel and Samsung devices, phone/tablet layouts, text editing, rotation, and rapid consecutive actions.

**5. WP2 — rendering backend contract and unified device policy**

Introduce a mobile render controller above CPU, GPU and remote implementations. Preserve existing scene preparation and display conversion; a backend changes execution, not the physics or document interpretation.

- Define immutable render requests, trace identity, display identity, backend capabilities, progress, cancellation and output types. Distinguish an accumulated linear-radiance result from a display image. Exposure/denoise changes may reuse data only when the output actually supports it.
- Carry generation/run identity on every chunk and frame. Sample ranges must be non-overlapping and counted once. On backend switches, either transfer compatible accumulation plus an explicit sample ledger or restart visibly; never add two complete images as if they were independent sample chunks.
- Make CPU live rendering and final export use the same scheduling policy. Reserve UI capacity, bound queues and memory, and release work on shutdown without waiting indefinitely for a whole export.
- Move expensive scene preparation and export probing off the UI thread. Android export currently constructs `OwnedScene` synchronously; include GPU initialization, TLS handshakes and HDR conversion in worker-owned preparation, with the same cancellation and stale-result checks as tracing.
- Extend Platform with cached power/resource observations: system power-save state, thermal status/headroom where supported, foreground state and memory pressure. Combine Android battery saver with the app's own setting. Treat unavailable readings as unknown, not a guarantee of a cool device.
- Keep thermal headroom polling at an appropriate cadence; the existing ten-second timer is a useful starting point. Add event-driven status/power updates and hysteresis to prevent rapid pause/resume or backend switching. Android documents headroom polling limits and device variability. [Thermal API guidance](https://developer.android.com/games/optimize/adpf/thermal).
- Prefer reduced concurrency, dispatch duty cycle and interactive resolution over changing the requested final image. Display `Cooling down`, `Battery saver`, or `Paused in background` with actual progress. Preserve completed samples where compatible.
- Govern Optimize, Retarget searches, Deep Solve, render exports and expensive analysis. They are not all short enough to remain exempt from the policy.

Acceptance: no unrestricted export/search bypass, no duplicate or stale samples, no thermal oscillation, and no silent reduction of final quality. Capture time to first preview, sustained samples/second, cancellation latency, peak memory and thermal behavior on representative phones before choosing defaults. Performance thresholds are release targets to set from measurements, not assumptions about every GPU-equipped phone.

**6. WP3 — authenticated remote rendering**

Reuse [indicatrix-net](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-net/README.md), [dispatch](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-dispatch/README.md), and [worker/coordinator services](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-worker/README.md). Do not copy desktop Slint callbacks into the phone app or create an unrelated HTTP renderer.

Connection and capabilities:

1. Finish the existing Remote settings page: address/name, enrollment or credential import, connection test, certificate identity, reachable/unauthorized/incompatible/library-only/render-ready status, transfer mode and cadence. Add asynchronous mobile connection management behind a UI-free service boundary.
2. Reuse the existing TLS and protocol compatibility rules, including engine/source identity. The inspected protocol constant is 24; use the code constant, not older comments mentioning v14. A successful socket connection alone is not proof of render compatibility.
3. Extract reusable socket ownership, timeouts and reconnect handling from desktop integration. `run_client_session` is a blocking protocol helper, not a complete connection manager; give it a dedicated worker and cancellable I/O ownership. One background worker owns both directions of the complete `rustls` connection, interleaving bounded reads with cancellation and dispatch commands; do not split TLS state by cloning its underlying socket.
4. Store imported credentials in app-private storage. Encrypt exported private-key material at rest with an Android Keystore-backed wrapping key, or explicitly implement a native signing bridge if choosing non-exportable TLS keys. Do not claim that writing a PEM into private storage makes it hardware-backed. Exclude secrets from settings, backups, logs and shared exports.
5. Establish actual scene/output support: render capacity, maximum pixels, minimum cadence, HDR/assets, concave geometry, material features, and DisplayOnly versus FullData. DisplayFrame/FinalImage support already exists at coordinators, but standalone workers currently refuse DisplayOnly. Reuse existing advertised capabilities and endpoint distinctions first; these output messages do not themselves require a new protocol. Where existing metadata cannot establish support, add versioned capability reporting and explicit unsupported-mode handling; older endpoints must not be assumed to support an unadvertised mode.

Rendering and assets:

6. Start with remote-only DisplayOnly rendering as the bandwidth-friendly choice when the endpoint supports it. Keep FullData explicit for local re-tonemapping or later hybrid use. A display image cannot be merged into CPU/GPU radiance accumulation. Offer a clear compatible alternative when connecting to a standalone worker.
7. Send the fully resolved render scene: material, camera, lighting, planes, tools and any derived rough/preform geometry that affects the local image. The existing wire scene carries resolved geometry, not a separate rough/preform authoring object. Never remove unsupported scene features to make a request succeed; either choose an exact compatible backend or refuse with a reason.
8. Resolve the HDR representation mismatch before claiming HDR parity. Mobile currently persists a downscaled private `IXENVMAP` representation, whereas worker asset loading expects hashed HDR bytes. Produce a protocol-supported representation of the same effective lighting map, with a content hash and bounded upload/cache behavior. Do not upload the private format as though it were `.hdr`, or silently substitute an original-resolution/different environment. Version an asset extension if exact existing-format interchange is insufficient.
9. Handle reconnect, cancellation, endpoint loss, metered-network changes and activity recreation. Discard old-scene frames after any edit or camera restart. A switch to local tracing must be visible and start with valid accumulation rules.
10. Add explicit final render/export with resolution and quality checks against endpoint and phone memory limits. For jobs intended to survive phone disconnection, implement durable server job identity, ownership, status/result retrieval, cancellation and retention around the existing rendering infrastructure. The current request/session APIs do not provide durable retrieval: this requires server persistence and an authenticated protocol extension. Deliver it as a separate remote sub-milestone. Until then, document interruption/restart behavior; do not present an interactive socket session as a durable remote job.

The manifest already grants INTERNET. Keep target SDK 36 unchanged for this work unless separately required. Android's local-network permission is opt-in at API 36 and required when targeting API 37; gate a later target upgrade deliberately rather than requesting the new permission unconditionally today. [Android local-network permission](https://developer.android.com/privacy-and-security/local-network-permission).

Acceptance: coordinator and standalone-worker cases; invalid/expired credentials; incompatible protocol/build; no render capacity; denied network access; cancellation and delayed frames; network loss during upload/render/download; large images; exact scene/HDR identity. No remote scene upload occurs before the user selects and configures remote rendering.

**7. WP4 — on-device GPU tracing**

Use the existing `indicatrix::renderer::gpu_backend` abstractions (`GpuBackend`, `GpuSceneRef`, `GpuAccumulate`, scene eligibility checks), with a mobile adapter and bounded resource policy. UI acceleration by Skia is not GPU path tracing.

1. Add an Android GPU feature/dependency path with appropriate native wgpu backends. Keep Vulkan optional in packaging; the current optional manifest declaration is not a runtime capability test. Retain CPU-only operation on older or incompatible devices. [Android Vulkan capability guidance](https://developer.android.com/games/develop/vulkan/native-engine-support).
2. Probe adapter features, buffer/binding limits, workgroup limits, allocation budgets and shader validation off the UI thread. Record a structured reason for every decline. Runtime support matters more than OS version or the presence of Vulkan.
3. Account for the current shader requirements: the megakernel requires 11 storage buffers, while the baseline limit can be eight; the wavefront path requires more. First integrate capable hardware. If representative target phones fail these requirements, add a compact mobile resource layout/kernel variant before claiming broad Android GPU availability. Share physics/shader logic and pack resources rather than implementing a different approximation.
4. Extend the reusable backend interface to accept bounded work/memory/duty settings. A lower-level frame renderer has chunk controls, but the public backend currently lacks the mobile scheduling control needed here. Use small measurable dispatch batches, limited in-flight work and bounded readback frequency.
5. Own compute resources separately from the Slint renderer unless an explicitly supported interop path is implemented. Start with the existing image handoff; optimize copies only after profiling demonstrates need.
6. Preserve `Done`, `Declined` and `Cancelled` distinctions and transactional accumulation. On initialization failure or device loss, release resources and use the selected policy's visible fallback. Keep bounded retry/cooldown behavior; never repeatedly recreate a failing device in the UI loop.
7. Route by real feature support, including anisotropy, zoned material, HDR and concave tools. The current GPU route rejects all concave-tool scenes, fluorescence/UV lighting and unsupported zoning configurations. Use exact CPU fallback for these cases; implementing concave GPU tracing is outside this milestone. Missing GPU support must preserve the exact CPU scene, not flatten or simplify it.
8. Apply the common governor to both preview and exports. Cancel/pause at dispatch boundaries and never block UI shutdown on an unbounded GPU wait. Respect memory pressure and keep the last valid displayed frame while restarting.

Acceptance: eligible Adreno and Mali devices, at least one low-memory device and one deliberately unsupported configuration; sustained operation, device loss, screen/background transitions and CPU fallback. Compare CPU/GPU results using the repository's established physical/numerical tolerances and controlled sample seeds; do not demand bit-identical floating-point images or accept visually plausible but physically different results.

**8. WP5 — history, snapshots and durable variants**

History UI:

- Expose `history_entries`, current position, Undo/Redo labels and coalesced edits. Use `history_snapshot` plus `HistorySnapshot::design_at` for asynchronous previews, and `jump_to_mapped` for committed navigation.
- Preview a position without moving the document. A committed jump must map or clear selection correctly, refresh the solve/render state and preserve the redo branch until a new edit occurs.
- Show actual operation labels, including candidate Apply and variant opening. Separate the active session timeline from any saved textual description log.
- Add snapshot/current comparison with tier, constraint, mast, material, rough and concave differences. On a phone, provide a readable single-column comparison; on a tablet, support side-by-side details.

Variants:

- Reuse [vault variant records](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-vault/src/model/design_variant.rs) and worker-owned library access. Add mobile wrapper operations for list, save, load, rename, notes and delete. Avoid a second storage engine or unnecessary schema duplication.
- Extract the pure replacement-edit logic from [desktop variant Apply](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-cut/src/gui/editor/variants/apply.rs) into the shared editor. Preserve its preflight-on-a-copy behavior and complete replacement, including authoring identities, constraints, relations, preform and concave tiers.
- Support Save Current, Save Historical Position and Save Candidate as Variant, with names, notes and parent/design identity. Capture the chosen state immutably before asynchronous persistence; do not later save whichever document happens to be current.
- Opening a variant is one atomic reversible action. Include dirty-document protection, stale/deleted-record handling and predictable selection invalidation.
- Define persistence for a new/unsaved document's variant family, including first library insertion and subsequent save/export. Autosave recovery and library variants must not accidentally create duplicate design families.

Acceptance: edit/jump/redo/new-branch sequences, empty/no-op history, async preview races, opening all supported design types as variants, one-step undo, restart persistence, parent identity, deletes during a pending load and save failures. Native save/reopen must retain the complete design represented by a variant.

**9. WP6 — Deep Solve**

Extract UI-free availability, result/report and mast-difference helpers from [desktop Deep Solve](C:/Users/ngussek/Dev/Rust/diagram_loader/apps/indicatrix-cut/src/gui/editor/deep_solve.rs) and its callbacks into the shared editor. Keep the numerical engine shared.

1. Use `solve_meet_points_verified_with` and `SolveControl` on the operation worker, with original document identity and printed targets captured at launch.
2. Wire existing `DocumentState.printed_proportions`. Native documents already restore these; imported formats without targets need explicit entry/import provenance through the existing printed-proportions parser. Never create verification targets by measuring the candidate that Deep Solve is supposed to verify.
3. Show availability reasons: missing printed proportions, no suitable unpinned/adopted tier, invalid solve inputs or unsupported geometry. Provide the appropriate target-entry/adoption route where available.
4. Present elapsed time and real engine progress. Use an indeterminate indicator if no honest percentage exists. Cancel stops the worker cooperatively and does not expose a completed or pinnable result.
5. Show agreement with printed proportions, per-tier mast differences and the engine's verdict. Running the diagnostic never edits constraints.
6. Make verified-mast pinning an explicit action through `EditorSession::pin_tier_mast`, with freshness checks and undo. Disallow pinning from an interrupted or stale report.

Acceptance: available/unavailable designs, typed target validation, catalogue/native metadata roundtrip, cancellation, document replacement mid-run, explicit pin/undo, and the same results as the shared desktop path for identical inputs.

**10. WP7 — material Retarget**

Add a Retarget workflow under Design actions. Selecting a new material normally remains ordinary material editing; Retarget is the explicit operation that adjusts the cutting design for that material.

1. Use `retarget::plan::build_plan` for target material/index, crown behavior, angle changes, critical-angle margins and notes. Keep horizontal facets horizontal and relation-driven angles under the shared relation engine.
2. Run `retarget::check::run_check` off the UI thread, with a generation-scoped cached analysis of the original stone. Surface preservation and validity diagnostics clearly rather than presenting any completed calculation as usable.
3. Offer Shift first, then the existing Retarget search via `retarget::search::run_search`: angle range, effort, keep-look/shape settings, supported girdle allowance, candidate count and cancellation. Share candidate presentation with Optimize.
4. Compare original and candidate geometry, relevant optical metrics, material and angle/mast changes. Preview the complete checked authored state.
5. Apply the validated geometry and target material atomically through shared session/edit APIs. For Shift, use `RetargetCheck::validated_edit` and its freshness/allowance checks; for search candidates use the certified complete candidate path. Apply is not reconstruction from the initial preferred-angle plan.
6. Offer Save as Variant without modifying the original. Re-check document identity before Apply or save operations that depend on it.

Release gate: the currently reported sapphire Retarget regression must pass in the shared engine before mobile Retarget is enabled for release. Also require preservation after Apply, undo/redo and native save/reopen for representative pinned and meet-derived designs. Do not add Android-only tolerances or bypass preservation to make the feature appear functional.

**11. WP8 — Optimize and candidate selection**

Use `optimize_view::plan::{build_run_plan, prepare_anchor_hinges, optimize_availability}`, the current `optimize_design_with`/`SearchHooks` engine, and shared candidate preview/Apply helpers. Prefer these current rich APIs over older web transport parameter structs that omit desktop options.

- Provide a simple entry view with objective preset, scope and effort; an Advanced section exposes custom weights, angle bounds, selected tiers, anchored-tier variation, shape/girdle restrictions, starts, seed, polish and candidate count as supported by the shared configuration.
- Explain unavailable or locked tiers and show why a run cannot start. Respect relation followers, fixed dimensional targets and preservation support.
- Capture all inputs once and start on the governed operation worker. Bound `max_lanes` and total resource use. Coalesce progress updates; keep navigation and Cancel responsive.
- Present ranked alternatives with metric changes and geometry/constraint differences. Use `try_build_candidate_preview_design` and `apply_candidate` so the accepted angles, depths, relations and provenance remain intact.
- Preserve existing cancellation semantics: real partial results may be available if the engine returns them; label them partial, validate them normally and never fabricate a completed search.
- Allow preview, comparison, Apply and Save as Variant. Changing a search control or the live design must not silently repurpose a result computed for older inputs.

Acceptance: multiple distinguishable candidates, deterministic seeds within the shared engine's supported determinism contract, bounds and scope enforcement, cancellation/partial results, stale rejection, exact preview-to-Apply reproduction, atomic undo, and intact original meet points after persistence. Resolve current shared preservation regressions before enabling mobile release. Concave editing does not imply that the planar optimizer can optimize concave designs; report unsupported combinations explicitly until the shared engine supports them.

**12. WP9 — concave-tier editing and end-to-end geometry**

Reuse [shared concave form parsing](C:/Users/ngussek/Dev/Rust/diagram_loader/crates/indicatrix-editor/src/loading/concave_form.rs), `concave_tier_form_fields`, and shared tier-save helpers. Do not map a concave row into the ordinary planar form.

1. Introduce typed flat/concave selection or consistently use the session's separate concave selection. Audit selection, deletion, history restoration and rendering so two different lists cannot share an ambiguous integer index.
2. Add creation and editing sheets for the supported tool codes CYL, CON, CIR, DSC and SPH, using the shared display names. Include name, signed angle, indices, instructions, tool azimuth, X/Y/Z, D/W, conditional tool angle and reciprocating/plunge motion.
3. Reuse arithmetic/index parsing and field-specific errors. Show meaningful units and required/optional fields. Keep tool parameters while switching supported form modes according to shared semantics; do not quietly discard authored values.
4. Commit through the shared concave save helper and `AddConcaveTier`, `ModifyConcaveTier`, `RemoveConcaveTier` edits. Enforce name clashes and dependent-state invalidation. Confirm destructive removal using the app's existing interaction conventions.
5. Add an explicit draft preview where practical without committing on each keystroke. Save commits one edit; cancel/Back leaves the live design unchanged. Connect the sheet to the unified Back contract.
6. Extend the mobile render scene pipeline, not just its capability checks. Its current shared `SceneSpec`/`OwnedScene` path carries no tool geometry, and `handle_trace_chunk` passes an empty tool list to geometry-aware CPU tracing. Carry resolved concave tools through immutable scene snapshots, render identity, CPU tracing and remote `SceneState.tools`. Extend the shared representation compatibly with existing web consumers, or introduce a shared geometry-aware scene type that both can use; avoid duplicating the physical tracer.
7. Verify actual geometry through Solid/Diagram/Render, cutting instructions, snapshots, variants, native serialization and supported ASC export. Keep documented approximation limits in metrics/solid visualization visible where relevant.
8. Require CPU/GPU/remote scene capability checks to retain tool geometry. Unsupported GPU scenes fall back to exact CPU when allowed; unsupported remote endpoints refuse explicitly. No backend silently renders only the flat hull.

Acceptance: roundtrip each tool/motion combination; signed zero and extreme valid angles; invalid dimensions/indices; duplicate names; cancel versus Save; add/edit/delete/undo/redo; history/variant selection restoration; native and supported ASC interoperability with the desktop.

**13. Delivery order and dependencies**

| Milestone | Deliverables | Dependencies / release condition |
|---|---|---|
| A | WP0 identity/lifecycle foundation and WP1 Back fix | First: removes navigation ambiguity for every new sheet and screen. |
| B | WP2 resource policy and backend contract | CPU behavior stays available; exports and long jobs are governed. |
| C | WP3 connection setup and interactive remote rendering | B; endpoint capabilities, authentication and exact asset interchange verified. |
| D | WP4 local GPU path and supported-device validation | B; can proceed alongside C. Reduced-binding shader work is required if the intended device set needs it. |
| E | WP5 history, comparison and variants | A; supplies safe alternative storage/comparison for optimizer workflows. |
| F | WP6 Deep Solve and initial WP7 Retarget Shift | A/E plus shared helper extraction; existing numerical regression gates must pass. |
| G | WP8 Optimize and WP7 Retarget search | Reuse F's operation/result presentation and E's comparisons/variants. |
| H | WP9 concave editor and backend capability coverage | A/E; UI can develop in parallel, with rendering integration completed against B–D. |
| I | Durable remote final jobs, recovery polish and release documentation | Explicit server job lifecycle and authentication/ownership verified; no fictitious background guarantees. |

Independent implementation lanes may work on Back, render backends, shared editor helpers and concave forms after agreeing on WP0 contracts. Keep ownership of shared AppState, settings and Slint model declarations coordinated to avoid conflicting interfaces.

Local background computation is not made reliable merely by moving it into WorkManager: long-running workers have platform constraints, including Android 16 job-quota considerations. Default local work to the documented foreground lifecycle; assess a dedicated, policy-compliant background feature separately if required. Server-owned durable work is the preferred way for a final render to continue without keeping the phone busy. [Android long-running worker guidance](https://developer.android.com/develop/background-work/background-tasks/persistent/how-to/long-running).

**14. Validation and definition of done**

Implementation should add targeted state-transition and interoperability coverage as each package lands. The owner runs builds/tests/Clippy; no test execution is implied by this plan.

| Validation layer | Required evidence |
|---|---|
| Shared core/editor | Existing desktop behavior retained; original meet networks preserved; preview/Apply identity; one-step undo; relation and dimensional-target handling; Deep Solve target provenance; variant replacement completeness. |
| Mobile core with mock Platform | Job replacement and cancellation races; autosave/recovery; thermal/power transitions; resource limits; remote connection and capability states; memory/error paths. |
| Render interoperability | Same effective scene/assets on CPU, eligible GPU and remote; established tolerance-based physical comparisons; correct accumulation/sample accounting; unsupported features never omitted. |
| Protocol/service | Coordinator and standalone-worker behavior; version/build mismatch; credentials; HDR/assets; late frames; reconnect/cancel; durable-job ownership/result retrieval where enabled. |
| Android devices | Minimum supported API and API 33–36 navigation paths; API 37 forward-compatibility where available without silently changing target SDK; Pixel/Samsung, Adreno/Mali, low-memory phone and tablet, unsupported GPU. |
| Lifecycle and UX | IME/Back interactions, cancelled predictive gestures, rapid navigation, pause/resume, rotation/multi-window, process death, external file intents, battery saver and thermal stress. All drafts and destructive actions have predictable handling. |
| Persistence and export | Native reopen, supported ASC export/import, saved variants, historical-state saves, target-material edits, concave tools, interrupted writes, storage-full failures. |

Record measured responsiveness, memory and sustained compute performance before release, not just initial cold-device speed. Update screenshots, Android README, user help and feature-availability messages only after the corresponding behavior passes validation. Replace the known Back workaround and placeholder remote/desktop-only messages with accurate behavior and remaining capability limits.

Completion means all requested workflows are usable from the Android UI, unsupported hardware/scene combinations are handled honestly, existing offline editing remains functional, and no new path bypasses the shared design-preservation or stale-result checks.
