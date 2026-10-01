# indicatrix-editor

GUI-free faceting-design editor logic, shared by the desktop editor and the wasm web
app.

`indicatrix-editor` is everything the CAD editor decides that is not UI glue: how an
edit, undo, redo or angle nudge changes a design; how the tier table, yield figures,
proportion verdicts and validation banner read; how a form's text parses into an
edit; what a Retarget proposal or a cutting sheet contains; and when a solve should
run. The desktop (`apps/indicatrix-cut`) and the web app (`apps/indicatrix-web`)
both call into it, so the same input gives the same design, the same rows and the
same bytes in both. It holds no GUI toolkit types, no threads, no clock and no
filesystem access, so it links unchanged into a `wasm32-unknown-unknown` build.

## Main types

- **`EditorSession`** — the design under edit, its undo/redo `History`, the tier
  multi-selection, and the generation/saved-generation pair behind "unsaved
  changes". `apply`, `apply_coalescing`, `undo`, `redo`, `nudge_angles`,
  `complete_orbit`, `mirror_indices` and `apply_optimize_outcome` return what
  changed (`EditChange`). Nudge coalescing takes a **caller-supplied timestamp** (a
  `Duration` since any fixed origin) instead of reading a clock: the desktop passes
  an `Instant`-derived duration, the web app passes `performance.now()`.
- **`loading`** — the tier form (including the `start:step:stop` and `base xN`
  index shorthands), preform, New Design and material forms, the RI-preservation
  rule, and a design from bare `.asc` text.
- **`manipulate`** — the GUI-free math of mouse-driven editing: the pick-frame camera
  projection, the angle/depth/index drag handles of the selected facet (`handle_layout`,
  `hit_test`, `drag_value`, one undo step per drag through
  `EditorSession::{set_tier_angle, pin_tier_mast, rotate_tier_indices}`), the tiers a
  drag drags along, the Slice tool's plane snapping (`slice_normal`, `snap_to_gear`,
  `slice_tier`), and the hint and toast wording both apps share.
- **`view_model`** — plain structs (`TierRow`, `IndexChip`, `CuttingRow`) and the
  builders that fill them, plus the yield/proportion texts, proportion verdicts and
  the validation-banner text. Each UI maps the structs to its own row types.
- **`material`** / **`material_lookup`** — the material combo and its cache, the
  RI-source text, gear presets and the gear-remap preview; the custom-over-built-in
  `EditorMaterialLookup` and the nearest-built-in search.
- **`optimize_view`**, **`retarget`**, **`cut_sheet`**, **`guide`** — Optimize's
  result tables and weight parser; Retarget proposals and their view model; the
  cutting-sheet HTML and diagram PNG bytes (via `indicatrix-solid`'s `diagram2d`);
  the guided walkthrough's steps and completion predicates.
- **`solve_policy`** — when to solve synchronously, auto-solve eligibility and
  debounce, the banner texts, the cancellable solve and the plane-cap diagnosis.

## Not in this crate

Threads, timers, file dialogs, the file system, the SQLite design library, Deep
Solve, and every toolkit model push stay with each caller. The desktop keeps its
`EditorState` (a wrapper around `EditorSession` that adds the Deep Solve/Optimize
handles and file bookkeeping), its Slint row adapters, and re-exports at the old
module paths so the rest of the desktop app compiles unchanged.
