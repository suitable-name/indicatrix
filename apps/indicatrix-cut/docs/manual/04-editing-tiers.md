# 4. Editing Tiers

## What you will do

This chapter covers the inspector's Tier tab: adding a new facet tier,
editing an existing one, detaching a tier (or a single facet within it)
from its symmetric family, reordering and removing tiers, and undo/redo.
It also covers the tier list's faster paths — inline angle editing,
keyboard nudging, duplicating a tier, and navigating the list from the
keyboard — for when you're adjusting angles on an already-built design
rather than typing a whole tier from scratch.

## The inspector

Below the tier table, a resizable, collapsible inspector panel has five
tabs: **Tier**, **Preform**, **Optimize**, **Schedule**, and **History**.
Clicking a tab pill also re-expands the panel if it was collapsed. This
chapter covers the Tier tab in full and the Preform tab's proportions/yield
fields; Optimize is Chapter 8, Schedule is covered in Chapter 3, and
History is Chapter 18.

Each tab pill is a tab stop with a hover tip, and a screen reader announces
it as a tab, picked or not. A small **?** button at the right of the tab row
opens the manual page for the open tab (the Tier tab opens the tier form, the
concave form opens "Adding a concave tier," and so on). The **Simple**
interface (Chapter 17) shows four tabs: the **Schedule** tab, which is a
read-only listing for the cutting sheet, is left out until you switch to
Advanced. The controls inside the other tabs that only matter to
experienced cutters are hidden in the same way, and each tab says so with
"Some advanced settings are in use. Switch to Advanced to see them." when a
hidden control holds something other than its default.

## The tier form

The Tier tab has these fields:

- **Angle (deg)** — the facet's cutting angle off the girdle plane. You
  can type a plain number (`41.5`) or a small calculation (`41.5+0.3`,
  `(90-41)/2`); the app works it out and keeps the result. Start the text
  with `=` and the angle is not a fixed number any more but **follows other
  tiers** — see "Relations between tiers," below. With the cursor in the
  field, **Up** and **Down** change a plain number by 0.1°, with **Shift**
  by 1° and with **Ctrl** by 0.01°. The new text is worked out by the app and
  written with two decimals and a decimal point, the same on every computer.
  A blank field steps from 0. On text that is not a plain number (a
  calculation, or a relation starting with `=`) the keys do nothing, so they
  never overwrite what you typed. Stepping only changes the form; nothing is
  saved until Save Tier.
- **Meets** — a drop-down with six choices that decide how the facet's
  depth is worked out:
  - **Unspecified vertex** — the facet is cut until its plane meets some
    vertex of the surrounding geometry, with no vertex named. Use this for
    a straightforward meet where nothing needs to be spelled out (a first
    culet facet, a simple meet against whatever's already there).
  - **Named facet(s)** — the facet is cut until its plane meets a vertex
    defined together with specific other facets, which you type into the
    field that appears (comma-separated facet names, e.g. `P1, P2, G1`).
  - **Exact scale value** — you type a real number directly: an authored
    dimension, not a derived one. This is how you set an **anchor** (see
    Chapter 3). The field's own hint reads "a real dimension, e.g. girdle
    half-width." A small calculation works here too (`0.5+0.15`).
  - **Cut to depth (mm)**, **Girdle thickness (mm)**, **Table width (mm)** —
    the three real-world **targets** — see "Targets: cut to depth, girdle
    thickness, table width," below.

  The Simple interface leaves **Exact scale value** out of this list, because
  an anchor is an authoring tool most designs never need. A tier that
  already uses it still shows it (and so does the **Girdle Facet Preset**
  button's result), so nothing is ever hidden from a tier that depends on it.
  The **Quick add** buttons (Table, Girdle, Culet) and a row's **Add Anchor**
  button still make anchors in the Simple interface. Switch to Advanced to
  pick Exact scale value for any tier.
- **Name** — the facet's name, referenced by other tiers' "Named facet(s)"
  field and shown in the tier list and cutting instructions.
- **Indices** — which index position(s) on the gear this tier occupies.
  A single value for one facet, several for a symmetric family (e.g.
  `0, 12, 24, 36, 48, 60, 72, 84` for an 8-fold family on a 96-tooth gear).
  Separate the values with commas, spaces or semicolons. Hover the label
  for a reminder.

  The index wheel is a ring, so the valid range runs from `0` up to **and
  including** the gear's tooth count. On a 96-tooth gear, `96` is allowed
  and means the same position as `0` (the app keeps the number exactly as
  you typed it, so a file that writes `96` still saves as `96`). A number
  above the tooth count, or below `0`, is refused with a message that
  names the range ("Indices run from 0 to 96 on this gear."). Listing the
  same position twice is refused as well, and `0, 96` counts as a repeat
  because both name one position.

  Two shorthands save typing. `start:step:stop` counts from `start` in
  steps of `step` and stops before `stop`, so `0:12:96` gives the eight
  values `0, 12, 24, 36, 48, 60, 72, 84`. A number followed by `xN` spreads
  `N` evenly spaced copies around the wheel, so `12 x8` gives
  `12, 24, 36, 48, 60, 72, 84, 0`. Values the shorthands generate wrap
  around the wheel, so they never run off the end.

When a save is refused, the message appears directly under the field it is
about (Angle, the Meets value, Name or Indices), and that field gets a red
border. Start typing in the field and the message clears. A message that
belongs to no single field appears at the bottom of the form.

A **Girdle Facet Preset (90°, scale = 1)** button sits with the form
fields. Click it to fill Angle, Meets, and the scale value in one step —
`90`, **Exact scale value**, and `1` — the exact combination Chapter 7's
worked example sets by hand for its own girdle tier. It only fills the
form; you still type the Name and Indices and click **Add Tier** yourself.
Use it any time you are about to author a girdle anchor and want the
three number fields right on the first try, rather than typing `90` and
remembering that the girdle's own scale convention is a half-width, so `1`
is a natural starting value.

The tab's title reads "Add Tier" when you are creating a new row, or "Edit
Tier #N" when editing an existing one (click a row, or select it any other
way — Chapter 3 — to load it here). The save button at the bottom of the
tab reads **Add Tier** in the first case and **Save Tier** in the second; a
**New** button next to it clears the form back to add-mode without
touching the tier list.

A new tier can also be started without the form: the Solid viewport's
**Slice** mode lets you draw a line across the stone and adds the tier it
defines, snapped to the index wheel, in one undo step (Chapter 13, "Slicing a
new facet with the mouse").

**Saving does not re-solve.** Clicking Save Tier (or Add Tier, or Remove,
or Apply Preform) updates the tier list's editable columns
immediately, but deliberately does *not* run a fresh solve — that is
Chapter 5's job, and it can take real time on a large design. Instead the
MAST and SOLVE columns, and the status strip, are marked visibly stale
until you click **Solve** (or until auto-solve runs, if you turned it on).
Undo and Redo follow the same rule; their exact behaviour is under "Undo and
redo," below.

For an already-saved tier, the Tier tab also shows a read-only **Solved**
section once you have loaded that row: its mast, its block, its SOLVE
strategy (with the same one-line explanation the tier table's own tooltip
shows), its margin over the critical angle, its orbit shape, which other
tiers it actually meets (by name, e.g. "meets tier 3 (C1)"), and its own
manufacturability warning, if it has one.

## The angle slider and the Typical menu

Under the **Angle (deg)** field, a pavilion or crown tier has a slider and a
**Typical** menu. They are a quicker way to fill in the same field. The field
stays: you can still type or calculate in it, and the slider follows whatever
the field says. You get both in the Simple and in the Advanced interface.

- **Dragging.** The slider runs from 30° to 55° for a pavilion and from 15° to
  55° for a crown. A drag snaps to 0.05° and lands on a mark when it passes
  close to one; marks are drawn every 0.5°. Hold **Shift** while you drag for
  fine mode: the slider moves ten times slower and snaps to 0.01°. Pressing on
  the rail away from the round handle jumps the handle there.
- **Keyboard.** Click the slider (or Tab to it). The arrow keys then move the
  angle by 0.05°, **Shift** with an arrow by 0.01°, **Page Up** and
  **Page Down** by 0.5°, and **Home** and **End** to the two ends.
- **The green band.** On a pavilion the slider marks a green band from 1° to
  6° over the stone's critical angle. The critical angle comes from the
  design's refractive index, which is the material you chose in Design
  Settings (Chapter 6). A line under the slider gives the numbers, for example
  "Green band: 41.4° to 46.4°". A high-index stone such as diamond has its band
  below 30°, so the slider then starts lower, at 25° for diamond, to show the
  whole band. On a crown, the line under the slider gives the usual crown
  range for that kind of stone instead.
- **Typical.** The menu lists common angles, each with a one-line reason.
  For a pavilion it offers **Standard for** your material (the middle of the
  published range for that kind of stone, raised when that would leave less
  than 2° over the critical angle), **Critical angle + 2° (bright, safe)** (the
  lowest angle the margin bar calls Safe) and **Critical angle + 4° (extra
  margin)**. For a crown it offers **Standard for** your material, **Low
  crown** (25°), **Medium crown** (32°) and **High crown** (40°). Choosing an
  entry only fills in the Angle field. An entry that gives the same angle as an
  earlier one is left out.

The slider does not save anything. Dragging it or choosing a preset changes the
form exactly as typing does: the margin bar under the field follows along, and
**Save Tier** (or **Add Tier**) still commits the tier as one undoable edit. The
slider writes plain numbers with two decimals. If the field holds a pavilion
angle as a plain number, the margin bar reads it as a pavilion angle, so the
bar is right whether you drag, type or choose a preset.

A tier has no slider when its angle is not a free number: a girdle tier (90°),
a flat tier (the table and the culet), and a tier whose angle follows other
tiers (its Angle field starts with `=`; see "Relations between tiers"). The
slider dims and stops reacting while the field holds text it cannot show, such
as `41.5+` typed half way. A calculation that works out, such as `41.5+0.3`, is
shown at its result, and dragging then replaces it with a plain number.

A new tier counts as a pavilion when its Name starts with P (the rule Save Tier
uses), so you type just the plain angle; a minus sign typed in front of the
angle does the same. Otherwise the slider treats it as a crown. The tier table
then shows the angle as a plain positive number with its side in the letter of
the CODE column.

## Losing an unsaved draft

If you are still typing into the tier form — an in-progress "Add Tier"
draft, or an edit to an existing tier you have not saved yet — and the
selection then moves to a different tier from somewhere else (a row click
elsewhere, a facet click in the 3D view or the diagram, Undo/Redo, or Load
Selected), the app does not silently throw your draft away. A small amber
notice appears in the Tier tab: "You have unsaved changes to..." with two
buttons, **Keep Draft** (leaves your typed values exactly as they are; the
table's highlight may already show a different row, but the form still
shows your draft) and **Discard Draft, ...** (loads the new selection,
discarding what you typed).

## Per-facet editing

Once you have an existing tier loaded, the Tier tab shows a row of small
chips below the form fields, one per index position the tier occupies. If
the tier has more positions than fit, the row scrolls sideways. Each chip
carries two small buttons, each at least 20 pixels square, with a hover
tip:

- The broken-chain button detaches *that one facet* from its symmetric
  family, leaving the rest of the tier's indices exactly as they were. Once
  detached, the chip shows a whole chain instead, and clicking it
  reattaches the facet. This is separate from, and more precise than, the
  whole-tier Detach/Reattach button below. The Simple interface leaves
  this button out.
- The cross button removes just that one facet from the tier.

The whole row of chips is **one Tab stop**, so Tab does not have to walk past
every chip to reach the fields below. With the row focused, **Left** and
**Right** move along the chips (**Home** and **End** jump to the first and the
last), **Enter** detaches or reattaches the chosen facet (Advanced only), and
**Delete** removes it. The chosen chip gets a coloured frame while the row
has the keyboard, and a screen reader announces the row as a list with the
chosen facet.

Below the chip row there are two rows of controls:

- A **position** field plus **+ Add** adds one more index position to the
  tier, together with the other facets that go with it in the stone's
  symmetry. **Mirror** reflects every one of the tier's indices about
  position 0.
- (Advanced) A **teeth** field and two **Rotate** buttons turn every one of the
  tier's indices around the gear by that many teeth. The button with the
  counter-clockwise arrow moves them up to higher index numbers; the one
  with the clockwise arrow moves them down to lower ones. Hover a button to
  see which is which. The Simple interface leaves this row out.

The position field and the teeth field take a plain number or a small
calculation (`24`, `96/4`, `(2+1)*2`), worked out by the app the same way as
the Angle field. If the text cannot be read, a message in the corner says why
and nothing changes. Pressing **Enter** in the position field is the same as
clicking **+ Add**.

None of these touch the design until you act on them — unlike the
Angle/Meets/Name fields above, they apply immediately as their own undoable
edit, one action at a time.

## Detach and Reattach

When a tier's Indices list names more than one position, the app treats
those positions as one symmetric family (an orbit — Chapter 3). By default,
editing that tier and clicking Save Tier is meant to move the *whole*
family together, keeping the design symmetric.

The **Detach** button on a tier's row exempts every one of that tier's
positions from this automatic orbit-consistency handling — the button then
reads **Reattach**, letting you put it all back. Use it when you genuinely
want a deliberately asymmetric design; leaving a tier attached is the
correct default for anything meant to stay symmetric. For detaching a
*single* facet out of an otherwise-attached tier, use the per-facet chip
controls above instead.

Editing a detached tier's other fields and clicking Save Tier rebuilds the
tier from the form and clears the Detach flag back to attached. Only the
dedicated Detach/Reattach button itself preserves the flag across other
edits — if you need a tier to stay detached, re-detach it after any Save
Tier edit.

**In the Simple interface** the Detach/Reattach button on a row, the per-facet
detach buttons on the chips, the Cheater Offset row and the imported-meet
details are hidden, not removed. Nothing about the design changes when you
switch between Simple and Advanced. When the tier you are looking at is
detached, has a cheater offset or has an imported meet, the Tier tab says
"Some advanced settings are in use. Switch to Advanced to see them." and the
table's toolbar says the same when any tier is detached.

## Preform, proportions, and yield

The inspector's **Preform** tab holds three groups, none of them tied to
any one selected tier — they describe the whole design. The Simple interface
keeps the shape, Half-Width, Length / Width, Depth, the proportions and the
Girdle Diameter; it hides the Girdle Y-Offset, the Specific Gravity Override,
the line naming where the effective RI came from and the Specific Gravity the
estimate used. They keep their values, and a line reads "Some advanced
settings are in use. Switch to Advanced to see them." when the Y-Offset or the
override holds one.

- **Preform** — the rough's shape (Block or Cylinder), Half-Width, Length /
  Width, Depth and the Girdle Y-Offset, with an **Apply Preform** button.
  Every number field takes a number or a small calculation (`1.2+0.1`,
  `3/2`), and each of the four has a slider under it:

  | Field | Slider range |
  | --- | --- |
  | Half-Width | 1 to 3 |
  | Length / Width | 1 to 2.5 |
  | Depth | 1 to 3.5 |
  | Girdle Y-Offset (mm) | plus and minus half the Depth, in millimetres |

  The stone's girdle half-width is 1 in these units, so a rough is never
  narrower than 1. Block and Cylinder roughs use the same ranges, because both
  have to enclose the same stone. The sliders move in steps of 0.05 (0.01 with
  **Shift**) and take the same keys as the angle slider (see "The angle slider
  and the Typical menu"). The Y-Offset slider works only once a **Girdle
  Diameter (mm)** is set in the Yield group, because its range is in
  millimetres; until then it is dimmed and says so. A slider only fills its
  field in and marks the tab "not applied yet" (the **Apply Preform** button is
  highlighted and shows an asterisk); nothing changes in the design until you
  click **Apply Preform**. A slider dims when its field holds text that is not a number.
- **Proportions** — the figures a cutter actually quotes, worked out from
  the last Solve: table size and length-to-width on one line, then Crown
  Height, Pavilion Depth, and Total Depth each on their own line. Every one
  of these reads "-" rather than a misleading zero whenever the design does
  not currently solve, or has no girdle plane to measure a depth from.

  Table %, Crown Angle, Pavilion Angle, Total Depth %, and Girdle % each
  carry a small **verdict chip** next to the number — **Within**, **Near**,
  or **Outside** a reference window, with a one-line reason on hover. The
  windows depend on the design's own shape (only a round-brilliant-family
  schedule gets a dedicated window today — anything else falls back to the
  same generic range) and material band (diamond's own tighter AGS/GIA
  "Excellent" round-brilliant ranges above RI 1.8; a wider, widely published
  lapidary rule-of-thumb range — 40-43° pavilion, 30-40° crown — below it).
  These are guidance, not a grading report: a chip reading Outside is a
  prompt to look closer, not a verdict on the design's worth. No chip is
  shown at all when there is nothing to judge yet (the design does not
  currently solve/close).
- **Yield** — the effective-RI readout and its source, Girdle Diameter
  (mm), a Specific Gravity Override for the carat estimate (blank uses the
  specific gravity of the material set in Design Settings — Chapter 6; both
  fields take a number or a small calculation, such as `6.5+0.25`, and have no
  slider because they are measured or looked up, not tuned), an
  **Apply Yield Inputs** button, and the resulting
  Volumetric Yield, Est. Carat Weight, and Specific Gravity Used, each
  blank until the next Solve.

## Inline angle editing

The tier list's ANGLE column is itself editable — you don't have to load a
tier into the Tier tab just to nudge its angle. A single click on the angle
cell **selects that row**, the same as clicking anywhere else in it (see
"Selecting a tier" in Chapter 3). To edit the value in place,
**double-click** the cell (or select the row and press **F2**): it turns
into a text field, and you type a new number, then either press **Enter**
(commits) or click elsewhere to move focus away (also commits). Press
**Escape** to back out without changing anything.

Typing something that isn't a valid number is rejected with a toast and the
cell reverts to the tier's real angle — it never silently keeps invalid
text on screen. Committing the exact same value the tier already has is a
silent no-op: it doesn't spend an undo step.

The cell takes the same kinds of text as the form's Angle field: a number,
a small calculation (`41+0.5`), or a calculation that names another tier
(`P1-2` means "the angle of P1, minus 2", worked out once). Text that starts
with `=` (`=P1-2`) makes the angle *follow* P1 instead — see "Relations
between tiers."

A tier whose angle follows a relation shows a small link icon in this cell,
and its tooltip reads "Angle follows: P1 - 2." Double-click and F2 do not
open an editor on such a cell — they show the hint "This angle follows a
relation. Edit it in the Tier form." instead.

Every commit here is a real, undoable edit — press **Undo** to step it back
like any other change — and, like every other tier edit, it marks the
design stale (MAST/SOLVE/the status strip) until you next click Solve, or
until auto-solve picks it up if it's turned on (Chapter 5).

## Nudging an angle with the keyboard or scroll wheel

With the inline angle cell open for editing (or with focus in the tier
form's own **Angle (deg)** field), the arrow keys step the value without
retyping it:

| Keys | Step |
| --- | --- |
| Up / Down | ±0.1° |
| Shift+Up / Shift+Down | ±1° |
| Ctrl+Up / Ctrl+Down | ±0.01° |

In the tier list the arrows change the number the ANGLE cell shows: Up makes it
bigger and Down makes it smaller, for a crown tier and a pavilion tier alike. A
pavilion tier's angle is stored as a negative number, but the list prints it
without the minus sign (the letter of the CODE column says which side it is on), and the
arrows follow what you see; the minus sign stays. A tier stops at 0° on its own
side instead of crossing into the other block.

The scroll wheel over the tier list's inline angle cell does the same
±0.1° step, but **only** while that cell is already open for editing, or
while you hold **Ctrl**. An ordinary scroll over the angle cell otherwise
just scrolls the tier list, like scrolling over any other cell — it no
longer edits the angle by accident.

**Inline cell vs. the form field.** A nudge on the tier list's inline
angle cell applies immediately as a real, undoable edit against that tier
— exactly like committing a typed value. A nudge on the Tier tab's own
Angle field, by contrast, only adjusts the form's scratch text; nothing is
applied to the design until you click **Save Tier**. The form is a staging
area for a tier you may still be composing (including a brand-new "Add
Tier" draft that doesn't exist in the design yet to nudge), so its nudge
step stays local until you explicitly save.

Nudging the same tier repeatedly in quick succession (within about half a
second between keystrokes/scroll ticks) collapses into a single undo step,
so pressing Undo once after a burst of nudges reverts the whole burst, not
just the last step.

You can also change a tier's angle by dragging a handle on its facet in the
Solid viewport, with snapping and live feedback; see Chapter 13, "Dragging a
facet: the angle, depth and index handles".

## Duplicating a tier

Click the small duplicate button on a tier's row (the two-sheets icon), or select a
row and press **Ctrl+D**, to append a copy of it to the end of the tier
list. The copy keeps the same angle, indices, and constraint, gets the same
name with a trailing apostrophe (e.g. `P1` → `P1'`), and the tier list's
selection moves to the new copy so you can immediately start editing it.
Duplicating is one undoable `Add Tier` edit, like typing a new tier by hand
and clicking Add Tier.

## Reordering tiers

The small up and down arrow buttons on a tier's row (and the same pair on
the command bar), or **Alt+Up** / **Alt+Down** on the
keyboard-selected row, swap that tier with its neighbour in the table. Each
swap is its own undoable edit. The table order decides the cutting order
inside each side of the stone: the pavilion and girdle tiers are cut first, in
table order, then the crown tiers, and the table tier last, wherever it sits in
the list. A tier that meets a facet further down its side of the list is still
cut after that facet: the cutting sheet puts it just after the last facet it
meets (Chapter 11).

## Keyboard navigation in the tier list

Click anywhere in the tier list to give it keyboard focus, then:

| Key | Action |
| --- | --- |
| Up / Down | Select the previous/next row (this *is* a real selection — see below — not just a cursor) |
| Home / End | Select the first/last row |
| Page Up / Page Down | Select 10 rows back/forward |
| Enter | Select the highlighted row (redundant with Up/Down, kept for habit) |
| Delete | Remove the highlighted row (Backspace does **not** do this — it is left free for text fields) |
| Ctrl+D | Duplicate the highlighted row |
| Alt+Up / Alt+Down | Move the highlighted row up/down in the table (and so in the cutting order of its side of the stone) |
| F2 | Open the highlighted row's angle cell for inline editing |
| Space | Add/remove the highlighted row from the multi-select group (the keyboard twin of Ctrl+click) |
| Ctrl+click a row | Add/remove that row from a multi-select group |
| Shift+click a row | Select the whole range from the last-selected row to this one |
| Tab | Move on to the buttons of the highlighted row, then out of the list |

Unlike an older version of this app, Up/Down/Home/End/Page Up/Page Down all
select immediately — there is no separate "keyboard cursor" that needs a
following Enter to actually load the row. The window's other global
shortcuts (Ctrl+Z/Y/S, Ctrl+F, Ctrl+1/2/3, F5, Escape — Appendix B) still
work while the tier list has keyboard focus.

The whole list is **one Tab stop**, so a long list does not put a Tab stop on
every button of every row between the list and the rest of the window. Tab
from the list walks the buttons of the row you are on (Adopt, Pin, Detach,
Anchor, the move arrows, Duplicate and Remove, as far as the interface shows
them); the other rows' buttons are not Tab stops, though a mouse or a screen
reader still reaches them. Every button has a hover tip, a coloured ring while
it has the keyboard, and answers Space and Enter. A screen reader announces the
list as "Tiers" and each row with its name, angle and state. In the Simple
interface (Chapter 17) the Mast and Orbit columns, the Adopt, Pin and Detach
buttons and the Steps / Mirror tools are left out.

## Selecting several tiers for a batched nudge

Ctrl+click a tier row to add it to a multi-select group (Ctrl+click again
to remove it) — selected rows get a cyan outline, and a small bar above the
table shows "N selected" with **Clear** and **Delete** buttons once two or
more are selected (Chapter 3).

Shift+click a row to select the whole range between it and whichever row
you selected last, inclusive of both ends, the spreadsheet convention.
Unlike Ctrl+click, which only ever adds or removes one row, a Shift+click
always replaces the current selection with the new range. If no row has
been selected yet in this session, Shift+click just selects that one row;
there is nothing yet to range from.

With two or more tiers selected, nudging
*any one of them* (inline cell arrows/wheel, not the form's field) moves
every selected tier's angle together, by the same step, as a single
undoable edit — one Undo reverts the whole group's nudge at once. Delete on
the "N selected" bar removes every selected tier as one Undo step per tier.

The bar also has an **Offset** box and button that move the whole group by a
number of degrees you type. Like the arrows, it moves the angles the list shows:
a positive number makes every selected angle bigger, crown and pavilion tiers
alike. The box takes a plain number or a small
calculation (`0.5`, `-0.25`, `1/4`, `0.3+0.2`), worked out by the app the same
way as the Angle field, with a decimal point on every computer. If the text
is empty or cannot be read, a message in the corner says so and nothing
changes. The offset moves the group the cursor row belongs to, so click one
of the selected tiers first; otherwise the message says to do that. Like any
nudge it is one undoable edit.

**Note:** the cyan outline disappears as soon as you actually nudge the
group (a nudge is itself an edit, and every edit refreshes the tier list) —
but the selection it represented keeps applying underneath, so a second
nudge right after still moves the same set of tiers together. Only the
visible outline needs a fresh Ctrl+click to reappear; the batching itself
isn't affected.

## Removing a tier

Click the **×** button on a tier's row, or select it and press **Delete**,
to remove it. Like every other edit in this list, removing a tier marks the
design stale; click Solve afterwards to see whether it still closes.

## Undo and redo

The **Undo** and **Redo** buttons step backward and forward through your
edit history (adding, saving, removing, detaching, reordering, and
per-facet editing of tiers; applying a preform; adopting a meet; a
Retarget; applying an Optimize result). They are greyed out when there is
nothing to undo or redo in that direction, and their hover text (and the
Edit menu) says exactly what they will do — "Undo: Set P1 angle to
41.0 degrees," for instance — instead of a bare "Undo."

What Undo and Redo do, step by step, and what they leave for you:

- **They do not run Solve.** The design is put back exactly as it was, but
  the numbers that come from a solve are not recomputed. The table's MAST
  and SOLVE columns, the solved readout in the Tier tab and the status strip
  are marked stale, the same way they are after Save Tier. A Deep Solve or
  Optimize result that is still on screen is marked "Stale: design
  changed". Click **Solve** to bring them up to date; if you turned
  auto-solve on (Chapter 5), it starts by itself after a short pause, and
  only on a design small enough for your chosen time limit.
- **The 3D preview is rebuilt at once.** The Solid viewport shows the
  restored design straight away, so the picture can be ahead of the numbers
  for a moment.
- **Your selection stays if it can.** The selected row stays selected when
  its row number still exists after the step, and its form is filled in again
  from the restored design. If the step added or removed a tier above it, that
  row number now shows a different tier. When the row number no longer exists
  (undoing an Add Tier on the last row, for instance), nothing is selected and
  the Tier tab goes back to "Add Tier". Tiers you ticked for a group (Select)
  follow their tiers when the rows shift, and drop out when their tier goes.
- **A draft is not silently lost.** If you were typing into the form and the
  step changes the tier you are editing, the Tier tab shows a notice ("Tier #N
  changed in the design after you started this draft") with **Reload From
  Design** and **Keep Draft**; if the selection moves to another tier, the
  notice under "Losing an unsaved draft," above, asks what to keep.
- **One step at a time.** Each press moves exactly one entry of the history;
  the History tab (Chapter 18) can jump several at once.

To see every step at once, with a small picture of the design at each one,
and to go straight back to the one that looked good instead of pressing
Undo over and over, open the inspector's **History** tab (Chapter 18).

## Row identity and cheater offsets

Every row in the tier table carries a stable identity of its own
(`TierId`, `indicatrix-cut-core/src/design/tier_id.rs`) that survives
add/remove/move/undo — a manufacturability warning badges the row it is
about directly, instead of the badge silently drifting onto the wrong row
if you insert or remove a tier above it before reading the warning.

A cheater/azimuth offset you record on a tier (the same field the cut sheet
prints) rotates that tier's own facet plane(s)
about the vertical axis before the solid, the tracer and the diagram build
from it — a positive offset rotates counter-clockwise about the vertical
axis.

## Targets: cut to depth, girdle thickness, table width

A tier can carry an authoring-level **target** instead of a raw scale
value — three of the Meets combo's six choices:

- **Cut to depth (mm)** — the facet's own plane offset, stated directly in
  millimetres rather than the model-unit scale value **Exact scale value**
  uses. Converts through the design's own mm-per-unit scale in one pass: the
  design is solved once with this tier bootstrapped at mast `0.0` to measure
  its own width, the millimetre figure you typed is converted to a mast
  through that measurement, and that mast becomes the tier's real scale
  reference.
- **Girdle thickness (mm)** — searches (bisects) this tier's own mast until
  the *whole design's* measured girdle thickness matches the millimetre
  figure you typed. You do not have to author this on the actual girdle
  tier — it resolves the same way regardless of which tier carries it,
  though ordinarily you would put it there.
- **Table width (mm)** — searches (bisects) this tier's own mast until the
  table facet's own measured width matches the millimetre figure you typed.

All three need a **Girdle Diameter (mm)** set on the Preform tab's Yield
group (Chapter 6) — that is the one figure the mm-per-unit conversion is
built from. The girdle-thickness and table-width searches also cost more
than an ordinary edit: each is up to 24 solves (a bisection search) rather
than one, so expect Solve to take noticeably longer on a design with one of
these authored.

The tier table's MEETS column shows the target itself, led by a small
arrow (e.g. "3.20 mm depth," "girdle 0.25 mm," "table 4.10 mm"); the adjacent MAST/MAST(mm)
columns show the *resolved* scale reference once the design has solved, the
same as for any other tier. Changing the Meets combo away from a target
kind and saving clears the target — the tier goes back to being an ordinary
meet/scale-reference tier, exactly as if the target had never been set.

### When a target cannot be resolved

Two things can stop a target from resolving into a real mast; both surface
as the status strip's (and the Solve toast's) problem text, the same way a
missing anchor does:

- **No girdle diameter set.** *"cut-to-depth needs a girdle diameter — set
  one in Design settings."* Remedy: set **Girdle Diameter (mm)** on the
  Preform tab's Yield group, then Solve again.
- **The bisection search could not bracket the target.** *"tier N's target
  could not be bracketed — widen or remove it."* This means the millimetre
  figure you typed is outside what the design can physically produce (for
  example, a table width wider than the preform itself allows). Remedy:
  type a more plausible figure, or clear the target and author an ordinary
  **Exact scale value** instead.

## Relations between tiers

A **relation** makes one tier's angle follow other tiers' angles instead of
being a number of its own. If P2 follows P1 with the relation `P1 - 2`, then
P2 is always two degrees shallower than P1: change P1 and P2 moves with it,
by itself, in the same undo step. Use a relation for the things that are
meant to stay in step — a ladder of step cuts, a main that sits a fixed
number of degrees from its break, a facet halfway between two others.

### Typing a relation

In the Tier tab, start the **Angle (deg)** field with `=` and write what the
angle should follow, then press Enter or **Save Tier**:

| You type | The angle becomes |
| --- | --- |
| `=C1-4` | 4° less than tier C1 |
| `=(P1+P3)/2` | halfway between P1 and P3 |
| `=P1*0.5` | half of P1 |
| `=[Crown Main]-2` | 2° less than the tier called "Crown Main" |

A tier's name stands for its angle measured from the girdle plane, always
as the positive number the tier table shows — a pavilion tier at 41° counts as 41. The tier you
are editing keeps its own side, so a pavilion tier that follows a pavilion
tier stays a pavilion tier. Names are matched exactly first, then ignoring
capital letters. A name with a space in it goes in square brackets. You can
use numbers, `+ - * /` and round brackets.

Text that does not start with `=` is never a relation: `41.5+0.3` is
worked out once and the tier gets the number 41.8.

Once saved, the tier is marked in three places:

- the Angle field now shows the relation after the `=` (for example
  `=P1 - 2`), so you can edit it there;
- a small note under the field reads "Follows a relation" and gives the
  angle it works out to now, with a **Remove relation** button;
- the tier table shows a small link icon in the angle cell; hover it to read
  "Angle follows: P1 - 2."

To change the relation, type a new one in the Angle field and save. To stop
following, click **Remove relation**: the tier keeps the angle it has right
now and becomes an ordinary tier again. Each of these is one undo step; the
Undo button reads "Undo: Set P2 = P1 - 2" or "Undo: Clear relation for P2."

### What is refused

Nothing changes when the app refuses a relation; the reason appears in
plain words directly under the Angle field:

- **A loop.** P1 cannot follow P2 while P2 follows P1: "P1 and P2 refer to
  each other in a loop." A tier cannot follow itself either.
- **A tier that does not exist.** A name that no tier has, or that two
  tiers share, is named in the message.
- **An angle that is not a facet angle.** The result must be more than 0°
  and at most 90°. If a later change would push a follower out of that
  range (nudging P1 so far that P2 would come out at 91°), that change is
  refused, naming the tier that would break.
- **A tier that cannot follow anything.** A table or culet tier (0°) and a
  girdle tier (90°) are fixed by their meaning, not by other tiers.
- **A plain angle on a tier that follows a relation.** Typing 38 into the
  Angle field of P2 while it follows `P1 - 2` is refused: "This angle
  follows a relation (P2 = P1 - 2). Edit the relation or remove it." The
  rest of the form (name, indices, Meets) still saves as long as the Angle
  field still holds the relation, or the angle the tier already has.

### Nudging and dragging

A tier that follows a relation has no angle of its own to nudge. A
double-click or F2 on its angle cell, Ctrl+scroll over the cell, and the
**Offset** box of a multi-selection change nothing for that tier and show
the hint "This angle follows a relation. Edit it in the Tier form." In the
Solid viewport its **angle handle is
hidden** — only the depth and index handles are drawn — and the hint line
says why when you select the tier. If you nudge several tiers together,
the tiers that follow a relation are skipped and the rest are nudged as
usual.

The other direction works as you would hope: nudge or drag the tier that is
being followed, and every tier that follows it moves along. The whole move,
followers included, is one undo step.

### Removing a tier other tiers follow

Removing P1 while P2 follows it is allowed. P2 keeps the angle it has and
stops following anything; a message names the tiers that were freed ("Removed
the relation of P2; its angle stays as it is."). One Undo brings back P1 and
the relation together.

### A ladder that stays linked

The **Steps / Mirror** panel above the tier list generates a ladder of step
cuts. Tick **Keep linked** before **Generate** and every tier after the first
gets a relation to the first (`Step1 + 2`, `Step1 + 4`, and so on), so
"later changes to the first tier move the others by the same steps."
Without the tick the ladder is made of ordinary tiers, as before.

### Saving and exporting

Relations are saved with the design (Chapter 11 describes the file version).
An `.asc` file has no way to say "follows," so exporting one writes each
tier's current angle as a plain number.

### Numbers elsewhere

The same small calculations work in the other number fields of the Edit tab:
the Exact scale value, the Cheater Offset, the Preform Half-Width, Length /
Width, Depth and Y-offset, the Girdle Diameter and Specific Gravity Override
of the Yield group, the gear reference angle and the Symmetry Order (which
must work out to a whole number), the multi-select **Offset** box, and the
position box of **+ Add** and the teeth box of **Rotate** in the Tier tab.
Hover the Angle, scale value, Cheater Offset, Offset or a Preform label for a
reminder.

## Next steps

Continue to Chapter 5 to solve the design and read what its status
messages mean.
