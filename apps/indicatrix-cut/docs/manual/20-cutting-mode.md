# 20. Cutting Mode

## What you will do

**Cutting mode** is the cutting instructions made for the bench. It takes the design's
steps in the order you cut them and shows **one step per page**, large enough to read
from the lap: the tier, the angle, the indices to cut and what the stone looks like
afterwards. You tick off what you have cut, and the program remembers where you got to,
so you can stop for the day and come back to the same step.

The cutting sheet of Chapter 11 and the Schedule tab of Chapter 3 hold the same
numbers in one long list. Cutting mode is for the moment you are actually cutting and
want one step at a time, the next one always in front of you.

This chapter covers opening cutting mode, reading a step, the keys, the done marks and
index ticks, what happens when the design changes under your marks, where the marks are
kept, and how to start over.

## Opening cutting mode

Open a design in the editor and solve it (Chapter 5), then start cutting mode in any of
three ways:

- Click **Cut Mode** in the editor's command bar.
- Choose **Edit menu, Cutting Mode...** in the menu bar.
- Press **Ctrl+K** and type `cutting` to run **Cutting Mode...** (Chapter 19).

Cutting mode covers the whole window. It builds the steps first (a moment on a large
design: "Getting the cutting steps ready...") and then opens on the first step you have
not marked done. A design you have never marked opens on step 1, and a design with
every step done opens on the last step.

The button and the menu item are dim, with the reason in their hint, while there is
nothing to cut: no design is open, the design has no tiers, or it does not solve
yet. The palette command is dim while no design is open; for a design that has no
tiers or does not solve it opens the screen with the same reason written on it
instead of failing silently. Cutting mode is also one of the advanced tools, so it is
dim while the guided walkthrough (Help menu, Guide: New Design Walkthrough) has them
locked.

## Reading a step

The page is split in two. On the left is the step itself:

- **Step k of N** at the top, then the **side** of the stone the step is on (Crown,
  Pavilion or Girdle) and the tier's code (P1, G1, C2, T), the label the cutting
  sheet gives it.
- **The angle**, in very large figures, in degrees as the cutting sheet gives it: the
  number you set on the machine. A tier with a cheater offset shows the offset
  separately under **Cheater**.
- **Indices.** One chip for each index the step cuts, in the order the cutting sheet
  lists them. A step with no index list is cut at index 0 and shows that one chip.
- **Instruction** gives the tier's own name and then what its facets meet, in the
  words the cutting sheet uses (Chapter 11): "Crown Main: Meet P1, P2".
- **Depth** appears when the step has a depth to cut to: the depth in millimetres and
  the mast reading, or the mast reading alone when the design has no size yet.
- **Cheater** shows the cheater offset in degrees, in amber so you do not miss it.
- **Tool** appears for a concave tier (Chapter 16): the tool code, the azimuth, the
  displacement and the details of the cut.
- **Your note** is the note you wrote on the tier, in amber.

On the right are two pictures that change with the step:

- **The stone after this step.** This is the Solid view's picture with the Cut slider
  set to the step (Chapter 13): the rough as it is cut by every step up to and
  including this one, with this tier's facets highlighted. The line under it says so.
  While cutting mode is open the viewport is in the Solid view, even if you had the
  Diagram view on.
- **The index wheel.** A ring numbered like the faceting machine's index gear, with
  the indices of this step marked as dots. The line under it names the gear. A
  small tick is drawn for every tooth and the numbers are written at the major
  divisions only, so a gear with many teeth stays readable.

At the bottom are the **progress line** ("4 of 12 steps done"), a progress bar, the key
reminder and the buttons.

## Moving between steps

| Button | Key |
| --- | --- |
| **Previous** | Left arrow, Page Up |
| **Next** | Right arrow, Page Down |
| **Mark step done** | Space or D (also goes on to the next step) |
| Close | Esc |

Previous and Next only look. They do not mark anything, and you can go back to any
step to read it again. The keys are also listed in Appendix B.

The **Mark step done** button marks the step and stays on its page; press it again
(it then reads **Mark not done**) to take the mark back. Space and D are for the
rhythm of cutting: they mark the step done and go on to the next one in one press. On
a step that is already done they just go on, and they never take a mark back. On the
last step they mark it and stay, because there is no step after it; the progress line
then reads "12 of 12 steps done".

A key pressed together with Ctrl, Alt or the Windows key does nothing in cutting mode.
Cutting mode keeps the keyboard while it is open, and the tier list and the other
panels are covered, so Esc is the way out.

## Done marks and index ticks

There are two kinds of mark, and both are kept for the design:

- A **step done** mark means you have cut the whole step. **Mark step done** sets it,
  and the same button then reads **Mark not done**, which takes the mark back. A
  finished step shows a **Done** badge in the header of the screen.
- An **index tick** means you have cut that one index. Click a chip to tick it; click
  it again to take the tick back. Ticks are for a step with many indices that you cut
  over several sittings: you can see which ones are left.

The two follow each other. All the chips of a step that is done show ticked. Ticking
the last index of a step marks the step done. Taking a tick back from a step that is
done takes the step's done mark away and leaves the other indices ticked, so no work
is lost. Taking the done mark back from a step clears its ticks.

The **progress bar** and the line next to it count the steps marked done.

## When the design changes

A mark belongs to the step as you cut it. If you change the step afterwards, the
mark no longer says anything true, so cutting mode does not trust it. Every mark
stores a **fingerprint** of the step's cutting values: its side, angle, indices,
depth or mast, cheater offset, and for a concave tier its tool values. When the
fingerprint no longer matches:

- The page shows an amber badge, **Changed since you marked it**, and the notice
  explains what to do: cut the step as it reads now, then mark it done again.
- The step does **not** count as done in the progress bar. The progress line adds
  "1 step changed since you marked it" so you can see how many are affected.

Changing the **name**, the **notes** or the **Meets** wording of a tier does not change
its fingerprint, since none of them changes the cut. Changing a tier's angle, its
indices, its depth or its cheater offset does. Renumbering the schedule does not
either: the mark follows the tier, not its row number.

Cutting mode watches the design while it is open. If you undo or redo, or open another
design, the steps are built again, and the page stays on the same tier if it is still
there.

## Starting again

**Reset progress**, at the bottom of the screen, removes every mark of the design you are
cutting: all the done marks and all the ticks. It asks first ("Remove every mark for
this design?"); **Reset** goes ahead and **Keep my marks** cancels. Esc also cancels
the question. After a reset the page goes back to step 1.

Reset progress touches only the design shown at the top of the screen. Other designs
keep their marks.

## Where your marks are kept

Marks are kept in **your library on this computer**, filed under the design's
identity. They are **not** written into the design file. That has three effects:

- Saving, copying or sending a design file never carries the marks, so you can pass
  a design to someone else and they start with a clean slate.
- Opening the same design again later, from the library or from its file, finds your
  marks again, as long as it is the same design and this computer's library is the
  one you used.

A mark is written as soon as you make it, so there is nothing to save. Closing the
program in the middle of a design loses nothing.

If a design has never been saved and so has no library identity yet, cutting mode asks
you to save it once; its marks could not be filed anywhere before that.

## Limitations

- Cutting mode follows the order of the cutting sheet (pavilion and girdle first,
  then the crown, the table last; Chapter 11). It does not reorder steps, and
  it does not check that you cut them in order.
- The picture of the stone is the viewport's own and is drawn after a short wait on a
  large design; the page says "The picture of the stone is still being drawn..." until
  it arrives.
- Marks live on this computer only. On another computer the same design starts with no
  marks.
- A design that does not solve cannot be stepped through; solve it first (Chapter 5).
- The selection in the tier list and the Cut slider are moved while cutting mode is
  open, and put back when you leave it.
