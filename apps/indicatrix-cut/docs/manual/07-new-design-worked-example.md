# 7. Creating a New Design From Scratch: A Worked Example

## What you will do

This chapter walks through building a simple design from nothing, tier by
tier, putting Chapters 3-6 together in practice. It builds an 8-fold round
brilliant-style stone: a girdle, pavilion main facets, crown main facets, and
a table.

**Shortcut: follow this chapter inside the app.** The empty state (shown
until a design is created, loaded or opened) has an "Open the Worked Example"
card that opens an in-app guide panel walking through the same steps below.
Reopen the guide any time from Help > Guide: New Design Walkthrough, or find it as
"New design walkthrough" in the tutorial browser (Help > Tutorials..., Chapter 22),
which also lists the other guided lessons and marks the ones you have finished. The card
does not build the stone for you: the guide starts at Step 1, where you create
an **Empty** design yourself, and you add every tier by hand -- or press **Next** on
a step you have not done, and the guide makes the same entries for you.

While the guide is open:

- Each step lists its actions as numbered lines, with the exact values to
  type, and outlines the control it is about: the New Design dialog, the
  inspector's Tier tab, the Design Settings panel, the Solve button, the tier
  table, or the Preform tab.
- A step moves on by itself once its goal is reached (the tier exists with the
  right name, angle and indices; the material is applied; the design solves
  to a closed stone; and so on). The status chip reads "Waiting for: ..."
  until then, and "Done" for a moment before the next step. Any route to the
  goal counts: the tier form, an inline edit, Undo/Redo, or auto-solve
  finishing first (the toolbar's quick-add buttons do not count for the
  girdle: quick-add Girdle names the tier "Girdle" and adds no indices). **Next** on a
  step you have not done does it for you (it types the listed values into the same form,
  so **Ctrl+Z** undoes it); the optional yield step, whose girdle diameter is your own
  stone's, shows **Skip step** instead, which moves on without it. **Back**
  returns to the previous step. The two reading steps -- checking the orbits
  (Step 8) and the closing note -- wait for **Next** (**Finish** on the closing
  note) instead.
- Controls the current step does not need are locked (dimmed, with a tooltip
  saying so), and so are their keyboard shortcuts -- including the tier
  table's per-row actions and the inspector tabs' contents. Selecting rows
  stays possible. Closing the guide with the
  x in its corner unlocks everything.
- The Simple interface leaves **Exact scale value** out of the Meets list for a
  new tier. The steps that ask for it (the girdle, the pavilion, the crown and
  the table) therefore show the Advanced controls while they are current, and
  say so in their explanation. The girdle step also names the **Girdle Facet
  Preset**, a shortcut that sets that entry for you.
- On a wide window's Edit tab the guide sits in its own column beside the tier
  table. Everywhere else -- a narrow window, the Live Render tab, and the
  catalogue tabs (Cutting Instructions, Files & Downloads) -- it floats near the
  window's bottom-right corner instead, so it never disappears mid-walkthrough.
  Drag the floating panel by its header to move it anywhere inside the window;
  it stays where you put it.
- The **▾** button in the guide's header collapses it (docked or floating) to a
  small "Guide · Step N of M ▸" pill; the guide keeps running. Click the pill
  to expand it again (a floating pill can also be dragged).

The steps below are the authoritative, fuller description -- the in-app
guide's text is a condensed version of them.

**Note on this example.** Every number below (angles, indices, scale values)
is a constructed, illustrative example consistent with how the tier form and
constraints actually work. Treat the angle values as a reasonable starting
point for exploration, not as an authoritative cutting angle table -- refine
them with Solve and Optimize (Chapters 5 and 8) once the design closes. The
scale values were chosen so that this stone solves, closes and fits inside the
blank with a thin girdle and a table of a sensible size; they are worked out in
Steps 2 to 5. If you
just want the finished stone, the New Design dialog's "Standard Round
Brilliant" template creates a verified eight-tier round brilliant in one step,
and its oval, cushion, emerald and princess neighbours do the same for those
outlines (see "Starting from a shape instead" below).

## Step 1: The New Design dialog

Click **New Design...** on the command bar (or File -> New Design..., or the
empty state's **New Design...** card) to open the New Design dialog. Its
"Start From" section is a template gallery in three sections -- Shapes,
Round variants and teaching designs, and Blank. For this walkthrough, choose
the **Empty** card (the single card under "Blank") and build the tiers by hand
below, so you see every step; the form then shows the full set of fields
listed next. Picking a stone such as "Standard Round Brilliant" instead seeds
its finished tier table immediately and shows a shorter form (material, index
gear and stone width); see "Starting from a shape instead" below.

- **Preform Shape**: Cylinder.
- **Half-Width / Length-Width / Depth**: `1.50` / `1.00` / `1.50` -- a
  blank with room around the stone below, so none of the preform's own walls
  clips it. The room under the culet is small (see the end of Step 5).
- **Index Gear**: `96`.
- **Symmetry Order**: `8`.
- **Mirror**: on.
- **Starting Material**: `(none)` -- you can change this any time afterward
  in the Design Settings panel (Chapter 6), including to a custom catalogue
  material.

Click **Create** (or press Enter while the form is valid). This replaces the
editor state with a brand-new, zero-tier design carrying exactly these
settings. The viewport shows the cylinder preform with a small "New design —
no tiers yet. Add the girdle first." hint and a **+ Add Tier** button above it. Every one of these
choices remains editable afterward through the Design Settings panel
(gear/symmetry/mirror) or the tier form itself. If a field does not parse, the
dialog stays open with your values and a message naming the field.

### Starting from a shape instead

The cards with a picture are ready-made stones. The five under **Shapes** are the
everyday outlines: the **Standard Round Brilliant** (8-fold), an **Oval
Brilliant** (2-fold, length to width 1.35), a **Cushion Brilliant** (4-fold), an
**Emerald Step Cut** (2-fold, three steps on the crown and three on the
pavilion, length to width 1.4) and a **Princess Cut** (a 4-fold square
brilliant). **Round variants and teaching designs** keeps the shallow and deep
round brilliants and the two teaching designs. Each card shows a small solid
picture of its stone (drawn in the background the first time you open the dialog,
so the picture tile may be empty for a moment) and a "Designed for ..." line: the
material and refractive index its facet angles were worked out for. Hover a card
for its description.

When a stone is picked, the lower half of the dialog shows three choices instead
of the Empty form's fields:

- **Material** -- the built-in materials, then your custom ones. It starts on the
  stone's own material. A line beneath says whether your choice changes the
  facet angles.
- **Index Gear** -- only the gears this stone's facets still land on. Every
  facet position is carried to the new gear the way the Design Settings panel's
  gear remap does it (Chapter 6), and a gear is listed only when that stays
  exactly symmetric, on whole teeth and with no two positions of one tier merging.
  96 is always there; the others are usually 80 and 64 (the emerald step cut also
  allows 72 and 120), and 77 is never offered because it is not a multiple of any
  of these symmetry orders. The Empty form still takes any gear.
- **Stone Width (mm)** -- how wide the finished stone is across the girdle. It
  becomes the design's girdle diameter, which scales weights and the cutting
  instructions. The box starts at 6.5; leave it empty to set the width later in
  the Edit tab, or type a number above 0 (at most 200). Create stays dimmed
  while the box holds something that is not a number.

**Create** builds the stone on its own rough, on the gear you chose, at the width
you typed, and solves it, so the viewport shows a closed stone at once.
Nothing is locked: the gear, symmetry and mirror can be changed afterwards like
any other design's. The design starts as the template, so Undo never goes back to
an empty schedule.

**When the material is not the one the stone was designed for.** If the refractive
index of the material you picked differs from the design index shown on the card by
more than 0.02, Create adapts the pavilion angles to the material before the stone
appears. It does what Retarget does with the "Shift" method (Chapter 14): each
pavilion angle moves by the change in the material's critical angle, the crown is
left where it is, and the masts are refitted so the stone still closes. The result
is kept only if Retarget's validity check passes (the stone closes, keeps its
girdle, including the thinnest point of the girdle band at its corners, keeps its
table, and no facet vanishes); the toast then reads "Pavilion angles adapted
from n ... to n ...; the crown is as authored." with the check's figures. If the adapted stone would
not be valid, Create keeps the template's own angles instead and says so in a
warning toast, with the reason. You can still run Retarget afterwards (Chapter
14) to try the Optimize method or a different crown policy. A material within 0.02
of the design index leaves the angles alone.

As with any new design, if the open design has unsaved changes, Create first asks
whether to save or discard them.

Because the gear has 96 teeth and the design is 8-fold, one representative
facet position per repeat, evenly spaced, is:

```
0, 12, 24, 36, 48, 60, 72, 84
```

(96 / 8 = 12 teeth apart.) You will reuse this same list of eight indices for
every symmetric tier in this example.

## Step 2: The girdle (add this first)

The girdle goes in first: its half-width sets the size of the whole stone,
and it is the anchor of the girdle block. Each of the three blocks (girdle,
pavilion, crown) needs an anchor of its own, an **Exact scale value** tier --
this step gives the girdle's, Steps 3 and 4 the pavilion's and the crown's (see
Chapter 3's "Anchors and blocks").

**The girdle angle must be exactly 90.0, not 0.0.** This app classifies a
tier's block from its angle (Chapter 3): 90 degrees is Girdle,
and the tolerance is very tight, so it has to be typed exactly. A 0.0
tier is Crown -- its facet normal points straight up, the same as the
table -- so it would give you a second, smaller table stacked on top of
the real one instead of a girdle, and the pavilion tiers below would end
up bounded only by the cylinder preform's own wall rather than by a real
girdle band.

1. Click **+ Add Tier** (the hint over the viewport, the command bar, or the
   tier table's toolbar), which opens the inspector's Tier tab in Add mode.
   **Angle (deg)**: `90.0` -- or click **Girdle Facet Preset**, which sets
   the angle to 90 and Meets to Exact scale value 1.0 in one click.
2. **Meets**: **Exact scale value**. Type the girdle's half-width, here
   `1.0`. This is the anchor of the girdle block. At 90 degrees the facet's
   normal points straight out from the centre, so this scale value is
   genuinely a half-width, the way it reads: the girdle is 2.0 wide, flat to
   flat. (The Simple interface leaves **Exact scale value** out of the Meets
   list for a new tier, so the guide's step shows the Advanced controls. If
   you work without the guide, click the **Girdle Facet Preset**, which sets
   it for you, or switch to Advanced to pick it from the list.)
3. **Name**: `G1`.
4. **Indices**: `0, 12, 24, 36, 48, 60, 72, 84`.
5. Click **Add Tier**.

Check the tier table's own CODE column (just left of ANGLE; Chapter 3) reads
**G1** for this row before moving on -- that is your confirmation the girdle classified
the way you intended.

## Step 3: Pavilion main facets

Eight facets, one per repeat, angled steeply below the girdle.

A **scale value** is the distance from the centre of the blank to the facet's
plane, measured straight out from the plane -- the facet's mast (Chapter 3). The
girdle is vertical, so its scale value is its half-width. A sloping facet's
plane sits at a distance that also depends on its angle, so the pavilion's and
the crown's numbers below are not 1.0. The solver cannot work them out from
meets alone: it needs one stated number in each block.

1. **Angle (deg)**: a starting value such as `40.0` (a typical pavilion
   main angle at this general RI range -- expect to refine this). Type the
   plain number: the **P** in the name you give in item 3 puts the tier on the
   pavilion side, below the girdle. Check the
   tier list's MARGIN column (Chapter 6) once this tier exists. With no
   material selected yet, the design's effective RI is still its legacy
   default (1.54), whose critical angle is 40.5 degrees, so `40.0` sits just
   below it: the P1 row reads **-0.5°** in red (Windows). That is expected
   for now; Step 6 picks a real material and fixes it.
2. **Meets**: **Exact scale value**, typing `0.56`. This is the anchor of
   the pavilion block. At 40.0 degrees it puts the culet, the point at the
   bottom, about 0.73 below the centre, and the pavilion's top edge about 0.11
   above the centre where it meets the girdle's flat face. (The Simple
   interface leaves **Exact scale value** out of the Meets list for a new
   tier, so the guide's step shows the Advanced controls; if you work without
   the guide, switch to Advanced.)
3. **Name**: `P1`.
4. **Indices**: `0, 12, 24, 36, 48, 60, 72, 84`.
5. Click **Add Tier**.

## Step 4: Crown main facets

Eight facets, one per repeat, angled above the girdle.

1. **Angle (deg)**: a starting value such as `34.5`.
2. **Meets**: **Exact scale value**, typing `0.70`. This is the anchor of the
   crown block. At 34.5 degrees the crown's plane meets the girdle's flat
   face about 0.16 above the centre. The girdle band is the gap between the
   pavilion's top edge and the crown's lower edge on that face: about 0.054
   thick, which is 2.7 percent of the stone's 2.0 width -- thin, but real. (The
   guide's step shows the Advanced controls for the same reason as Step 3.)
3. **Name**: `C1`.
4. **Indices**: `0, 12, 24, 36, 48, 60, 72, 84`.
5. Click **Add Tier**.

The two numbers belong together. With the girdle at `1.0` as typed in Step 2
and the angles in degrees, the pavilion's top edge on the girdle face is at
`(sin(40) * 1.0 - 0.56) / cos(40)` and the crown's lower edge at
`(0.70 - sin(34.5) * 1.0) / cos(34.5)`. If you change one of the two, change
the other so the crown's edge stays above the pavilion's, or the girdle band
gets thinner and finally vanishes.

## Step 5: The table

One flat facet at the top, not indexed around the gear.

1. **Angle (deg)**: `0.0`.
2. **Meets**: **Exact scale value**, typing `0.46`. A flat table faces
   straight up, so its scale value is simply its height above the centre.
   0.46 makes the table about 57 percent as wide as the stone, with a crown
   about 0.30 high. (As in Steps 3 and 4, the guide's step shows the Advanced
   controls.)
3. **Name**: `T`.
4. **Indices**: leave blank.
5. Click **Add Tier**.

**Why not Unspecified vertex?** That meet cuts the table until its plane passes
through some vertex of the facets already there. This crown has only its eight
main facets, which meet in a single point at the top, so the next vertex below
that point is the girdle's top edge: the table would sink to the girdle and cut
the whole crown away. A crown with star or bezel facets has vertices in between,
and a table can meet one of those; this simple stone does not, so you state its
height instead.

The finished stone has these figures (Step 9 shows where to read the table
percentage, the crown height, the pavilion depth and the total depth): girdle
2.0 wide and 0.054 thick (2.7 percent of the width), table 57 percent of the
width, crown 0.30 high (14.9 percent), pavilion 0.84 deep (42.0 percent), total
depth 1.19 (59.6 percent). The culet sits 0.73 below the centre and the table
0.46 above it. The blank from Step 1 reaches 0.75 above and below the centre,
so no wall of the blank touches the stone, but the culet is only 0.019 above
the blank's floor. If you refine the pavilion later and make it steeper or
deeper, the stone will reach the floor and the blank will clip it: make the
blank deeper first, in the inspector's Preform tab.

## Step 6: Pick a real material

In the Design Settings panel (Chapter 6), set **Material** to, say,
`Diamond`, and click **Apply Material**. Watch the **Effective RI**/
**Critical Angle** readouts update, and re-check the pavilion tier's MARGIN
column at the new (higher) RI -- diamond's critical angle (~24.4 degrees) is
much smaller than the legacy 1.54 default's (40.5 degrees), so the P1 row
turns from red to green and reads **+15.6°** (Safe). If you picked a lower-RI
material instead, you might see it stay at Marginal or Windows; adjust the
pavilion angle if so, per Chapter 6's retargeting walkthrough.

## Step 7: Solve and check

1. Click **Solve**.
2. Read the status strip (Chapter 5):
   - `Closed solid -- volume ...` -- you have a valid stone. With the values
     above the line reads about `Closed solid -- volume 1.7270 (model
     units^3), L/W 1.000, H/W 0.596, C/W 0.149, P/W 0.420.` Continue to
     step 8.
   - `<Block> has no anchor: add a tier with an exact scale value.` -- that
     block still has no anchor, for example because one of the Meets
     entries in Steps 2 to 5 was left on **Named facet(s)** or **Unspecified
     vertex**. Set one tier of that block to **Exact scale value** (or add one
     more such tier, for the pavilion a pavilion depth or culet-point
     dimension) and Solve again, or use the tier table's own **Add Anchor**
     button on one of that block's rows. The guide leaves the tier form and the
     tier table open on this step, so you can make the fix without leaving it.
   - `Degenerate` or `Unbounded` -- these name the tier(s) most likely
     responsible; check the angle and constraint on the tier you most
     recently added, or on whichever tier the message names -- a facet
     meeting the wrong neighbour, or an angle too shallow to intersect its
     neighbours, is the usual cause.

## Step 8: Check the orbits

For each multi-index tier (girdle, pavilion mains, crown mains), check the
**ORBIT** column reads `orbit x8`. If it instead reads something like `6/8
orbit` in bold amber, an index position is missing or inconsistent --
compare the tier's Indices field against the intended list.

## Step 9: Optional -- yield and rendering

1. On the inspector's Preform tab (Chapter 4), in its Yield section, set
   **Girdle Diameter (mm)** to a real size (e.g. the girdle half-width you
   set in Step 2, doubled and converted to millimetres). Leave **Specific
   Gravity Override** blank: the carat-weight estimate then uses the
   specific gravity of the material you set in Design Settings in Step 6
   (Diamond). Neither field changes the render. Click **Apply Yield
   Inputs**, then Solve again --
   Volumetric Yield and Est. Carat Weight show values, and the
   same tab's Proportions group shows table %, crown height, pavilion
   depth, total depth, and length-to-width for this stone. (With auto-solve
   on, the solve happens by itself.)
2. Switch to the Live Render tab to see the stone rendered -- with "Linked
   to design" on (the default), it already shows the Diamond you picked in
   Step 6. Pick a lighting preset and check the brilliance/windowing/
   extinction readouts. (The in-app guide completes this step on Apply Yield
   Inputs and lists the Live Render visit on its closing step.)

## Step 10: A note on manufacturability

Every index above lands exactly on a real gear tooth by construction (they
are all multiples of 12 on a 96-tooth gear), so this example should not
trigger the "index does not land on a gear tooth" manufacturability warning
described in Chapter 5. If you later change the index gear via the Design
Settings panel's gear combo, review the remap confirmation's red-highlighted
rows (Chapter 6) before confirming -- a lossy remap (e.g. 96 to 80 teeth) can
introduce exactly this kind of off-tooth index.

## Next steps

Continue to Chapter 8 to learn Deep Solve, Optimize, Adopt, and Apply -- the
tools for verifying and improving a design once it closes.
