# 8. Deep Solve, Optimize, Adopt, and Apply

## What you will do

This chapter covers the editor's four more advanced tools: verifying a
solve against the catalogue's own printed proportions (Deep Solve),
searching for better tier angles (Optimize), converting an imported tier
back to real meet geometry (Adopt), and committing an Optimize result
(Apply). It also covers why an imported tier's "mast error" blocks
Optimize, and how to fix it.

## Why every imported tier starts pinned

When you load a real `.asc` file into the editor (Chapter 3), the app pins
**every** tier to its exact recorded depth — even a tier whose file
actually said "meets facets P1, P2." This is deliberate: it reproduces the
source file's geometry exactly, with no risk of a re-solve drifting from
the original the moment you open it. Internally, an import measured on the
app's own reference corpus found that trusting meet-point geometry alone,
instead of pinning, would have silently moved the geometry of roughly nine
designs in ten the moment someone clicked Solve — only about 11% of
designs had every meet-derived tier land within 10% of its real recorded
mast. Pinning avoids that risk entirely, at the cost that a freshly loaded
design has **zero free tiers** — nothing the solver derives, and nothing
Optimize can move on its own — until you tell the app to trust a tier's
real geometric meaning instead of its pinned number. That is what Adopt is
for. (Optimize can also turn pinned tiers about their girdle edges when you
tick **Vary anchored tiers**; see "Optimize," below.)

## Adopt

A tier's **IMPORTED** column (Chapter 3) shows an **Adopt** link whenever
the source file's own meet instruction is still recoverable — the app
keeps this instruction (internally, `imported_meet`) alongside the pinned
value from the moment of import specifically so it can be offered back to
you later, one tier at a time. Click it to convert that one tier from
"pinned depth" back to the file's actual geometric intent (for example,
"meets P1, P2" instead of an exact number). Its tooltip reads: "File says
this tier ‹meets ...›. Click Adopt to switch to it so the solver and
Optimize can move this facet."

Two more buttons adopt several tiers at once, both as a single Undo step:

- **Adopt all** — adopts every tier in the design that still has a
  recoverable imported meet. Tooltip: "Adopt every tier's imported meet at
  once, as one Undo step."
- **Adopt sel.** — adopts only the tiers currently selected (Ctrl-click or
  Shift-click, Chapter 4). Tooltip: "Adopt the imported meet of every
  selected tier, as one Undo step."

**Adopt re-solves immediately.** Unlike every other tier edit in this app,
clicking any of the three Adopt actions does not leave the panel stale —
each one runs a full solve right away and keeps the status strip showing
the design's real, current state. This is deliberate: the value being
adopted is exactly what the tier already showed after the design's last
real solve, so re-solving here pays the same whole-schedule cost a normal
Solve always pays, not a new one.

### Why a mast may differ from the file's after adopting

Before you adopt a tier, its mast is a stored number — read straight from
the file, byte for byte. The moment you adopt it, that number stops being
authoritative: the tier's constraint becomes a real meet ("meets P1, P2"
or an unspecified vertex), so the solver derives its mast
from where that facet's plane actually meets its neighbours,
the same as any other free tier. Two things follow from that:

- **Floating-point differences.** A freshly solved meet-vertex value is a
  different numeric pathway than reading the file's own recorded mast
  field verbatim, so the two can differ at the level of rounding even when
  nothing else about the design has changed.
- **Genuine geometric drift.** Because the adopted tier's mast is
  derived from its neighbours rather than pinned, any later edit to a
  neighbouring tier's angle, indices, or constraint — including adopting
  or optimizing a *different* nearby tier — can change where that meet
  vertex actually lands, and so change the adopted tier's live mast. This
  is real geometry moving, not noise: it is the whole point of adopting a
  tier, so the solver and Optimize are free to move it.

If you want a tier to stop moving again once you like where it landed,
use **Pin** (Chapter 3) to freeze its current solved mast back into an
exact scale-reference anchor — the reverse of Adopt.

## Trusting the SOLVE column, and fixing "mast errors"

Chapter 3's "Trusting the SOLVE column" table lists every label this
column can show. Two of them —**Least-squares est.** and **FAILED
(untrusted)**, both shown in bold amber — are what this chapter means by a
tier having a "mast error": the solver either had to fall back to a
per-block estimate because no real meet-vertex candidate existed, or could
not produce even that. A tier in either state, or still showing **not
solved** / **blocked** / **no anchor yet**, has an *uncertain* solve
strategy, and Optimize will not move it (see below) until it is fixed.

To fix an uncertain tier:

- **No anchor yet** — its own block has no Exact-scale-value tier at all.
  Use the row's **Add Anchor** button (Chapter 3) or Chapter 4's Girdle
  Facet Preset, then Solve.
- **Blocked** — a *different* block is missing its anchor. Fix that block
  first; this tier needs nothing done to it directly.
- **Least-squares est. / FAILED** — the tier's own **Meets** constraint
  does not resolve to a real vertex: a **Named facet(s)** reference names
  a facet that does not actually intersect this one where you expect, or
  the tier's angle/indices put its plane somewhere with no usable meet at
  all. Open the Tier tab (Chapter 4), check the facet names and angle, and
  correct them — or, if you are confident in the current solved position,
  **Pin** it as an exact anchor instead of chasing a meet.

## Deep Solve

**Deep Solve** is a separate, much slower, external verification pass — it
answers "does this design's geometry actually reproduce the proportions
printed in the catalogue?" rather than "does this design close?"

- It compares candidate mast solutions against the design's own printed
  proportions from the catalogue: **Vol/W³** (volume over width cubed),
  **L/W** (length/width), **C/W** (crown/width), **P/W** (pavilion/width),
  and **H/W** (height/width) — the same five figures a Deep Solve run's
  own log prints, e.g. "Vol/W3 0.6013".
- It reports a verdict plus a report of exactly what it changed:

  | Field | Meaning |
  |---|---|
  | Initial score | Combined deviation from the printed proportions before any repair. |
  | Score after calibration | Deviation after a coarse anchor-calibration pass (equal to the initial score if nothing adjustable was found). |
  | Final score | Deviation of the configuration actually returned. |
  | Accepted | Whether the final score is within verification tolerance — a real pass/fail. |
  | Overrides applied | How many individual vertex-level decisions the search changed. |
  | Anchor moves applied | How many anchor (scale-reference) values the search adjusted, counted separately from overrides. |
  | Pipeline runs | How many full solves the search tried in total (1 means the plain solve was already accepted, or nothing was found worth trying). |

  The verdict line itself reads either "**ACCEPTED** — reproduces the
  printed figures to verification accuracy (not a correctness proof, only
  a strong external signal)" or "**not accepted** — still deviates from
  the printed figures."
- **It never changes the design on its own.** Deep Solve is a read-only
  diagnostic; it reports and suggests, and nothing it finds is written
  back to the tier list unless you separately make the same change
  yourself, or use Pin (below).
- It runs off the main thread and can take real time — a mean of roughly
  68 solves per design on the app's own reference corpus, which can mean
  minutes on a large design.
- **Clicking Cancel abandons only the on-screen wait.** The background
  computation keeps running to completion regardless; you simply stop
  waiting for its result, which is then discarded rather than shown.

Deep Solve greys itself out, with the specific reason shown right on the
button when you hover it, in exactly two cases:

1. **No printed proportions to check against** — a brand-new design, or
   one loaded from a reconstructed schedule with no catalogue proportions:
   "Deep Solve needs this design's printed proportions (Vol/W^3, L/W, C/W,
   P/W, H/W) from the catalogue to verify against -- unavailable for a new
   or placeholder-reconstructed design."
2. **Nothing to repair** — every tier is already pinned to its recorded
   mast (nothing has been Adopted yet): "Every tier is currently pinned to
   its recorded mast, exactly as imported -- Deep Solve has nothing to
   repair until you convert a tier to a meet constraint (the tier list's
   Adopt action)."

If the design was edited since it was loaded, the log also appends a
caveat that the printed-proportion targets "may no longer describe the
design you are holding" — worth rereading before trusting an old verdict
against a design you have since changed.

### Pin to verified mast

Once Deep Solve has run, its Details popup lists any tier whose mast it
adjusted, each with its own small **Pin** button. Clicking it converts
that one tier's constraint to an exact scale-reference anchor at Deep
Solve's own verified mast value — a real, undoable edit, and the tooltip
says so plainly: "Pin this tier to Deep Solve's verified mast, as an exact
scale-reference constraint. A no-op (with an explanatory toast) once this
run is stale." If the design has changed since that Deep Solve run
finished, clicking Pin does nothing but tell you to re-run Deep Solve
first — it never pins a value that might no longer be the right one.

## Optimize

**Optimize** is a coordinate search over the angles of the tiers it is
allowed to move. It looks for a few different good designs, ranks them, and
shows you each one next to the design you started with. Nothing changes in
your design until you click **Apply**.

Everything is on the inspector's own **Optimize** tab, from top to bottom:
choose what to favour, say what may change, run, and pick a result. A small
**?** button at the right of the tab row opens this section.

**In the Simple interface** (Chapter 17) the tab shows only what most designs
need: the **Objective** list (the seven presets; **Custom** appears only when it
is already chosen), the **Optimize** and **Cancel** buttons with the time
estimate and progress bar, and the whole candidate list with **Preview**,
**Compare** and **Apply**. The "What may change" ticks, the angle ranges, the
Budget, Starts, Seed and Candidates fields and Polish are hidden, not removed: they
keep their values, the search uses them, and nothing about a run changes. When
any of them is not at its default, the tab says "Some advanced settings are in
use. Switch to Advanced to see them." The rest of this section describes the
Advanced tab.

### Objective

The **Objective** list picks what the search should favour. Each choice sets
the same four weights; they only differ in which one counts most. One line
under the list says what the chosen entry favours.

| Objective | Windowing | Extinction | Tilt brilliance | Yield | Tone |
|---|---|---|---|---|---|
| Balanced | 1 | 1 | 1 | 0 | - |
| Brilliance | 1 | 1 | 4 | 0 | - |
| Low windowing | 4 | 1 | 1 | 0 | - |
| Low extinction | 1 | 4 | 1 | 0 | - |
| Keep weight | 1 | 1 | 1 | 3 | - |
| Lighten dark rough | 1 | 1 | 1 | 0 | 3 lighter |
| Intensify pale rough | 1 | 1 | 1 | 0 | 3 deeper |

Windowing and extinction score lower-is-better; tilt brilliance scores
higher-is-better. **Keep weight** also makes the weight of the finished
stone count, so the search prefers angles that leave less of the rough
behind (the same "how much of the preform is being thrown away" figure the
Preform tab's Volumetric Yield reports).

The last entry, **Custom**, shows the three weight fields, the **Yield
weight** slider (`0` to `3`) and the **Tone** slider (`-3` to `3`: left pulls
the face-up colour lighter, right deeper and stronger, `0` leaves the colour
out) so you can set them yourself. A weight is any
positive number, and higher matters more relative to the others. The fields
accept a calculation such as `1 + 0.5`. Choosing a preset first and then
**Custom** starts you from that preset's numbers.

#### Lightening dark rough and intensifying pale rough

The face-up **tone** is the colour of the light that comes back out of the
table, looking straight down at the stone. It is worked out from the body
colour in Design settings and how far the light travels through it, under the
lighting preset the Live Render shows when you click **Optimize**. So
Incandescent and Daylight can rank the candidates differently, and the swatch
is that light's colour as your screen shows it. The UV lamp presets use
daylight for the tone.

- **Lighten dark rough** looks for the angles that return the lightest colour
  (higher `L*`). Pick it when the rough is so dark that the stone looks black.
- **Intensify pale rough** looks for the angles that return the strongest
  colour (higher `C*`). Pick it for pale material such as aquamarine.
- In **Custom**, the **Tone** slider mixes either goal in with the other
  weights.

A stone's colour depends on its size, so set the **girdle diameter** in
Design settings: the search sizes the body colour by it, like the Live Render
does. A line under the objective says which size is used, or that none is set.
A material with no body colour gives the objective nothing to do, and the same
line says so.

The candidate list gains a **Tone** column (a dot in the predicted colour and
the lightness `L*`), and the picked candidate shows two tone rows and a before
and after swatch pair. Green marks the figure the chosen goal pulls on. The
limits: the tone is measured table-up only, and the physical colour editor's
recipes work the same way but are not needed.

### What may change

Three tick boxes decide which tiers the search may move.

- **Vary anchored tiers (keeps each facet's girdle edge)** — a tier pinned
  to a scale value normally stays put (Chapter 3). With this ticked, the
  search may turn a pinned tier too, about the edge where that facet meets
  the girdle, so the girdle outline stays the same while the angle
  changes. The tick starts **on** for a design where every tier is pinned,
  which is every freshly imported design, and **off** for a design that
  already has free tiers. Once you tick or untick it yourself, the app
  stops choosing for you.
- **Keep the girdle (at least half its thickness)** — on by default. A
  candidate whose girdle would become thinner than half of the girdle you
  started with, or that would lose the table facet, is thrown away before
  it can be ranked.
- **Only selected tiers** — off by default. When it is on *and* at least
  one tier is selected in the tier list, only the selected tiers may move.
  With the box ticked but nothing selected, Optimize moves every tier it
  could move, the same as with the box unticked.

A tier whose angle follows a relation (Chapter 4, "Relations between
tiers," for example `P1 - 2`) is never moved by the search itself. It goes
along with the tier it follows: every candidate is scored with the relations
worked out, and applying a candidate moves the followers too.

A horizontal tier (the table, the culet) and a vertical one (the girdle) are
never moved: at or beyond about 89.5° a facet is vertical by definition, so
there is nothing to optimize on it.

### Angle ranges

Each tier the search may move can change by `5°` either way, never past
`0.5°` or `89.5°`. Press **Angle ranges...** to see them. The table lists
the tier number, its name, its current angle and two fields, **MIN** and
**MAX**. Type your own limits to make a range narrower or wider; **Reset
ranges** puts the defaults back.

- Write the angles the way the tier list shows them: plain positive numbers,
  flattest first (`36` to `46` for a pavilion tier at 41°, which is also how
  the table starts). The sign is optional, because a tier stays on its side
  of the girdle: `40` and `-40` mean the same for a pavilion tier.
- The fields accept a calculation such as `41.5 + 0.5`. Text that is not a
  number stops the run with a message naming the tier and the field.
- The two ends may be typed in either order. A range that would leave the
  tier's current angle outside it is widened to include it.
- A tier that follows a relation is listed with "follows a relation" and no
  fields. The line above the table counts how many tiers can change and
  how many follow a relation.
- The ranges you typed belong to the tiers shown. If the list of tiers the
  search may move changes (you tick **Vary anchored tiers**, add a tier, or
  load another design), the table starts again from the defaults.

### Run

- **Budget** — the evaluation budget, `800` by default (it was `200` before
  the search learned to use several starting points). A blank field means
  `800`. It accepts a calculation such as `100 * 4`, and the largest budget
  is `100000`.
- **Starts** — how many different starting arrangements the search tries,
  `1` to `32`, `8` by default. A blank field means `8`; `1` is the plain
  single search. See "Several starting points," below.
- **Seed** — the search seed, `0` by default, for a reproducible run: the
  same seed on the same design gives the same result. A blank field means
  `0`. It must be a whole number, `0` or more.
- **Candidates** — how many different good designs to keep, `1` to `5`,
  `3` by default. Each extra candidate costs one more full-fidelity scoring
  at the end (see "How fast is it," below).
- **Polish** — a checkbox, on by default. Leaves the search's second
  (simplex) stage enabled; see "How the search runs," below. Turning it
  off disables that stage entirely.

Under the fields a line gives **Estimated time**. Before your first run on a
design it is a rough guess from the number of tiers; after a run it is
worked out from how fast that run actually went, so the next estimate gets
better.

**Optimize** starts the run and **Cancel** appears beside it while it
goes. A thin bar under the buttons shows how far the search has got. A text
in a budget, seed or range field that cannot be read stops the run before it
starts, with a message naming the field.

While a run with several starts goes, the status line says which start it is
on and the best score so far, for example "start 3 of 8, best 12.41, 412 of
~983 evaluations". While the app is choosing its starting points it says
"screening starting points" first. When the run ends, the status line names
how many starts ran and which one gave the best result ("across 8 starts (the
best came from start 4)"); start 1 is the search from your own design.

**There is no Fast/Full "fidelity" control in the UI.** The search always
uses a fast, single-pose evaluation internally while it searches, and
always brackets the whole run with one slow, full-fidelity evaluation
before and one after — see "How fast is it," below. This split is fixed
and not something you choose.

### How the search runs

The first stage is a coordinate search: it moves one free tier's angle at
a time, scored under a fixed canonical light pose (not necessarily the
light pose the viewport currently shows — drag the light and these
figures can disagree with what you see until you re-run Optimize).
The default scoring light is the **Grading tray** (the evenly lit
hemisphere with a head shadow), the standard light-return definition: brilliance
counts every direction the sky lights, whatever the design. The desktop scores under
the lighting preset the Live Render shows when you press **Optimize**, so pick
the Grading tray first for the standard figure. Under a **Studio** rig
(D65 Daylight, Incandescent, Ring Lights and the like) brilliance is measured
against that rig's own lamps, and a small note above the Optimize button says
so; the lit models (light tent, daylight and the others) count all lit
directions. Some
designs have a ridge where two angles (a crown break and a pavilion main,
say) have to move *together* to hold a critical-angle relation — a search
that only ever moves one angle at a time can grind to a halt on a ridge
like that without reaching the real best point. Once the coordinate
search's own step has shrunk small enough that it's clearly grinding
rather than improving, and **Polish** is checked, a second, deterministic
simplex pass takes over and moves every free tier's angle at once,
following exactly that kind of ridge. The polish pass never discards a
genuine improvement from the first stage — it only replaces the result if
it actually finds something better — and it never touches a pinned tier
either, same as the coordinate search.

### Several starting points

A coordinate search walks downhill from where it starts, so it can settle in
a good-but-not-best arrangement. With **Starts** above 1 the search first
tries a spread of trial arrangements (a quick screening, counted in the
budget), keeps the best of them that differ clearly from each other, and runs
the same search from each. Your own design is always start 1, so the result is
never worse than a single search from it. The starts share the budget, and
the run stops early when extra rounds stop improving anything. Only the
best few starts (as many as **Candidates**) get the polish stage and the
full-fidelity scoring. The starts run side by side on several processor
cores, and the result does not depend on how many cores your computer has: the
same design, settings and seed give the same options. A budget too small to
give every start a few sweeps of the free tiers quietly uses fewer starts;
with **Starts** at 1 the search is exactly the single search, as before. The
web app always runs a single start.

### How fast is it

Every candidate evaluated during the search itself is scored at a single,
canonical camera pose (table-up) — about 2 ms on a small design. The
report you see before and after the run, by contrast, is always scored at
full fidelity: the same 4-axis, 181-point tilt sweep the Tilt Performance
dialog uses (724 sample points total), which takes over a second on its
own. Measured, on the app's own small reference fixture (12 tiers): about
7 ms per search evaluation, so a 200-evaluation budget finishes in
roughly a second and a half, plus the two full-fidelity report
evaluations bracketing it. (The default budget is now 800, and the
evaluations of several starts run side by side, so the wall-clock time of a
default run is a few seconds on that fixture.) On a large, heavily
meet-derived design (over a
hundred tiers), each evaluation re-solves the whole schedule and can cost
several seconds, so a 200-evaluation budget can take minutes — 200
evaluations at roughly 6 seconds each is on the order of twenty minutes on
the app's own large reference fixture; with the default budget, lower
**Budget** or **Starts** for such a design. There is no cheap incremental
resolve to fall back on once tiers are genuinely free; every evaluation
pays for a real solve.

Each candidate you ask for adds one more full-fidelity scoring (about 1.3
seconds), once, at the end of the run. The **Estimated time** line under
the run fields adds all of this up: the budget, the size of the design, the
candidates and the two bracketing scorings. It starts as a rough guess and
is replaced by the speed of your last run on the same design.

Optimize runs off the main thread with real progress and a real Cancel —
unlike Deep Solve's Cancel, this one really does stop the search close to
mid-run (within about one evaluation's latency), and a cancelled run still
shows the best partial result found up to that point, clearly marked as
partial. While it runs, the status line names whichever phase is actually
underway: "scoring the starting point at full fidelity" before the search
proper begins, "screening starting points" while a several-start run chooses
its starts, "N of ~M evaluations" during the coordinate search (with "start
K of S, best X" in front when there are several starts),
"(polish) N of ~M evaluations" during the polish pass, and "scoring the
result at full fidelity" at the very end.

### Why Optimize can be disabled outright, and what "nothing free to move" means

Unlike Deep Solve, which stays clickable with a caveat, Optimize's button
is genuinely disabled the moment there is nothing for it to do: with zero
free tiers, a run would move nothing and use zero evaluations. Hovering
the disabled button explains why, in these words (the app's own
`optimize_hint`, roughly quoted):

> Every tier is currently pinned as a scale reference -- a freshly
> imported design starts this way, and that is correct, not broken.
> Optimize has nothing free to move until you adopt at least one tier's
> real meet constraint (the tier list's Adopt action) or author one by
> hand.

This is the same situation "Why every imported tier starts pinned" (above)
describes: nothing is wrong with the design, it simply has not had any
tier Adopted yet. There are two ways out. Adopt at least one tier, or tick
**Vary anchored tiers** on the Optimize tab to let the search turn the
pinned tiers about their girdle edges instead; with that box ticked the
button is available, and its hint says how many tiers it will turn. (For an
imported design the box starts ticked.) Once at least one tier can move, the
button explains what it *will* do — the number of tiers it can move, the
per-evaluation cost estimate above, and that it runs off the UI thread and
can be cancelled.

If a run starts and no tier turns out to be free, the status line says
"Nothing could be changed with these settings" and suggests checking **Only
selected tiers** and the angle ranges, rather than reporting an empty
result.

### Reading the candidates

When a run finishes, the status line says what it found, and the
**Candidates** list shows the results ranked best first. The first row,
**Start**, is the design you started with, so you always have something to
compare against. Each candidate row shows:

- **#** — the rank, 1 for the best.
- **Score** — the blended score of the four weights. Lower is better.
- **Wind.**, **Bril.**, **Ext.** — windowing, tilt brilliance and extinction
  as percentages.
- **Yield** — the share of the rough that is thrown away.
- **Tone** — the face-up lightness `L*` of the light the stone returns, with a
  dot in its predicted colour. 100 is colourless.
- **Tiers** — how many tiers the candidate changes. A tier that follows a
  relation counts when it moves along.

A figure is **green** when it beats the starting design's and **red** when it
is worse, so a trade-off (better windowing for a little less brilliance, say)
is visible at a glance instead of being hidden in one number. The candidates
are kept apart on purpose: each differs from the others by at least half a
degree on some tier, so a later row is not just the first one with a tier
nudged slightly.

Click a row to pick it. The picked candidate's before and after figures
(each of the three objective components, the yield and the combined score)
appear under the list, followed by a "Tiers this would change" table that
names each tier with its from and to angles. Picking a row also turns on
**Preview**, which shows that candidate as a ghost in the shared viewport
without touching the real design; clicking **Start**, or unticking
**Preview**, goes back to the live design. **Compare...** opens the
before/after compare window (Chapter 14) on the picked candidate.

The candidate list is **one Tab stop**. With it focused, **Up** and **Down** move
through **Start** and the candidates, and the row you land on is picked and
previewed exactly as if you had clicked it. A coloured frame shows the row the
keyboard is on, and a screen reader announces each row as "Starting stone" or
"Candidate 2" with its score.

If nothing could be improved, the status line says so plainly ("No better
arrangement found ... nothing was changed"), the list shows only the **Start**
row, and there is nothing to apply. A run that was cancelled still lists the
best candidates found up to that point, and the status line says the result
is partial.

### A result is only a report until you click Apply

Nothing in the list changes the design on its own. Click **Apply this
candidate** to commit the picked candidate as one real, undoable edit — this
is the only thing that turns an Optimize run into an actual change. The edit
sets the new angles, moves each turned tier's mast, and re-works every
relation, so tiers that follow a changed tier move with it, all in the same
undo step. One **Undo** puts the whole design back.

Apply is only available while:

- you have picked a candidate, and
- the design has not been edited since that result was computed. If it has,
  the list stays on screen for reference with a note ("The design changed
  after this run. Run Optimize again to apply a candidate."), and Apply
  stays off. Nothing is applied against a design the result no longer
  describes.

Applying clears the list, because it described the design as it was before.
Run Optimize again if you want a fresh search from the new design.

After Apply, treat the design as any other edit: click **Solve** again to
confirm it still closes, and check the manufacturability warnings.

Retarget's own Optimize mode (Chapter 6) runs this exact same search, seeded
from a material-shifted starting point, so everything above about staging,
free tiers, and the two-stage search applies there too.

## Putting it together

A typical improvement workflow after loading a catalogue design:

1. **Load** the design (Chapter 3) — everything starts pinned.
2. **Adopt** the tiers you want the solver (and Optimize) to be free to
   move — typically the ones you actually intend to refine. Use **Adopt
   all** or **Adopt sel.** if you want several at once.
3. **Solve** to confirm it still closes with those tiers meet-derived.
4. Optionally run **Deep Solve** to check the result still matches the
   catalogue's printed proportions, and **Pin** any tier it corrects.
5. Run **Optimize** with the Objective that reflects what you care about
   most.
6. Compare the candidates with the **Start** row, then **Apply** the one
   that looks like a real improvement.
7. **Solve** once more and re-check manufacturability warnings before
   exporting.

## Next steps

Continue to Chapter 9 for rendering and export, Chapter 11 for saving and
file formats, or Chapter 14 for Snapshot/Compare and the tilt-curve
persistence rule.
