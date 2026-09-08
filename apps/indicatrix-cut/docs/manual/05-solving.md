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
and the status banner ticks forward with elapsed time, e.g.:

> Solving... (103 tiers) -- 2.3s elapsed

You can keep editing other parts of the design while this runs. If you make
an edit that changes the design before the background solve finishes, its
result is discarded rather than applied over your newer edit — the banner
already shows the correct "not solved" state for what you just changed, and
you can Solve again once you're ready. At 16 tiers or under, Solve still
completes immediately, matching the app's original behaviour.

While stale, the status banner reads:

> Not solved -- click Solve to compute masts and validate this design.

and the tier list's MAST/SOLVE columns show placeholders rather than
numbers that might no longer be true.

## Auto-solve: skipping the click on small and medium designs

The **AUTO-SOLVE** control next to the Solve button (Off / 150ms / 300ms /
1s / 3s) lets small and medium designs skip the manual Solve click
entirely. After any edit, if this design's *last measured* solve took less
than the selected budget, a fresh solve is scheduled automatically — after
a short pause (so a burst of keystrokes only triggers one solve, not one
per keystroke) — and the tier list, status banner, and viewport update on
their own once it completes, exactly as if you had clicked Solve.

- **Off** disables this entirely: edits only ever mark the design stale, the
  same behaviour the app has always had.
- Any other setting is a *ceiling*, not a guarantee: it is judged against
  THIS design's own last real solve time, which starts unmeasured on a
  fresh or freshly loaded design (auto-solve is tried optimistically until
  the first real measurement comes in) and updates after every solve,
  background or manual.
- Once a design's own solves exceed the selected budget, auto-solve
  switches itself off for that design and says so in the banner:

  > Auto-solve off for this design: last solve took 5.9s.

  Solve still works normally by hand; only the automatic trigger stops.

This setting is remembered across sessions.

## Reading the status banner after Solve

| Banner text | Meaning | What to do |
|---|---|---|
| `Closed solid. -- volume X.XXXX` | Success. A finite, watertight stone. The volume figure (in the app's internal model units, cubed) is shown alongside. | Nothing — proceed to check the Cutting Schedule, run Deep Solve/Optimize, or export. |
| `Cannot solve: no scale-reference tier for: <block(s)>` | One or more of crown/pavilion/girdle has no anchor tier at all (Chapter 3). | Add at least one tier of kind **Exact scale value** to each named block, then Solve again. |
| `Degenerate: only N distinct vertex(es), volume <value or "non-finite">` | The facets you have described do bound a region, but it isn't a valid solid — too few real corners, or a volume that comes out zero, negative, or not a finite number. | Usually means two or more facets are meeting somewhere they shouldn't, or a tier's angle/constraint is wrong. Check recently edited tiers, especially ones sharing an anchor or a named-facet reference. |
| `Unbounded: plane(s) [<indices>] never close the solid.` | One or more facet planes never actually meet enough others to close the stone off in some direction — geometrically, the shape "leaks" to infinity along that plane. In practice this should be rare once every block has an anchor. | Check the facet(s) at the listed plane index/indices — a missing meet partner, or an angle so shallow/steep it fails to intersect its neighbours, is the usual cause. |

A message reading "Cannot solve..." or "Degenerate..." or "Unbounded..." is
shown as a problem (styled in red/amber); "Closed solid." is shown as
success.

## Manufacturability warnings

Alongside the status banner, Solve also refreshes a list of
manufacturability warnings — practical cutting problems the geometry check
above does not catch, such as an index that does not land on an achievable
gear-tooth position. A warning of that kind reads along the lines of:

> tier N (name): index requested does not land on a gear tooth; nearest
> achievable is &lt;value&gt; (&lt;error&gt; deg azimuth error)

These warnings are informational — they do not block Solve or export — but
they flag rows worth revisiting before you cut the stone for real. Like the
status banner, they go stale (cleared, not left showing an outdated result)
the instant you make another edit, and only reappear after the next Solve.

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
