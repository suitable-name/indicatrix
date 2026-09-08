# 8. Deep Solve, Optimize, Adopt, and Apply

## What you will do

This chapter covers the editor's four more advanced tools: verifying a
solve against the catalogue's own printed proportions (Deep Solve),
searching for better tier angles (Optimize), converting an imported tier
back to real meet geometry (Adopt), and committing an Optimize result
(Apply).

## Why every imported tier starts pinned

When you load a real `.asc` file into the editor (Chapter 3), the app pins
**every** tier to its exact recorded depth — even a tier whose file
actually said "meets facets P1, P2." This is deliberate: it reproduces the
source file's geometry exactly, with no risk of a re-solve drifting from
the original. The trade-off is that a freshly loaded design has **zero
free tiers** — nothing the solver derives, and nothing Optimize can move —
until you tell the app to trust a tier's real geometric meaning instead of
its pinned number. That is what Adopt is for.

## Adopt

A tier's **IMPORTED** column (Chapter 3) shows an **Adopt** link whenever
the source file's own meet instruction is still recoverable. Click it to
convert that one tier, one at a time, from "pinned depth" back to the
file's actual geometric intent (for example, "meets P1, P2" instead of an
exact number).

**Adopt re-solves immediately.** Unlike every other tier edit in this app,
clicking Adopt does not leave the panel stale — it runs a full solve right
away and keeps the status banner showing the design's real, current state.
In practice the newly adopted tier's live-solved mast converges to the
same value it was pinned at, so adopting a tier is safe: it changes *how*
that facet's depth is described, not, in the ordinary case, the depth
itself.

## Deep Solve

**Deep Solve** is a separate, much slower, external verification pass — it
answers "does this design's geometry actually reproduce the proportions
printed in the catalogue?" rather than "does this design close?"

- It compares candidate mast solutions against the design's own printed
  proportions from the catalogue: Vol/W³ (volume over width cubed), L/W
  (length/width), C/W (crown/width), P/W (pavilion/width), and H/W
  (height/width).
- It reports an **ACCEPTED** or **not accepted** verdict plus a deviation
  score — a strong external signal, not a formal proof of correctness.
- **It never changes the design.** Deep Solve is a read-only diagnostic; it
  reports and suggests, and nothing it finds is written back to the tier
  list unless you separately make the same change yourself.
- It runs off the main thread and can take real time — a mean of roughly
  68 solves per design on the app's own reference corpus, which can mean
  minutes on a large design.
- **Clicking Cancel abandons only the on-screen wait.** The background
  computation keeps running to completion regardless; you simply stop
  waiting for its result, which is then discarded rather than shown.

Deep Solve is unavailable — with an explanatory hint in place of the
button — when the loaded design has no printed proportions to check
against at all (a brand-new design, or one loaded from a reconstructed
schedule with no catalogue proportions). It also has "nothing to repair"
once every tier is already pinned to its recorded mast; adopt at least one
tier first.

## Optimize

**Optimize** is a coordinate search over the angles of the design's **free**
tiers (Chapter 3) — it does not touch any tier pinned as an Exact scale
value. Each candidate is scored on:

- **Windowing** — lower is better (less see-through washout).
- **Extinction** — lower is better (less dead, dark area).
- **Tilt brilliance** — higher is better (how well the stone stays bright
  as it's tilted, not just held dead flat).

The three weight fields (**Weights: windowing / extinction / tilt
brilliance**) let you decide how much each factor matters relative to the
others for this run.

**Optimize runs in two stages.** The first is the coordinate search above,
moving one free tier's angle at a time. Some designs have a ridge where two
angles (a crown break and a pavilion main, say) have to move *together* to
hold a critical-angle relation — a search that only ever moves one angle at
a time can grind to a halt on a ridge like that without reaching the real
best point. Once the coordinate search's own step has shrunk small enough
that it's clearly grinding rather than improving, a second, deterministic
polish pass takes over and moves every free tier's angle at once, following
exactly that kind of ridge. The polish pass never discards a genuine
improvement from the first stage — it only replaces the result if it
actually finds something better — and it never touches a pinned tier either,
same as the coordinate search. Its own evaluation count and any improvement
it found are folded into the same result you already see; there is nothing
extra to read separately.

**A freshly loaded catalogue design starts with Optimize disabled**,
because every tier is pinned on import (see "Why every imported tier
starts pinned" above) — there is nothing free to move. Adopt at least one
tier, or author a new free tier by hand, before Optimize has anything to
search over.

Optimize runs off the main thread with real progress and a real Cancel — a
cancelled run still shows the best partial result found up to that point,
clearly marked as a partial/cancelled result rather than a finished one.

**A result is only a report until you click Apply.** Optimize's outcome
sits in the results panel — showing each of the three objective components'
before/after values separately, plus the combined weighted score — and does
nothing to the design on its own. If nothing improved, the panel says so
plainly rather than applying a no-op change.

## Apply (Optimize's Apply button)

Click **Apply Optimize Result** to commit a held Optimize outcome as a real,
undoable edit to the tier list — this is the only thing that turns an
Optimize run into an actual change. It is only available while:

- Optimize has actually found and reported a result, and
- the design has not been edited since that result was computed (if it has,
  the app tells you the result may no longer apply and asks you to re-run
  Optimize before applying it).

After Apply, treat the design as any other edit: click **Solve** again to
confirm it still closes, and check the manufacturability warnings.

## Putting it together

A typical improvement workflow after loading a catalogue design:

1. **Load** the design (Chapter 3) — everything starts pinned.
2. **Adopt** the tiers you want the solver (and Optimize) to be free to
   move — typically the ones you actually intend to refine.
3. **Solve** to confirm it still closes with those tiers now meet-derived.
4. Optionally run **Deep Solve** to check the result still matches the
   catalogue's printed proportions.
5. Run **Optimize** with weights reflecting what you care about most.
6. Review the before/after report, then **Apply** if it looks like a real
   improvement.
7. **Solve** once more and re-check manufacturability warnings before
   exporting.

## Next steps

Continue to Chapter 9 for rendering and export, or Chapter 11 for saving
and file formats.
