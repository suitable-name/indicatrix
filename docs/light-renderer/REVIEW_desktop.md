# Lightweight renderer: desktop source review

Review scope is the new lightweight renderer and its integration points only. Existing unrelated behavior, the two AI crates and earlier optimizer/Android projects are outside scope. No build, test, Clippy, GPU probe or git command was run.

## Findings corrected

- **P1: An inherited Path-Traced pause froze the new Ray-Traced view. Fixed.** The new mode hides the Pause/Resume action, but local and remote scheduling consumed the raw pause flag. Both now use `RenderContext::effective_paused()`. Ray rendering proceeds while the user's Path pause setting remains available when switching back. Local/remote transition regressions were added but not run.
- **P2: Edit's new Ray-Traced image did not display renderer refusals. Fixed.** `solid_viewport.slint` now checks `ViewportModel.trace_refusal` for mode 4, dims the stale image, displays the refusal, and suppresses the generic empty-state message. This uses the existing renderer's UV/fluorescence refusal state without changing the physics or substituting a different engine.
- **P2: Repeated GPU preparation defeated the intended scene cache. Fixed.** Desktop live rendering now retains immutable owned GPU uploads beside CPU preparation and a shared plane arena, keyed by scene generation and dimensions. Failed or replaced-device snapshots are discarded. GPU guide generation also uses the cached plane arena instead of rebuilding it for every pixel; CPU scanlines reuse it across taps. Export frames reuse one frozen GPU upload across their local claims and retire a rejected upload for that frame. See the core and remote review records for the shared API and export details.

## Reviewed boundaries

- Engine selection from typed Edit/Live modes, callbacks, settings restore/reset, scene identity, dirty state, target taps and display generation.
- Local desktop CPU/GPU selection, per-generation preparation, absolute sample claims, no Ray denoise, refusal before local claims, and remote fixed-tap epochs.
- Independent still/video renderer choices; frozen engine propagation into output parameters, snapshots, jobs and execution; final-picture versus full-data remote output; local contribution and cancellation.
- Export preparation, absolute tap allocation through the existing shared cursor, whole-claim GPU fallback and no spectral calibration for Ray.
- Compare mode mapping, generation rejection, per-side refusal, fixed tap status and independent analytic metrics.
- Persistent versus draft thumbnail fingerprints, renderer changes and recounts, local/remote sample budgets, fluorescence checks before dispatch.
- Guide mode validation, visible tutorial targets, keyboard/palette mappings, UI approximation text, output controls and relevant documentation.
- Analytic tilt requests retain the default Path engine metadata; selecting a beauty renderer does not change those metrics requests.

The lead reviewed these fixes and their shared API consumers. No additional concrete desktop source blocker remains in the reviewed paths. CPU/GPU numerical agreement, toolkit compilation/layout, actual remote transfer behavior, image quality and performance remain unverified. Existing f32 sum regrouping precision limits remain documented rather than asserted away.
