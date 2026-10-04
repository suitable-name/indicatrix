# 16. Concave Tiers

## What you will do

Most facets are flat: a lap grinds one plane, and the stone stays convex. A
**concave tier** is cut with a shaped **tool** instead — a cylinder, a cone, a
ring, a disc or a sphere — and leaves a groove, a bowl or a dimple. This chapter
covers adding and editing such a tier in the tier table and the inspector, how it
is written on the cutting sheet, how the solid view, the render and the
performance numbers treat it, what each file format keeps of it, and what it
cannot do yet. Everything here is additive: a design with no concave tiers behaves,
prints and saves exactly as it did before.

## How a concave tier is written

A concave tier follows the standard notation for fantasy and concave cuts: it is
another tier in the same table, written on **two lines**.

| Column | Line 1 (the facet) | Line 2 (the tool) |
|---|---|---|
| Tier | the tier's name | the tool: `CYL`, `CON`, `CIR`, `DSC` or `SPH` |
| Angle | the facet's angle φ off the girdle plane | the tool's azimuth θ |
| Indices | the index-wheel positions | the displacement `X = …, Y = …, Z = …`, each as a ratio of the stone width |
| Instructions | your free-text cutting instructions | `D/W =` the tool's diameter over the stone width, then (for a cone or disc) `angle = …°`, then `reciprocating` or `plunge` |

For example, a cylinder groove at −42° reads:

```
Groove   -42.00 deg ...  indices [0, 2, 4, 6, 8, 10, 12, 14]
CYL                  0.00°   X = 0.000, Y = 0.150, Z = 0.030   D/W = 0.250, reciprocating
```

The two lines are one tier: they share one shaded band in every table. Concave
tiers are cut after the flat faceting of their section — at the end of the pavilion
and girdle section, and at the end of the crown, directly above the table.

## Adding a concave tier

Press **+ Add Concave Tier** in the tier table's toolbar, or **+ Concave** in the
command bar. The inspector opens on the Tier tab with the concave form, blank. Fill
in:

- **Name**, **Angle (deg)**, **Indices** and **Instructions** — the facet line.
  The angle is φ, from −90° to 90°; indices are typed as in the flat form. A concave
  tier may not take a flat tier's name, and no tier can *meet* it by name.
- **Tool** — Cylinder (`CYL`), Cone (`CON`), Circle (`CIR`), Disc (`DSC`) or Sphere
  (`SPH`).
- **Theta (deg)** — the azimuth of the tool's axis around the stone. Blank is 0.
- **X / Y / Z** — the tool's displacement, as ratios of the stone width. Blank is 0.
- **D/W** — the tool's diameter over the stone width. It must be positive.
- **Tool angle (deg)** — the included angle of a cone or disc. The field is enabled
  for those two tools only, and is required for them.
- **Reciprocating** — ticked, the tool is stroked back and forth; unticked, it is
  plunged straight in.

Press **Add Concave Tier**. The tier appears in the table, is selected, and the
solid view and the render show the cut.

A field that cannot be used is outlined in red and named in the message under the
form (and in a toast): `angle`, `indices`, `tool`, `theta`, `x`, `y`, `z`, `diameter`,
`tool angle` or `name`. Retyping a field clears its outline. The inspector keeps
your draft the same way it does for a flat tier: if you click another row while the
form has unsaved changes, it asks whether to keep the draft or load the other row.

## The concave rows of the tier table

Concave rows come after the flat ones, in the order you authored them. They differ
from a flat row in a few ways:

- The **MEETS** column shows a **Tool** badge instead of a meet constraint; hover
  it to see the tool line (tool, θ, displacement, D/W, motion).
- MAST and MARGIN read `-`, and SOLVE reads *Tool cut*: a concave tier has no mast,
  no solve strategy and no critical-angle margin of its own.
- The **angle cell is read-only** — edit a concave tier's angle in the inspector.
- Pin, Detach, Adopt, Add Anchor and Complete orbit are not offered, and a click
  with Ctrl or Shift selects the row alone: the multi-select group is made of flat
  tiers.
- **Move** (▲▼, Alt+Up/Down), **Duplicate** (Ctrl+D) and **Delete** (Delete) work.
  A concave tier moves only among the concave rows, because their order is their
  cutting order within a section. Nothing can meet a concave tier, so removing one
  never asks "Remove anyway?". Each is one Undo step.

Clicking a concave row loads it into the concave form. Selecting a flat row
returns the inspector to the flat form.

Selecting a tool facet by clicking it in the solid view is not wired yet: hovering
it shows its name, tool code, θ and D/W, but the click selects nothing.

## The cutting sheet and the schedule

The text and HTML cutting sheets, the Edit tab's **Schedule** tab and the
catalogue's cutting table all print a concave tier as its two lines in one band,
with the zebra striping alternating per tier. Flat tiers print exactly as before.
The catalogue's **Copy Instructions** puts each tool line under its facet line.

## The solid view, the render and the numbers

The solid view, the 2D diagram and the render show the stone with every tool's cut
removed. The tool's facets have their own ids, so hover and picking name them.

Rendering a stone with tools:

- The **path tracer runs on the CPU**, whatever the local compute setting: the GPU
  kernels do not know about tools, so a concave stone is never offered to the GPU
  (the GPU status pill stays quiet, since nothing is wrong). A render of a concave
  design is slower than the same design without its tools.
- A **remote worker** receives the tools with the scene and traces the same stone,
  so local, remote and combined renders agree.
- The **optical metrics** (brilliance, windowing, extinction, scintillation, fire),
  the tilt curves and the tilt hover preview are computed on the stone with its
  tools.
- A design the tiers of which do not resolve to tools — a tier that fails
  validation, or more tool placements than the limit — is drawn as its flat stone
  alone, and the tier table's warning badge says why. A half-resolved set of tools
  would be less truthful than none.

## Saving and exporting

| Format | What it keeps of a concave tier |
|---|---|
| `.indicatrix` | Everything, exactly. This is the lossless carrier; open it to get your concave tiers back. |
| `.asc` | Two footnotes per tier (the facet line and the tool line), in cutting order. A concave tier is never written as an `.asc` tier. Re-opening the `.asc` alone reads them as ordinary footnotes. |
| `.gcs` | The same footnotes, in the file's footer fields; no tier record. Experimental, like the rest of the `.gcs` export. |

**Export Edited .asc** and **Export as Gem Cut Studio (.gcs)...** tell you before
writing: *"N concave tier(s) cannot be written to this format and are left out of
the tier list; the .asc keeps them as footnotes, the .indicatrix file keeps them in
full"*, and ask **Export Anyway**. A design with no concave tiers is never asked.
In the library, exporting a design whose `.asc` is rebuilt from its stored table
asks the same question, and writes its concave rows as footnotes rather than as flat
tiers.

**Save** writes the `.indicatrix` file (with the paired `.asc` carrying the
footnotes) and needs no question: nothing is lost.

## Limitations

- **The GPU does not trace tools.** A concave stone renders on the CPU; see above.
- **The Rough Planner fits the flat outline.** It honours a design's concave tiers
  in its volume, carat and yield and draws the tool cuts (Chapter 15), but places
  the flat stone's outer hull in the rough, which a cut never enlarges. Concave
  tiers do not enter the catalogue's vault search as geometry, only as a count.
- **The Compare window** (Chapter 14) draws a design's flat stone: it does not take
  the tools into account.
- **Clicking a tool facet** in the solid or diagram view does not select its tier.
- **One tool, one cut.** A tier's tool is placed once per index and each cut is
  taken from the flat stone's measured width; concave edits never move the stone's
  width, length or height, which come from the flat tiers.
- **Library previews** of a design that has no real design file are rebuilt from the
  stored angle table, which has no place for a tool: its concave rows are left out of
  that placeholder stone.

## Next steps

Chapter 11 describes the file formats in full; Chapter 13 covers the solid view,
whose hover and pick work on tool facets; Chapter 9 covers rendering and export.
