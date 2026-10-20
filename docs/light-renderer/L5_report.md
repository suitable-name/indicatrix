# L5 Guides, Help and Compare

Implemented the owned guide/help/Compare work from §§6.4 and 14. No cargo, builds, tests, Clippy or git commands were run. The two AI crates were not touched.

## Completed

- `Goal::ViewMode` and `Perform::ViewMode` accept 0–4, and still reject negative/unknown values. Stage comments and catalogue/tour/viewing prose describe five choices and keys 1–5.
- The Solid Picking lesson includes a visibly driven Ray-Traced step with `Goal::ViewMode(4)` and an honest approximation explanation. It is inserted after the existing indexed filmed steps 4/5, preserving their recipes.
- Video targets resolve mode 4 and the highlight group spans all five real pills. No synthetic event or model callback substitutes for input.
- Both Rust and Slint Help topic ids now use `13-solid-inspection-view#the-five-view-modes`, matching the renamed manual heading. The existing toolbar help choice reaches this five-mode section without another topic property.
- Compare has explicit index mapping 0 Solid, 1 Traced, 2 Ray; unknown indices remain Solid. The new third pill is `Ray Traced`, matching Compare's existing unhyphenated labels.
- Renderer choice travels in each worker request. Existing per-view generation checks reject late results across engine switches. The solid preview still follows drag immediately, while either traced engine uses the established settle/release scheduling.
- Ray comparisons call the shared `bridge::preview_render::render_rgba_at_pose_ray` with default `RayOptions` (8 taps), existing pose, exact tools/frost and bounce cap. No denoiser or path-tracer changes.
- Preparation errors produce attributed Before/After refusal text and hatch placeholders, with no silent fallback. The refusal survives late solid-frame updates until a new request resets it.
- Adaptive slow-concave SPP remains path-specific; Ray-Traced retains its fixed tap count. Status text says taps and includes “tracing” while busy so existing screenshot settling recognizes it.
- Compare shows an approximation caveat and leaves its analytic optical figures unchanged.
- Tutorial video Compare capture clicks all three renderer choices. The secondary-window coverage table explicitly invokes renderer index 2.
- Added screenshot function `editor_shots::retarget_dialog_ray`; root was asked to register it in the screenshot runner.

## Source tests added or extended (all unrun)

- Goal validator moves 4 into valid modes and uses 5 as the first unknown mode.
- Perform validates 4, rejects 5/-1 and derives the mode-4 recipe from its goal.
- Compare index test covers 2 and unknown values.
- Ray frame admission covers generation changes, late solid rejection and rejection when Solid is selected.
- Ray status reports 8 taps and preserves unresolved-material warnings.
- Shared Ray preview is called for a real Diamond fixture and checked for repeatable RGBA bytes and expected size.
- Guide hook test ties the authored Ray step to the actual Slint pill and filmed mode-4 route.

Existing Compare tests were mechanically continued in `tests_more.rs` with `include!`, retaining their module scope and every original assertion. Existing host tilt functions moved intact into `host_tilt.rs` with `include!`. These keep the formerly oversized files below 800 lines.

## Files and pre-format line counts

| File | Lines |
| --- | ---: |
| crates/indicatrix-editor/src/guide/goal.rs | 446 |
| crates/indicatrix-editor/src/guide/goal_tests.rs | 301 |
| crates/indicatrix-editor/src/guide/perform.rs | 612 |
| crates/indicatrix-editor/src/guide/perform_tests.rs | 685 |
| crates/indicatrix-editor/src/guide/stage.rs | 727 |
| crates/indicatrix-editor/src/guide/tour.rs | 184 |
| crates/indicatrix-editor/src/guide/catalog.rs | 337 |
| crates/indicatrix-editor/src/guide/tutorials/viewing/solid.rs | 536 |
| crates/indicatrix-editor/src/guide/tutorials/viewing/app.rs | 473 |
| apps/indicatrix-cut/src/gui/help/topics.rs | 363 |
| apps/indicatrix-cut/ui/models/help.slint | 170 |
| apps/indicatrix-cut/docs/manual/13-solid-inspection-view.md | 446 |
| apps/indicatrix-cut/ui/models/compare.slint | 126 |
| apps/indicatrix-cut/ui/components/compare_view.slint | 680 |
| apps/indicatrix-cut/src/gui/editor/compare/session.rs | 539 |
| apps/indicatrix-cut/src/gui/editor/compare/host.rs | 784 |
| apps/indicatrix-cut/src/gui/editor/compare/host_tilt.rs | 87 |
| apps/indicatrix-cut/src/gui/editor/compare/render.rs | 586 |
| apps/indicatrix-cut/src/gui/editor/compare/tests.rs | 771 |
| apps/indicatrix-cut/src/gui/editor/compare/tests_more.rs | 170 |
| apps/indicatrix-cut/tests/guide_highlights.rs | 178 |
| apps/indicatrix-cut/tests/tutorial_videos/targets.rs | 654 |
| apps/indicatrix-cut/tests/tutorial_videos/stage.rs | 468 |
| apps/indicatrix-cut/tests/tutorial_videos/perform_actions.rs | 462 |
| apps/indicatrix-cut/tests/tutorial_videos/compare_capture.rs | 273 |
| apps/indicatrix-cut/tests/ui_screens/editor_shots.rs | 404 |
| apps/indicatrix-cut/tests/ui_harness/scenarios/coverage/rows_windows.rs | 116 |

## Integration requests and reading limits

- Root: add `("retarget_dialog_ray", editor_shots::retarget_dialog_ray),` after the existing `retarget_dialog_1` screenshot runner row. Its new file is a new image, with no existing pin changed.
- L13: the manual heading/opening now says five modes; the rest of chapter 13 still needs the planned comprehensive Ray-Traced prose. This lane deliberately owns the heading/help-anchor synchronization, not all documentation.
- Root implemented/re-exported the shared Ray preview helper with the exact agreed existing argument shape, replacing `spp` by `RayOptions` and returning `Result<Vec<u8>, WhittedDecline>`.
- No contract deviation. `Renderer::Ray` follows the plan's exact spelling.
- Lead formatting should recheck `compare/host.rs` (784 lines before formatting).
- Toolkit parsing, actual image settling/layout, runtime material refusals and device behavior remain unverified without owner builds/tests.
