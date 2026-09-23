# 5. Solving

## What you will do

This chapter explains what Solve actually does, what each status message
means, and how to fix the failures you will actually run into.

## What Solve does

Click **Solve** (or press **F5**) to run a full, live meet-point solve: the
app works out the mast (solved depth) of every tier from its stated
constraints, and builds the resulting solid to check that it is a real,
closed stone. This can take real time — from under half a second on a
small design to several seconds on a large one (over a hundred tiers).
Every other edit in the app (Save Tier, Remove, Apply Preform, Undo, Redo)
intentionally skips this and just marks the design **stale** instead.

Above 16 tiers, Solve runs in the background rather than freezing the
editor: the button relabels to **Solving...** (disabled until it finishes)
and the status strip ticks forward with elapsed time, e.g.:

> Solving... (103 tiers) -- 2.3s elapsed

You can keep editing other parts of the design while this runs. If you make
an edit that changes the design before the background solve finishes, its
result is discarded rather than applied over your newer edit — the strip
already shows the correct "not solved" state for what you just changed, and
you can Solve again once you're ready. At 16 tiers or under, Solve still
completes immediately, matching the app's original behaviour.

While stale, the status strip reads:

> Not solved -- click Solve to compute masts and validate this design.

and the tier list's MAST/SOLVE columns show placeholders rather than
numbers that might no longer be true.

## Auto-solve: skipping the click on small and medium designs

The auto-solve control next to the Solve button (Off / 150ms / 300ms /
1s / 3s) lets small and medium designs skip the manual Solve click
entirely. It carries no caption of its own on screen -- only a hover hint
-- so if you have not read this section, it is easy to walk past without
knowing what it does. After any edit, if this design's *last measured*
solve took less
than the selected budget, a fresh solve is scheduled automatically — after
a short pause (so a burst of keystrokes only triggers one solve, not one
per keystroke) — and the tier list, status strip, and viewport update on
their own once it completes, exactly as if you had clicked Solve.

- **Off** disables this entirely: edits only ever mark the design stale, the
  same behaviour the app has always had.
- Any other setting is a *ceiling*, not a guarantee: it is judged against
  THIS design's own last real solve time, which starts unmeasured on a
  fresh or freshly loaded design (auto-solve is tried optimistically until
  the first real measurement comes in) and updates after every solve,
  background or manual.
- Once a design's own solves exceed the selected budget, auto-solve
  switches itself off for that design and says so in the strip:

  > Auto-solve off for this design: last solve took 5.9s.

  Solve still works normally by hand; only the automatic trigger stops.

This setting is remembered across sessions.

## Reading the status strip after Solve

One **status strip** runs the width of the Edit tab: always a single coloured
dot plus one line, and a **Log** link at the right that opens every current
message in full. The solver's own state always leads there — see "The
status strip's priority order" below — so a running or failed solve can
never end up hidden behind something less important.

| Message | Meaning | What to do |
|---|---|---|
| `Closed solid -- volume X.XXXX.` | Success. A finite, watertight stone. The volume figure (in the app's internal model units, cubed) is shown alongside. | Nothing — proceed to check the Cutting Schedule, run Deep Solve/Optimize, or export. |
| `<Block> has no anchor: add a tier with an exact scale value.` (one such sentence per missing block, e.g. two sentences if both crown and pavilion are missing one) | One or more of Crown/Pavilion/Girdle has no anchor tier at all (Chapter 3). | Add at least one tier of kind **Exact scale value** to that block, then Solve again — or use the **Add Anchor** button the tier table shows on that block's own rows (Chapter 3). |
| `Degenerate: only N distinct vertex(es), volume <value or "non-finite"> -- check tier 5 (Girdle), tier 8.` | The facets you have described do bound a region, but it isn't a valid solid — too few real corners, or a volume that comes out zero, negative, or not a finite number. The trailing "check ..." clause, when the app can tell, names the tier(s) most likely responsible. | Check the named tier(s) first; otherwise, two or more facets meeting somewhere they shouldn't, or a wrong angle/constraint, is the usual cause. |
| `Unbounded: tier 3 (P1) never close the solid.` (one tier, or several by name, separated by commas) | One or more facet planes never actually meet enough others to close the stone off in some direction — geometrically, the shape "leaks" to infinity along that plane. In practice this should be rare once every block has an anchor. | Check the named tier(s) — a missing meet partner, or an angle so shallow/steep it fails to intersect its neighbours, is the usual cause. |

A message naming a missing anchor, or reading "Degenerate..." or
"Unbounded..." is shown as a problem (styled in red); "Closed solid" is
shown as success (green).

### The status strip's priority order

Only one message shows in the strip at a time, in this order — a running
solve or a real problem always wins, so you can never mistake a stale or
broken design for a current, working one just because nothing else
happened to be flagged:

1. **Solving...**, while a background solve is in progress.
2. A real validation problem: a missing anchor, Degenerate, or Unbounded —
   this also covers the plain "Not solved -- click Solve..." staleness
   marker below.
3. Manufacturability warnings, once there is at least one and nothing
   above applies.
4. A rough-fit warning (the design no longer fits its own preform), once
   there is one and nothing above applies.
5. **Closed solid**, once the design genuinely solves and nothing above
   applies.
6. Deep Solve's own in-progress or verdict message (Chapter 8).
7. The catalogue material-suggestion banner (Chapter 6), last of all.

Click **Log** at the right of the strip to see every one of these — the
validation message, Deep Solve's status, every manufacturability warning
in full, the rough-fit note, and the material suggestion — at once, rather
than only the single highest-priority line. Nothing the strip has no room
to show is ever unreachable, only one click away.

## Manufacturability warnings

Alongside the status strip, Solve also refreshes a list of
manufacturability warnings — practical cutting problems the geometry check
above does not catch. Every message is prefixed `tier N (name): ...`
(N is the tier's position in the schedule). There are exactly five checks;
the first two only run once the design is a closed solid, the other three
always run:

| Check | Fires when | Exact message |
|---|---|---|
| Vanishing facet | A later tier's cut removes this facet's plane from the solid entirely — it contributed nothing to the final shape. | `tier N (name): X/Y facet(s) cut away entirely by a later tier` |
| Undersized facet | The surviving facet's area is below about 1% of the stone's width (linear). | `tier N (name): facet spans only X.XX% of the stone's width, below the Y.YY% minimum` |
| Index off the gear | The tier's authored index does not land on a real gear tooth. | `tier N (name): index X does not land on a gear tooth; nearest achievable is Y (+Z.ZZZZ deg azimuth error)` |
| Meets a later tier | A **Named facet(s)** reference points at a tier that is not cut until later in the schedule (or at itself). | `tier N (name): meets tier M (target name), which is not cut until later in the schedule` |
| Meet name not export-safe | A **Named facet(s)** target's own name contains characters (spaces, commas, semicolons, leading/trailing punctuation) that would not survive a plain `.asc` export/re-import round trip. | `tier N (name): meet target name(s) "..." would not survive a plain .asc export/re-import -- rename without spaces, commas or semicolons, and without leading/trailing punctuation` |

These warnings are informational — they do not block Solve or export — but
they flag rows worth revisiting before you cut the stone for real. Each
warning also badges its own tier's row with a small ⚠ glyph in the tier
table (Chapter 3), so you do not have to open Log to see which rows are
affected. Like the status strip, they go stale (cleared, not left showing
an outdated result) the instant you make another edit, and only reappear
after the next Solve.

## Abandon Solve

While a solve is running in the background, the Solve button's row shows
an **Abandon** button. Clicking it stops the solver within a sweep
or pipeline run — typically a few milliseconds, on the order of 1 ms —
not just discard the eventual answer client-side: `dispatch_background_solve`
threads a `SolveControl::with_cancel` through `Design::solve_with`, the
same real mid-run cancellation Deep Solve and Optimize already use for
their own Cancel buttons (Chapter 8). The status strip goes back to
**Stale** immediately, on the same click, and the status strip's
activity list (the small chips showing whatever is currently running)
drops the entry right away too.

The button is named "Abandon" rather than "Cancel" for one honest
reason: it discards whatever partial progress the solver had made,
exactly like Cancel Deep Solve/Cancel Optimize — the difference from
those two is only that a plain `Design::solve` has no partial RESULT to
keep even if it wanted to (Deep Solve's own search can report a
best-so-far; a meet-point solve is all-or-nothing), so "Abandon" reads
slightly more accurately for this one button. Functionally, all three
Cancel/Abandon buttons stop their worker immediately.

## Activity strip and cancelling

Every operation in this app that takes more than about a fifth of a second
appears as a small chip in the status strip, next to the message
described above — not just Solve. Deep Solve, Optimize, Retarget's own
Optimize search, computing the Tilt Performance curves, exporting a tilt
video, and the live path-traced view all register here while they run, so
you can always see at a glance what the app is doing, for how long, and
whether it has real progress to report.

Each chip shows:

- The operation's label and how many seconds it has been running.
- A thin progress bar when the operation can actually measure a completion
  fraction (Optimize's evaluation count, a tilt video's frames rendered) —
  most solves cannot report a fraction at all (a meet-point solve is
  all-or-nothing until it finishes), so those chips instead show a small
  pulsing dot: "running," with no promise of how much longer.
- A **✕** cancel button, where the operation actually has something real to
  stop. Clicking it is exactly the same as clicking that operation's own
  dedicated Cancel/Abandon button — from the strip, from the Deep Solve/
  Optimize panel, or from the Retarget/Tilt Performance dialog, cancelling
  stops the same real worker either way.

A camera drag or orbit in the live 3D view never gets its own chip — only a
genuine full-quality trace does, and only once accumulation has actually
begun; the chip disappears the moment that trace converges.

## The status strip's small state label

Separately from the one-line message described above, the status strip
also carries a small coloured state word of its own: **Stale**,
**Solving**, **Failed**, or **Solved**. This is a coarser signal than the
message text — for instance, every validation failure (missing anchor,
Degenerate, Unbounded) shows the same **Failed** state label even though
the message itself names the specific problem — so read the message for
the actual cause and treat the state label only as an at-a-glance summary.

## Why Solve does not run on every keystroke

If every edit triggered an immediate solve, a multi-second solve on a large
design would freeze the editor after every single tier edit. The app never
does that: a solve only ever runs in response to the explicit Solve button,
or auto-solve's own debounced, budget-gated trigger above — and either way,
it runs in the background rather than blocking the UI once the design is
large enough for that to matter. Until a fresh solve completes, the app
keeps the last *solved* geometry visible in the viewport and tier list
rather than showing something silently out of date — so what you see is
always either a real, trustworthy solve, or clearly marked as not current
("Not solved..." or "Solving...").

## Next steps

Continue to Chapter 6 if you need to adapt a design for a different
material's refractive index, or Chapter 7 for a full worked example of
building a design from nothing.
