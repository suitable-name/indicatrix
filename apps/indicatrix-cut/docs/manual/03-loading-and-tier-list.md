# 3. Loading a Design Into the Editor and Understanding the Tier List

## What you will do

This chapter gets a design open in the editor and explains every column of
the tier list, so the numbers on screen make sense before you touch
anything.

**Limitation.** The editor described in this chapter and Chapters 4–8 is
only present in the standard build of the app (see Chapter 1). If your copy
has no "Edit" sub-tab next to "Live Render," it was built without it.

## The editor's toolbar

The command bar is two rows of buttons. Each row ends in empty space rather
than spreading its buttons out, and a single hint line underneath the two
rows shows an explanation of whatever button you are currently hovering:

- **Row 1, left** — New Design..., Load Selected, Undo, Redo.
- **Row 1, right** — Solve, and the auto-solve delay control next to it —
  labelled "Auto-solve:" (Off / 150 ms / 300 ms / 1 s / 3 s — Chapter 5).
- **Row 2, left** — a Tier group (**+ Add Tier**, **Duplicate**, **Delete**,
  and an up/down pair to reorder), acting on whichever tier is currently
  selected in the list below — the same actions the tier list's own
  per-row buttons and its "+ Add Tier" header button already offer
  (Chapter 4), collected here too since they are easy to miss on first use.
- **Row 2, middle** — Deep Solve, Optimize, and **Retarget...** (an automated
  proposal for adapting a design to a different material — Chapter 6). Deep
  Solve and Optimize grey out when there is nothing for them to do, and
  hovering a greyed-out button shows the specific reason right on the
  button rather than somewhere else you would have to go looking.
- **Row 2, right** — Export Edited .asc, Save Native, and Open Native. The
  File menu's version of the first uses the identical label; both trigger
  the same export. See Chapter 11 for what each of the three actually
  writes.

Undo and Redo say what they are about to do, rather than a bare "Undo"
and "Redo" — hover either button, or open the Edit menu, and you will see
something like "Undo: Set P1 angle to -41.0 degrees" or "Redo: Add tier
C1'". This works for every kind of edit the app makes, including a Retarget
or an Apply Optimize Result.

## Some terms first

- **Tier**: one row of the cutting schedule — one facet, or one symmetric
  family of identical facets cut at the same angle and depth.
- **Index**: the position on the cutting machine's dividing head (its
  "gear") where a facet is cut. A design with a 96-tooth gear has 96
  possible index positions around the stone.
- **Orbit**: the complete set of index positions that a symmetric tier
  occupies. On an 8-fold, 96-tooth design, one orbit of a facet that
  appears once per repeat has 8 members, 12 teeth apart.
- **Girdle, crown, pavilion**: the three vertical "blocks" of a stone — the
  girdle is the narrow band at the widest point, the crown is everything
  above it (culminating in the table), the pavilion is everything below it
  (culminating, on most cuts, in a culet or point).
- **Meet point**: the point where three or more facet planes intersect. A
  facet's actual depth is determined by where its plane meets its
  neighbours — you rarely set a depth number directly; you say what the
  facet *meets*, and the app works the depth out.
- **Mast**: the solved distance of a facet's plane from the centre, along
  its own normal — this is the number a real cutting machine's mast gauge
  would read. It is always a solver *output*, never something you type in.
- **Anchor**: a tier whose scale is stated directly rather than derived from
  meeting other facets. Every block (crown, pavilion, girdle) needs at
  least one anchor, or its overall size has nothing to be measured against.

## Loading a design from the catalogue

1. Select a design in the catalogue panel (Chapter 2).
2. Open the 3D tab's **Edit** sub-tab.
3. Click **Load Selected**.

What happens next depends on the design and where it lives:

- **A remote-library design**: Load Selected fetches that entry's own
  `.asc` file over the network from the worker and loads it exactly like a
  local one, with a matching toast, `Loaded '<name>' into the editor.`,
  naming the file itself rather than the catalogue title (a remote fetch
  never carries the two together). This needs a real attached `.asc` file
  on the worker's side; a remote design with none fails with
  a clear error message instead of the placeholder reconstruction a local
  design in the same situation gets (Chapter 12). There is nothing to
  switch or reconfigure first; you do not need to be browsing your local
  library to load a remote design into the editor.
- **No design selected**: "No diagram selected to load."
- **Local design with an attached `.asc` file**: the editor parses that
  file and builds the tier list from its exact recorded masts. Every tier
  starts pinned to that exact value (an anchor) — see "Pinned vs. free
  tiers" below. You get a toast: `Loaded '<title>' into the editor.`
- **Local design with no attached file, but a stored cutting-schedule
  table**: the editor reconstructs a schedule from that table, but it has
  no way to recover the original masts, so every mast is filled with a
  placeholder of `0.0`. You get an informational toast: "Loaded a
  reconstructed schedule -- mast distances are placeholders (no attached
  .asc file was found); adjust masts before exporting." Do not export this
  as-is — solve it and, where needed, add real scale-reference values
  first.
- **Neither**: "This diagram has no cutting-schedule data to load."

**Loading solves immediately.** As soon as a design loads, the app runs a
full solve, so the tier list's MAST and SOLVE columns are already filled in
by the time you see it.

## Starting a new design instead

Click **New Design...** (or File → New) to open the New Design dialog rather
than loading an existing design: pick the preform shape and dimensions, the
index gear, symmetry order, mirror, and a starting material, then click
**Create**. Every one of these stays editable afterward — gear, symmetry,
mirror and material through the Edit tab's Design Settings panel (Chapter
6), the preform through the inspector's own Preform tab (Chapter 4) — see
the worked example in Chapter 7.

A brand-new design starts with zero tiers, and its very first Add Tier
defaults to angle 0.0 and "Unspecified vertex" — the same as any other
blank draft, with no round-brilliant or other starter template offered.
Chapter 7's worked example takes you through building one by hand.

## Reading the tier list

Each row is one tier. The columns are:

| Column | Meaning |
|---|---|
| **#** | The tier's position in the schedule. |
| **⚠** | A warning glyph when this tier has its own manufacturability warning (Chapter 5) — hover it to read the warning. Blank otherwise. |
| **C/P/G** | The block this tier's angle actually classifies into — Crown, Pavilion, or Girdle, regardless of what you meant it to be (see "Anchors and blocks" below). Hover for the full block name. |
| **ANGLE** | The facet's cutting angle, in degrees off the girdle plane. Negative angles are pavilion facets; zero or positive angles are crown facets, by this app's convention. A single click on this cell selects the row, like clicking anywhere else in it; double-click it (or press F2 on the selected row) to edit the value in place — see Chapter 4. |
| **NAME / INDICES** | The facet's name (or "(unnamed)"), and the index position(s) it occupies. These two columns stretch with the width of the dock; every other column stays a fixed width. |
| **MEETS** | What the tier's constraint actually is: a pin glyph and the stated value for an anchor, "meets `<names>`" for named facets, or a plain "meet" for an unspecified vertex. |
| **MAST** | The solved depth — filled in only after a successful Solve, shown as `-` or `?` when the design hasn't been (re-)solved since this row last changed. |
| **SOLVE** | How confident that mast value is — see "Trusting the SOLVE column" below. Hover the cell for a one-line explanation of exactly why, when the app has one to give. |
| **MARGIN** | For a pavilion tier, how far its angle sits above the design's critical angle — Chapter 6. A dash for crown and girdle tiers. |
| **ORBIT** | Whether the tier's indices form one clean symmetric family — see "Orbits" below. When it is amber (incomplete), click it to fill in the missing symmetric positions automatically. |
| **IMPORTED** | Shows an **Adopt** link when there is a better constraint recoverable from the original file — see Chapter 8. |

To the right of those, per row: a small up/down pair to move the tier
earlier or later in cutting order (the same as Alt+Up/Alt+Down — Chapter
4), a **Detach / Reattach** toggle, a duplicate button, and a remove ("×")
button — all covered in Chapter 4. A row needing an anchor (Chapter 5)
also shows an **Add Anchor** button that jumps the inspector straight to
that tier with "Exact scale value" already selected, ready for you to type
the number and save. **Add Anchor does not create a new tier or fill in a
number for you** — it only selects the existing tier the app already
flagged as the one missing an anchor, and opens the Tier form to it with
the constraint kind preset; you still type the actual scale value and
click Save Tier yourself.

The first time (each session) a design you're editing lacks an anchor, a
one-time card explains why the solver needs one before Add Anchor even
does anything: relative "this meets that" statements fix a block's shape
but never its size, so one tier per block has to state a real, authored
depth. Click **Got it** to dismiss it, or tick **Don't show again** to
stop it appearing for good — that choice is remembered across sessions.

A tier whose SOLVE strategy is trusted (see below) also shows a **Pin**
button, the mirror image of Adopt (Chapter 8): click it to freeze that
tier's *current* solved mast as an exact **Exact scale value** anchor,
converting a free (meet-derived) tier back into a pinned one. This is
useful once Solve or Optimize has found a mast you want to lock in place
so further edits elsewhere cannot move it. Pin is hidden on a tier that is
already an anchor, and on any tier whose SOLVE strategy is still uncertain
(amber) — pinning a stale or placeholder mast would freeze the wrong
number, so the app does not offer it until the value is trustworthy.

Above the column headers is a small filter box: type a few letters of a
name, index, block, SOLVE strategy, or meet constraint and every
non-matching row dims (rather than disappearing) so the row numbers and
Up/Down/Home/End behaviour stay exactly as they would with no filter set.

If two or more rows are Ctrl-clicked into a multi-select group (Chapter 4),
a small bar above the table shows "N selected" with **Clear** and
**Delete** buttons — Delete removes every selected tier as one Undo step
per tier. Shift-click a row instead to select every tier between it and
whichever row you selected last, inclusive; see "Selecting several tiers
for a batched nudge" in Chapter 4 for how that range then behaves.

If there are no tiers yet, the list reads: "No tiers yet -- open the Tier
tab below to add one."

### Trusting the SOLVE column

The SOLVE column names the method that produced a tier's mast. The exact
label is one of:

| Label | Meaning | Trust it? |
|---|---|---|
| **Scale reference** | An anchor tier (Meets: Exact scale value) — the mast is exactly the number you typed, not derived. | Yes |
| **Dependency order** | A real meet vertex, settled directly in the solver's first pass. | Yes |
| **Joint group** | A real meet vertex from a mutually-dependent group the solver had to settle together rather than in strict order. | Yes |
| **Least-squares est.** (bold amber) | No usable meet-vertex candidate; this is a per-block estimate, not real solved geometry. | No — treat the design as not actually finished |
| **FAILED (untrusted)** (bold amber) | The solve could not even produce an estimate; the mast shown is a placeholder. | No |
| **not solved** (bold amber) | This row has changed since the design's last successful Solve. | No — click Solve |
| **blocked** (bold amber) | This tier's own block is fine, but *another* block in the design has no anchor, so nothing solves yet. | No — fix the other block (Chapter 5) |
| **no anchor yet** (bold amber) | This tier's own block is the one missing an anchor. | No — Add Anchor (below) or Chapter 5 |
| **stale (‹label›)** (bold amber) | A cached result is shown without a fresh solve, e.g. "stale (Least-squares est.)" — the parenthesised label is what it was at the last real solve. | No — click Solve |

Hover any SOLVE cell for a one-line explanation specific to that tier when
the app has one to give.

### Orbits

The ORBIT column tells you, at a glance, whether a symmetric tier's listed
index positions form a complete, evenly spaced family under the design's
stated fold count:

- `orbit x4` (or `x8`, etc.) — one complete, clean orbit.
- `2 orbits` — several complete families folded together.
- `3/4 orbit` — an incomplete or inconsistent set of indices, shown in bold
  amber. This is the app telling you that editing or Optimize could
  propagate this tier onto index positions you did not expect, or that a
  facet is simply missing from its family.
- Blank — a single-facet tier with nothing to link (e.g. the table).

This matters because editing one row of a multi-index tier normally moves
the *whole* orbit — see Detach in Chapter 4 if you want to break that.

### Selecting a tier

Clicking a row, pressing Enter on it, using the Up/Down/Home/End/Page
Up/Page Down keys (Chapter 4), or clicking one of its facets in the Solid
viewport or the Diagram view (Chapter 13) all select the *same* tier
everywhere at once: the row highlights and scrolls into view if it was off
screen, the tier form on the inspector's Tier tab re-seeds with that tier's
values, and the tier's whole symmetric orbit tints in both the Solid
viewport and the Diagram view. Selecting a tier one way always shows up
everywhere else.

If you were mid-edit on an unsaved tier draft when the selection moves, the
inspector does not silently throw your draft away — see "Losing an
unsaved draft" in Chapter 4.

Orbiting the 3D view (dragging to rotate the stone) no longer selects
whatever facet happens to be under the cursor when you release the mouse —
only a genuine click, one that barely moved between press and release,
counts as a pick.

### The inspector's Schedule tab

Separately from the catalogue's own Cutting Schedule tab (Chapter 1), the
Edit tab's inspector has its own **Schedule** tab: a read-only FACET /
ANGLE / INDEX table built from this design's own current, solved tier
list, not the catalogue's stored original. It reads "Not solved -- click
Solve to see this design's own cut order here" until you do. It exists so
you can eyeball the cutting order without leaving the Edit tab; for a
copyable file, use Export Edited (above).

### Pinned vs. free tiers

- A **pinned** tier is one whose "Meets" constraint is **Exact scale
  value** — its size is a stated number, not derived. Every design loaded
  from a real `.asc` file starts with *every* tier pinned to its exact
  original mast, so the geometry reproduces the source file precisely.
- A **free** tier is one whose constraint is **Unspecified vertex** or
  **Named facet(s)** — its depth is derived by the solver from where its
  plane actually meets its neighbours.

This distinction matters for two later chapters: Optimize (Chapter 8) can
only move free tiers, and a freshly loaded catalogue design starts with
*zero* free tiers — you convert a pinned tier back to a free, meet-based
one with **Adopt** when you want to let the solver (or Optimize) move it.

### Anchors and blocks

Each of the three blocks — crown, pavilion, girdle — needs **at least one**
tier of kind **Exact scale value** in it. Meet-point geometry alone can
never fix a whole block's overall size, because shifting every facet in a
block along its own axis by the same amount preserves every internal meet
point — something has to state the real-world size directly. If a block
has no such anchor, Solve fails with a message naming exactly which
block(s) are missing one; see Chapter 5. Every tier the app puts in that
missing block also grows a row-level **Add Anchor** button (see "Reading
the tier list" above) that jumps straight to the fix.

A tier's block is not what you intended it to be, it is whatever its angle
actually classifies as: crown or pavilion by the sign of the angle, and
**girdle only at exactly 90 (or -90) degrees** — the tolerance is very
tight, so "about 90" still classifies as crown or pavilion, not girdle.
Check the tier list's own C/P/G column (above) if a block you expected to
be anchored still reads as missing one; the tier you added may have
classified somewhere you did not expect. Chapter 7's worked example shows
this in practice.

## Next steps

Continue to Chapter 4 to add or edit tiers, or Chapter 5 to understand
Solve and its status messages.
