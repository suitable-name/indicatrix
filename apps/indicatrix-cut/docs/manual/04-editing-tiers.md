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

Below the tier table, a resizable, collapsible inspector panel has four
tabs: **Tier**, **Preform**, **Optimize**, and **Schedule**. Clicking a tab
pill also re-expands the panel if it was collapsed. This chapter covers the
Tier tab in full and the Preform tab's proportions/yield fields; Optimize is
Chapter 8, and Schedule is covered in Chapter 3.

## The tier form

The Tier tab has these fields:

- **Angle (deg)** — the facet's cutting angle off the girdle plane.
- **Meets** — a drop-down with three choices that decide how the facet's
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
    half-width."
- **Name** — the facet's name, referenced by other tiers' "Named facet(s)"
  field and shown in the tier list and cutting schedule.
- **Indices (comma-separated)** — which index position(s) on the gear this
  tier occupies. A single value for one facet, several for a symmetric
  family (e.g. `0, 12, 24, 36, 48, 60, 72, 84` for an 8-fold family on a
  96-tooth gear).

The tab's title reads "Add Tier" when you are creating a new row, or "Edit
Tier #N" when editing an existing one (click a row, or select it any other
way — Chapter 3 — to load it here). The save button at the bottom of the
tab reads **Add Tier** in the first case and **Save Tier** in the second; a
**New** button next to it clears the form back to add-mode without
touching the tier list.

**Saving does not re-solve.** Clicking Save Tier (or Add Tier, or Remove,
or Apply Preform, or Undo/Redo) updates the tier list's editable columns
immediately, but deliberately does *not* run a fresh solve — that is
Chapter 5's job, and it can take real time on a large design. Instead the
MAST and SOLVE columns, and the status strip, are marked visibly stale
until you click **Solve**.

For an already-saved tier, the Tier tab also shows a read-only **Solved**
section once you have loaded that row: its mast, its block, its SOLVE
strategy (with the same one-line explanation the tier table's own tooltip
shows), its margin over the critical angle, its orbit shape, which other
tiers it actually meets (by name, e.g. "meets tier 3 (C1)"), and its own
manufacturability warning, if it has one.

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
chips below the Indices field, one per index position the tier occupies.
Each chip carries two small controls:

- A scissors/undo glyph — detaches or reattaches *that one facet*, leaving
  the rest of the tier's indices exactly as they were. This is separate
  from, and more precise than, the whole-tier Detach/Reattach button below.
- A cross glyph — removes just that one facet from the tier.

Below the chip row, a small field plus **+ Add** appends one more index
position to the tier, and **Rotate ↻** / **Rotate ↺** / **Mirror** rotate or
mirror every one of the tier's indices at once around the gear. None of
these touch `Design` until you act on them — unlike the Angle/Meets/Name
fields above, they apply immediately as their own undoable edit, one action
at a time.

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

## Preform, proportions, and yield

The inspector's **Preform** tab holds three groups, none of them tied to
any one selected tier — they describe the whole design:

- **Preform** — the rough's shape (Block or Cylinder), Half-Width, Length /
  Width, and Depth, with an **Apply Preform** button.
- **Proportions** — the figures a cutter actually quotes, worked out from
  the last Solve: table size and length-to-width on one line, then Crown
  Height, Pavilion Depth, and Total Depth each on their own line. Every one
  of these reads "-" rather than a misleading zero whenever the design does
  not currently solve, or has no girdle plane to measure a depth from.
- **Yield** — the effective-RI readout and its source, Girdle Diameter
  (mm), a Yield Material and Specific Gravity Override for the carat
  estimate (a separate control from Design Settings' own Material combo —
  Chapter 6), an **Apply Yield Inputs** button, and the resulting
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

## Duplicating a tier

Click the small duplicate button on a tier's row (the ⧉ glyph), or select a
row and press **Ctrl+D**, to append a copy of it to the end of the tier
list. The copy keeps the same angle, indices, and constraint, gets the same
name with a trailing apostrophe (e.g. `P1` → `P1'`), and the tier list's
selection moves to the new copy so you can immediately start editing it.
Duplicating is one undoable `Add Tier` edit, like typing a new tier by hand
and clicking Add Tier.

## Reordering tiers

The small ▲/▼ buttons on a tier's row, or **Alt+Up** / **Alt+Down** on the
keyboard-selected row, swap that tier with its neighbour in cutting order.
Each swap is its own undoable edit.

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
| Alt+Up / Alt+Down | Move the highlighted row up/down in cutting order |
| F2 | Open the highlighted row's angle cell for inline editing |
| Ctrl+click a row | Add/remove that row from a multi-select group |
| Shift+click a row | Select the whole range from the last-selected row to this one |

Unlike an older version of this app, Up/Down/Home/End/Page Up/Page Down all
select immediately — there is no separate "keyboard cursor" that needs a
following Enter to actually load the row. The window's other global
shortcuts (Ctrl+Z/Y/S, Ctrl+F, Ctrl+1/2/3, F5, Escape — Appendix B) still
work while the tier list has keyboard focus.

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
Edit menu) now says exactly what they will do — "Undo: Set P1 angle to
-41.0 degrees," for instance — instead of a bare "Undo." Undo/redo does not
re-solve either — the same staleness rule applies, so click Solve again
after undoing or redoing if you need current mast values.

## Next steps

Continue to Chapter 5 to solve the design and read what its status
messages mean.
