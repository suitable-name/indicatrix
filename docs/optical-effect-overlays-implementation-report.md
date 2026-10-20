# Optical-effect overlays: implementation report

Status: source implementation delivered for independent review. Release readiness is **not verified**. No builds, cargo commands, tests, Clippy, benchmarks, git commands, SQLite reads or UI rebuilds were run by the implementing agent. The two AI crates and optimizer-preservation work were outside this change. Section 13 exact spectral-path attribution remains deferred.

## Implemented scope

- The legacy aperture function constructs its original ray and calls a shared `classify_ray`. Its returned Fire `(angle, weight)` remains separate, preserving `evaluate.rs`'s original caller-side `f32::mul_add` order. The ten-bounce tracer keeps the original arithmetic and `RayFate` interface; an optional allocation-free detail sink adds entry/exit facet, reflections and termination reason.
- `image_types.rs` and `image.rs` provide checked, bounded deterministic perspective evaluation through the renderer camera. Original projection dimensions keep draft aspect exact; optional checked tile origins support output-resolution exports. Counts include unknown/invalid/entry-blocked hits, coverage, qualifying and rejected Fire pairs, raw weighted angular sum, centre-ray attribution and mixed surfaces. Zero-index-spread Fire is guarded only in the image path; historical scores are untouched.
- The pure renderer compositor provides distinct class patterns, combined base shares, independent Fire stipple, normal/high-contrast palettes, opacity, neutral map mode and straight-alpha source-over. The fixed model-1 Fire range is 0–0.1 weighted degrees. It has no image-maximum normalization or local metric floor. Export palette bytes are converted into the output RGB space after beauty processing; beauty alpha stays unchanged.
- Desktop `bridge/optical_overlay` captures resolved plane/tool/material/environment inputs, owner identity and capability notices. One worker owns one replaceable request and one result slot, checks cancellation per sample and between quality passes, and retains one matching map. Draft is at most 128 pixels on the long edge/one sample; refinement at most 320/four samples. Draft delivery is given a polling opportunity before refinement. Off cancels and releases map data/texture. Presentation changes reuse cached channels. A separate refresh works without restarting paused beauty accumulation.
- `scene_generation` travels from immutable frame inputs through local `DisplayWork`/`FramePayload` to the actual UI setter. Local and both remote display routes recheck it immediately before setting the beauty image and updating the displayed stamp. Scene identity includes the geometry owner. The controller separately checks the displayed frame, source dimensions, requested serial and current generation before map publication. Preview resolutions have their own displayed dimensions. No diagnostic switches enter the beauty scene key or remote protocol.
- The Slint global, panel and independent transparent layer expose master switch, four effects, opacity/reset, map mode, refresh, persistent enabled-effect legend, explicit status/progress and caveats. Pointer mapping shares the beauty image rectangle; the overlay itself consumes no orbit gestures. UI lexing errors in adjacent subtraction expressions were discovered by the owner's server build and corrected by the parent reviewer. Slint compilation and visual layout have not been verified here.
- Captured editor facet labels use geometry-owner generation and exact active plane/tool Arcs, then travel immutably with the snapshot. Missing mappings are explicitly identified; concave tool IDs never index the planar vector. Entry and exit roles are separate, and centre-ray/mixed-subpixel wording avoids unsupported whole-pixel attribution.
- Live HUD requests now capture head shadow with HDR identity. Active-session tilt sweeps capture the finished `SceneSnapshot`, including the full material override stack and HDR/head-shadow inputs. HDR cache identities retain their Arcs to avoid address reuse. Standardized catalogue/optimizer evaluation and stored scores remain separate.
- Hover preview uses a single replaceable-request worker instead of a thread per hover. It resolves finished geometry, rechecks scene/hover generation, uses the common signed/fractional pose helper, and renders the same material/tools/finish/fluorescence/environment as its map. The 96-pixel preview and explicit 360-pixel inspection publish image/map together. Errors show Unavailable with the old image hidden. Hover does not move the live camera.
- Still and tilt-video exports have separate explicit opt-ins, frozen presentation and actual output-resolution four-sample diagnostic tracing in bounded row tiles. Normal export returns before any diagnostic validation/allocation. Still companions record model, pose, material, lighting policy, quality, units, selection and limitations without source/profile paths. Videos trace each exact pose, carry a bottom model/Fire caption separate from top performance rows, and write frame-named metadata. Direct and queued jobs preserve options through an additive serde-default job field outside `SceneState`; old jobs default to clean output.
- Rendering/tilt manual sections and a built-in five-step tutorial explain semantics, limitations, sampling domains, interaction and export. The tutorial video route drives real controls/pointer gestures, selects Custom dimensions, answers the real folder picker, and waits for actual PNG plus metadata artifacts. It never injects completion events.

## Source API verification

Authoritative source interfaces inspected before use:

| Interface | Source checked | Use |
|---|---|---|
| `Camera::generate_ray`, camera basis/pole convention | `crates/indicatrix/src/optics/raytracer/camera.rs` | Perspective image sampling and exact projection aspect |
| `PreparedEval`, `StoneArena`, `trace_wavelength`, Fire gate | `color/metrics/{prepared,ray_trace,classify,evaluate}.rs` | Shared classifier, tools, indices, unchanged legacy reduction |
| `ExitLighting`, head-shadow semantics | `color/metrics/{lighting,visibility}.rs` | Existing Studio/HDR cone and lit-model adjustable shadow; exposure/backdrop/glare intentionally do not alter analytical qualification |
| `ToolPrimitive::validate`, facet layout | `geometry/tool.rs`, `geometry/plane.rs` | Input checks and safe concave IDs |
| Resolved material override stack | `render_thread/context/materials.rs`, `render_setup` adapters | Actual selected material, no Diamond fallback |
| Frame/display ownership | `context/frame.rs`, `context/scene_identity.rs`, `display_thread.rs`, `frame_helpers.rs`, remote tick/update | Stamp propagation to actual local/remote setters |
| `SceneSnapshot::capture_finished` | `bridge/export_thread/scene_snapshot.rs` | Finished export/tilt geometry and material/finish/environment capture |
| `trace_spectral_ray_with_finish_soa_geom` | `optics/raytracer/transport/mod.rs` and re-exports | Hover beauty includes tools, frost and fluorescence |
| RGB transfer/matrices/encoding | `color/space.rs` | Display palette profile conversion without beauty tone mapping |
| Export/queue/video and job serde interfaces | `export_thread/worker/export.rs`, `render_jobs/{convert,execute}`, `indicatrix-render-jobs/job.rs` | Frozen opt-in, unchanged remote wire scene, queued roundtrip |
| Tutorial guide/film route interfaces | `guide/tutorials/viewing*`, tutorial video director/scripts/targets | Manual interpretation checkpoints and real controls |

## Static checks and authored regression coverage

Standalone `rustfmt --emit stdout --edition 2024 --config skip_children=true --quiet` parsed and formatted every Rust file in the exact changed-file manifest. The second complete pass parsed 58 Rust files with no syntax failures; all were below 800 formatted lines (largest: `render_thread/mod.rs`, 780). A final pass follows the final edits. This is syntax checking, **not type checking**. Existing large test/gesture modules were split intact to satisfy the line ceiling.

Authored coverage, not executed:

- Identical fan ray versus shared diagnostic ray classification and Fire bit signatures.
- Count conservation, misses, deterministic repeatability, cancellation and checked dimensions.
- Zero-dispersion raw Fire and Fire-on/off base-class equivalence.
- Invalid plane/camera basis refusal and concave-tool participation.
- Full-map versus row-tile RGBA equality including global pattern origins and alpha.
- Publication permission for out-of-order results, A→B→A generations, Off/hidden views and resized frames.
- Exposure/sample-only optical cache reuse versus head-shadow/material/tool invalidation.
- Clean export early-return identity, explicit diagnostic metadata/quality, transparency preservation and ASET refusal.
- Optional job presentation roundtrip, old-job clean default and unknown-model refusal.
- Built-in guide catalogue and five manual checkpoints, plus the filmed interaction/export route.

## Owner verification and remaining release work

No measured performance promises or visual-reference claims are made. The owner must run the following commands on the normal development/server environment (adjust features to the supported desktop build):

```text
cargo test -p indicatrix color::metrics
cargo test -p indicatrix renderer::optical_overlay
cargo test -p indicatrix-render-jobs
cargo test -p indicatrix-editor guide::tutorials::viewing
cargo test -p indicatrix-cut --lib
cargo test -p indicatrix-cli
cargo tutorial-videos -- viewing-optical-diagnostics
```

Use the implementation plan's full UI/backend/export/performance matrix after compilation. Specifically exercise reordered local/remote delivery, reconnects, hidden/closed windows, Off/on, paused edits, owner changes, material edits under the same name, HDR reloads, resize/preview transitions, all four tilt axes/signs, fractional video poses, transparent exports, ordinary export identity, queue resume and frozen settings. Check keyboard access, narrow layout, DPI, themes, high contrast and colour-vision simulations. Record cancellation latency, Off allocations and draft/refinement timing on the reference machine. No benchmark or reference image was produced because execution/rendering was prohibited in this work session.

Reference fixtures constructible without private databases: `StandardGemCuts::standard_round_brilliant`, `emerald_cut`, an elongated scaled outline, a shallow-pavilion authored variant, and a cylinder-tool groove such as the core concave test; use Diamond, constant-index Cauchy, Quartz/Sapphire and a strongly absorbing override under a fully specified ISO/studio/HDR environment. Compare identical rays for classifier compatibility; image-area percentages need not equal the legacy fan. Do not pin a universal ordering by cut name or RI.

Remaining supported-model boundaries are deliberate: the analytical pass does not reproduce the spectral renderer's advanced transport; ASET disables it; a missing generation-bound tier mapping yields explicit surface IDs rather than invented tier names; hover is a low-sample preview. Exact transport attribution (plan section 13), GPU diagnostic acceleration, remote map transport, scintillation maps and automatic catalogue score migration remain deferred. Readiness must not be claimed until owner-run type/UI/backend/export/visual/performance checks pass. The parent reviewer will complete an independent source/API review and address any residual integration issues.

The exact edited-file list is `docs/optical-effect-overlays-changed-files.txt`. Unrelated working changes were preserved; no resets were used.

## Independent source review completed

The parent reviewed camera projection, classification and Fire semantics, local/remote publication stamps, cache ownership, asynchronous cancellation, Slint controls, export settings through queued jobs, and tutorial input routes. Review findings were addressed in the implementation. The parent directly corrected all reported Slint subtraction expressions, checked the changed Slint files for the same lexical mistake, refreshed late generation-bound facet labels without retracing, and retained hidden cached optical data across exposure/sample-only frame transitions. The filmed tutorial now shows Brilliance and Fire separately and visibly refreshes before waiting for the Fire result.

Final parent verification confirmed that all 71 manifest paths exist and all 58 Rust files remain below 800 lines. The parent's final Rust edits passed standalone rustfmt parsing. Source review is complete; compilation, runtime correctness, visual presentation and the release gates above still require the owner's checks. No builds, tests or Clippy were run by the parent.


Final static pass: all 58 manifest Rust files parsed with standalone rustfmt; no syntax errors, missing manifest paths or files at/above 800 formatted lines. Maximum remained 780 lines. The parent-owned `optical_overlay/mod.rs` was parsed but not rewritten during this pass. The parent verified `image-rendering: pixelated` against installed Slint 1.18.1 compiler/common enum source. Parent review also refreshed late matching facet labels without retracing and retained bounded matching data during exposure/sample-only beauty-generation transitions; incompatible overlays remain hidden while awaiting the new beauty frame. These are source/static results only; no Slint compilation or Rust type checking was performed by the implementing agent.
