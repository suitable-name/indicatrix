# Complete tutorial videos: implementation plan

Date: 2026-10-18

Update: the source-based follow-up for the later eight-finding report is in `docs/tutorial-video-remaining-issues-plan.md`. Use it for current diagnosis, implementation order and retry gates; this document retains the original broader completeness requirements.

Status: source analysis and implementation plan only. No application or recorder implementation was changed for this plan. No builds, tests, Clippy, or video generation were run. The user owns all execution and validation.

## Outcome and retry status

Each tutorial must visibly teach its complete workflow: navigation, scrolling, field focus, every authored input, dropdown choice, confirmation, and the resulting state. A correct final design alone is not evidence of a correct tutorial video. The in-app instructions, permitted controls, prerequisites, and completion goals must also describe a workflow a learner can actually follow.

The current checkout is **not ready for a full release recording retry**. The six existing modified files contain useful partial fixes, but the recorder still bypasses the UI and can omit steps while reporting success. A targeted retry becomes useful after work packages 1–4 below; a full retry requires all work packages, including native dialogs, secondary windows, and publication checks. These are readiness gates, not a claim that the changes have already been implemented or validated.

Scope is all 70 built-in scenarios from the supplied report, including the 34 with zero findings. That report stops many scenarios at their first failure; later steps need inspection too. Generated “Build this design” lessons are not part of those 70, but changes to shared guide data and navigation must preserve their behavior.

## Confirmed problems in this checkout

Paths below are relative to the repository root. Line references describe the checkout inspected for this plan and will move during implementation.

| Finding | Evidence | Consequence |
|---|---|---|
| A drawn click is often the only click | `apps/indicatrix-cut/tests/tutorial_videos/director.rs:434`, `click_on`; the cached `uiprobe 0.18.6` recorder explicitly documents `Recorder::click` as ripple-only | The viewer sees a click animation followed by a directly invoked callback, rather than the control being operated. |
| Tier inputs are optional in the filming path | `perform_routes.rs:433–542`, `film_tier` | Missing Add Tier, Angle, Name, Indices, Meets, or commit targets can be ignored; the tier is then saved directly from recipe values. |
| The typing helper repairs failed input silently | `perform_routes.rs`, `click_and_type` | Accessibility value setters can mask missing focus or broken key delivery. |
| Offscreen fields cannot reliably be found | `perform_routes.rs:377–427`, `input_spot_in` / `input_spot` | Resolution returns before scrolling when the field is absent. Fixed screen fractions and a `940`-pixel threshold do not describe the actual inspector viewport. |
| Target checks are insufficient | `targets.rs:321`, `Spot::usable` | A center inside the window does not prove a control is inside its scroll clip, enabled, or unobscured. |
| Row commands can hit the wrong row | `perform_actions.rs:249`, `film_command` | It still takes the first matching visible table button. The new `label_start_near_y` helper has no callers. |
| Recorder actions can execute whole guide recipes | `perform_routes.rs:128` and `perform_actions.rs:297`, `invoke_go_on` | A failed or incomplete visible action can silently trigger extra work and advance unrelated steps. |
| Steps can disappear without a finding | `director.rs:308–317` | “Step went by before it could be shown” is a note followed by success. |
| Auto-advance races the recording | `ui/components/guide_panel.slint:426`; `pacing.rs` | The 700 ms Done timer runs while captions, glides, typing, and holds advance mock time. The new sequence guard can stop filming as soon as an early state goal becomes true. |
| Existing Perform recipes are not presentation scripts | `crates/indicatrix-editor/src/guide/perform_table.rs` and `perform_table/tiers.rs` | A generic Tier form edit does not teach inline editing, facet chips, multi-select Offset, or linked ladder cell editing. Manual steps can also contain actions that must be demonstrated. |
| One generic design is used for incompatible prerequisites | `director.rs::prepare_design`, `routes.rs::OPEN_DESIGN_CARD`; shared `tutorials/tiers/meets.rs` and `tutorials/solving/optimize.rs` | Every `RequiresOpenDesign` gets Standard Round Brilliant. Imported-meets needs imported constraints; Deep Solve needs printed catalogue proportions and an adopted tier. |
| Several goals prove less than the instructions require | Shared `guide/goal.rs`, `tutorials/viewing/app.rs`, and desktop `gui/editor/optimize_panel/mod.rs` | Tier matching does not fully check named targets/exact scale; Ctrl+E twice can complete on the first opening; choosing any objective can count when the text specifies Low Windowing. |
| Event injection can produce apparently successful videos | `routes.rs:399–421`, `config.rs`, `INDICATRIX_VIDEOS_ALLOW_INJECT` | The supplied report's notes match the enabled injection path. Many zero-finding scenarios therefore do not establish that the listed UI operation occurred. Missing members of a multi-event goal can also become notes. |
| Previously passing workflows also bypass controls | `perform_actions.rs`; New Design and concave routes in `perform_routes.rs`; `stage.rs` | Model writes substitute for dropdowns, checkboxes, generator fields, navigation, and other interactions. Concave tool and motion choices are injected into the save callback. |
| Capture covers only the main window | `director.rs:129`, `Recorder::start(ui.window(), ...)`; `view()` | Separate Compare, Rough Planner, and Help windows are not automatically included. |
| File dialogs are not filmed | `src/harness_support.rs`, `src/gui/pickers.rs`, and `routes.rs::sheet_route` | The harness queues picker answers and bypasses the native dialog, omitting location, filename, Open/Save, and overwrite interactions. |
| Privacy is checked after frames were emitted | `routes.rs::sheet_route`, `director.rs::health` | Hiding the success toast before a final check does not remove a path already recorded. It also hides useful learner feedback. |
| Publication precedes all health checks | `director.rs::play` finishes the recorder before `main.rs::register` calls `ctx.check_health` | A finding can arrive after output is finalized. `uiprobe::Recorder::finish` also publishes the MP4 before writing its sidecars. |

### Existing edits: retain intent, do not assume completion

The working tree already changes `director.rs`, `perform_actions.rs`, `perform_routes.rs`, `targets.rs`, shared `guide/perform_table.rs`, and shared `guide/stage.rs`. Preserve these edits while integrating the implementation.

- The row snapshot/edit fallback prevents the previously observed blank-name/index overwrite in some cases. It still performs an invisible save and must be removed from the release recording path once genuine editing works. Keep full-row before/after comparison as a diagnostic.
- The sequence/step guards reduce overrun but do not guarantee every authored action was filmed. A satisfied goal must not truncate the script.
- The row-position helper was added but is not wired into `film_command`; the reported row targeting fix is incomplete.
- The preference stage change and the authored Table deletion are present. They need user validation with real input. The Table deletion is only the first missing route in its tutorial.
- “Previously validated” in the pasted handover is historical information, not evidence that this checkout now produces complete videos.

## Completion contract

For each tutorial, require all of the following before output is published:

1. Every catalog step is visited once in order and has a chapter. Any revisit is explicitly authored. No silently omitted or auto-skipped steps.
2. Each numbered instruction is classified as an action or an observation. Every action has an explicit recorded route, including actions embedded in a `Goal::Manual` step. Observation-only steps show the relevant result long enough to read it.
3. Required controls are visible, enabled, unambiguous, and unobscured. Navigation and scrolling that reveal them are recorded.
4. Every authored value is entered through the appropriate widget. Expressions such as `40 + 1` are typed as expressions when that is what the lesson teaches. Unchanged defaults can be pointed out and read; omitted edit fields retain their existing values.
5. The app receives real pointer/key/wheel input and performs its normal validation. The recorder must not repair the outcome with model writes, save callbacks, a whole-guide Perform, or fabricated events.
6. A step completes only after its action script, its expected intermediate/final observations, and its real guide goal are satisfied. A goal already true at entry does not excuse omitting the operation being taught.
7. All windows and dialogs involved in the operation are captured. File choice is part of a save/open/import/export lesson.
8. Long solver/render waits may be sped up or cut with a short caption. User input, menu navigation, field edits, and confirmation must remain visible at readable speed.
9. All final health/privacy checks succeed before the MP4, captions, chapters, and recording manifest become the current successful result. Old files must never be mistaken for fresh success.

Setup before the recording may seed isolated files, a fixture library, settings, and a declared prerequisite design. Show the prerequisite in the introduction. Setup must not secretly perform the very action the lesson is supposed to teach. For example, an imported-meets lesson may have an ASC file ready on disk, but its Open/import/adoption actions must be shown if the instructions ask for them.

## Work package 1 — Separate presentation scripts from Perform and inventory every step

Primary files:

- Shared guide data: `crates/indicatrix-editor/src/guide/{model.rs,steps.rs,tour.rs,perform.rs,perform_table.rs,perform_table/tiers.rs}` and `guide/tutorials/**`.
- Video harness: `apps/indicatrix-cut/tests/tutorial_videos/{main.rs,config.rs,director.rs}`.
- New video-only modules, for example `script.rs`, `scripts/`, and `coverage.rs` under `tests/tutorial_videos/`.

Implement:

1. Give steps stable identifiers within each guide; do not key presentation scripts only by mutable prose titles or positional indices. Keep existing guide IDs/progress persistence compatible. Update shared constructors and generated lessons deliberately.
2. Add a video-only `VideoRecipe` registry keyed by guide/step identity. Keep the app's `Perform` as its convenience “do this step for me” feature. A video recipe describes how a person performs the operation, not just the desired resulting state.
3. Include narration/observation beats and typed operations for clicking, double-clicking, replacing/typing text, opening/selecting combo choices, toggling, scrolling, modifiers, dragging, waiting, inspecting, and returning from secondary surfaces. Domain routes may compose these primitives; avoid one giant event-name switch.
4. Associate every instructional action with the operation(s) that demonstrate it and an observable postcondition. Mark truly observation-only content explicitly. Audit Manual steps such as Solve then Undo, Ctrl+Tab twice, and turning the stone/light.
5. Introduce a preflight mode that checks the complete catalog without encoding video: missing scripts, unmapped selectors, unsupported action types/surfaces, missing fixtures, invalid guide/step references, and injection settings. Check runtime visibility during recording; static preflight cannot prove it.
6. Reject `INDICATRIX_VIDEOS_ALLOW_INJECT` for release recording. If diagnostic injection remains for development, keep it outside the publishable tutorial-video command and output directory. A successful injected run cannot qualify as release coverage.
7. Fail on any required unsupported action, including one member of a composite goal. `All` requires every authored requirement; `Any` uses one deliberately chosen complete branch, rather than flattening all event names and invoking them.
8. Give each video a prerequisite fixture specification and check its capabilities before recording. Replace the universal `RequiresOpenDesign` template with the appropriate fixture: imported meets, printed catalogue proportions plus an adopted tier for Deep Solve, or the ordinary template where sufficient. Preserve explicit guide template choices; do not “fix” all tutorials by switching them to one different template.

Done when the inventory accounts for every instruction of all 70 scenarios and reports every missing route before an expensive run. A script being present is not yet evidence that it works.

## Work package 2 — Make recorded input strict and visible

Primary files: video `targets.rs`, `perform_routes.rs`, `perform_actions.rs`, `routes.rs`, `stage.rs`; relevant components under `apps/indicatrix-cut/ui/components/`. Add a small shared `input.rs` module for the recorder.

Implement:

1. Separate the cursor/ripple overlay from input delivery. A recorded click moves the pointer, dispatches exactly one press/release to the resolved control, records its pressed/released state, and checks the expected effect. Do not both press a widget and invoke its callback afterward.
2. Send real focus, selection, character, modifier, Enter/Escape, and wheel events. For replacement text show select-all/clear and character-by-character input at a readable cadence. Verify focus and resulting widget value. Remove `set_accessible_value` repair and direct model mutations from recorded action paths.
3. Open dropdowns visibly, locate the requested option by semantic label in the active popup, select it, and check the selected label/value. Do not set a numeric index invisibly. Toggle checkboxes only if the requested state differs, and show the resulting checked state.
4. Resolve fields inside the intended dialog/form/row, by role and stable component identity. Add useful labels or read-only instrumentation where the existing UI is ambiguous. Existing accessibility actions may help diagnose wiring, but they must not replace the click/typing/drag behavior being taught.
5. Resolve scroll containers before their offscreen descendants. Scroll a bounded amount, capture the scroll, pump layout, and reacquire the target until its clickable rectangle lies within the visible clip. Use actual geometry, not screen fractions or fixed y coordinates. Fail with target/container diagnostics when no progress is possible.
6. Reject hidden, disabled, occluded, ambiguous, or stale targets. Requery after layout changes, popups, row moves, and scale changes. Never interpret `None` as permission to proceed invisibly.
7. Bind tier operations to the intended row identity; use row-scoped descendants, not the first label prefix. Track identity through rename/reorder and verify the neighbor order afterward. Do not rely on stale indices or unconstrained nearest-y matches.
8. Make staging use these same actions during recording: view switches, panel expansion, inspector tabs, parameters tabs, and library toggles. Remove manual clearing of `stage_mismatched` as a substitute for satisfying the stage. Presentation-only camera framing must not substitute for a taught viewport gesture.
9. Add action evidence to the recorder trace: step/action ID, surface, selector, visible bounds, input dispatched, before/after values or state, and frame/time interval. Readback is allowed; writes that fake the expected result are not.

Done when missing focus, offscreen fields, wrong rows, and inert buttons produce immediate useful failures rather than direct-save fallback success.

## Work package 3 — Control progression without weakening guide correctness

Primary files:

- Video `director.rs`, `pacing.rs`, and `perform_routes.rs`.
- `apps/indicatrix-cut/ui/models/guide.slint` and `ui/components/guide_panel.slint`.
- `apps/indicatrix-cut/src/gui/editor/guide/{runtime.rs,progress.rs,perform.rs,stage_nav.rs}`.
- Shared `guide/goal.rs` and tutorial definitions where goals finish too early.

Implement:

1. Add a recording presentation policy, enabled through harness support, that holds automatic advance while a step's full script and result presentation are active. Goal evaluation and normal UI callbacks continue running. Do not freeze the entire event loop, solver, or render work.
2. Let the director own one advancement after every required action/observation has finished and the real goal holds. Reading steps use their actual Next/Finish button. Action steps release the normal Done transition once; the harness never writes `step_done`, `step_index`, or a synthetic goal event to force success. Reset the policy on completion, failure, relaunch, and teardown.
3. Unexpected guide closure, identity change, index change, or skipped step becomes a finding. Remove the current note-and-success path. Sequence execution stops on failure or unexpected identity change, not merely because an intermediate broad goal is true.
4. Remove recorder calls to `GuideModel.go_on`/whole-step Perform. An unfinished command must not cause the app to perform the rest of the recipe invisibly.
5. Correct production goals independently: split a multi-operation step where needed, or require the relevant actions and their resulting state. An early match must not advance a human learner past later listed instructions. Keep normal app auto-advance behavior outside the recording hold policy.
6. Use step-entry baselines or operation/revision identities for “changed angle,” “saved,” “solved,” and similar goals. Distinguish successful completion from attempted action, stale cached results, or a value already true on entry. For Save/Open/Import/Export, require real success and the expected file/design result.
7. Use actual operation completion and monotonic real-time deadlines for background waits. Keep video pacing time separate. Show the operation starting and the result arriving, even when the intervening computation is accelerated. Replace timer-race constants with deliberate reading and result holds.
8. Strengthen concrete weak goals: named Meets must check the referenced facets, Exact Scale must check its value, quick-add must check its intended preset, and choosing Low Windowing must check that objective. Ctrl+E twice and opening then closing the shortcuts overlay must include the return/close operation. Do not treat an unparameterized event as proof of a specific value or an ordered pair of operations.

Done when one step cannot consume another step's actions, a satisfied entry goal cannot erase its demonstration, and all listed operations remain possible with the guide's normal locks.

## Work package 4 — Repair the new-design and tier workflows first

Primary files: video New Design/tier/concave/generator routes; shared `guide/perform_table/tiers.rs`, `guide/tutorials/tiers/**`, `guide/steps.rs`; desktop inspector/tier-form/table components and their normal callbacks.

For every new flat tier, record this sequence:

1. Navigate to Edit/Tier as the instructions require.
2. Find and click **+ Add Tier**; verify the form is in add mode.
3. Reveal, focus, and type each authored field in instruction order: angle, constraint/Meets selection and value, name, indices, and any other taught field.
4. Show the fully populated form and any validation message. For a fresh form, verify defaults the lesson relies on instead of assuming previous values were cleared.
5. Find and click the actual **Add Tier** button once.
6. Verify exactly one expected tier was added, show its table row, and show the relevant preview/result.

For editing, select the named row through the taught route and load the form. Change only authored fields; preserve omitted values by retaining them in the real loaded form. Click Save Tier, verify the same tier changed and row count stayed stable, and compare untouched fields. Remove the direct-save fallbacks, forced selection/reset pulses, and row-count-as-save-success heuristic from release recording.

Other required tier routes:

| Tutorials | Required visible behavior and correctness work |
|---|---|
| `tiers-add-a-tier`, `tiers-edit-a-tier`, `tiers-index-shorthands`, `new-design-walkthrough` | Complete form entry/commit; demonstrate raw shorthand input and its interpreted result; ensure template/preform/gear/material choices are shown in New Design. |
| `tiers-facet-chips`, `tiers-inline-angle` | Actual chip Add/Remove/Detach actions and actual table cell double-click, typing, Enter/blur commit; do not substitute a Tier form edit. |
| `tiers-quick-add` | Click the real Girdle, Table, and Culet quick-add buttons. The button callback values are expected-result references, never the recorder implementation. Check the current UI's actual presets. |
| `tiers-meets-named-facets`, `tiers-meets-mm-targets`, `tiers-pin-to-mast` | Select the actual constraint option, type its expression/value, show solved relationships, then show pin/adopt operations. Preserve each lesson's correct starting template and geometry. |
| `tiers-adopt-imported-meets` | Supply an ASC fixture with genuinely imported constraints; visibly open/import it, adopt the instructed single/all constraints, and verify metadata/state changed correctly. Ordinary solved-template setup is not an equivalent prerequisite. |
| `tiers-step-series`, `tiers-linked-series`, `tiers-mirror-to-other-block`, `tiers-relations` | Fill generator/mirror/relation controls; show linking checkbox, generated rows, inline rung edit, propagated result, and any detach/unlink operation. Preserve names and constraints. |
| `tiers-arithmetic`, `tiers-cheater-offset`, `tiers-tier-notes` | Type the instructed expression/text, demonstrate facet-position arithmetic where requested, and show the normalized value or saved note/offset. |
| `tiers-duplicate`, `tiers-move`, `tiers-delete`, `tiers-multi-select` | Show the correct row action, editing the copy, bounded moves, actual selection modifiers/checkboxes, batch Offset and batch Delete, and confirmation if present. Sequential single-row operations are not a substitute for a batch lesson. |
| `tiers-concave-cylinder-cone`, `tiers-concave-sphere-disc` | Choose each actual tool type and motion/reciprocation option, fill every tool-specific field, commit through the concave form, and display the resulting geometry. Reaudit despite their previous zero findings. |

A targeted retry of New Design and representative tier videos is useful only after packages 1–4 are implemented. It is an intermediate validation run, not readiness to regenerate the release suite.

## Work package 5 — Author the remaining workflows and correct the lessons

Primary files: shared `guide/tutorials/{solving/**,viewing/**}`, `guide/perform_table.rs`, `guide/stage.rs`; video `scripts/` and action routes; the corresponding desktop UI/callback modules.

Cover the rest of the 70-scenario suite, including steps after each reported first failure:

| Tutorials | Required work |
|---|---|
| `welcome-tour` | Show actual menu/tab/panel navigation and observations; no direct tutorial/browser/palette/model shortcuts. |
| `solving-solve`, `solving-auto-solve`, `solving-status` | Real Solve/F5 and auto-solve dropdown, deliberate edit while off, stale/running/completed states, removal and restoration of the anchor. Explicitly author the Exact Scale = 1 repair instead of relying on an under-specified inferred recipe. |
| `solving-verdict`, `solving-verdict-fixes` | Click the verdict badge and inspection/fix controls. Include later “Add a Table with a Fix,” off-tooth index `12.5`, and snap-to-teeth actions; the existing Table deletion alone is insufficient. Require actual repaired design state. |
| `solving-preform`, `solving-yield-carat` | Type width `1.80` through the actual Preform field, apply it, show geometry/yield changes; film all taught yield inputs and material choices. |
| `optimizing-deep-solve`, `optimizing-optimize` | Show preset/ranges/objective controls, launch the actual computation, observe completion, select and inspect a candidate, apply it, and perform all later Solve/Undo instructions. Never synthesize started/finished/candidate events. |
| `optimizing-retarget-shift`, `optimizing-retarget-optimize`, `optimizing-sweep` | Open the right dialog, choose material/method or sweep tier/range, run, inspect result, apply the candidate/angle, show changed geometry and Undo when taught. Use deterministic fixtures/options, not fabricated results. |
| `solving-history`, `solving-variants`, `solving-snapshot-compare` | Film edit, snapshot/variant name, save, history row jumps/back/forward/Undo, variant reopen, Compare and return. Capture the separate Compare window. |
| `solving-raw-text` | Open the actual editor, select/type the instructed text, show validation/diff, Apply and Undo. Verify the intended design change instead of reporting only “editor opened.” |
| `viewing-solid-picking`, `viewing-drag-handles`, `viewing-diagram` | Use geometry-derived screen coordinates/read-only handle information for picking and pointer-down/move/up drags. Demonstrate angle/depth/index changes and cancel where listed; show diagram mode, enlarge controls and diagram-specific handles. Verify the intended facet/tier and resulting delta. |
| `viewing-slice`, `viewing-cut-slider` | Film drawing the slice, preview, flip/symmetry changes, Keep/commit and Undo; show actual slider motion and stop positions. Verify that committed slice adds the correct facet rather than merely raising an event. |
| `viewing-live-render`, `viewing-design-lighting` | Show real view/lighting selections, stone/light movement, saving and forgetting design lighting, and the visible result. |
| `viewing-custom-material`, `viewing-material-coefficients` | Fill all instructed material names, coefficients, colors/options through the actual dialog, save, then verify it is used by the design/render. |
| `output-cutting-mode` | Show cutting steps, progress/mark controls, and their effects. |
| `output-export-asc`, `output-export-gcs`, `output-cutting-sheet`, `output-diagram-png`, `output-save`, `output-open` | Film menu/shortcut, real file dialog, name/location/format and confirmation, return and visible success. Verify output content or reopened design; use the surface work in package 6. |
| `library-import`, `library-search`, `library-filters`, `library-load-design` | Real import chooser, typing and clearing search, filter selections/reset, card selection and Load Selected; no property setters or fabricated library events. |
| `library-rough-planner` | Capture the planner window; fill rough/design/planning controls, run, show results, and close/return as instructed. |
| `prefs-interface-mode`, `prefs-high-contrast`, `prefs-interface-scale`, `prefs-larger-handles` | Real controls and visible before/after; reacquire bounds after scale/layout changes. Verify the solid viewport stage for larger handles instead of overriding a mismatch. |
| `app-command-palette`, `app-keyboard-shortcuts`, `app-help-viewer`, `app-glossary` | Actual keyboard chords, palette query typing and command choice, every listed shortcut, real Help/glossary navigation and capture of the Help window. |

For each row, reconcile four things together: instructional text, enabled controls/staging, authored inputs/actions, and the true result checked by the goal. Do not weaken a goal merely to make the video pass. If the text requests an impossible operation, fix the lesson or underlying app behavior and update the script accordingly.

The existing guide simulations are useful source references for intended geometry and values. They are not proof of visible interaction coverage: `guide/perform_tests.rs` currently permits walking to stop at unperformable/UI steps, and checking that some steps are covered is weaker than this requirement.

## Work package 6 — Record every surface, including real file dialogs

Primary files: video `director.rs`, `main.rs`, `config.rs`; new `surface.rs`/capture adapter; `apps/indicatrix-cut/src/harness_support.rs`, `src/gui/pickers.rs`; existing secondary-window accessors in `tests/ui_harness/support/secondary.rs` and harness support.

Architectural decision: retain the deterministic Slint recording path for workflows it can actually capture, and add a **native desktop recording lane for tutorials requiring native dialogs**. Do not replace real dialogs with a recorder-only imitation and call that a complete tutorial. Production in-app picker redesign is outside this fix.

Implement:

1. Introduce a recording-session boundary for input, frame capture, timestamps, captions, and active surface. Model Main, secondary Slint windows, popup surfaces, and owned native dialogs explicitly. Coordinate conversion, focus, pointer position, and frame sizing belong here.
2. Check capability early. The installed `uiprobe 0.18.6` `Recorder` borrows one fixed Slint window; it is not a multi-window or desktop recorder. Implement the required adapter/segments and shared timeline in owned code, or an explicit upstream dependency change. Do not edit Cargo's cached source or assume a source-switch API exists.
3. For secondary Slint windows, capture the actual opened component, keep a consistent output canvas, and route subsequent input to that window. On close, return to the main window visibly. Include active popups, warning/error notices, and window transitions in health/capture checks.
4. Run native-dialog lessons using the real desktop backend for the whole lesson, so the actual `rfd` dialog and its interaction can be captured. Add an explicit picker policy that bypasses queued harness answers only in this native recording mode. Keep ordinary UI-harness isolation behavior intact.
5. Supply actual desktop window capture and native input/control discovery for Windows first, the user's current platform. Do not claim Linux/macOS dialog support until implemented. Detect unavailable capture/input capabilities before launching a long recording.
6. Use a neutral, isolated demonstration directory with prepared files and controlled starting folder. Film filename typing or file selection, directory navigation where instructed, Save/Open, and overwrite confirmation if the lesson calls for it. Verify operation completion through the app and resulting file/design.
7. Serialize the native desktop lane; parallel native input on the same desktop would corrupt focus and actions. Keep independent offscreen scenarios parallel where supported. Track capture targets by process/window ownership to avoid unrelated desktop content.

This is a necessary capability gate for the complete save/open/import/export lessons, not optional polish. If unavailable, those videos must fail preflight as unsupported instead of silently omitting dialogs.

## Work package 7 — Pacing, privacy, evidence, and safe publication

Primary files: video `pacing.rs`, `director.rs`, `routes.rs`, `config.rs`, `main.rs`; new trace/publishing module; relevant production export confirmation formatting.

Implement:

1. Keep the relevant instruction/caption with its action, including keyboard chords and dropdown choices. Show each completed field and committed result. Ensure the cursor, guide panel, and captions do not hide the control being taught. Calculate reading time per cue rather than cramming many cues into one fixed cap.
2. Require a captured frame interval for each action and its result. Record scroll and drag intermediate frames. Treat long background computation separately; label speedups/cuts and preserve start/result frames.
3. Prevent private paths before frames reach the encoder. Use safe demonstration paths and appropriate filename-only user-facing success text where sufficient; do not merely dismiss the cutting-sheet toast. Preserve success feedback and all genuine errors. A changed production toast must remain useful outside recordings.
4. Route frame emission through a common health/privacy gate covering every active surface. This needs a capture/recorder pre-frame hook: checking only before and after `Recorder::hold` is insufficient because it pumps the app and emits multiple frames internally. Cache stable checks if needed, but rescan after text/input, navigation, dialogs, asynchronous notices, and model changes that affect visible text. Account for non-accessible text and OS surfaces; an accessibility-only main-window scan is not a full privacy guarantee. Reject or sanitize before encoding, never after publication.
5. Write a per-tutorial trace/manifest: catalog/step identity, source revision or source digest including a dirty checkout, fixture identity, backend, action coverage, step order, timestamps, input/readback results, output locations, and final status. Diagnostic trace text must not introduce private data into published artifacts.
6. Record into a per-run staging directory. Run the final guide/trace/output/health checks, including the current post-film `ctx.check_health`, before publishing. Complete encoding and all sidecars before promoting the bundle.
7. Publish a complete versioned bundle and update a small current-success manifest last. If retaining flat filenames, use explicit staging/rollback and let the manifest identify a consistent set; do not claim multiple independent filesystem renames are atomic. Preserve old valid output on failure and report it as **unchanged/stale for this run**.
8. Fail for missing chapters/actions, callback/injection fallback use, unexpected surfaces, early advancement, leftover picker answers, timeout, panic, error, or forbidden visible text. Report the exact guide/step/action/control and last observed state. Preserve a safe diagnostic frame where useful.
9. Update `README.md` and `apps/indicatrix-cut/docs/manual/22-tutorials.md` for the implemented workflow and actual recording limitations. Update outdated recorder comments claiming honest completeness before that is enforced.

Done when a zero-finding result means complete observed interaction coverage and all published files belong to the same successful run.

## Implementation order and integration boundaries

1. Inventory and script contract (package 1); prototype/confirm secondary/native capture capabilities early (package 6) so those limitations are not discovered at the end.
2. Strict input/targeting and progression ownership (packages 2–3). These are prerequisites for trustworthy route work.
3. New Design and tier routes (package 4). This addresses the reported continuity problem and provides an early user retry point.
4. Remaining routes and guide corrections (package 5), with package 6 implemented for their surfaces.
5. Per-frame safety, final coverage checks, and publication (package 7), followed by the user's targeted and full validation.

After the common action/script interfaces are stable, independent route families can be implemented in parallel: tiers, solver/history/optimization, viewport, and file/library/preferences. Shared driver, guide-model, and selector changes need a single integration owner. Do not let route workers reintroduce local callback fallback helpers.

No physics/kernel changes, unrelated project cleanup, or dependency upgrades are implied. Any `uiprobe` or capture dependency change must be justified by the required input/capture capability. Shared guide data changes need compatibility review for desktop, web, mobile, and generated lessons; desktop-specific filming logic stays in the video harness.

## User-run validation and retry gates

These are future checks for the user to run after implementation. None were run while preparing this plan. Implementers should add focused regression coverage for meaningful failure modes; the user alone runs tests and Clippy.

Required regression coverage:

- Add versus edit: one commit, no duplicate rows, unchanged fields preserved, real invalid input cannot be repaired by a hidden save.
- Missing/offscreen/disabled/occluded fields, wrong focus, ambiguous row controls, row reordering and inline edits.
- A goal true at entry, a goal true midway through a script, slow solves, deliberate Undo, and unexpected guide close/change; none may lose an authored action.
- Every catalog action has a supported route; Manual steps with demonstrations are included. Negative coverage must fail, not stop a walkthrough successfully.
- Correct lesson prerequisites and expected geometry, relation/series behavior, imported constraint adoption, and real save/export results.
- Multi-window capture, native file selection, pre-frame privacy, and failure before publication preserving the previous complete bundle.

**Targeted retry gate:** packages 1–4 implemented, the selected scripts pass preflight, injection disabled, and no direct-save/go-on fallback remains. The user can then generate a small candidate batch using the existing substring filters, for example:

```powershell
# Run by the user only, after the targeted retry gate is reached.
$env:UIPROBE_JOBS = 'serial'
Remove-Item Env:INDICATRIX_VIDEOS_ALLOW_INJECT -ErrorAction SilentlyContinue
cargo tutorial-videos -- new-design-walkthrough tiers-edit-a-tier tiers-inline-angle tiers-move tiers-concave-cylinder-cone
```

Watch the actual MP4s, not just the report. Confirm the + Add Tier press, every value, dropdown popup, visible Save/Add press, correct row, result hold, and every chapter. Confirm an edit preserves all untouched values and that the move/concave workflows show the intended control operations.

**Broader retry gate:** finish and inspect representative recipes for preform, linked-series/multi-select, verdict fixes, Optimize/retarget, raw text/history, drag/slice, preferences, Compare/Help/planner, and a native file workflow. Check filenames/paths and proof of real output. Implemented preflight must cover the entire catalog before a full run.

**Full retry gate:** all 70 tutorials have complete scripts, all required capture surfaces work, strict input/progression is active, and package 7 prevents incomplete output publication. The user then runs the full suite with injection disabled. The runner must serialize native desktop scenarios while allowing independent offscreen scenarios to parallelize.

**Release acceptance:** no findings, no skipped/unrecorded actions, no fallback/injection notes, complete ordered chapters/action evidence for every tutorial, correct design/file outcomes, and human review of the resulting videos for readability and continuity. The supplied 34 previous passes do not carry forward as exemptions.

When implementation reaches the targeted or full gate, report that specific gate to the user with the changed files and remaining limitations. Do not say “fixed” or “safe to regenerate all release videos” solely because this plan exists or because an unexecuted code change appears plausible.
