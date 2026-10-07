# 21. Angle Sweeps

## What you will do

This chapter covers **Edit → Angle Sweep...**: a way to try one tier's angle over a
range, one angle after another, and read how the stone's figures change. Use it when
you wonder "what does 40.5° do to this pavilion compared with 41.5°?" and would
rather see the whole row of answers at once than type each angle, solve and look.

A sweep gives you three things from the same run: a **table** with a row for every
angle, a **chart** of the figures against the angle, and the same rows as **CSV** text
for a spreadsheet. When you find an angle you like, one button makes it the tier's real
angle.

A sweep never changes your design while it runs. Every angle is tried on a private
copy of the design. Only the button **Use this angle** (see below) edits the design,
and it is one undo step.

## Opening the dialog

Open a design in the editor, then choose **Edit → Angle Sweep...**. You can also press
**Ctrl+K** and type `sweep` to run **Angle Sweep...** (Chapter 19). The menu item and
the palette entry are dim while no design is open, and while the guided walkthrough
(Help → Guide: New Design Walkthrough) has the advanced tools locked.

The dialog is a window of its own over the main one. **Esc** closes it; while a sweep
is running, **Esc** stops the sweep first.

## Choosing the tier, the range and the step

The top of the dialog is the form.

- **Tier.** The list holds every tier whose angle you can vary, written as the tier
  table writes it: the row number, the name and the angle, for example
  `6. Pavilion Main (41.00°)`. The dialog opens on the tier selected in the tier
  table when that tier can be swept, otherwise on the first one in the list. A tier
  with an old-style name (`1`, `A`) is listed under its standard code (`P1`, `C1`),
  as the tier table shows it.
- **From, To and Step**, all in degrees. When you choose a tier they start at five
  degrees either side of its angle in steps of half a degree, the flatter end in
  **From** (`36` and `46` for a pavilion tier at 41°). Each field takes
  arithmetic, as the angle fields of the tier form do: `41.5 - 3` is 38.5. The two
  ends can be given in either order.

Some tiers are not in the list:

- The **table** and the **culet** are flat, and a **girdle** tier is vertical. They
  have no slant to sweep.
- A tier whose angle **follows a relation** (Chapter 4) is not free, so it cannot be
  swept. A line under the form names the tiers that were left out for that reason.
  Sweep a tier it reads instead: the tiers that follow it move with it on every
  angle of the sweep, exactly as they do when you edit it by hand.

Angles are plain positive numbers, here as everywhere in the editor. The tier decides
which side of the girdle they are on, so a pavilion tier is swept from `36` to `46`
and a crown tier from `30` to `40` in the same way. A minus sign typed in front of an
end is ignored (`-40` means `40`). A range has to stay between 0.1° and 89.9° from
flat. The rows come flattest first.

A sweep tries at most **200 angles**. The design's own angle is always among them, even
when it falls between two steps of your grid, so the table always has the design as it
is to compare against; it is marked **Current**.

The line under the fields says what the sweep will do, or what is wrong with the form
and how to put it right. For example, "21 angles from 36.00 to 46.00 degrees. This
takes about 10 seconds." The time is a rough guess from the size of the design and the
number of processor cores; it is a guide, not a promise.

### Averaging the tilt performance

The check box **Also average the tilt performance (slower)** adds three more figures to
every angle: the brilliance, windowing and extinction of the Tilt Performance graph
(Chapter 2), averaged over all four tilt axes and every tilt angle. They answer a
different question from the quick figures: how the stone behaves as it is tilted, not
only straight down on the table. They cost about a second and a half of work for every
angle, so a long sweep takes much longer with this on, and the time estimate says so.

## Running a sweep

Press **Run sweep**. The dialog shows a progress bar and "3 of 21 angles done". The
program keeps working while the sweep runs, and a sweep uses all the processor cores
but one, so the rest of the program may feel slower until it is over.

**Cancel** (the same button while a sweep runs) stops it. The rows already finished
stay in the table, and the chart is drawn from them. Closing the dialog also stops a
running sweep, and then the rows are dropped.

Each angle is solved and checked on its own. An angle at which the design cannot be
cut, for example because a facet vanishes, a relation cannot be kept, or the solver
finds no closed stone, still gets its row. The row is marked **Not valid**, its
figures are dashes, and the **Notes** column says why.

## Reading the table

Each row is one angle. The columns are:

- **Angle**, in degrees, and **Status**: **Current** for the design's own angle, **Not
  valid** for an angle that gives no stone, and empty for the others.
- **Brilliance %**, **Windowing %** and **Extinction %**, scored looking straight down
  on the table (the same quick score Optimize searches with, Chapter 8), in the
  material and under the lighting preset the viewport uses. Higher brilliance is
  better; lower windowing and lower extinction are better.
- **Fire** (the fire index) and **Scint. %** (scintillation). More of either is more
  sparkle, but neither is "better" in a fixed sense, so they are marked like
  brilliance, as the highest.
- **Yield %**, the finished stone's volume as a share of the rough's volume. It is
  shown only when the design's rough has a volume.
- **Tilt brill. %**, **Tilt wind. %** and **Tilt ext. %**, only when you asked for the
  tilt averages.
- **Notes**: remarks in plain words. They give the reason a row is not valid, and they
  warn about an angle that works but changes something: for example, that the girdle
  band is gone at that angle, or that some facets vanish or come out very small.

In every column the **best figure is bold and green**. Windowing and extinction are
best when lowest, the rest when highest. Figures that agree to the two decimals shown
are marked together, and when every row agrees nothing is marked. Only valid rows are
compared.

Click a row to select it. The chart marks it, and **Use this angle** (below) names it.

## Reading the chart

The chart draws one line per figure over the swept angles, the flattest angle at the left
and the steepest at the right. Because the figures have different sizes, **each line is
scaled to its own lowest and highest value**: the top of a line is that figure's highest
value in this sweep and its bottom the lowest, so two lines can only be compared by
their shapes, not by their heights. Read the legend under the chart ("Brilliance 77.10
to 79.60 %") for the real numbers behind the top and the bottom of each line. A gap in
a line is an angle that is not valid.

- The buttons above the chart, one per figure, switch its line on and off. The chart
  starts with brilliance, windowing and extinction.
- A thin vertical line and the word **current** mark the design's own angle.
- Move the pointer over the chart and the sentence under it follows the nearest angle:
  its figures for the lines on the chart, and why it is not valid when it is not.
- Click the chart to select the row nearest the pointer. The table scrolls to it.

## Using an angle

Select a valid row other than the current one. The button at the bottom then names the
angle, for example **Use 42.00°**. Press it and the tier takes that angle, on the same
side of the girdle as before. It is one
undo step (Chapter 4): **Ctrl+Z** puts the old angle back. Tiers that follow a relation
to the tier follow in the same step. The toast says what was set.

As after any change of an angle, the solved values in the tier table are out of date
until the design is solved again (Chapter 5); the Solid view solves it in the
background.

The dialog stays open, with the **Current** mark moved to the angle you used, so you can
try another row. Every row stays true: each is the design with only that tier's angle
changed.

**Use this angle** is refused when the design has changed since the sweep ran (for
example after an Undo from the menu, or after another design was opened), because the
rows no longer describe the design. Run the sweep again. It is also dim for the current
angle, for a row that is not valid, and while a sweep runs.

## CSV

**Copy CSV** puts the table on the clipboard, and **Save CSV...** asks for a file and
writes it. The file name starts as the design's title, the tier and "angle sweep". Both
buttons are dim until a sweep has rows.

The first line holds the column names: `tier`, `angle_deg`, `current`, `valid`, then
one column per figure (`brilliance_pct`, `windowing_pct`, `extinction_pct`,
`fire_index`, `scintillation_pct`, `yield_pct`, and the three `tilt_` columns when you
asked for them), then `facet_warnings` (how many facets vanish or come out very small
at that angle) and `notes`. Numbers are written with a point and four decimals, the
angle as a plain positive number, and an angle that is not valid has empty figure cells. A spreadsheet opens it directly; if
yours expects a decimal comma, import it as comma-separated text with a point as the
decimal mark.

## Limitations

- A sweep varies **one tier** at a time. Trying two tiers together, a grid of angles, is
  not offered; sweep one, use its best angle, then sweep the other.
- The figures are the quick **table-up** score, under one lighting preset. A design that
  wins here can still lose in the full render; confirm a choice with the Tilt
  Performance graph (Chapter 2), or tick the tilt option.
- The tools of **concave tiers** (Chapter 16) are not part of the figures: the stone is
  scored on its flat facets, as Optimize does.
- The rows are not saved with the design and are gone when the dialog closes. Save the
  CSV if you want them later.
- A sweep tries at most 200 angles, and a tier that follows a relation, a table, a culet
  and a girdle tier cannot be swept.
- A very small design may finish in seconds, and a large design with tier targets
  (Chapter 8) may take much longer than the guess in the form says. Cancel stops it.
