# Tutorial videos: remaining failures and updated implementation plan

Updated: 2026-10-18. Source baseline inspected: `d0611bdd` (`fixes`), initially clean working tree.

This is a source-based diagnosis and implementation plan, not an implementation or validation result. No tests, Clippy, builds, or video generation were run. The supplied report has no source identity, so its correspondence to this revision cannot be independently established. Runtime causes that need additional observations are identified below rather than asserted as proven.

This document supersedes the status and priorities in `docs/tutorial-video-implementation-plan.md`. That document remains the broader completeness specification; its historical descriptions of pending edits must not be treated as current status.

## What the new report establishes

Findings fell from 36 to 8, and 62 of 70 scenarios now report zero findings. More state transitions complete, but this is not equivalent to 62 complete instructional videos.

There are two remaining tracks:

1. Eight visible failures: incorrect action dispatch, missing workflows, incomplete prerequisites, and weak success/readiness checks.
2. False-positive coverage: synthetic completion events and equivalent state edits still stand in for the actions the tutorial teaches. This affects previously passing scenarios as well as the eight failures.

The Notes explicitly say that Deep Solve, Optimize, variant saves, slicing, file operations, and other actions were not shown. In addition, some newly added routes inject events directly without producing those Notes. Turning off `INDICATRIX_VIDEOS_ALLOW_INJECT` alone will not remove all injection.

**Retry status:** not ready for another full release-video run. First implement the strict completion policy and the specific workflows below, then have the user run focused recordings with injection disabled. A larger finding count under strict mode is an honest coverage result, not evidence that the application regressed.

## Changes that are present, and what remains

- `film_command` now uses the row-y helper for repeated table buttons; the previous plan's statement that this helper has no callers is obsolete. It still falls back to a global matching button, and nearest-y selection has no row-boundary requirement.
- The imported-meets lesson now gets an imported ASC fixture in `prepare_design`. This addresses its data prerequisite, but opening it before capture does not demonstrate an Open operation if the lesson asks the viewer to perform one.
- New event/check routes, quick-add handling, and shared recipes have been added. Some perform the wrong workflow or announce an event without performing it.
- Tier-save and input-value repair fallbacks remain. The director still accepts an already skipped step, and manual steps still advance without executing their listed demonstrations.
- The cutting-sheet route now hides its toast. That removes feedback and is not a general pre-frame privacy solution.
- Capture is still initialized for the main Slint window, file choosers still use queued answers, and recorder finalization still precedes the outer health check.

## Diagnosis of the eight failures

All paths below are repository-relative; short video filenames refer to `apps/indicatrix-cut/tests/tutorial_videos/`.

### 1. `tiers-pin-to-mast`, step 2 — the specific runtime failure is not yet proven

Confirmed code defects:

- `perform_routes.rs:445–465` recognizes Pin by inspecting the shape of a generic `TierEntry`: no angle, no indices, and Meets kind 2. It does not express a distinct Pin action.
- It does not wait for the target tier to have a current, certain solved depth. The instructions explicitly require waiting for the solved state first.
- It chooses the nearest visible Pin button, or any Pin button if row lookup fails. If the intended row has no Pin button yet, a different row's button can be chosen.
- `click_and_invoke` reports `true` whenever a handle exists. That proves neither that the control was enabled nor that the requested tier changed. The Pin route returns immediately on that flag.
- The direct `pin_to_mast` callback in `src/gui/editor/callbacks/tier_actions/adopt.rs:183–202` silently returns if no solved mast is cached. It reads the cached entry without verifying its generation matches the current design.
- The reported diagnostic prints row 0, Pavilion Main, whereas this lesson targets Upper Girdle. It cannot tell us which row was clicked, whether Upper Girdle was solved, or what its constraint became.

Fix:

1. Introduce an explicit video action `PinSolvedDepth { tier }`; remove inference from `TierEntry.meets`.
2. Wait for the latest relevant solve/preview generation, and for Upper Girdle's actual Pin control to exist and be enabled. Show that solved state before clicking.
3. Scope the click to Upper Girdle's row, with no global/nearest-other-row fallback.
4. Record the solved mast before the click. Verify that the same tier becomes `ScaleReference` at that mast, that the value did not arbitrarily become `0.75`, and that the Pin button disappears. Preserve its angle/name/indices.
5. Harden the production callback against stale solve data: verify matching design generation and row identity, and report a readiness reason rather than silently doing nothing when pinning is unavailable.

The missing-current-solve and wrong-row paths are plausible explanations of the observed timeout. Capture targeted diagnostics before claiming which one occurred. Do not fix this by increasing the timeout after an ineffective click.

### 2. `tiers-linked-series`, step 1 — linked generation is not verified

`perform_actions.rs:22–86` fills optional fields, optionally toggles Keep Linked, clicks Generate, and checks only whether any tier name starts with `Step`. If ordinary unlinked tiers were generated, `has_step1` suppresses the linked fallback and the actual goal still fails. The goal in shared `tutorials/tiers/series.rs` requires Step1/Step2/Step3 at 30/34/38 degrees and Step2/Step3 linked to Step1.

The report's six-row total is consistent with generation, but it does not show the three Step rows or their relations. A failed checkbox action or field-targeting problem is a hypothesis, not established by row count.

Fix:

1. Scope every field to the Steps / Mirror form, including its Index Positions field. Read back name `Step`, start `30`, increment `4`, count `3`, indices `0:12:96`, and the intended blank anchor.
2. Find Keep Linked by checkbox role and container. Read its checked state; click only if false, then require checked=true before Generate. Do not blindly toggle or ignore a missing checkbox.
3. Click Generate once. Verify three new tier identities, angles and indices, plus relation drivers/offsets: Step2 = Step1 + 4 and Step3 = Step1 + 8; Step1 is undriven.
4. Delete the `has_step1` success heuristic and direct generation fallback from filming. A wrong result must fail with all three generated rows and checkbox/form readbacks.
5. Continue the lesson with a real inline edit of Step1 to 32 and verify 32/36/40, then Remove Relation on Step3 and verify Step2 remains linked.

### 3. `tiers-arithmetic`, step 2 — a definite dispatch collision

The recipe in `guide/perform_table/tiers.rs:255` asks to edit Table's exact scale to the expression `0.5-0.15`. This exactly matches the new Pin heuristic in `film_tier`. The recorder takes the Pin branch and returns without typing the expression or clicking Save Tier.

Table is already an exact-scale tier, so it normally has no Pin button. The route can pin another row or re-pin Table's existing depth through the callback. Neither changes Table to `0.35`.

Fix:

- Remove the heuristic. A scale-form edit must remain a scale-form edit; only the explicit Pin action invokes the Pin workflow.
- Select Table, visibly choose Exact Scale Value, type the literal expression `0.5-0.15`, show it, and click Save Tier. Verify the parsed value is `0.35` and unrelated fields remain unchanged.
- Fix the next step too: the current recipe replaces the indices with `0:6:96`, while the lesson teaches typing `96/16` in the facet-position box and pressing + Add. Film that actual route and verify both rings remain present.
- This collision also affects other scale-only recipes, including the solving-status anchor repair. Their previous passes need rechecking.

### 4. `solving-verdict-fixes`, step 4 — no Add Table Fix route

The step has `Goal::TierExists("Table")`, but neither an appropriate presentation script nor a route exists. `director.rs::without_recipe` rejects that standalone goal, explaining the GAP exactly.

Fix the workflow rather than adding a generic TierExists-to-add-tier shortcut:

1. After removing Table, wait for the edited design's solve/verdict to be current.
2. Open the verdict badge, locate the missing-table reason, and click its **Add a Table** action. The existing `verdict_popover.slint:251–259` exposes a reason-scoped accessible label and routes to `VerdictModel.fix_reason`.
3. Verify the actual Fix applied: Table restored, success feedback, one undoable change, and fresh verdict state. Do not use the quick-add button or direct save instead.
4. Author the subsequent steps now: edit only Crown Main's `12` index to `12.5`, save, solve, open the off-tooth reason, and click **Snap to Teeth**. These Check goals are also not handled by the current generic label dispatcher.

### 5. `solving-variants`, step 5 — earlier saves may never have happened

The report explicitly injects `variant_saved`. `routes.rs` has no actual save-variant route. The earlier steps can therefore advance without persisting Variant 1 or Variant 2.

The failing step checks “the template's own crown and pavilion.” `director.rs::handle_check_goal` only recognizes half-width, detach, and adopt labels; an unknown label returns `Ok(())` without doing anything. There is no route that opens Variant 1. This explains why the original crown is never restored.

Fix the entire save/open chain:

1. Click History → Variants through the real controls.
2. Save the first variant through Save as Variant, name input, and Save. Wait for its persisted row/key. Record its actual identity, not an assumed list position.
3. Change Crown Main to 38 through the taught UI and genuinely save Variant 2.
4. Click Open within Variant 1's row, using the stable key/name. `ui/components/editor_inspector/variants_section.slint` already distinguishes actions with labels such as Open Variant 1; `src/gui/editor/variants/actions.rs::open_variant` is the real apply path.
5. Verify Crown Main is 34.5, Pavilion Main matches the original, the Working from marker identifies Variant 1, and history has an Open variant entry.
6. The next “Undo the open” must press Undo and restore 38; do not derive a fresh angle edit from its TierMatches goal. Then implement the actual compare actions and capture their window.

### 6. `solving-raw-text`, step 2 — the route does not edit or apply text

`routes.rs:502–512` only animates a possible Compare With click and raises `raw_text_diff_shown`. It does not open Edit as Text, edit Crown_Main, choose Snapshot, return to text, or Apply.

There is an additional dispatcher bug: `without_recipe` handles `Goal::All` by switching on each subgoal, but silently ignores unhandled types. Here it ignores `TierMatches(Crown Main, 36)` and later drives only the event. The route's event can be set while the full goal remains false; reporting that the event “did not report” is misleading because the predicate checked was whole-step completion.

Fix:

1. Capture a genuine snapshot first, including the event from the application.
2. Open Edit as Text via its menu or command palette, focus its text editor, locate the Crown_Main tier line, and visibly replace only its angle token `34.5` with `36`. Preserve all other ASC tokens and lines.
3. Wait for the real parse/validation preview; show what Apply will change.
4. Open Compare With → Snapshot, show the added/removed line, then Back to the Text → Apply.
5. Check the comparison event independently when the diff actually opens (`src/gui/editor/raw_text/mod.rs:613`), and check the final design/closed dialog after Apply. The next Undo must restore the original in one history operation.
6. Remove generic decomposition of state predicates into actions. A composite goal verifies a complete authored script; it does not describe the workflow's order.

### 7. `viewing-drag-handles`, step 4 — depth drag is misrepresented as Pin

The shared recipe at `guide/perform_table.rs:171` substitutes `TierEntry::of("Crown Main").meets(2, "0.75")` for dragging D. It hits the same Pin heuristic as arithmetic. Pinning the current mast cannot demonstrate or guarantee a changed depth. The goal `depth_moved` explicitly checks for a constraint/depth differing from the initial design.

Fix:

1. Give angle/depth/index dragging explicit presentation actions. Do not add a scale-form workaround, even though it might satisfy the final state.
2. Select the intended facet/tier with actual pointer input. Read the current handle geometry from `ui/models/manipulate.slint`; transform it through the viewport origin and current layout/scale.
3. Press D, move through recorded intermediate positions along its axis far enough to exceed snapping tolerance, and release. Verify the intended handle was grabbed and the target tier's mast actually changed.
4. Verify one history operation for the gesture. Demonstrate Undo, index drag, snapping changes and cancellation where instructed.
5. Reaudit the passing angle/diagram steps: their shared recipes also substitute numeric tier edits for drags.

### 8. `viewing-slice`, step 6 — the prerequisite slice may not exist

The immediate GAP is an unhandled `TierCountAtLeast` goal with no Keep route. More importantly, the report injected `slice_drawn`, `slice_flipped`, `slice_cuts_stone`, and `slice_symmetric_toggled`. Those events do not create a provisional facet or move it into the stone. Adding a Keep callback alone cannot fix that missing state.

Fix the complete sequence:

1. Switch Slice on visibly, draw a real line on the stone with pointer down/move/up, and verify the provisional tier exists without changing committed tier count/history.
2. Click Flip twice as taught and verify return to the original side. The current one-event goal must not advance after the first click while the second remains unshown.
3. Drag provisional D inward until the cut is valid and Keep is enabled; display the preview and status.
4. Toggle Symmetric off and on, showing both results and retaining the final instructed state. Prevent premature advance after the first toggle.
5. Click Keep in the hint bar. Verify one new committed tier, correct selection, Slice off, provisional state cleared, and one undo entry.
6. Press Undo and verify the prior design/tier count returns. Do not create a substitute tier just to satisfy the count.

## Cross-cutting fixes required before accepting any pass

### A. Replace inferred operations with explicit presentation actions

Create a video-specific script registry, separate from `Perform`, keyed by stable guide/step identity. Start with these operations: PinSolvedDepth, EditTierConstraint, GenerateLinkedSeries, ApplyVerdictFix, SaveVariant, OpenVariant, EditRawText, DragHandle, DrawSlice, and KeepSlice. They execute through shared real input primitives and use read-only observations for assertions.

Keep core `Perform` free of GUI types. Where a shared recipe now misrepresents the in-app “do this for me” action, either implement the real domain operation or remove the automatic recipe until available. Never label a different workflow as the taught operation.

Current examples that must be replaced, even though their videos pass:

- Apply an Optimize candidate → hardcoded Crown Main angle 35.
- Retarget → set Topaz/Sapphire and hardcoded pavilion angle 39/38.5.
- Sweep → hardcoded pavilion angle 42.
- History “Go forward again” → recreate two edits rather than navigate history/Redo.
- Angle/diagram drag → edit a form value.

These are in `crates/indicatrix-editor/src/guide/perform_table.rs:104–189`. They reproduce selected goal states but bypass the features being taught.

### B. Make success independent of injection and fallback mutation

- Reject release recording if `INDICATRIX_VIDEOS_ALLOW_INJECT` is enabled.
- Remove direct `GuideModel.invoke_event` calls from video routes, including unconditional ones in verdict, auto-solve, variants, multi-select, compare, raw-text and snapshot routes. Normal production callbacks should be the only source of those events.
- Remove direct save/generate/model-write repairs and `GuideModel.go_on` fallback execution from filmed actions. Do not silently ignore input/selector errors or discarded `Result`s.
- A handle's existence is not success. Check role, visibility, clipping, enabled state, active surface and the expected actual effect. Require a unique row-scoped target.
- Unknown action/goal handling must return an unsupported-route failure immediately. Delete `handle_check_goal` prose substring routing and the silent default arms in composite-goal dispatch.
- Track action evidence independently from domain goal success. For example: variant row saved, checkbox checked, raw text diff opened, drag released, actual file produced.

### C. Finish progression, coverage, and diagnostics

- Hold guide auto-advance during a filmed step until every authored action and observation is shown and the real goal holds. Retain normal goal evaluation; never force `step_done` or a fabricated event.
- Do not accept “step went by before it could be shown.” Do not stop a script when an intermediate predicate first becomes true. Audit manual steps with real instructions too.
- Preflight the entire 70-guide catalog, including every later step after these eight first failures, before invoking ffmpeg. A static script inventory does not replace runtime checks, but finds absent routes cheaply.
- Replace `row0`-only errors with action-specific diagnostics: target tier key/name/row, selected row, requested and actual constraint/value, current/solved generation, generated Step rows and relations, checkbox state, available variant keys, raw-text validity/diff mode, provisional slice status, active handle and input trace. Include the last safe frame and exact guide/step/action.
- Route-event checks must distinguish “event absent” from “event occurred, remaining state predicate false.” Name the failing member of a composite goal.

### D. Retain the full visual-completeness gates

The previous plan's surface, privacy and publication work still applies:

- Capture actual secondary windows and file dialogs; a queued picker answer or a synthetic exported/saved event cannot count as a full tutorial. No native capture capability has been established by this report.
- Prepare safe paths and preserve useful success feedback; dismissing a toast is not evidence that every encoded frame was safe.
- Validate all actions, chapters, health and outputs before promoting a complete MP4/captions/chapters bundle. Do not publish before the outer health check.
- A report with zero findings must explicitly mean no injected events, no substitute workflows, no skipped actions, and correct observed results. Otherwise retain a diagnostic-only status and do not publish it as release-ready.

## Ordered implementation work

| Order | Changes | Completion evidence |
|---|---|---|
| 1 | Strict release policy, explicit action/script dispatch, focused diagnostics, unknown-route failures | An unsupported step fails immediately; injection and substitute actions cannot qualify as success. |
| 2 | Remove the generic Pin heuristic; implement genuine Pin, constraint editing, and verified linked generation | Correct target/value/solve-generation readbacks; exact expression shown; Step2/3 relations verified. |
| 3 | Implement complete verdict Fix, variant save/open, and raw-text workflows | Correct real persisted/edited state, action evidence, and one-step Undo behavior. |
| 4 | Implement actual handle drags and the entire provisional-slice lifecycle | Pointer trace, expected geometry change, valid Keep, and correct history. |
| 5 | Replace false-pass Optimize/retarget/sweep/history recipes and every synthetic event route | All taught operations are actually executed and displayed; later missing steps found in preflight. |
| 6 | Complete capture surfaces, progression, privacy, and publication gates | Every catalog step/action is captured; output is promoted only after all checks. |

Before route implementation, define the script/input/observation interfaces once so individual fixes do not add more local heuristics. Prefer small modules for tier, variant/text, verdict and viewport workflows instead of expanding the director's goal switch.

## User-run verification and when to retry

No commands below were run for this analysis. The user alone runs tests and Clippy.

Add focused regression coverage as part of implementation for:

- Pin versus exact-scale expression edit versus depth drag being distinct actions.
- Missing/stale solve and wrong-row Pin never being accepted.
- A generated unlinked ladder failing a linked-ladder assertion, even when all three names exist.
- Variant save/open and raw-text diff/apply actually changing persistent/design state.
- Composite goals exposing missing state members without injecting events.
- Provisional slicing requiring a real cut before Keep, then committing/undoing once.
- Unknown routes, hidden inputs, injection settings, and early guide progression failing rather than producing output.

**First focused retry:** after orders 1–2, generate Pin, linked-series, arithmetic, and solving-status. Inspect the actual MP4s: correct row, checkbox, expression input, Save/Pin click, state result. A zero-finding state-only report is insufficient.

**Eight-scenario retry:** after orders 1–4, the engineer can tell the user these eight are ready for targeted validation. Use a new candidate output folder so unchanged old MP4s cannot be mistaken for new results. Existing substring-filter syntax is:

```powershell
# User-run only, after implementation; not a command already executed.
Remove-Item Env:INDICATRIX_VIDEOS_ALLOW_INJECT -ErrorAction SilentlyContinue
$env:UIPROBE_JOBS = 'serial'
$env:INDICATRIX_VIDEOS_OUT = Join-Path (Get-Location) 'videos/candidate-eight-fixes'
cargo tutorial-videos -- tiers-pin-to-mast tiers-linked-series tiers-arithmetic solving-verdict-fixes solving-variants solving-raw-text viewing-drag-handles viewing-slice
```

Unsetting the flag is necessary but insufficient until unconditional injections are removed from source. These recordings may reveal later steps that the old run never reached; inventory and implement them before declaring the tutorial fixed.

**Full retry:** only after all 70 scripts pass completeness preflight, every needed surface can be recorded, and the release path cannot inject events or substitute final-state edits. Run the full suite, then review the videos for visible inputs, readable pacing and continuity. Include previously passing optimization, history, file, material and library videos in that review.

The final handoff should say either “ready for your targeted validation” or “ready for your full validation,” with the exact coverage and limitations. This updated plan alone does not make either claim.
