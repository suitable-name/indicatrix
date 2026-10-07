# 18. History and Variants

## What you will do

This chapter covers the inspector's **History** tab. It has two views, picked with
the **Steps** and **Variants** buttons at the top of the tab.

**Steps** is a list of every change you have made to the open design, each with a
small picture of the design as it was at that step. Use it when you have tried
several things, one of them looked good, and you want to go straight back to it
instead of pressing Undo over and over.

**Variants** is a list of named copies of the design that you chose to keep. Use it
when you want to keep a design as it is now, go on and try something else, and be
able to come back, or compare two versions side by side. The second half of this
chapter ("Variants") covers it.

Undo and Redo themselves are described in Chapter 4. The Steps view is the same
history seen as a list, with a way to go to any step in one click.

## Opening the tab

Open the inspector below the tier table (Chapter 4) and click **History**, the
fifth tab pill. You can also press **Ctrl+K** and type `history` to run
**Inspector: History Tab** (Chapter 19). Until you change something, the list holds
only the **Start** row.

While a guided lesson has Undo and Redo switched off, the History tab is switched
off for the same reason: its pill is dim, and it carries the same short note
about why.

## Reading the list

The newest step is at the top. Each row shows:

- **A picture** of the design at that step (see "The pictures," below).
- **What the step did**, in the same words the Undo button's hover text uses,
  for example "Set P1 angle to -41.0 degrees."
- **The step number**, as "Step 3." A step you have undone reads "Step 3, undone."
- **A "Now" mark** on the step the design stands at right now. Its row has a
  coloured outline.
- **A small save button** at the right end of the row (Advanced interface only).
  It keeps the design as it was at that step as a variant (see "Saving a
  variant," below). The Simple interface leaves it out; **Save as variant...** in
  the Variants view keeps the design as it is now.

Hover a row to see what a click on it does. A small **?** button at the top
right of the tab opens this chapter.

The last row is always **Start**: the design as it was when you opened or
created it, before your first change.

Steps **after** the current one are the ones you have undone. They are shown
dimmed and marked "undone." They stay in the list, ready to go forward to, until
you make a new change (see "A new change after going back," below).

Several quick nudges of the same angle (for example by keeping an arrow key
pressed) count as one step, exactly as they do for Undo. The row shows the words
of the latest nudge.

## Going to a step

Click a row to go to that step. The design changes to what it was at that step,
and the tier table and the Solid view update at once; the Solid view solves the
design again in the background, as it does after Undo or Redo. A jump does not
press Solve for you, either: as after Undo, the solved values in the tier table
are out of date until you click Solve, or until auto-solve runs, if you turned it
on (Chapter 5). If the row you had selected is beyond the end of the list at that
step, the selection is cleared; otherwise it stays on the same row number, which
can now show a different tier. Chapter 4 ("Undo and redo") describes the same
rules for Undo and Redo.

The whole list is **one Tab stop**: press Tab until the list has a coloured
outline on a row, or click a row. Then:

- **Up** and **Down** move the outlined row to a newer or an older step.
- **Enter** (or **Space**) goes to the outlined row's step.
- **Home** moves to the newest step and **End** to **Start**.
- **Tab** moves on to the save button of the outlined row (Advanced interface),
  then out of the list. The other rows' save buttons are not Tab stops, so a
  long list never takes many presses of Tab to get past.

A screen reader announces the list as "History steps" and each row with its
words, its step number and whether it is the current step.

Moving the outlined row only looks; nothing changes until you press Enter. After
every jump the outline goes back to the step the design stands at.

If a step cannot be reached (this should not happen, but a step is a recorded
edit and an edit can in principle fail to replay), a message says so and the
design stays at the last step it reached.

## Going back is not a step

A jump is not an edit. It adds nothing to the history, and every step stays where
it was. That means you can always change your mind:

1. Click an older step to look at it.
2. Click the step you came from, which is now marked "undone," to return to it.

**Undo** and **Redo** keep working after a jump. Undo moves one step older than
where you stand, Redo one step newer, exactly as if you had pressed them that
many times.

## A new change after going back

If you go back to an older step and then change the design, the steps that came
after the one you stood at are dropped from the list. This is the same rule as
after pressing Undo and then editing, and the dropped steps cannot be brought
back. So if you want to keep a later step, go back to it (or note its step
number) before you try something else from the older one.

## The pictures

Each picture is about 72 by 72 pixels at normal size, and it is sharper on a
high-density screen. Every picture is drawn from the same angle, so two steps can
be compared at a glance. They use the flat shading of the Solid view.

- Pictures are drawn **in the background, only for the rows in view.** You can
  scroll and click while they fill in, and a row shows "Drawing" until its picture
  is ready. A long list does not slow the program down.
- A step whose design **does not solve** (for example a tier that no longer meets
  anything) shows "Does not solve" instead of a picture. You can still go to that
  step.
- The pictures show the flat facets. A tool-cut (concave) facet (Chapter 16) is not
  drawn in a picture, as in the compare window (Chapter 14).
- A picture is kept while the program is running, so scrolling back to a row, or
  jumping to another step and back, does not draw it again. A picture is drawn
  again when the step's words or its design change, or when the window's scale
  changes.

## What the history includes

The History tab shows the same history that Undo and Redo use, so it holds the
same kinds of change: adding, saving, removing, detaching and reordering tiers,
per-facet editing, a handle drag in the Solid view (one drag is one step), applying
a preform, adopting a meet, a Retarget and applying an Optimize result.

Creating a new design, opening a file, or loading a library design starts a
new history: the list begins again with **Start**. The history is not saved with
the design file.

## Limitations of the Steps view

- The list does not search or filter, and a step cannot be renamed.
- The history belongs to the design that is open. There is no way to bring back
  the steps of a design you have closed. To keep a design for good, save it as a
  variant (below).
- A picture shows only the solid. Whether the design was solved, or marked stale,
  at that step is not shown.

## Variants

### What a variant is

A **variant** is a named copy of the design that you keep so you can come back to
it. Save one before you try something risky, or when you have two good ideas and
want to look at them next to each other.

A variant keeps the whole design as it was when you saved it: the cutting
instructions, the rough, the girdle size, the material and the concave tiers
(Chapter 16). It does not keep the lighting you chose, the undo history or any files
attached to the design.

Variants are kept in your library on this computer, under the design's id, and
**not in the design file**. Saving a variant never changes the file, and a design
file you send to someone does not carry your variants. Chapter 11 ("What the
library keeps about a design besides the file") explains the id: Save As keeps it,
so a copy of a design shows the same variants as the original; a new design that you
never save gets a new id each time it opens, so its variants cannot be found again
after you close it. Save the design once to give it a lasting id.

### Opening the Variants view

Open the inspector, click the **History** tab, then **Variants**. You can also press
**Ctrl+K** and run **Inspector: Variants** (Chapter 19). While a guided lesson has
Undo and Redo switched off, the whole tab is switched off, and Variants with it.

The view lists only the variants of the open design, found by its id. Open another
design and the list changes with it. If no design is open, the view asks you to open
or create one.

### Saving a variant

There are three ways to start:

- Click **Save as variant...** at the top of the Variants view. This keeps the design
  as it is now.
- Click the small save button on a row of the **Steps** view (Advanced interface). This
  keeps the design as it was at that step; the form says which step ("Keeps the design
  as it was after step 3: ..."). Use it to rescue a step you went past.
- Press **Ctrl+K** and run **Save as Variant...**.

A small form opens above the list:

- **Name** starts as "Variant 1", "Variant 2" and so on, one more than the highest
  number in use. Change it to something you will recognise. A name cannot be empty and
  can have up to 80 characters. Two variants may share a name.
- **Note** is optional. Write why you kept it, for example "steeper crown, more
  brilliance". A note can have up to 400 characters.

Click **Save** or press Enter to save, or **Cancel** or press Esc to leave without
saving. A message under the fields says what is wrong if the name or note cannot be
used. Every field and button has a hover tip, and a screen reader names the fields
"Variant name" and "Variant note".

If you chose a step and the history has changed since (you made a new change after
going back, which drops steps), a message asks you to choose the step again, rather
than saving something other than what you picked.

A small picture is drawn for the variant in the background, from the flat Solid view
at the same angle as the Steps pictures. A design that does not solve is saved
without a picture and shows "No picture" in the list.

### Reading the variants list

The newest variant is at the top. Each row shows:

- **A picture** of the design.
- **The name**, and under it who it was made from and how long ago ("from: Variant 2
  · 12 minutes ago"). Older than a month it shows the date. Hover over the age to see
  the exact time in UTC.
- **The note**, if there is one.
- **A "Working from" mark** on the variant that the open design was last opened from
  or saved as. The next variant you save is marked as made from it.

Each row has four buttons: **Open**, **Rename**, **Note** and **Delete**. They are
switched off while a save, open or comparison is being worked out ("Working..."); their
hover tips then say why. Each is a Tab stop with a coloured ring while it has the
keyboard, and Space or Enter presses it. A screen reader reads the variant's name
with the button ("Open Variant 2").

**Rename** and **Note** open the same small form with the current text in it.
**Delete** asks "Delete this variant?" first. A deleted variant cannot be brought back.
Your current design is not changed by any of these.

### Opening a variant

Click **Open** on a row. The open design becomes the variant, as **one undo step**:

- Press **Undo** once and the design is exactly what it was before, including the
  rough, the girdle size, the material and any concave tiers. **Redo** brings the
  variant back.
- The step appears in the Steps view like any other change. The earlier steps stay
  where they are.
- No tier is selected afterwards, because the tiers may be a different list.
- As after Undo or a jump to a step, the Solid view solves in the background, but the
  solved values in the tier table are out of date until you click Solve, or until
  auto-solve runs, if you turned it on (Chapter 5).
- If the open design is already the same as the variant, nothing changes and a message
  says so.

Opening a variant works on the design that is open, not on the file. Save it with
**Save** when you want to keep the result in the file.

#### Relations come with the variant

A variant brings its own **relations** (Chapter 4, "Relations between tiers"), the same
way it brings its own angles. If the open design has a tier whose angle follows a
relation and the variant has a different angle for that tier, nothing stops the open:
the variant's angles and the variant's relations replace the open design's. A tier that
followed a relation in the open design but not in the variant is a plain tier
afterwards, and the other way round. It is still **one undo step**: Undo brings back
the old angles and the old relations together.

#### When a variant cannot be opened

- If the variant's relations cannot hold (two tiers that read each other in a loop, or
  an angle that would come out above 90 degrees), the program says so in a sentence and
  the design is not changed.
- If the variant was deleted in the meantime, a message says so and the list is read
  again.
- If the design changed while the variant was loading (for example you opened another
  file), a message asks you to open the variant again.

### Branches: where a variant came from

When you open a variant, or save one, the program remembers it as the variant the open
design came from. The next variant you save is marked "from:" that one. So:

- Save "Variant 1", change something, save "Variant 2": Variant 2 reads "from:
  Variant 1". A run of saves reads as a chain.
- Open Variant 1 again, change something else and save "Variant 3": Variant 3 also
  reads "from: Variant 1". That is a second branch.

The program remembers this only while the design stays open. If you open the file
again, the first variant you save has no "from:" line. If the variant a row came from
is deleted, the "from:" line disappears.

### Comparing designs

Below the buttons, **Compare two designs** has two boxes. Each lists **Current design**
(the design as it is now) and every variant, the newest first. If two variants have the
same name, the time they were saved is added to tell them apart. Pick two different
entries, then:

- **Compare pictures** opens the compare window (Chapter 14) with the first design on
  the left and the second on the right. You can turn both with the mouse, switch to the
  difference overlay, and see the optical figures. This window only looks: it has no
  Keep or Discard buttons. To go to a variant, use **Open** on its row.
- **Compare text** (Advanced interface only) shows the cutting instructions of the two
  designs, written the way Export writes them, line by line. Lines only in the second
  design are marked as added, lines only in the first as removed. Long runs of unchanged
  lines are folded into one row ("12 unchanged lines"). Click **Back to variants**, or
  press Esc, to return to the list.

The text holds the cutting instructions only. A different rough, girdle size or
material shows in the pictures, not in the text; the summary line says so. A design
that does not solve cannot be written as text, and the comparison says which one.

Both comparisons need two entries. With no variants yet, the strip asks you to save one
first.

### Limitations of variants

- Variants are kept on this computer only. They are not in the design file and do not
  travel with it, and deleting a design from the catalogue does not delete them.
- A variant cannot be edited in place. Open it, change the design, and save a new
  variant. A name or note can be changed with **Rename** and **Note**.
- The list does not search or filter, and a variant cannot be exported by itself. To
  keep one as a file, open it and use **Save As**.
- A picture shows the flat facets only; tool-cut (concave) facets are not drawn
  (Chapter 16), as in the Steps pictures.
- Comparing as text needs a design that solves.
