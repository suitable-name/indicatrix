# 7. Creating a New Design From Scratch: A Worked Example

## What you will do

This chapter walks through building a simple design from nothing, tier by
tier, putting Chapters 3-6 together in practice. It builds an 8-fold round
brilliant-style stone: a girdle, pavilion main facets, crown main facets, and
a table.

**Note on this example.** No built-in round-brilliant template ships with
the app -- there is no "starter design" library to pick from. Every number
below (angles, indices) is a constructed, illustrative example consistent
with how the tier form and constraints actually work, not a design copied
from a file in the app. Treat the angle values as a reasonable starting
point for exploration, not as an authoritative cutting angle table -- refine
them with Solve and Optimize (Chapters 5 and 8) once the design closes.

## Step 1: The New Design dialog

Click **New** (or File -> New) to open the New Design dialog:

- **Preform Shape**: Cylinder.
- **Half-Width / Length-Width / Depth**: `1.50` / `1.00` / `1.50` -- a
  generously sized blank so the schedule below never itself clips against
  the preform's own walls.
- **Index Gear**: `96`.
- **Symmetry Order**: `8`.
- **Mirror**: on.
- **Starting Material**: `(none)` -- you can change this any time afterward
  in the Design Settings panel (Chapter 6), including to a custom catalogue
  material.

Click **Create**. This replaces the editor state with a brand-new,
zero-tier design carrying exactly these settings via
`indicatrix_cut_core::Design::fresh_from_spec` -- unlike the pre-A5 bare "New" action,
every one of these five choices is yours to make up front, and every one
remains editable afterward through the Design Settings panel (gear/symmetry/
mirror) or the tier form itself.

Because the gear has 96 teeth and the design is 8-fold, one representative
facet position per repeat, evenly spaced, is:

```
0, 12, 24, 36, 48, 60, 72, 84
```

(96 / 8 = 12 teeth apart.) You will reuse this same list of eight indices for
every symmetric tier in this example.

## Step 2: The girdle (add this first)

The girdle needs to be added before the crown and pavilion, because it gives
both blocks something to meet, and it needs its own anchor.

1. In the tier form: **Angle (deg)**: `0.0` (the girdle plane itself).
2. **Meets**: **Exact scale value**. Type the girdle's half-width, e.g.
   `1.0`. This is the design's mandatory anchor for the girdle block -- see
   Chapter 3's "Anchors and blocks."
3. **Name**: `G1`.
4. **Indices**: `0, 12, 24, 36, 48, 60, 72, 84`.
5. Click **Add Tier**.

## Step 3: Pavilion main facets

Eight facets, one per repeat, angled steeply below the girdle.

1. **Angle (deg)**: a starting value such as `-40.0` (a typical pavilion
   main angle at this general RI range -- expect to refine this). Check the
   tier list's MARGIN column (Chapter 6) once this tier exists: with no
   material selected yet, the design's effective RI is still its legacy
   default (1.54), so this reads comfortably Safe.
2. **Meets**: **Named facet(s)**, typing `G1`, so the pavilion mains
   explicitly close against the girdle you just added. (If your first
   pavilion tier ends up being the only anchor available to the pavilion
   block once you check Solve's status, use **Exact scale value** here
   instead, or add a further scale-reference tier -- such as a pavilion
   depth or culet reference -- until the pavilion block has one.)
3. **Name**: `P1`.
4. **Indices**: `0, 12, 24, 36, 48, 60, 72, 84`.
5. Click **Add Tier**.

## Step 4: Crown main facets

Eight facets, one per repeat, angled above the girdle.

1. **Angle (deg)**: a starting value such as `34.5`.
2. **Meets**: **Named facet(s)**, typing `G1`.
3. **Name**: `C1`.
4. **Indices**: `0, 12, 24, 36, 48, 60, 72, 84`.
5. Click **Add Tier**.

## Step 5: The table

One flat facet at the top, not indexed around the gear.

1. **Angle (deg)**: `0.0`.
2. **Meets**: **Unspecified vertex** (a simple "cut to centrepoint" meet),
   or **Exact scale value** if the table size is a dimension you want to
   state directly.
3. **Name**: `T`.
4. **Indices**: leave blank.
5. Click **Add Tier**.

## Step 6: Pick a real material

In the Design Settings panel (Chapter 6), set **Material** to, say,
`Diamond`, and click **Apply Material**. Watch the **Effective RI**/
**Critical Angle** readouts update, and re-check the pavilion tier's MARGIN
column at the new (higher) RI -- diamond's critical angle (~24.4 degrees) is
smaller than the legacy 1.54 default's, so `-40.0` degrees should still read
comfortably Safe. If you picked a lower-RI material instead, you might see
it move to Marginal or Windows; adjust the pavilion angle if so, per
Chapter 6's retargeting walkthrough.

## Step 7: Solve and check

1. Click **Solve**.
2. Read the status banner (Chapter 5):
   - `Closed solid. -- volume ...` -- you have a valid stone. Continue to
     step 8.
   - `Cannot solve: no scale-reference tier for: <block>` -- that block
     still has no anchor. Add one more **Exact scale value** tier to it
     (for the pavilion, a common choice is a pavilion depth or culet-point
     dimension) and Solve again.
   - `Degenerate` or `Unbounded` -- check the angle and constraint on the
     tier you most recently added; a facet meeting the wrong neighbour, or
     an angle too shallow to intersect its neighbours, is the usual cause.

## Step 8: Check the orbits

For each multi-index tier (girdle, pavilion mains, crown mains), check the
**ORBIT** column reads `orbit x8`. If it instead reads something like `6/8
orbit` in bold amber, an index position is missing or inconsistent --
compare the tier's Indices field against the intended list.

## Step 9: Optional -- yield and rendering

1. In the Yield panel, set **Girdle Diameter (mm)** to a real size (e.g. the
   girdle half-width you set in Step 2, doubled and converted to
   millimetres) and pick a **Yield Material** for the carat-weight estimate
   (this is a separate control from the Design Settings panel's own
   Material combo -- see Chapter 6's note on the two). Click **Apply Yield
   Inputs**, then Solve again -- Volumetric Yield and Est. Carat Weight
   should now show values.
2. Switch to the Live Render tab to see the stone rendered -- with "Linked
   to design" on (the default), it already shows the Diamond you picked in
   Step 6. Pick a lighting preset and check the brilliance/windowing/
   extinction readouts.

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
