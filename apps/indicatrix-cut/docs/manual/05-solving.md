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

The old stack of separate banners is gone. In its place is one **status
strip** running the width of the Edit tab: always a single coloured dot
plus one line, and a **Log** link at the right that opens every current
message in full. The solver's own state always leads there — see "The
status strip's priority order" below — so a running or failed solve can
never end up hidden behind something less important.

| Message | Meaning | What to do |
|---|---|---|
| `Closed solid -- volume X.XXXX.` | Success. A finite, watertight stone. The volume figure (in the app's internal model units, cubed) is shown alongside. | Nothing — proceed to check the Cutting Schedule, run Deep Solve/Optimize, or export. |
| `<Block> has no anchor: add a tier with an exact scale value.` (one such sentence per missing block, e.g. two sentences if both crown and pavilion are missing one) | One or more of Crown/Pavilion/Girdle has no anchor tier at all (Chapter 3). | Add at least one tier of kind **Exact scale value** to that block, then Solve again — or use the **Add Anchor** button the tier table now shows on that block's own rows (Chapter 3). |
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
above does not catch, such as an index that does not land on an achievable
gear-tooth position. A warning of that kind reads along the lines of:

> tier N (name): index requested does not land on a gear tooth; nearest
> achievable is &lt;value&gt; (&lt;error&gt; deg azimuth error)

These warnings are informational — they do not block Solve or export — but
they flag rows worth revisiting before you cut the stone for real. Each
warning also badges its own tier's row with a small ⚠ glyph in the tier
table (Chapter 3), so you do not have to open Log to see which rows are
affected. Like the status strip, they go stale (cleared, not left showing
an outdated result) the instant you make another edit, and only reappear
after the next Solve.

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
