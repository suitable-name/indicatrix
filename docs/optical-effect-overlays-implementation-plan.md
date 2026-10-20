# Optical-effect overlays in the render: implementation plan

Status: proposed implementation, based on the source inspected on 2026-10-18. This document does not implement the feature. No builds, tests, Clippy, or benchmarks were run. The verification work below is for the owner to execute.

## 1. Recommended design and release contract

Add a camera-aligned optical diagnostic pass alongside the existing render. It classifies rays using the same analytical definitions as the optical metrics, retains their image positions, and produces a separate transparent overlay. The overlay can be enabled independently for Windowing, Extinction, Brilliance, and Fire. Keep the original rendered image and its radiance accumulation untouched.

Use the existing CPU analytical tracer for the first implementation, regardless of whether the beauty image comes from the CPU, GPU, or remote workers. This avoids implementing four subtly different classifiers and avoids changing the remote protocol. Compute asynchronously, cancel obsolete work, and associate every result with the exact scene and camera it describes.

There is a necessary distinction in the product promise: the existing metrics are an optical diagnostic model, not a decomposition of the spectral renderer's radiance. The first release should explicitly say **Optical diagnostics**, with Fire described as **dispersion potential**. It can explain the geometric light paths behind an image, but cannot claim to identify every actually visible dark or coloured pixel for every advanced material effect. Section 4 defines this boundary and section 13 defines the additional work for exact transport attribution.

The feature is complete at this scope only when all four maps, current-view alignment, honest model explanations, stale-result protection, tilt-preview integration, and explicit export behaviour are delivered. A transparent coloured rectangle, a stretched copy of the metrics grid, or a facet-angle risk map is not an acceptable implementation.

### Included

- Desktop Live Render in path-traced mode, for the currently displayed design or library entry.
- Individual effect layers, combined light-path view, opacity, legend, and keyboard-accessible controls.
- Current camera pose, resolution/aspect, actual displayed geometry, resolved material, and active lighting.
- Concave geometry through the existing tool-aware intersector.
- Local CPU, GPU, hybrid, and remote beauty images using the same local diagnostic pass.
- Tilt Performance's hovered-pose preview and an explicit way to inspect that pose in a larger diagnostic view.
- Deliberately enabled diagnostic still and tilt-video exports, with captions identifying the diagnostic model.
- Per-pixel explanations and meaningful facet attribution.

### Deferred extensions

- Changing the optimizer's objective, recalibrating historical metrics, or recomputing the library's stored scores automatically.
- A spatial scintillation map; scintillation depends on motion and is not one of the four requested effects.
- GPU acceleration of the diagnostic pass until profiling demonstrates a need.
- Browser/mobile UI and remote diagnostic-map transport. Keep the core independent of desktop APIs so those clients can adopt it later.
- Full spectral-path attribution, discussed separately in section 13. Do not silently present the analytical pass as that implementation.

## 2. Existing implementation and integration points

Paths in this document are relative to the repository root. New filenames and API names below are proposals, not existing interfaces.

| Area | Current source | Consequence for this feature |
|---|---|---|
| Metric definitions and grid | `crates/indicatrix/src/color/metrics/{mod,evaluate,fan}.rs` | The current fan is an 18×18 disc with five angular aperture samples; it is not a render-sized perspective image. |
| Per-ray effect classification | `color/metrics/classify.rs` | Already separates Windowed, Extinct, Returned, and EntryBlocked, and computes qualifying Fire contributions. Extract and reuse this logic. |
| Analytical path tracing | `color/metrics/ray_trace.rs` | Uses Snell refraction, TIR, Fresnel weights, a fixed ten-bounce budget, and the tool-aware stone intersector. |
| Prepared geometry and lighting | `color/metrics/{prepared,lighting,visibility}.rs` | Reuse plane preparation, wavelength indices, environment sampling, and head-shadow classification. |
| Aggregation and cache | `color/metrics/{accumulate,cache}.rs` | Preserve existing counter ordering and floating-point reductions; new image statistics need their own sampling identity. |
| Camera | `crates/indicatrix/src/optics/raytracer/camera.rs` | `Camera::generate_ray` supplies the perspective rays used by the renderer, including image aspect and pixel jitter. |
| Render scene | `crates/indicatrix/src/renderer/frame_scene.rs` | `FrameScene` has camera, dimensions, planes, finish, material, bounce limit, and environment. Tools and fluorescence also travel alongside it in some render APIs. |
| First-hit guides | `renderer/guide_pass.rs` and `apps/indicatrix-cut/src/bridge/frame_cache/guide_pass.rs` | Existing depth/normal/facet IDs and path signatures are useful for matching and edge handling. They do not contain optical effect classifications. |
| Live render scheduling | `apps/indicatrix-cut/src/bridge/render_thread/{mod,context/frame,context/scene_identity,metrics_worker}.rs` | Existing asynchronous metrics and scene-generation patterns can be reused, but an image map needs stricter publication checks. |
| Display publication | `bridge/render_thread/{display_thread,frame_helpers,redraw_gate}.rs` | Carry scene identity to the displayed frame; a current result must not be composited over a previous scene's frame. |
| Remote display | `apps/indicatrix-cut/src/gui/remote/orchestrator/tick/update.rs` | This also writes `ViewportModel.render_image`; it must participate in the same frame-identity contract. |
| Live viewport UI | `ui/models/viewport.slint`, `ui/components/gem_viewport.slint`, `gem_viewport/toolbar_actions.slint` | The image uses `image-fit: contain`; overlay sizing and pointer mapping must share its actual image rectangle. |
| Existing Solid hints | `crates/indicatrix-solid/src/facet_map/overlay.rs` | Pavilion critical-angle hatching is a different, static risk indication. Keep its wording and behaviour distinct. |
| Tilt | `apps/indicatrix-cut/src/gui/tilt/{tilt_profile,tilt_hover_preview}.rs` | Reuse the exact axis/tilt-to-camera conversion. The existing hover preview has its own render and does not drive the live viewport camera. |
| Finished geometry | `apps/indicatrix-cut/src/gui/editor/finished_stone.rs` | Live views follow the Cut slider; curves and exports use the finished stone. An overlay must follow its own base image. |
| Export | `bridge/export_thread/scene_snapshot.rs`, `gui/render/render_export/`, `gui/tilt/video_export/` | Capture diagnostic options together with each export scene and apply overlays after beauty-image processing. |

### Consistency gaps that must be resolved first

The current paths do not all build the same environment:

- The live renderer applies backdrop, surface glare, and head-shadow settings to its selected analytic environment, or uses the loaded HDR map.
- `metrics_worker::Request` includes a preset/light pose and optional HDR map, but does not carry the live head-shadow setting in its request. Its analytic environment construction uses preset defaults.
- `tilt_profile::sweep_all_axes` constructs an environment from preset and light pose. Its current request/cache does not represent the same HDR/head-shadow inputs as the live render.
- The tilt hover preview separately constructs its scene and environment. Sharing a pose formula alone does not guarantee matching geometry, material, and illumination.

Resolve these differences through an immutable scene/lighting snapshot shared by the live diagnostic, HUD, active-session tilt sweep, and tilt preview. Preserve deliberately standardized library/optimizer scoring by making its lighting policy explicit. Do not accidentally make catalogue searches depend on the user's last viewport lighting.

## 3. User experience

Add an **Optical diagnostics** control beside the Live Render controls. It opens a compact panel with:

1. A master switch, Off by default.
2. Checkboxes for Windowing, Extinction, Brilliance, and Fire.
3. A single opacity slider, initially 45%, with a reset action.
4. A mode choice: Overlay on render or Diagnostic map. In map mode, the stone's diagnostic colours remain readable against a neutral background.
5. A persistent legend showing enabled effects, units, and whether the result is Draft, Refining, Ready, or Unavailable.
6. A short model description: “Analytical light paths at this viewing angle.” Fire's legend says “Dispersion potential.” Additional limitations appear when the selected scene uses effects outside the analytical model.

Single-layer views are the clearest default interaction. Multiple base layers may be selected; the combined view uses the three base-class fractions to mix their colours. Fire is a separate stipple/contour treatment over returning regions, avoiding an arbitrary four-colour blend. Supply a high-contrast palette and distinguishable patterns so colour alone is not required. Final palette selection should be checked in light and dark themes and with colour-vision simulations; do not reuse ASET's colour semantics.

The legend reports **visible-area diagnostic shares** when it reports percentages. Leave the standard metrics HUD intact and label its sampling differently. A tooltip explains why the two can differ. Never quietly overwrite the existing brilliance/windowing/extinction scores with image-area values.

Hovering a diagnostic pixel displays the relevant classification/fractions, Fire contribution if available, and the visible surface's tier/facet label. Where attribution is reliable, also identify the leakage exit facet. Do not call the frontmost crown facet “the leaking facet” merely because the leak is seen through it. No automatic selection or tier editing is required for the first release.

When orbiting, resizing, changing a material, or editing the stone, hide any incompatible overlay immediately and show Updating. A low-resolution preview may appear as soon as a matching result exists. Never keep an old coloured map attached to a moving stone. While paused, diagnostic settings can recompose a cached matching map; an explicit diagnostic refresh can run without restarting beauty accumulation. A scene change while paused invalidates both the association and the displayed diagnostic readiness.

Switching to Solid view suspends diagnostics and explains that its existing hatching shows critical-angle risk. Switching back restores the chosen settings, but only displays a matching result. Material-resolution refusal, an unsolved design, or missing geometry produces an unavailable state, not a misleading zero map.

## 4. Scientific semantics and limitations

### Windowing

Reuse the current diagnostic definition: a traced ray that refracts out at an exit with `n_out.y < -0.05` is windowed. This can happen after internal reflections; the UI must not equate it solely with a zero-bounce straight-through ray. Preserve the predicate for metric compatibility. Carry the exit facet and bounce count when possible so a tooltip can distinguish direct leakage from leakage after reflection.

### Brilliance

Reuse Returned: the d-line path exits upward, passes the existing illumination threshold, and is outside the model's observer-shadow cone. This is a light-return classification, not the display pixel's brightness in nits, an energy percentage, or a guarantee of white appearance in a coloured material. Existing base percentages count rays rather than their radiance weights.

### Extinction

Reuse Extinct for diagnostic compatibility, but preserve a reason: observer shadow, insufficient illumination, side exit, no further intersection, or bounce-budget exhaustion. The current `RayFate::Absorbed` name also covers trapping/termination; the analytical trace does not implement the renderer's full absorption/scattering physics. Do not describe every such result as physically absorbed light.

Budget exhaustion is especially important: expose it as an estimate with a reason, not a proven permanently dark path. Keep the legacy metrics' ten-bounce policy initially. A different diagnostic budget needs a different model version and must not silently alter optimizer or saved-score semantics.

### Fire

Reuse the current per-returned-ray F/C companion tracing: 486.1 nm and 656.3 nm, with the d-line at 589.3 nm. Preserve the gate requiring the F and C exits to have the same facet and bounce count, and preserve the transmittance-product/exit-cosine weighting. Store the unfloored local contribution `angle_deg × weight`; do not apply the aggregate `fire_index` floor of 0.1 to pixels, since that would paint Fire where the raw contribution is zero.

The existing display multiplier of 275 is a scale, not a percentage calibration. Either expose raw weighted angular separation with clear units, or use that same documented scale for local contributions. Define a fixed, versioned colour-transfer range after inspecting representative scenes. Never normalize every image to its own maximum: that would make a weakly dispersive stone look as fiery as a strongly dispersive one and cause flicker during tilt.

Fire can coexist with Brilliance. It is not a fourth mutually exclusive ray fate and must not be included in a “sums to 100%” pie chart. Distinguish zero contribution, uncomputed Fire, and rejected companion-path pairs in the data model.

Visible fire means dispersed coloured light in the observer's image; GIA's research separates brightness, fire, and scintillation on that basis. The current angular-separation index is an estimate of dispersion potential, not a direct measurement of every rendered coloured flash. That is why the legend needs the qualifier rather than silently claiming spectral-image equivalence. [GIA: evaluating light performance](https://www.gia.edu/gia-news-research/gia-researchers-evaluate-light-performance-diamond-cut0).

### Material and lighting coverage

The analytical tracer principally reads scalar dispersion indices, geometry, and its illumination model. It does not reproduce all of the spectral renderer's polarization/birefringence, body-colour absorption, zoning, fluorescence, inclusions/scattering, frosted surfaces, edge rounding, or surface glare. Pass the full resolved scene into capability evaluation so the UI knows which limitations apply, even where those fields do not alter the analytical calculation.

For supported geometric diagnostics on those scenes, show a persistent concise notice such as “Geometric estimate; body absorption is not included.” Where an effect would be materially misleading, disable that effect with a specific reason. Determine these capability rules before release with representative scenes; never fall back to Diamond, omit concave tools, or remove an advanced material property to make an overlay appear supported.

ASET is already a diagnostic lighting view; `metrics::lighting` scores that preset using ISO Hemisphere semantics. Initially disable this additional overlay while ASET is selected, with an explanation, rather than combining two incompatible false-colour legends. Revisit only with an explicitly designed combined view.

## 5. Core API and data model

Split ray generation from classification. The old grid path should build exactly the same rays as today and pass them into a shared classifier. The new image path supplies perspective rays from `Camera::generate_ray` instead. Both use the same prepared geometry, wavelength indices, and classification policy.

Suggested new modules:

- `crates/indicatrix/src/color/metrics/sample.rs`: shared ray-level outcome and optional path diagnostics.
- `color/metrics/image.rs`: camera-image sampling and aggregation.
- `color/metrics/image_types.rs`: request/result/options/coverage structures.
- `crates/indicatrix/src/renderer/optical_overlay.rs`: pure palette, opacity, and compositing functions.

Keep helpers private or crate-visible where possible. Export a small caller-facing image-evaluation API from `color::metrics`; avoid exposing all of `PreparedEval` and its implementation details.

Proposed request fields:

- Exact camera basis/origin/FOV and image dimensions.
- `StoneGeometry` including planes and tools; corresponding stable facet mapping generation.
- Resolved `GemMaterial`, lighting/environment snapshot, finish/effect capability metadata.
- Model version, quality tier, deterministic sampling sequence, requested channels, and cancellation token.

Proposed result fields:

- Scene/request identity, dimensions, computed-channel mask, quality level, status, and limitations.
- Coverage/hit count and base class counts per pixel.
- Raw Fire sum plus validity/qualification counts, only when requested.
- First-hit facet IDs; optional leakage-exit/reason data, or a cancellable single-pixel retrace for detailed hover information.
- Aggregate diagnostic counts and timings derived from the same samples as the image.

Represent miss, EntryBlocked, invalid numerical results, cancellation, and not-yet-computed data explicitly. Do not encode those states as Extinction or as a valid zero intensity. Validate finite inputs and checked image-size arithmetic before allocation.

For each pixel, if `S` rays were sampled and `H` hit the stone:

- Coverage is `H / S`.
- Each base fraction is its count divided by `H`, with zero-hit pixels transparent.
- Fire intensity is the raw qualifying sum divided by `H`, rather than divided by only the successful Fire samples.
- Base fractions plus the separately represented entry-blocked/unclassified share sum to one for valid hit samples. Do not normalize away unclassified observations.

At borders, multiply overlay alpha by stone coverage. Use deterministic spatial subpixel samples; do not reuse the metrics grid's broad angular aperture offsets as pixel antialiasing. The existing five-ray eye-cone sampling and the new perspective pixel sampling serve different purposes.

First-hit facet IDs must use the same planes/tools and generation as the displayed image. Concave IDs can lie above the flat-plane count; do not use an unqualified plane-vector lookup for them. A pixel with several subpixel facets needs coverage-aware hover wording or a centre-ray identification, not an unsupported assertion that all its samples hit one facet.

## 6. Camera alignment and aggregation correctness

The old metric fan covers a girdle-scaled disc and uses a camera basis without the render's per-pixel perspective origins/directions. Its documented coverage is particularly limited for elongated outlines. Retain it for existing metric compatibility; do not turn that sparse grid into a full-screen overlay by interpolation.

The diagnostic image pass covers the actual render rectangle and traces its full silhouette, including elongated and asymmetric stones. Use the render camera's pixel convention, axis handedness, FOV, pole handling, aspect ratio, and origin. A low-resolution map must have the same aspect and framing as its base image.

`image-fit: contain` introduces letterboxing. Put the beauty image and overlay inside one shared image rectangle or compute the same contain transform for both. Hover conversion must reject the letterbox area. Verify at non-square windows, high DPI, portrait images, and resized panels.

Image-area averages are not expected to equal the standard fan-based metric percentages. Compare classifier results on **identical rays** for correctness, and expose sampling-domain differences to users. For the image's own statistics, derive the denominator from its hit samples, not the rectangular background area or a mean of per-pixel fractions with unequal coverage.

Do not denoise categorical masks with the radiance denoiser. A coarse-map upscale may use nearest-neighbour with a Draft indication, or an edge-aware reconstruction guarded by geometry/path information. A first-hit facet ID alone is insufficient to preserve internal optical boundaries: different internal paths can occur within one crown facet. Avoid presenting a blurred leak boundary as a precise cut boundary.

## 7. Scene identity, lifecycle, and concurrency

Introduce a dedicated asynchronous diagnostic worker, using the existing metrics worker's scheduling patterns but its own bounded work queue and cancellation. Do not run a full image pass on the UI thread or block normal metrics on a high-resolution Fire map.

Use three separate identities:

1. **Optical input key:** geometry/tools, resolved dispersion/material interpretation, environment contents/version and relevant lighting settings, diagnostic model version, camera, image dimensions, sampling settings, and requested computed channels.
2. **Displayed-frame identity:** the immutable scene/camera/dimensions that actually produced the beauty frame. Propagate it through local, remote, hybrid, cached, and resized-preview publication paths.
3. **Presentation settings:** selected layers, palette, opacity, pattern and legend. These should recompose cached results without retracing or resetting beauty accumulation.

Use immutable `Arc` inputs and content/version identities, not just material names, design names, or a pointer to a mutable object. HDR reloads at the same path must invalidate correctly. Derive keys from captured inputs rather than relying on every UI callback to remember to set a dirty flag.

Reuse existing `SceneIdentity` where it helps reject stale frame delivery, but do not simply add the overlay switches to the render-scene key. They must not launch a new remote beauty epoch or reset path-tracing samples. The existing render key also includes fields such as exposure and target sample count which need not cause an analytical retrace; separate data reuse from permission to display over a particular frame.

Publication sequence:

1. Capture a resolved scene and its identity under the existing short-lived lock.
2. Enqueue it, replacing older queued diagnostic work.
3. Cancel obsolete computation cooperatively at row/tile boundaries and within expensive Fire work where needed.
4. Compute without holding UI/render-context locks.
5. Before publishing, compare the result with the latest requested key.
6. On the UI thread, compare again with the currently displayed frame identity. Publish the image, map, legend/statistics, and readiness coherently; otherwise discard or retain only as a bounded cache entry.

Recheck after any queued UI closure, since a result can become obsolete while waiting in the event queue. Closing a window, switching designs, toggling Off, losing a material, or hiding the relevant view cancels work and prevents late publication. No unbounded thread-per-hover or thread-per-orbit design.

Live diagnostics use the currently displayed Cut-slider geometry. Tilt analysis and normal high-resolution exports use their existing finished-stone contract. Include this scope in the snapshot and legend; never put a finished-stone map over an intermediate-cut image.

## 8. Scheduling, quality, and resource budget

Full-resolution analytical tracing is not free. One Fire sample can require three multi-bounce paths, and the current metrics pass evaluates far fewer positions than even a modest image. Existing guide-pass timings are not a benchmark for this work.

Implement progressive quality:

- While moving: hide incompatible maps; optionally compute a matching coarse draft under a small capped work budget.
- After a short quiet period: evaluate a draft with one deterministic sample per pixel, initially around a 96–160 pixel long edge. These are starting tuning values, not measured performance promises.
- When idle: refine the map in tiles at a higher resolution/sample count. Reuse sample prefixes so refinement is deterministic.
- Compute base channels together; they share the d-line trace. Run Fire companions only when Fire is enabled or explicitly requested for export.
- Export: explicit higher-quality pass at the export scene's dimensions, tiled to respect a memory ceiling. Offer a recorded diagnostic sampling resolution if full-resolution tracing is too expensive; do not silently upscale a draft and call it full quality.

Proposed performance gates, to be measured by the owner on a documented reference machine: UI input and cancellation remain responsive; toggling opacity/layers over cached channels requires no ray tracing; Off allocates no image-map buffers and schedules no diagnostic rays; the first standard-scene draft should target sub-second delivery. If that target fails, reduce draft work before adding a GPU implementation.

Bound the worker count against the existing render and batch pools. CPU beauty rendering, GPU beauty rendering with CPU diagnostics, and simultaneous tilt work must not each claim all cores. Use a memory budget rather than an unlimited cache of poses. For scale, four `f32` channels alone at 1920×1080 occupy about 31.6 MiB; coverage, IDs, result copies, and RGBA textures add more. Avoid storing full paths per pixel or cloning large arrays on every UI redraw.

## 9. Display composition, state, and exports

Prefer a separate transparent Slint image layer for interactive viewing. It leaves `ViewportModel.render_image`, denoise guides, radiance buffers, saved thumbnails, and remote accumulations intact. Add a small diagnostic UI model rather than continuing to expand a large existing global if that would make ownership unclear.

Suggested UI model fields: enabled, requested effect flags, opacity, display mode, overlay image, status, progress, limitations text, visible-area summary, and hover text. Define an explicit status enum in Rust and one mapping to UI values. Do not infer readiness from whether an image handle happens to be nonempty.

For file output, apply the same colour/pattern transfer function after denoising, exposure/white balance, and tone mapping. Use a documented alpha/compositing convention and validate the interactive and exported appearances against each other. The diagnostic signal must not enter the beauty denoiser or be tone-mapped as scene radiance. Preserve transparent-background coverage and avoid colouring background pixels.

Normal exports remain overlay-free unless the user explicitly selects **Include optical diagnostics**. Freeze that selection, legend, model version, quality, and scene when the export starts. A user changing live overlay opacity halfway through an export must not change later frames.

Diagnostic stills should include a legend or companion metadata identifying pose, material, lighting, effect selection, model limitations, and sampling quality. Reuse the existing export pipeline and privacy conventions; do not write absolute source/profile paths into a legend. Catalogue thumbnails and previews stay clean unless a separate diagnostic export is requested.

Tilt videos use a fixed diagnostic transfer scale across all frames and the exact frame pose from `camera_pose_for_axis_tilt`, including fractional angles. Compute an image map for each requested pose; interpolating percentage curves or spatial masks between angles is not a substitute. Keep only bounded frame work in memory, honour cancellation, and reuse the existing finished-geometry snapshot.

## 10. Tilt Performance integration

Use the same scene snapshot for the selected curve, hovered thumbnail, and diagnostic map. Resolve the current preset/HDR/head-shadow mismatch before describing them as matching. Keep the standardized library sweep separate and visibly identify its lighting policy when comparing against a current-view diagnostic.

The existing signed tilt convention must be preserved: positive/negative tilt selects opposite azimuths; elevation is `90° - abs(tilt)`. Reuse the existing helper rather than creating another formula. Verify all four axes and both sides of zero, especially table-up orientation.

The current small hover image is only 96×96. Add a diagnostic toggle there and a larger inspection view or an explicit **Inspect this angle** action. If that action changes the main camera, it must be user initiated and should retain a way to return to the previous pose. Merely moving the mouse across the chart must not hijack the live viewport.

A hover request cancels the previous one, and its image and map publish together for the same angle. A pending map must not be paired with a newer thumbnail. Existing tilt curves omit Fire companion tracing on their fast path; a Fire overlay at the hovered pose is a separate computation, not an excuse to compute Fire for every curve point or to introduce a new Fire curve implicitly.

## 11. Implementation work packages and dependencies

### A. Lock semantics and capture reference cases

Document exact ray predicates, Fire units/transfer scale, unsupported-effect policy, and image-area versus fan-sampling semantics. Select fixtures already constructible from source: round brilliant, shallow pavilion, step cut, elongated outline, concave tool cut, colourless and strongly absorbing materials, and zero dispersion. Add the fixture descriptions to the feature documentation; no private database is required.

Deliverable: agreed data contract and expected qualitative results. Acceptance: every UI effect has an algorithm, denominator, limitation policy, and explanatory label.

### B. Refactor the classifier without changing old metrics

Extract ray-level classification from `classify_aperture_sample`, preserving the existing ray-generation and floating-point accumulation order. Extend optional diagnostic outcomes with first-hit/exit IDs and termination reason. Avoid building path-debug allocations on legacy scalar-metrics calls. Keep the ten-bounce policy and Fire gate unchanged.

Deliverable: shared classifier used by the existing grid and new image sampling entry point. Owner verification: identical legacy metric results for identical inputs and sample order; exact equality of old/new classifications for identical rays.

### C. Resolve scene/environment consistency

Introduce immutable resolved optical-scene inputs and explicit current-view versus standardized-analysis lighting policies. Connect live metrics, diagnostic requests, current-session tilt sweeps, and hover previews. Include relevant settings in caches, especially HDR identity and head shadow. Version any persisted analysis whose semantics actually change; do not invalidate every stored library curve just because the overlay UI was added.

Deliverable: one visible scene has one documented optical interpretation. Owner verification: preset/light/head-shadow/HDR changes reach all intended consumers, while standardized library/optimizer calculations remain standardized.

### D. Implement image maps and composition

Add deterministic camera sampling, all four channels, coverage and invalid-state handling, cancellation, bounded allocation, and the pure colour-transfer/compositor functions. Implement planar and concave geometry through the same intersector. Start CPU-only; no spectral shader changes are needed for this package.

Deliverable: an optical map for an arbitrary captured camera/stone/lighting tuple, independent of the desktop UI. Owner verification: projection alignment, class conservation, zero-dispersion Fire, border coverage, numerical robustness, and repeatability across worker partitions.

### E. Add the worker and frame publication contract

Create a focused `bridge/optical_overlay/` module with request/key/state/worker responsibilities. Integrate with local and remote frame publication and window shutdown. Separate colour-setting changes from optical recomputation. Audit all `set_render_image` call sites rather than integrating only with `render_thread`.

Deliverable: stale maps cannot be shown, and computation never blocks the UI. Owner verification: deliberately reordered completion events, rapid camera/design changes, remote reconnects, window closure, and render-size transitions.

### F. Add the UI and facet explanations

Implement the panel, legend, image layer, interaction mapping, accessible names, keyboard paths, theme treatment, and per-pixel explanations. Reuse the appropriate facet map for the captured geometry. Treat `.slint` work as one coordinated UI package to limit rebuild cycles; do not repeatedly regenerate the UI for minor layout experiments.

Deliverable: the four requested effects can be inspected without changing the design or the beauty render. Owner verification: individual and combined effects, palette/opacity, narrow windows, DPI scaling, pointer pass-through, material refusals, and Solid/Path-Traced switching.

### G. Integrate tilt and explicit diagnostic export

Share captured scene/pose with tilt previews, add larger-pose inspection, and wire opt-in still/video exports through the same compositor. Snapshot settings once and keep Fire scaling fixed through a sweep. Preserve normal exports and the finished-stone policy.

Deliverable: the diagnostic explanation can be inspected at tilt angles and exported reproducibly. Owner verification: matching pose/image/map, positive/negative tilt, all axes, cancellation, remote beauty results, transparent backgrounds, and unchanged overlay-free files.

### H. Acceptance, documentation, and tutorial

Add manual sections beside rendering and tilt performance, explaining definitions, colour legend, sampling domains, advanced-material limitations, and the difference from Solid hatching and ASET. Add a short built-in guide showing enable, select an effect, orbit/tilt, inspect a region, and export with a legend. Its video route must film real controls and inputs under the project's tutorial rules.

Deliverable: documentation, accessible interaction coverage, reference images, benchmark results, and release notes. The owner runs the agreed tests and visual review. Do not mark the feature complete from source inspection alone.

Critical path: A → B/C → D → E → F → G → H. Core and UI preparation can be scheduled separately after their interfaces are settled, but merging isolated layers without the frame-identity contract is not a releasable intermediate state. Keep new Rust files below 800 lines and split by responsibility before a large file becomes the integration bottleneck.

## 12. Verification matrix and completion criteria

These are proposed checks, not checks run while writing this plan.

| Area | Required check |
|---|---|
| Legacy compatibility | Old fan samples still produce the same metrics with unchanged inputs; Fire-disabled requests do not alter base classifications. |
| Ray semantics | Direct pavilion exit, reflected leakage, lit crown exit, head shadow, unlit exit, side exit, and budget exhaustion have explicit expected outcomes. |
| Fire | Zero-dispersion material gives zero raw Fire; rejected F/C paths contribute no Fire; opacity changes do not change underlying values; no per-image maximum normalization. |
| Sampling | Misses are transparent; base/unknown shares conserve hit counts; silhouette samples use coverage; full outline is sampled for elongated cuts. |
| Camera | Same ray/projection convention as the beauty render at multiple aspects, FOVs, distances, pole crossings, resolutions, and DPI scales. |
| Concave geometry | Tools participate in intersections, IDs resolve safely, and matching maps change when a tool changes. |
| Lighting | Relevant preset, direction, HDR reload, and head-shadow changes invalidate the correct results; exposure/palette changes do not rewrite optical class data. |
| Scene ownership | Library selection, design edits, custom-material edits under the same name, Cut slider, finished exports, and compare/secondary contexts cannot share an incompatible cached map. |
| Async races | A→B→A camera movement, out-of-order worker results, pending UI callbacks, disable/re-enable, hidden views, and shutdown never publish an incompatible result. |
| Backends | The same diagnostic input yields the same map over CPU, GPU, and remote beauty images; remote protocol/radiance sums remain unchanged. |
| UI | All controls are labelled and keyboard reachable; overlay does not consume orbit gestures; status and limitations are readable without colour alone. |
| Export | Off yields unchanged beauty output; On includes requested channels and correct legend, geometry and pose; all video frames use frozen settings and a consistent scale. |
| Performance | Off has no diagnostic jobs or large buffers; cancellation is bounded; repeated opacity changes use the cache; memory is bounded at high resolutions. |

Use layered verification: pure classifier/data tests first; map/reference fixtures second; worker/publication integration checks third; a small UI interaction suite and manual image review last. Avoid asserting that “higher RI always means more brilliance/fire” or that every named cut has one universal appearance: lighting and geometry affect those outcomes. Pin numerical expectations only for fully specified fixtures and model versions.

Release gates:

1. All four effects are usable at the current angle with documented semantics and no fabricated precision.
2. No stale overlay can be displayed over incompatible geometry or a different camera image.
3. No accidental changes to normal rendering, optimization objectives, stored library analysis, or ordinary exports.
4. Shared lighting inputs and explicit sampling-domain labels explain HUD/map/curve comparisons.
5. Advanced-material limitations and budget-exhaustion estimates are visible where relevant.
6. Owner-run correctness, UI, export, performance, and visual checks pass on the supported desktop backends.

## 13. If exact attribution of the spectral render is required

The roadmap's wording can also be interpreted more strongly: mark the light that actually contributes to every rendered pixel, including absorption, anisotropy, and scattering. The analytical overlay described above should not be marketed as meeting that stronger contract.

That version needs a separate design and implementation package:

- Add auxiliary per-pixel transport outputs to the actual spectral estimator, retaining path weights through reflection/refraction splitting, wavelength sampling, polarization, scattering, and fluorescence.
- Define whether Windowing is transmitted energy through pavilion exits, probability of a geometric leakage path, or both. A single Monte Carlo path label cannot substitute for a weighted estimator.
- Define Extinction using a physically specified reference/illumination model. A Russian-roulette termination is a sampling event, not physical extinction; neither is a dropped invalid sample.
- Separate surface glare, returned internal light, body absorption, scattering, and unlit paths rather than forcing every energy contribution into the current three count buckets.
- Define visible Fire attribution independently of gemstone body colour and coloured illumination. Simply thresholding RGB saturation would mislabel Ruby, coloured light, and fluorescent emission. Candidate estimators require controlled spectral/reference comparisons and convergence analysis.
- Implement matching CPU and GPU auxiliary buffers and reductions, with equivalence tests. For remote/hybrid rendering, version capability negotiation and accumulate the additional weighted sums correctly across sample ranges; do not average already-normalized worker maps.
- Keep auxiliary channels out of beauty denoising unless a separately validated reconstruction is provided. Establish noise/confidence reporting and convergence rules.

This is substantially larger than exposing existing metric samples. Build it after the diagnostic map has established the UI, image alignment, scene snapshots, and export contracts, or select it explicitly as the release's required scope before implementing transport changes. The shared presentation layer can support both later, but their legends, statistics, cache keys, and model versions must remain distinct.
