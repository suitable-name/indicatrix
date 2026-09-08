# 3. Loading a Design Into the Editor and Understanding the Tier List

## What you will do

This chapter gets a design open in the editor and explains every column of
the tier list, so the numbers on screen make sense before you touch
anything.

**Limitation.** The editor described in this chapter and Chapters 4–8 is
only present in the standard build of the app (see Chapter 1). If your copy
has no "Edit" sub-tab next to "Live Render," it was built without it.

## The editor's toolbar

The top row of buttons is grouped into two labelled clusters:

- **Edit** — New, Load Selected, Undo, Redo.
- **Verify & Solve** — Solve, Deep Solve, Optimize. Deep Solve carries the
  caption "checks against the printed proportions, never changes the
  design"; Optimize carries "searches free tiers, needs Apply." Both also
  have a longer explanation on hover.

**Export Edited .asc**, **Save Native**, and **Open Native** sit to the
right, ungrouped — they're file I/O, not editing or verifying, so they get
their own space rather than being folded into either cluster. See Chapter
11 for what each of the three actually writes.

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

- **A remote-library design cannot be loaded.** If you are currently
  browsing a remote worker's library (Chapter 10), Load Selected refuses
  with a toast: "Switch to the local library to load a design into the
  editor." A remote design record never carries the original file bytes
  over the network, so there is nothing to load into the editor. Switch
  back to your local library first (use the library-source control in the
  Remote panel), then load.
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

Click **New** (or File → New) to open the New Design dialog rather than
loading an existing design: pick the preform shape and dimensions, the
index gear, symmetry order, mirror, and a starting material, then click
**Create**. Every one of these stays editable afterward — gear, symmetry,
mirror and material through the Edit tab's Design Settings panel (Chapter
6), the preform through the tier form's own Preform section (Chapter 4) —
see the worked example in Chapter 7.

## Reading the tier list

Each row is one tier. The columns are:

| Column | Meaning |
|---|---|
| **#** | The tier's position in the schedule. |
| **ANGLE** | The facet's cutting angle, in degrees off the girdle plane. Negative angles are pavilion facets; zero or positive angles are crown facets, by this app's convention. |
| **NAME / INDICES** | The facet's name (or "(unnamed)"), and the index position(s) it occupies, in brackets. |
| **MAST** | The solved depth — filled in only after a successful Solve, shown as `-` or `?` when the design hasn't been (re-)solved since this row last changed. |
| **SOLVE** | How confident that mast value is — see "Trusting the SOLVE column" below. |
| **ORBIT** | Whether the tier's indices form one clean symmetric family — see "Orbits" below. |
| **IMPORTED** | Shows an **Adopt** link when there is a better constraint recoverable from the original file — see Chapter 8. |

There is also a **Detach / Reattach** toggle and a remove ("×") button per
row, both covered in Chapter 4.

If there are no tiers yet, the list reads: "No tiers yet -- fill in the
form on the right and click Save Tier."

### Trusting the SOLVE column

The SOLVE column names the method that produced a tier's mast:

- **Dependency order** or **Joint group** — a real, geometrically derived
  value. Trust these.
- **Least-squares est.** or **FAILED (untrusted)** — shown in bold amber.
  These are estimates or placeholders, not real solved geometry. Treat a
  design with any row in this state as not actually finished.
- **not solved** / **no anchor yet** — also flagged amber; the tier has
  never had a successful solve, usually because its block is still missing
  an anchor (see Chapter 5).

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
block(s) are missing one; see Chapter 5.

## Next steps

Continue to Chapter 4 to add or edit tiers, or Chapter 5 to understand
Solve and its status messages.
