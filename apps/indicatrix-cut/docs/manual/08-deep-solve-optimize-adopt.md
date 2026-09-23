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
Optimize can move — until you tell the app to trust a tier's real
geometric meaning instead of its pinned number. That is what Adopt is for.

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

Once Deep Solve has run, its Log popup lists any tier whose mast it
adjusted, each with its own small **Pin** button. Clicking it converts
that one tier's constraint to an exact scale-reference anchor at Deep
Solve's own verified mast value — a real, undoable edit, and the tooltip
says so plainly: "Pin this tier to Deep Solve's verified mast, as an exact
scale-reference constraint. A no-op (with an explanatory toast) once this
run is stale." If the design has changed since that Deep Solve run
finished, clicking Pin does nothing but tell you to re-run Deep Solve
first — it never pins a value that might no longer be the right one.

## Optimize

**Optimize** is a coordinate search over the angles of the design's **free**
tiers (Chapter 3) — it never touches a tier pinned as an Exact scale
value, and it never moves the girdle, even a girdle tier that happens not
to be pinned: a facet authored at or beyond about 89.5° is vertical by
definition, so there is nothing to optimize on it.

Its controls live on the inspector's own **Optimize** tab:

- **Weights: windowing / extinction / tilt brilliance** — three number
  fields. Any positive number; higher matters more relative to the other
  two. Windowing and extinction score lower-is-better; tilt brilliance
  scores higher-is-better.
- **Yield weight** — a slider, `0` to `100%`, `0` by default. How much
  throwing away preform volume should cost, blended in alongside the three
  optical weights above (all four share the same normalized sum, the same
  way the three optical weights already did). At the default `0%` this
  reproduces every previous Optimize run exactly — nothing changes until you
  move the slider. Raise it when you want Optimize to also favor angles
  that leave less of the rough behind, not just angles that look better
  optically; how much of the rough a given angle set leaves behind is the
  same "how much of the preform is being thrown away" figure the Preform
  tab's own Volumetric Yield reports.
- **Budget / Seed** — two fields, defaulting to `200` (the evaluation
  budget) and `0` (the search seed, for a reproducible run). Typing
  something that does not parse falls back to these same defaults.
- **Polish** — a checkbox, on by default. Leaves the search's second
  (simplex) stage enabled; see "How the search runs," below. Turning it
  off disables that stage entirely.
- **Only selected tiers** — a checkbox, off by default. When it is on
  *and* you have at least one tier selected in the tier list, every other
  free tier is pinned to its own current mast before the search starts, so
  Optimize only ever moves the tiers you selected. With the box checked
  but nothing selected, Optimize falls back to moving every free tier, the
  same as with the box unchecked.

**There is no Fast/Full "fidelity" control in the UI.** The search always
uses a fast, single-pose evaluation internally while it searches, and
always brackets the whole run with one slow, full-fidelity evaluation
before and one after — see "How fast is it," below. This split is fixed
and not something you choose.

### How the search runs

The first stage is a coordinate search: it moves one free tier's angle at
a time, scored under a fixed canonical light pose (not necessarily the
light pose the viewport currently shows — drag the light and these
figures can disagree with what you see until you re-run Optimize). Some
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

### How fast is it

Every candidate evaluated during the search itself is scored at a single,
canonical camera pose (table-up) — about 2 ms on a small design. The
report you see before and after the run, by contrast, is always scored at
full fidelity: the same 4-axis, 181-point tilt sweep the Tilt Performance
dialog uses (724 sample points total), which takes over a second on its
own. Measured, on the app's own small reference fixture (12 tiers): about
7 ms per search evaluation, so a default 200-evaluation budget finishes in
roughly a second and a half, plus the two full-fidelity report
evaluations bracketing it. On a large, heavily meet-derived design (over a
hundred tiers), each evaluation re-solves the whole schedule and can cost
several seconds, so the same 200-evaluation budget can take minutes — 200
evaluations at roughly 6 seconds each is on the order of twenty minutes on
the app's own large reference fixture. There is no cheap incremental
resolve to fall back on once tiers are genuinely free; every evaluation
pays for a real solve.

Optimize runs off the main thread with real progress and a real Cancel —
unlike Deep Solve's Cancel, this one really does stop the search close to
mid-run (within about one evaluation's latency), and a cancelled run still
shows the best partial result found up to that point, clearly marked as
partial. While it runs, the status line names whichever phase is actually
underway: "scoring the starting point at full fidelity" before the search
proper begins, "N of ~M evaluations" during the coordinate search,
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
tier Adopted yet. Once at least one tier is free, the button explains what
it *will* do instead — the number of free tiers it can move, the
per-evaluation cost estimate above, and that it runs off the UI thread and
can be cancelled.

### A result is only a report until you click Apply

Optimize's outcome sits in the Optimize tab's own results box — showing
each of the three objective components' before/after values separately,
plus the combined weighted score — and does nothing to the design on its
own. Below that, a "Tiers this would change" section lists exactly which
tiers would move, by name, with their from/to angles, plus a **Preview**
checkbox: turn it on to see the candidate result as a ghost overlay in the
shared viewport without touching the real design, turn it off to go back
to the live design. If nothing improved, the box says so plainly rather
than applying a no-op change.

Click **Apply Optimize Result** to commit a held Optimize outcome as a
real, undoable edit to the tier list — this is the only thing that turns
an Optimize run into an actual change. It is only available while:

- Optimize has actually found and reported a result, and
- the design has not been edited since that result was computed (if it
  has, the app tells you the result may no longer apply and asks you to
  re-run Optimize before applying it, rather than discarding the pending
  result outright — a retry stays cheap).

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
5. Run **Optimize** with weights reflecting what you care about most.
6. Review the before/after report, then **Apply** if it looks like a real
   improvement.
7. **Solve** once more and re-check manufacturability warnings before
   exporting.

## Next steps

Continue to Chapter 9 for rendering and export, Chapter 11 for saving and
file formats, or Chapter 14 for Snapshot/Compare and the tilt-curve
persistence rule.
