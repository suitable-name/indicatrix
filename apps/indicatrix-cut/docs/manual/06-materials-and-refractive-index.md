# 6. The Design Settings Panel: Material, Refractive Index, Gear and Symmetry

## What you will do

This chapter is about a question every cutter eventually asks: "I like this
cut, but I want to cut it in a different gem material -- what do I need to
change?" The Design Settings panel (above the tier list in the Edit tab) gives
a design a single, editable material, refractive index, index gear and
symmetry. This chapter explains what each control does, and then covers
**Retarget**, the command bar's automated proposal for adapting a design's
angles to a different material.

## What refractive index is, briefly

**Refractive index (RI)** measures how strongly a material bends light. It
determines a stone's **critical angle** -- the angle, measured from a
facet's normal, beyond which light hitting that facet from inside the stone
reflects internally instead of escaping. A cutting design's pavilion angles
are chosen so that light entering through the crown strikes the pavilion
facets steeper than the critical angle, reflects internally (rather than
leaking out the bottom, which shows as **windowing** -- a washed-out,
see-through patch) and exits back out through the crown towards the viewer.
**Birefringence** is a related but separate property: some materials split
light into two rays with slightly different refractive indices, part of
what gives certain gems (peridot, zircon, and others) their characteristic
doubling.

A cutting design that performs well in one material's RI will not
necessarily perform well in another -- a design cut for diamond (RI about
2.417) at a given pavilion angle may window badly if cut in quartz (RI
about 1.54) at the same angle, because quartz's critical angle is larger.

## The Design Settings panel

Above the tier list, the Edit tab shows:

- **Material** -- a combo box listing, in order: `(none)`, the built-in
  presets (Diamond, Sapphire, Ruby, ...), any custom materials you have
  saved in the catalogue's material editor, and a final `Custom RI...` entry.
  Picking a built-in or custom name sets the design's material by name;
  picking `Custom RI...` (or `(none)`) clears the name so a typed RI
  override is the only thing describing this design's optics -- useful for
  a species with no built-in preset at all (garnet, for instance).
- **RI Override** -- blank uses the selected material's own refractive
  index; typing a number (greater than 1.0) overrides it. This is the same
  override an untouched catalogue import may already carry internally (see
  "Loading a design" below) -- typing here does the same thing by hand.
- **Effective RI** / **Critical Angle** -- read-only. Effective RI is
  exactly what a subsequent "Export .asc"/"Save Native" writes to the
  schedule's `I` line: the override when set, else the selected built-in's
  own n_D, else the schedule's legacy recorded value. Critical Angle is
  `arcsin(1 / effective RI)` in degrees. **Note:** a *custom catalogue*
  material's own real RI is used for the optimizer, the tilt curve, and the
  viewport (when linked -- see below), but the effective-RI/export figure
  above only ever derives from a built-in name or an explicit override, not
  from a custom material's own number, unless you also set an RI override to
  match it. This mirrors a documented limitation in the underlying material
  model (see the manual's Troubleshooting chapter) rather than a bug in this
  panel.
- **Index Gear** -- 96, 80, 77, 72, 64, 120, or Custom. Changing it opens a
  **remap confirmation** listing every tier's old and new indices; a
  position that does not land on a whole tooth after the change (e.g. moving
  from 96 to 80 teeth is not always evenly divisible) is shown in red.
  Nothing is applied until you click **Confirm Remap** -- Cancel leaves the
  design untouched. Confirming applies two separate, independently-undoable
  edits (the index remap, then the new gear/symmetry/mirror), so Undo can
  step back through them one at a time.
- **Symmetry Order** / **Mirror** -- how many repeats the design has around
  the stone, and whether each repeat is also mirrored. **Apply Symmetry**
  applies both together without touching the gear or re-deriving any index.

Every one of these is a real, undoable `indicatrix_cut_core::Edit` -- Undo/Redo cover
material, RI, gear and symmetry changes exactly like a tier edit.

## The tier list's MARGIN column

Each pavilion tier (a tier with a negative angle) now shows how many degrees
its authored angle sits above the design's own effective critical angle,
colour-coded:

- **Green** (Safe) -- 2 degrees or more of margin.
- **Amber** (Marginal) -- less than 2 degrees, but still past the critical
  angle. A small edit, a manufacturing tolerance, or a different material
  could tip this into windowing.
- **Red** (Windows) -- already below the critical angle for the design's
  current effective RI. Light leaks straight through this facet.

Crown and girdle tiers (angle zero or positive) show a plain dash -- this
column only means something for a pavilion facet.

## The viewport's "Linked to design" checkbox

The Live Render tab's Render Material dropdown has a small **"Linked to
design"** toggle next to it, on by default. While it is on, the dropdown
follows the Edit tab's own Design Settings material automatically -- pick a
new material there, and the viewport (and the optimizer, and the tilt curve
cache) all update to match, so what you render is always what you are
editing. Turn it off to pick an independent render material without
touching the design at all, exactly like before this panel existed.

## Loading a design: the material suggestion

Loading a catalogue design never changes the schedule's own recorded RI on
its own. If the schedule's RI is within 0.01 of a built-in preset's own n_D,
the status strip's Log (Chapter 5) offers **"Set material to X (RI ...)?"**
-- accepting it sets the design's material to that preset (for the
optimizer/tilt-curve/viewport to use its real dispersion), but if that
preset's own n_D would otherwise move the *exported* RI by more than 0.01,
the app pins an explicit RI override to the schedule's own original value
first, so accepting the suggestion never silently changes what a
subsequent export writes. Dismissing it ("No Thanks") leaves the design
exactly as loaded, with no material name set at all.

## Retarget: an automated proposal

Click **Retarget...** on the command bar's second row (Chapter 3) to open
a dialog that proposes new pavilion and crown angles for a different
material, for you to review before anything is applied.

- **Target Material** -- a combo of built-in and custom materials, plus a
  **Custom n_D** field for a typed refractive index. It starts on the
  design's own current material. A readout below shows exactly what the
  picker resolves to: the material's name, its n_D, and its critical
  angle.
- **Mode** --
  - **Shift** (the default) -- a deterministic move that keeps every
    pavilion tier's own margin over the critical angle fixed at what it
    already was. Always available, and fast.
  - **Optimize** -- seeds from the same shift, then runs the same
    coordinate search Chapter 8 describes over the design's free tiers, to
    find the target material's own best score for windowing, extinction,
    and tilt brilliance. Runs in the background with a progress readout
    that names its current stage (e.g. "Optimizing... 12 of ~200
    evaluations, 1.4s elapsed" -- see Chapter 8 for what each stage means);
    **Cancel** stops the search without closing the dialog.
- **Crown Handling** -- how much of the pavilion's shift the crown tiers
  follow: a **Fraction of shift** slider (0% leaves the crown untouched,
  which is common faceting practice and the default), or a **Scale crown
  by ratio instead** checkbox, which scales every crown tier's own angle
  by the ratio of the two materials' critical angles rather than following
  the pavilion's shift.
- **The proposal table** -- one row per affected tier: its block, name,
  old angle, new angle, margin, and risk badge, so you can see exactly
  what would change and whether it still reads Safe before committing to
  anything.

Two things stop a proposal from being built at all, shown in place of the
table: **"Design does not solve: ..."** when the design does not currently
close, and a list of tiers to **Adopt** first when Optimize mode needs a
tier that is still pinned as an imported scale reference (Chapter 8) --
Retarget's Optimize mode can only move free tiers, exactly like the
standalone Optimize.

Click **Apply** to commit the whole proposal as **one** undoable edit --
the material change and every tier's angle shift together, so a single
Undo reverts both at once. Applying re-solves the design; check the status
strip afterward the same as any other edit.

## Doing it by hand

Retarget's Shift mode already automates the mechanical part of this, but
understanding the reasoning is worth knowing, and Design Settings' MARGIN
column is what you would watch either way:

1. Load the design and note its current pavilion main angle(s) and its
   effective RI (the Design Settings panel's own readout).
2. Work out the critical angle for the target material (critical angle =
   arcsin(1 / RI), measured from the facet's normal). A higher RI gives a
   smaller critical angle; a lower RI gives a larger one.
3. Set the Design Settings panel's Material combo to the target material
   (or type a Custom RI). Watch the tier list's MARGIN column: any pavilion
   tier that turns red now windows at the new RI.
4. On the inspector's Tier tab (Chapter 4), select each red or amber
   pavilion tier in turn and edit its **Angle (deg)** field so the MARGIN
   column reads green again, then **Save Tier**.
5. Click **Solve** (Chapter 5) and check the status strip still reads
   "Closed solid."
6. With "Linked to design" on, the Live Render viewport already shows your
   target material -- check the **windowing** and **extinction** readouts
   (Chapter 2) at a range of tilt angles using the tilt-performance graph.
   Iterate on the crown and pavilion angles until performance looks right.
7. Optionally, once at least one tier is a free (meet-based) tier rather
   than a pinned scale reference (see Adopt, Chapter 8), run **Optimize** to
   fine-tune angles for windowing/extinction/tilt-brilliance under the
   design's own material -- Optimize resolves the SAME material (built-ins,
   custom catalogue materials, and any RI override) the viewport and tilt
   curve use, so there is no separate "which material did Optimize actually
   score against" question.

## Next steps

Continue to Chapter 7 for a full worked example of building a design from
nothing, using the New Design dialog and putting the tier form, constraints,
and Solve together in practice.
