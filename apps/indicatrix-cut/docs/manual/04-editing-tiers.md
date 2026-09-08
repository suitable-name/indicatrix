# 4. Editing Tiers

## What you will do

This chapter covers the tier form: adding a new facet tier, editing an
existing one, detaching a tier from its symmetric family, and removing a
tier, plus undo/redo. It also covers the tier list's faster paths — inline
angle editing, keyboard nudging, duplicating a tier, and navigating the
list from the keyboard — for when you're adjusting angles on an
already-built design rather than typing a whole tier from scratch.

## The tier form

The form on the right of the Edit sub-tab has these fields:

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

The form's title reads "Add Tier" when you are creating a new row, or "Edit
Tier #N" when editing an existing one (click a row to load it into the
form). The save button reads **Add Tier** in the first case and **Save
Tier** in the second; a **New** button next to it clears the form back to
add-mode without touching the tier list.

**Saving does not re-solve.** Clicking Save Tier (or Add Tier, or Remove,
or Apply Preform, or Undo/Redo) updates the tier list's editable columns
immediately, but deliberately does *not* run a fresh solve — that is
Chapter 5's job, and it can take real time on a large design. Instead the
MAST and SOLVE columns, and the status banner, are marked visibly stale
until you click **Solve**.

## Detach and Reattach

When a tier's Indices list names more than one position, the app treats
those positions as one symmetric family (an orbit — Chapter 3). By default,
editing that tier and clicking Save Tier is meant to move the *whole*
family together, keeping the design symmetric.

The **Detach** button on a tier's row exempts that tier's positions from
this automatic orbit-consistency handling — the button then reads
**Reattach**, letting you put it back. Use Detach only when you genuinely
want a deliberately asymmetric design; leaving a tier attached is the
correct default for anything meant to stay symmetric.

**Limitation.** Detach is a whole-tier toggle, not a per-facet one — there
is no control in this app for detaching a single index position out of a
multi-index tier while leaving the rest attached. Also note: editing a
detached tier's other fields and clicking Save Tier rebuilds the tier from
the form and clears the Detach flag back to attached. Only the dedicated
Detach/Reattach button itself preserves the flag across other edits — if
you need a tier to stay detached, re-detach it after any Save Tier edit.

## Inline angle editing

The tier list's ANGLE column is itself editable — you don't have to load a
tier into the form on the right just to nudge its angle. Click the angle
value to turn it into a text field, type a new number, and either press
**Enter** (commits) or click elsewhere to move focus away (also commits).
Press **Escape** to back out without changing anything.

Typing something that isn't a valid number is rejected with a toast and the
cell reverts to the tier's real angle — it never silently keeps invalid
text on screen. Committing the exact same value the tier already has is a
silent no-op: it doesn't spend an undo step.

Every commit here is a real, undoable edit — press **Undo** to step it back
like any other change — and, like every other tier edit, it marks the
design stale (MAST/SOLVE/the status banner) until you next click Solve, or
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

The scroll wheel does the same ±0.1° step when the cursor is hovering the
tier list's inline angle cell, whether or not that cell is currently open
for editing.

**Inline cell vs. the form field.** A nudge on the tier list's inline
angle cell applies immediately as a real, undoable edit against that tier
— exactly like committing a typed value. A nudge on the tier *form's* own
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

Click the small duplicate button on a tier's row (next to Detach/Reattach),
or select a row and press **Ctrl+D**, to append a copy of it to the end of
the tier list. The copy keeps the same angle, indices, and constraint, gets
the same name with a trailing apostrophe (e.g. `P1` → `P1'`), and the tier
list's selection moves to the new copy so you can immediately start editing
it. Duplicating is one undoable `Add Tier` edit, like typing a new tier by
hand and clicking Add Tier.

## Keyboard navigation in the tier list

Click anywhere in the tier list to give it keyboard focus, then:

| Key | Action |
| --- | --- |
| Up / Down | Move the keyboard cursor between rows (scrolls the row into view) |
| Enter | Load the highlighted row into the form on the right (same as clicking it) |
| Delete / Backspace | Remove the highlighted row |
| Ctrl+D | Duplicate the highlighted row |
| F2 | Open the highlighted row's angle cell for inline editing |

The keyboard cursor (a thin highlight ring) is separate from the form's
selection: moving it with Up/Down alone doesn't change what's loaded in the
form until you press Enter or click a row. The window's other global
shortcuts (Ctrl+Z/Y/S, F5, 1/2/3 — Appendix B) still work while the tier
list has keyboard focus.

## Selecting several tiers for a batched nudge

Ctrl+click a tier row to add it to a multi-select group (Ctrl+click again
to remove it) — selected rows get a cyan outline. With two or more tiers
selected, nudging *any one of them* (inline cell arrows/wheel, not the
form's field) moves every selected tier's angle together, by the same step,
as a single undoable edit — one Undo reverts the whole group's nudge at
once.

**Note:** the cyan outline disappears as soon as you actually nudge the
group (a nudge is itself an edit, and every edit refreshes the tier list) —
but the selection it represented keeps applying underneath, so a second
nudge right after still moves the same set of tiers together. Only the
visible outline needs a fresh Ctrl+click to reappear; the batching itself
isn't affected.

## Removing a tier

Click the **×** button on a tier's row to remove it. Like every other edit
in this list, removing a tier marks the design stale; click Solve
afterwards to see whether it still closes.

## Undo and redo

The **Undo** and **Redo** buttons step backward and forward through your
edit history (adding, saving, removing, and detaching tiers; applying a
preform; adopting a meet; applying an Optimize result). They are greyed out
when there is nothing to undo or redo in that direction. Undo/redo does not
re-solve either — the same staleness rule applies, so click Solve again
after undoing or redoing if you need current mast values.

## Next steps

Continue to Chapter 5 to solve the design and read what its status
messages mean.
