# 15. Planning a Rough

## What you will do

This chapter covers the **Rough Planner**: a separate window in which you
model a piece of rough (a block, a cylinder or a water-worn pebble, with flat
cuts taken off it), tell the planner how many stones you are willing to cut and
from which designs, and get the ten heaviest ways to cut it. You will learn how
to build the model, how the live weight check works, what the planner does when
you press **Plan**, how to read the 3D view, the result cards and every metric
on them, how to jump from a result to the designs in your library, how to save
plans and share them as files, and what the planner cannot do.

## Opening the Rough Planner

Choose **Library → Plan Rough...** from the menu bar. The planner opens in its
own window, not in a dialog on top of the main window, so you can keep working
in the library, click designs, and move the planner to a second screen.

- Closing the planner window only hides it. Your model, the results and the
  selection are kept until you quit the app; choosing **Plan Rough...** again
  brings the window back as you left it. A plan that is running keeps running
  while the window is hidden. Closing the main window closes the planner and
  cancels a running plan. Because that would lose work, closing the library
  window asks first when the planner holds results that were never saved or a
  plan is still running: the question names what is at risk ("The Rough Planner
  has 3 unsaved results." or "A rough plan is still running."), has no **Save**
  button unless the design in the editor also has unsaved changes, and **Discard**
  closes everything.
- Inputs are not remembered between launches: the planner starts empty every
  time you start the app. Anything you want to keep, you save (see "Saved
  plans" below).
- The planner only works with your own local library. If a remote library is
  selected, the planner shows "Switch to the local library to plan a rough." in
  red, and **Plan** refuses with the same message (Chapter 1 covers switching
  libraries, Chapter 10 the remote ones).
- The **?** button in the planner's title bar opens this chapter.

## The window at a glance

The window has a title bar and three columns. It opens at 1280 x 820 (smaller on
a screen that cannot hold that) and cannot be made smaller than 1040 x 560.

- **Title bar** — **New**, **Saved plans** (it reads **Close Saved** while the
  saved list is open), **Save selected (n)** and **?**.
- **Left column** — the rough and the plan. A scrolling upper part holds the
  **ROUGH** section (shape, size, material, weighed carat, the live readout) and
  the **SHAPE** section (Undo, Redo, the cut buttons and the cut list). The
  **PLAN** section (stone count, candidate designs, losses, the **Plan** button
  and the progress bar) stays pinned at the bottom.
- **Centre** — the 3D view, with its buttons above the image and a hint line
  below it, so no control ever covers the picture.
- **Right column** — **RESULTS**: the summary line, the result cards, and a
  footer with **Show all in library**, **Save selected (n)** and **Save all**.
  While the saved list is open it takes the place of the results; switching
  between the two keeps the scroll position of each. The two questions the
  planner can ask you (the name row when you save, and "Replace ... unsaved
  layouts?" when you plan again, see below) open at the top of this column,
  above both lists.

In a window narrower than 1180 pixels the right column gets narrower and the
small thumbnails on the result cards are hidden; the cards are laid out so that
their texts are shortened with an ellipsis rather than running out of the column.

Everything can be reached from the keyboard: Tab moves between the buttons, the
choices, the fields, the result cards and their tick boxes, and Space or Enter
presses the one that has the focus (it gets a bright outline). Each control has a
name for screen readers. The shortcuts are listed at the end of the chapter.

## Modelling the rough

### The starting shape

The **ROUGH** section starts with three choices, **Block**, **Cylinder** and
**Pebble**.

- **Block** and **Pebble** take **Rough X (mm)**, **Rough Y (mm)** and
  **Rough Z (mm)**: the three sides of the box the rough fits in. A pebble is
  the ellipsoid inscribed in that box, so it is smaller than a block of the same
  size (an ellipsoid fills about half of its box).
- **Cylinder** takes **Diameter (mm)** and **Length (mm)**, and an **Axis**
  choice, **X**, **Y** or **Z** (**Y** by default): the direction the cylinder
  runs along.

Type a number such as `12.0`; a decimal comma also works. Each size must be
greater than 0 and no more than 2000 mm ("Dimensions must not exceed 2000
mm."). A size field that is still empty is not an error: the planner simply
shows nothing until the rough has a size, and **Plan** answers "Enter the rough
size in mm." A field is applied when you press Enter or leave it.

The +Y axis points up in the view, and the three axes are the rough's own: the
cut plan and the stone sizes are reported in this X, Y and Z.

Switching between the shapes keeps the size as far as the shapes allow. Block
and Pebble share their box. A cylinder becomes the box that surrounds it. A box
becomes an upright cylinder whose diameter is the smaller of X and Z and whose
length is Y. Cuts are kept when you switch; see below for what happens to edge
and corner cuts on a round shape.

### Material and carat: the weight check and Fit to weight

- **Material** — the material the stones will be cut from, listed as its name
  and "(SG x.xx)". Only materials with a known specific gravity are listed: the
  built-in ones that have one, plus any custom material from your catalogue that
  records one (marked "(catalogue)" if it shares a name with a built-in). The
  default is Quartz. The specific gravity turns volume into carats (Chapter 6
  covers materials). If no material has a specific gravity, the planner says so
  and **Plan** is disabled.
- **Carat (ct)** — the model's own weight: the volume of the modelled rough times
  the material's specific gravity, rewritten whenever the size, the cuts or the
  material change. Type the weight from your scale over it to compare the two.

Below these fields the planner shows a live readout of the model, for example
"Model 257 mm³ · 3.40 ct (Quartz, SG 2.65)", and, once you have typed a weighed
carat over the model's own, a coloured **weight check**:

| Difference between model and weighed carat | Chip |
| --- | --- |
| within 5 % (inclusive) | green: "matches weighed 3.31 ct (+2.7 %)" |
| more than 5 % and up to 15 % | amber: "close to weighed (+9.8 %)" |
| more than 15 % | red: "check the model: 31 % heavier than weighed" (or "lighter") |

The check never stops you from planning: a red chip means "look at your size and
cuts again", not "wrong". Since the scale is the more reliable of the two
measurements, the **Fit to weight** button next to the field scales the whole
model to the typed weight: every size, every edge and corner setback and every
face depth is multiplied by the cube root of the carat ratio (a 10 % heavier
weight makes the rough 3.2 % larger in each direction), as one undo step.
Afterwards the field shows the model's carat again and the chip disappears. The
button is available while the chip shows; a typed weight within a hundredth of
a percent of the model's only hands the field back to the model.
A typed value that is not a positive number switches the check off: the model,
the view and the volume keep updating regardless. **Plan**, **Fit to weight**
and saving, however, stop and the red chip shows "Weighed carat must be a number
in ct." (or "Weighed carat must be greater than 0 ct.") until you correct or
clear the field.

### Cutting the rough

Real rough is rarely a perfect box. The **SHAPE** section lets you take flat
cuts off the starting shape. Every cut removes material with one flat plane, so
the model always stays convex (see "Limitations"). Each cut is measured against
the starting shape, not against the cuts before it, so you can edit or delete
any cut without disturbing the meaning of the others.

There are three kinds of cut, added with **+Edge**, **+Corner** and **+Face**.
Each appears as a numbered row in the list below the buttons.

- **Edge** — takes a wedge off one of the twelve edges of the box. A drop-down
  chooses the edge by the two faces that meet there (Top-Front, Top-Back,
  Top-Left, Top-Right, Bottom-Front, ... Back-Right). **Setback A (mm)** and
  **Setback B (mm)** say how far the cut reaches from the edge across the two
  faces: A along the first-named face, B along the second. Neither may be longer
  than the face it is measured along. New edge cuts start on Top-Front.
- **Corner** — takes a corner off the box. A drop-down chooses one of the eight
  corners by its three faces (Top-Front-Left ... Bottom-Back-Right).
  **Setback A**, **Setback B** and **Setback C** (mm) are how far the cut reaches
  along the three box edges that meet at the corner, in the order of the three
  faces named: A is measured along the axis that the first-named face is
  perpendicular to. For Top-Front-Right, A is down the vertical edge. New corner
  cuts start on Top-Front-Right.
- **Face** — a flat face in any direction, for a sawn or broken surface. It has
  **Azimuth (°)**, **Elevation (°)** and **Depth (mm)**. Elevation 90° is
  straight up (+Y) and -90° straight down; at elevation 0°, azimuth 0° faces +X
  and azimuth 90° faces +Z. The depth is measured from the rough's outermost
  point in that direction, and it must be less than the rough's thickness in that
  direction. New face cuts start facing straight up. Face cuts work on every
  shape.

Edge and corner cuts need a block: **+Edge** and **+Corner** are greyed out for
a cylinder or a pebble, which show "Edge and corner cuts need a block". If you
switch a shape that already has edge or corner cuts to a cylinder or a pebble,
the rows stay, and the model reports "Cut 2: edge and corner cuts are only
supported on block rough." until you delete them or switch back.

New cuts get default sizes so that they are visible at once: setbacks of 15 % of
the shortest side involved (the shorter of the two sides an edge cut runs across,
the shortest side of the box for a corner cut), and a face depth of 10 % of the
rough's extent in the cut's direction. Both are rounded to 0.1 mm and are at least
0.1 mm.

Click a cut's row to select it. Its face is outlined in the 3D view, and the row
shows how much it removes, for example "removes 84 mm³ (1.11 ct)" (84 mm³ of Quartz, SG 2.65, is
1.11 ct). Clicking into any field of a row selects the row too, and a row you
have just added scrolls into view with its first field ready to type over. The **×** at
the right of the row title deletes the cut. While you type in a cut field, the
picture and the readout follow the number at once. If a value is not allowed, for
example a setback longer than its face, the row shows the reason in red under
the fields and the model shows the same message; **Plan** is greyed out until the
model is valid.

### Clicking in the 3D view

You can also make cuts in the view itself, without touching the buttons.

- **Edge or corner.** With a block in the view, move the pointer over an edge or a
  corner of the block. When it is within 10 pixels of one, the hint line below
  the image reads "Click to cut edge Top-Front" or "Click to cut corner
  Top-Front-Right", and the edge or corner is marked. A corner wins over an edge
  next to it. Click, and the planner adds that cut with the default setbacks and
  selects its row.
- **Face from view.** Press **Face from view**. The button lights up and the
  hint line shows "Face from view: click rough face (Esc to cancel)". Move the
  pointer over any face of the rough: the face is highlighted and the hint reads
  "Click to cut this face flat". Click, and the planner adds a **Face** cut whose
  direction is that face's, with the default depth. The mode switches itself off
  after one cut; press the button again or press Esc to leave it early.
- **Selecting a cut.** Pointing at the face of an existing cut shows "Click to
  select cut 2"; click to select its row.

A single click adds its cut after a moment (the time a double-click takes); a
double-click on an edge, corner or face adds the cut at once and leaves the camera
where it is, while a double-click on empty space resets the view (see below). Cuts
can only be made while the view shows the rough (the **Rough** side of the
**Rough | Result** switch, see "The 3D view") and no plan is running. The view
shows the rough before the first plan and after you change the model. While a plan
runs it still shows the rough, but the model is locked, so a click in it adds no
cut. **Face from view** is greyed out while the view shows a result (an armed pick
can still be switched off with Esc).

### Undo, Redo and New

**Undo** and **Redo** step through your model edits, up to 100 steps each way.
One step is: adding or deleting a cut, changing which edge or corner a cut is on,
finishing an edit in a field (Enter, or leaving the field), switching the shape,
opening a saved plan, or **New**. Typing itself is not a step until you finish
the edit. **Ctrl+Z** undoes and **Ctrl+Y** (or **Ctrl+Shift+Z**) redoes; while a
text field has the keyboard focus it keeps these keys for its own typing.

**New** in the title bar replaces the model by an empty block and clears the
weighed carat and the selected cut. It is itself one undo step. It does not touch
the plan settings or the results on screen.

The buttons that change the model (Undo, Redo, +Edge, +Corner, +Face, Face from
view, the × of a cut row) and every field of the rough, the cuts and the plan are
disabled while a plan is running, so that the model on screen stays the one being
planned.

## Planning

The **PLAN** section sets up what the planner may use and how much it must
leave.

- **Stones (up to)** — from 1 to 99. This is a *maximum*, not a target: a layout
  may cut fewer stones than this if that weighs more. Set it to 1 and you get the
  best ten single stones for this rough.
- **Candidate designs** — which designs the planner may use:
  - **Current filter (N designs)** — only the designs the library's search and
    filters currently show (Chapter 2). This is the default, and the way to say
    "only round brilliants" or "only designs I can cut on my machine". While a
    "Show in library" filter from the planner is active (see "Library links"),
    this means exactly those designs.
  - **Whole library (N designs)** — every design in the library, except entries
    you have flagged as ignored.
  - **Excluded designs (N)** — not a third choice, but a list under the two
    choices that appears as soon as one design is excluded. An excluded design
    is left out of **Current filter** and of **Whole library** alike, before
    anything is measured, and neither count includes it. Use it for a design that
    keeps winning but that you do not want to cut, such as a plain cube. The
    design stays in the library, in every search and in the preview and tilt
    batches; only the planner leaves it out, and a small "not planned" note marks
    its card in the library list.

    To exclude a design, press **Exclude** on its row in a result card (the pill
    then reads **Excluded**), or choose **Exclude from planner** in the
    right-click menu of its card in the library (Chapter 2). To bring it back,
    click **Excluded designs (N)** to unfold the list and press **Restore** next
    to the design, or **Restore all** for every one; **Include in planner** in
    the library menu restores one design too. The pills in the planner are greyed
    out while a plan runs. The mark is stored in your library, so, unlike the
    planner's inputs, it survives a restart. Excluding or restoring a design
    never changes the results on screen: a note tells you to plan again. Like the
    rest of the planner, it works on the local library only.

  The counts follow the library while the planner is open, so you do not need to
  reopen it. They are worked out when you choose **Library → Plan Rough...**,
  about a second after you change the library's search or filters, each time you
  press **Plan**, after a "Show in library" button, when you switch libraries and
  after a change to the exclusions. **Plan** itself reads the library filter and
  the exclusions afresh when you press it, so a count that is out of date never
  changes what is planned. Counts of a thousand or more are written with
  separators ("Current filter (1,234 designs)"), and a count of one reads "1
  design". While the counts are being worked out they read "(counting...)".
- **Losses** — a one-line summary of the four loss settings ("kerf 0.30,
  allowance 0.20, skin 0.00, min 1.00 mm"). Click the **Losses** line to unfold
  the fields and again to fold them away (they fold away by themselves while a
  plan runs, to leave room for the progress bar):
  - **Saw kerf (mm)** — the material lost to the saw blade at every cut.
    Default 0.30 mm. It applies to the sawn layouts; an exact single stone (see
    below) involves no saw cut.
  - **Allowance / side (mm)** — extra material kept on every side of each stone
    for preforming and polishing, so the finished stone is smaller than the piece
    it comes from by twice this amount along each axis. Default 0.20 mm.
  - **Rough skin (mm)** — the outer crust trimmed off every face of the rough
    before any cutting. Default 0.00 mm. A skin that leaves nothing of the rough
    gives "The skin allowance leaves nothing of the rough."
  - **Min stone width (mm)** — the narrowest finished stone the planner will
    accept, measured across the stone's smaller horizontal width. Default
    1.00 mm. A rough smaller than this plus the allowances gives no layouts at
    all.

Kerf, allowance and skin cannot be negative and the minimum width must be greater
than 0. Kerf, allowance and skin may not exceed 50 mm and the minimum width may not
exceed 1000 mm; a larger value is refused with a message such as "Kerf must be at
most 50 mm.". Skin plus allowance must leave something of the rough, and a minimum
width larger than the rough gives no layouts.

Press **Plan** (or Ctrl+Enter). **Plan** is greyed out while the model is invalid,
and a note beside it says so; the red message that explains it is under **ROUGH**
on the left.

If the results on screen are a plan you have not saved, the planner asks before it
throws them away. A question opens at the top of the right column, "Replace 4
unsaved layouts?", with **Replace** and **Keep**. **Replace** starts the plan;
**Keep** (or Esc) leaves the results as they are and starts nothing. Results you
have saved do not trigger the question, and the word "unsaved" next to **RESULTS**
tells you when they are the kind that would.

While the plan runs the button reads **Cancel**, the stage line and a
percentage show the progress, and the result list is emptied. **Cancel** is
available at every stage and returns the planner to idle with "Planning
cancelled."; the stage line reads "Cancelling..." until the plan has stopped. Esc
also cancels a running plan.

### The first run: measuring the designs

To fit a design into a rough the planner needs to know how big the finished stone
is: its footprint, its height, its volume and its convex outline. It gets these by
solving each design's facets into a solid and measuring it, which takes a moment
per design.

The first time a design is used in a plan, that measuring happens and the result
is stored in your library database. The progress line reads "Measuring designs
120 / 3,299" while it works. Every later plan reads the stored figures instead,
so it starts straight on the planning stage. If the sizes of your designs are
already stored from an earlier version of the app and only their outlines are
missing, the line reads "Measuring design outlines 120 / 3,299 (one-time)". On a
large library the first run over **Whole library** can take a while; a run over a
small filter measures only those designs.

A few things to know about the stored measurements:

- They are per design, not per plan: they do not depend on the rough, the
  material or any loss setting, so changing those never triggers a re-measure.
- If you cancel while measuring, the designs already measured stay stored, and
  the next run continues from there.
- A design is measured again automatically when its geometry changes: when you
  re-import a design over an existing one, save a design over its catalogue
  entry, or when a mirrored library updates it. If the app itself later changes
  how designs are measured, every stored measurement is treated as missing and
  re-measured once.

After measuring, the stage line steps through the planning stages: "Planning:
pruning designs", "Planning: sizing the pieces", "Planning: cut orders (k of 6
finished)", "Planning: alternatives", "Planning: single-design layouts",
"Fitting single stones: screening / exact search / polishing", "Refining", and
finally "Preparing the results...".

### What "fits" means

The planner builds its ten results from two kinds of layout, ranked together.

**Sawn layouts, one stone per piece.** The rough is sawn the way rough is
actually sawn: cuts always run edge to edge across the whole part, in three
stages. The first cut direction divides the rough into **slabs**, the second
divides each slab into **bars**, and the third divides each bar into **pieces**.
The planner tries all six orders of the three axes. Every saw cut costs one
kerf, and every piece holds at most one stone.

The stone is the design scaled evenly to the largest size that fits inside its
piece less the allowance, with its table facing any of the six rough faces. For
this the planner treats a stone as the smallest box around it: the footprint of
the finished stone (its length and width, measured on the stone's outline
however it is turned about the table) times its height. Stones of different
designs can be mixed in one layout.

- On a **block without cuts** the pieces are boxes inside the block.
- On a **cut block, a cylinder or a pebble** the saw still runs across the
  rough's bounding box, so a piece can be partly air. The stone box must fit in
  the part of the piece that is really rough, less the allowance. A piece that
  sticks out of the rough holds a smaller stone, placed off-centre where the
  rough is; a piece entirely outside the rough holds none.

**Exact single stones.** For one stone the planner does better than a box: it
uses each design's real convex outline, turns it freely in every direction,
scales and slides it, and looks for a very good fit: a large copy that lies
entirely inside the rough with the skin and the allowance taken off all round
(the search is a search, not a proof of the best possible fit; see "Limitations"). A single stone in a
round or cut rough can therefore sit tilted, with its table not facing any rough
face. To keep this affordable across thousands of designs, the planner first
scores every design coarsely, then searches orientations in detail for a
shortlist of at least 48 of the best-scoring ones and polishes the best of those.

The two kinds of layout are merged and ranked together, so a tilted single stone
can be number 1, with a sawn layout of several stones right behind it. When the
same single design is found both ways, only the heavier of the two is listed.

### Why one big stone often wins, and why several sometimes do

A finished stone's weight grows with the *cube* of its size. Cutting a rough in
half along one axis does not give two half-weight stones; it gives two stones
that are each only a fraction of the size, plus you lose a kerf and two more
allowances. On a rough with a roughly even shape, one large stone that fills it
therefore usually weighs more than two or three smaller ones, even when you allow
many stones. That is why a plan with **Stones (up to)** set high often puts a
layout of only one or two big stones at the top: the number is a limit, not a
goal.

A long, thin rough is the opposite. A single stone must fit inside the
cross-section, so it can only grow as big as the thin sides allow, and the extra
length goes unused. Cutting the rough into several pieces along its length gives
each piece the full cross-section to work with, and several stones of that size
weigh far more than one stone of the same size that leaves most of the rough
behind. Raising **Stones (up to)** on such a rough shows this directly: the top
layouts become a row of stones, and the yield climbs.

A mix of different designs can beat any single design when the rough has an
awkward shape: a broad, shallow stone from one design in one part of the rough
and a slimmer one in another.

## The 3D view

The centre column shows the rough being modelled, or, once you select a result,
that result: the stones inside the rough. The **Rough | Result** switch at the
left of the button row chooses which; **Result** is greyed out until there is a
result to show. Three things are drawn only in the result view, and each has its
own toggle in the button row above the image.

**Rough being modelled.** The rough is drawn as a solid. Its own faces are pale
grey and the faces of your cuts are tinted amber while **Cut faces** is on. The
selected cut is outlined.

**A selected result.** The stones are drawn as their real designs, each design in
its own colour (eight soft colours that repeat), the same colour as the swatch on
its design row. The rough is drawn around them as a faint glass volume with its
edges. The saw pieces are drawn as amber outlines. The toggles are **Rough**,
**Saw** and **Stones**.

The view's controls:

| Control | What it does |
| --- | --- |
| Drag with the left mouse button | Orbit around the rough |
| Mouse wheel | Zoom |
| Double-click on empty space | Reset the view |
| Double-click an edge, corner or face (rough view only) | Add its cut at once |
| **Rough \| Result** | Show the rough you are modelling, or the selected result |
| **Front**, **Top**, **Side** | Snap the camera to that view (**Side** looks at the right side) |
| **Reset** | The starting view, from the front-top-right |
| **Cut faces** (rough view only) | Tint the faces of your cuts |
| **Rough**, **Saw**, **Stones** (result view only) | Show or hide that layer |

**While you drag the picture, or while the wheel is turning, the view is drawn at
half resolution, and at full resolution again as soon as you let go** (after the
wheel has been still for about a tenth of a second). That keeps orbiting smooth
with a lot of stones on screen. A drag that moves less than three pixels before
you release still counts as a click.

Hovering shows what is under the pointer in the hint line below the image. In the
rough view, that is the edge, corner or face you could cut (see above). In the
result view, hovering a stone shows its design, its weight and its place in the
saw plan as the slab, bar and piece number that the cut plan uses (see "The cut
plan"; an exact single stone is not sawn and has no such place). Clicking a stone
selects its design row in the result card and outlines all stones of that design;
clicking empty space clears the outline.

When a plan finishes, result #1 is selected and shown. Click a result card to
show another one. If you then change the model on the left, the view goes back
to the rough you are editing; the result cards stay where they are, still
belonging to the model they were planned for, and clicking a card shows its
stones again. The **Rough | Result** switch takes you between the two at any time.

## Reading the results

When the plan finishes, the summary line under **RESULTS** reads like "10 layouts
from 2,431 designs -- 3.2 s", and up to ten result cards appear below it,
scrolling inside the column. There are fewer than ten when fewer distinct
layouts are possible, and also when the ranking rule that lets at most three
layouts share one set of designs (see "How the list is ranked") leaves fewer. If nothing fits, the column shows "No layout fits: the
rough is too small for the minimum stone width, or no measured design is small
enough."

### The result card

Each card shows, in its header:

- a **checkbox**, for saving (see "Saved plans"); it is named "Keep result N for
  saving" for screen readers;
- the **rank**, "#1" being the heaviest;
- the **total weight** in carats, for all the stones in the layout together
  (volume × specific gravity ÷ 200, the same figure the yield tools use).

The next line gives:

- the **yield**: the finished stones' volume as a percentage of the modelled
  rough's volume, so a cylinder or a cut block is measured against what is really
  there, not against its box;
- the **stone count** ("1 stone", "6 stones");
- **Cut plan**, which folds the sawing instructions in and out (one card at a
  time).

Under that, a line reads "*n* cuts · kerf loss ≤ *x.xx* ct" — the number of
saw cuts and an upper estimate of the weight the saw turns to dust — and, if you
entered a weighed carat, "38.9 % of 9.54 ct weighed": the finished stones' weight
as a share of the weighed rough. The kerf figure costs each cut with the
cross-section of the part it saws, taken from the rough's bounding box, so it is
overstated for a cut or round rough (hence the "≤"). An exact single stone (see
"What "fits" means") is not sawn, so it has no "cuts" line. At the right of these
lines is the **Result in library** button (see "Library links"). Long lines are
shortened with an ellipsis when the column is narrow.

Below that sit a small thumbnail of the layout (hidden in a narrow window; it
shows a grey placeholder until it has been drawn, and all ten are drawn one after
another, in rank order) and one **design row** for each design used, most stones
first.

### The design row

Each design row shows:

- a **colour swatch** matching the design's stones in the 3D view, and the
  design's library preview if it has one;
- "*count* × *design name*" — the name is a link that selects the design in the
  library, exactly as if you had clicked its card, while the planner stays open;
- a short status chip, only on saved plans (see "Saved plans"), with the full
  wording on the line under the name;
- an **Exclude** pill at the right end of the same line. Press it to keep the
  design out of every later plan (see "Candidate designs"); the pill then reads
  **Excluded**, and pressing it again brings the design back. The results on
  screen stay as they are and a note tells you to plan again. The pill is greyed
  out while a plan runs, for a design that has been deleted from the library, and
  while a remote library is selected;
- a line with the finished size in millimetres and the weight of each stone. When
  the stones of one design differ, the line gives their weight range and the
  largest stone's size instead. The size is the box around the stone in the
  rough's own X x Y x Z;
- a list of metrics. Clicking anywhere else on the row selects it, which outlines
  the design's stones in the view; clicking it again clears the outline.

The metrics, where a group of stones of unlike size shows each figure as a range:

| Metric | What it says |
| --- | --- |
| **Weight** | "0.62 ct · 29 % of the total", "0.62 ct each ...", or a range "0.48-0.62 ct ...". The percentage is this design's share of the layout's weight |
| **Size** | "L x W x D = 7.10 x 5.10 x 3.40 mm" — in the stone's own terms, not the rough's: length along the stone's long side, width across its short side, depth from table to culet |
| **Ratios** | "L/W 1.39 · D/W 67 %" — the design's proportions: length to width, and depth as a percentage of width |
| **Volume** | The finished stone's volume in mm³ |
| **Orientation** | "table faces Top (+Y)" when the table faces a rough face (Top +Y, Bottom -Y, Right +X, Left -X, Front +Z, Back -Z), or "table tilted 23° from Top" against the nearest face. Different orientations in one group are listed with their counts |
| **Fill** | "stone uses 58 % of its piece": the stone's volume over the volume of its piece where the rough is really there. For an exact single stone, which has no piece of its own, "stone uses 41 % of the model" |
| **Optics** | The design's brilliance, extinction and windowing at zero tilt (table up), from its stored tilt curves, for example "brilliance 71 % · extinction 12 % · windowing 4 % (preview material: Quartz)"; "not generated yet" if the design has no stored curves (Chapter 14 covers generating them) |

### The cut plan

Click **Cut plan** on a card to fold out the instructions for sawing that layout;
click it again to fold them away. The plan is text, for example:

```
Cut order: slabs across X, bars across Y, pieces across Z (each cut loses 0.30 mm)
Stone sizes are X x Y x Z, in the rough's own axes.
Cut X into 3 slabs: 8.10 / 8.10 / 7.80 mm
Slab 1 (8.10 mm): cut Y into 2 bars: 6.00 / 5.70 mm
  Bar 1 (6.00 mm): no cut along Z (one piece, 9.00 mm)
    Piece 1: Design name, table faces Z, 5.10 x 5.10 x 3.40 mm, 0.62 ct
```

Read it from the top. The first line gives the order of the saw cuts and what each
cut costs. The next lines split the rough into **slabs** with the first cut, each
slab into **bars** with the second, and each bar into **pieces** with the third.
The numbers restart at every level: "Piece 1" is the first piece of its bar, "Bar
1" the first bar of its slab. A stone's place in the plan is its slab, bar and
piece number, and the hover text in the 3D view uses the same three numbers.
"No cut along Y" means that stage leaves the part whole. Every piece then names
the design cut from it, which rough face its table faces, the finished stone's
size along the rough's X, Y and Z, and its weight. The sizes listed for the cuts
are the lengths of the pieces *after* the kerf has been taken out; the layout is
built so that they, the kerfs between them, and the skin add up to the rough's
box exactly. On a cut or round rough the pieces may hold air, and the stone is
smaller than its piece. An exact single stone is not sawn: there are no saw stages to follow, so its
card has no saw cuts line, and its plan does not describe slabs, bars and
pieces. It gives the design and the box around the stone, and the **Orientation**
metric says how the stone sits.

### How the list is ranked

The list is ranked by weight only. Equal weights are broken in favour of fewer
stones (fewer cuts, fewer stones to polish). Two layouts with the same designs in
the same numbers are listed once, and at most three layouts may use the same set
of designs, so the list stays varied rather than showing "1 x A", "2 x A" and
"3 x A" one after another. Alongside the best mix of designs, the planner also
looks for the next-best mixes that avoid the design the best one uses most, so
more than one mixed layout can appear.

## Library links

The planner talks to the library in three ways:

- **Click a design name** in a result. The design is selected in the main window,
  as if you had clicked its card. The planner stays open.
- **Result in library** on a card narrows the library to *all the designs of that
  result*, and **Show all in library** in the footer narrows it to the designs of
  *all* the results. The library list then shows only those designs under a
  labelled banner, for example "Rough plan · result #3 (4 designs)" or "Rough plan
  · all 10 results (17 designs)"; a plan opened from the saved list says `Saved
  plan "Aqua pebble" · result #2 (3 designs)`. The banner has a **Clear** link that
  restores the full list, and any change to the search box or the filters clears
  it too. Designs that have been deleted from the library are left out, and if
  none is left the planner tells you so.
- While such a banner is active, **Current filter** in the planner means exactly
  those designs, and the count next to it says how many.

With a remote library selected, all of these are disabled, and the results show
"Switch to the local library to open designs from these results." They work again
as soon as you switch back to the local library.

## Saved plans

Plans are saved in your library database, so they survive restarts, and they can
be exported as files and imported again. Saving is always your explicit choice;
the planner does not save anything by itself.

### Saving

Tick the **checkbox** on the cards you want to keep, then press **Save selected
(n)** in the title bar or the footer (it is greyed out until something is ticked),
or **Save all** in the footer. Ctrl+S does the same as **Save selected**, and
saves every result when none is ticked. A name row opens at the top of the right
column, above both the results and the saved list. Its title says what is about
to be saved, "Save 3 results" for the ticked ones or "Save all 10 results". It
suggests a name built from the shape, size and material and the date (UTC), such
as "Pebble 18x11x10 mm Aquamarine 2026-09-30" or "Cylinder Ø12x30 mm Quartz
2026-09-30". Edit the name, then press Enter or **Save** (Esc or **Cancel**
discards). A confirmation reads "Saved 3 results as "..."." It shows under **RESULTS** and at the top of the saved list.

A saved plan contains the model and the settings *the results were planned for*
(not whatever is in the left column now), the material and weighed carat, whether
the plan used the current filter or the whole library, and every ticked layout
with its cut plan and the exact place of every stone. It also records each used
design's title and a fingerprint of its shape. It does not record which designs
were excluded from planning: that belongs to the library, not to a plan. You
cannot save while a plan is running: the save buttons are greyed out and
Ctrl+S does nothing until it finishes or you cancel it.

### The saved list

**Saved plans** in the title bar (or Ctrl+O) opens the list in place of the
results; the **Results** button at its top returns. Each entry shows its name,
date, and a summary such as "Pebble · Aquamarine · 3 results", and has these
buttons:

- **Open** — loads the plan (see below).
- **Rename** — edit the name in place (the field starts with the current name
  selected); **Save** or Enter applies it. The field closes when the list has
  refreshed with the new name, so a name the planner refuses stays open beside its
  message.
- **Export** — write the plan to a file (see below).
- **Delete** — asks "Delete plan?" first; **Yes, delete** confirms.

**Import...** at the top of the list loads a file. Notes and errors from the
saved-plan actions (a plan opened or exported, a file refused) appear directly
under the list's heading; a note about designs that have changed is shown in amber.

### Opening a saved plan

**Open** puts the plan's model, settings, weighed carat, material and choice of
candidate designs (**Current filter** or **Whole library**; a file without that
choice reads as **Current filter**) back into the left column (the model as one
undo step, so you can undo your way back), and shows the saved
layouts in the results exactly as they were saved, with a banner above them: `Saved
plan "..." · 2026-09-30 · Re-plan uses the current model and settings`. The layouts are shown
from the file; nothing is recomputed. Select any card and the 3D view shows its
stones just as they were saved. A design that is excluded from planning today is
shown like any other, with all its stones; only its **Excluded** pill tells you.

**Re-plan** in the banner re-plans with the current model and settings, the ones
in the left column now (the loaded ones unless you have changed them since), using
today's library and today's exclusions: a design you excluded after saving is left
out of the new plan, and one you restored is back in. If the results on screen
are not saved, the planner asks before replacing them (see "Planning"). If the
plan's material is no longer in your material list, the planner says so and keeps
the material that is selected now.

You cannot open or import a plan while a plan is running: the **Open** and
**Import...** buttons are greyed out until it finishes or you cancel it, and a
request that still gets through is refused with "Wait for the running plan to
finish, or cancel it, first."

### When the library has changed: the status chips

A saved plan stores each design's entry number and a fingerprint (its length to
width, height to width, and volume to width cubed). On opening, every design is
checked against the library, and a chip on its design row says what was found; the
chip is short and the full wording is on the line under the design's name:

- **deleted** ("design deleted", red) — the design is no longer in the library. Its name is
  no longer a link, and in the 3D view its stones are drawn as plain grey boxes.
- **changed** ("design changed since saved", amber) — the design is still there but its shape
  no longer matches the fingerprint (to within a millionth). It is drawn with its
  current shape, which may no longer match the sawn box the plan was made for.
  A design that has never been measured cannot be compared and counts as unchanged.
- **by title** ("matched by title", cyan) — the entry number is gone, but exactly one design in
  the library has the same title (capital letters ignored) and the same
  fingerprint, so the planner uses that one. This is how a file exported from
  another library finds its designs.

### Export and import

**Export** on a saved plan asks where to write it. The suggested file name is the
plan's name with `.indicatrix-rough.toml` after it, with characters that file
names cannot hold replaced by underscores; a name you type that does not already
end in `.toml` gets `.indicatrix-rough.toml` added. The dialog's file filter reads
"Indicatrix rough plan" and matches `*.toml`. The file is plain text (TOML), named
after the plan as it is called now, so a rename is exported.

**Import...** reads such a file (up to 16 MB), stores it as a new saved plan under
the name in the file, and opens it, with the status check above. A file that is
not a rough plan, or that a newer version of the app wrote, is refused with a
message ("This plan was made by a newer Indicatrix ... Update Indicatrix to open
it."), and a file with an invalid value names the field that is wrong. Keys the app
does not know are ignored.

## Keyboard shortcuts

These work anywhere in the planner window. Ctrl is Cmd on macOS. They are the
planner's own; they are not part of the main window's list in Appendix B.

| Shortcut | Action |
| --- | --- |
| Ctrl+Enter | Plan (the same as **Plan**, and only when it is available) |
| Ctrl+S | Save the ticked results (all results when none is ticked); opens the name row. Not while a plan runs |
| Ctrl+O | Open the saved list |
| Ctrl+Z | Undo the last model edit (a focused text field keeps it for its own typing) |
| Ctrl+Y, Ctrl+Shift+Z | Redo |
| Tab, Shift+Tab | Move the focus between buttons, choices, fields and result cards |
| Space, Enter | Press the button, choice, tick box or card that has the focus. In a size, weight or cut field: apply it. In the name row: save |
| Space, Enter on a view toggle | Switch the toggle that has the focus (**Cut faces**, or **Rough**, **Saw**, **Stones** in the result view) on or off; the focused toggle gets a bright outline, like every other control |
| Esc | Answer "Keep" to the replace question; otherwise close the name row; otherwise leave **Face from view**; otherwise cancel a running plan |

## Limitations, and how to read the numbers

- **Convex only.** Every cut removes material with one flat plane, and the shape
  stays convex. The planner cannot model a concave notch, a hollow, or keep-out
  zones for inclusions and cracks. Model the largest clean convex part of the
  rough you would actually cut from.
- **A cylinder and a pebble are approximations, always slightly on the small side.**
  The cylinder is a prism with 64 sides inscribed in the circle, whose volume is
  99.84 % of the true cylinder's (0.16 % less). The pebble is a polyhedron with
  162 faces inside the ellipsoid, whose volume is at least 97 % and less than
  100 % of the ellipsoid's. The planner therefore never plans a stone into material
  that is not there. The first, coarse screening pass of the single stones uses a
  coarser 16-sided cylinder and a 42-face pebble.
- **Several stones use the box model.** In a cut, round or pebble rough, and in a
  plain block too, a layout of several stones treats each stone as the smallest box
  around it, so a round stone is treated as needing a square footprint. Only a
  single stone is fitted with its real outline and free rotation.
- **The saw grid has a limited resolution.** The planner looks for saw positions
  on a grid, and then refines the best layouts by moving their cuts
  continuously. To keep the plan quick and its memory use bounded, the grid has at
  most 48 steps per axis for a plain block and at most 20 per axis for a cut,
  cylindrical or pebble rough, and fewer where the table of pieces would otherwise
  grow too large: a **Stones (up to)** of 99 in a 20 mm cube uses 16 steps per
  axis. With many stones the pieces are only a few grid steps across, so the
  layouts found for a high stone count in a shaped rough can be a little worse
  than the best possible one; the continuous refinement of the best layouts
  recovers part of the difference.
- **The single-stone search is a search.** It screens every design coarsely and
  searches orientations in detail for a shortlist of at least 48, so a design that
  only fits well in an unusual orientation can be missed.
- **The model is only as good as your measurements.** The planner assumes a
  flawless rough, stones that come out exactly to their design's proportions,
  and allowances and kerf exactly as you typed them. Real rough has inclusions,
  cracks and colour zoning that decide where you actually cut, and each stone
  starts as a preform that you shape before faceting. Calipers and a scale give
  you a few percent of error each. Use the weight check to catch a model that is
  clearly off, treat the weights and yields as an estimate for clean, inclusion-free
  rough after your kerf and allowance (real yield also depends on inclusions and
  on how closely you follow the plan; the note under the result cards says the
  same), use the ranking to compare options and choose where to look first,
  and do the final marking-up on the rough itself.

## Which designs are skipped

The planner can only use designs it can measure as a closed solid.

- **Designs known only by their angle table.** Some catalogue entries carry a
  table of angles but no design file to build a solid from. The app can draw an
  approximate stone for them, but its size and volume are not real, so the
  planner does not use them.
- **Designs whose facets do not close on their own.** A schedule that relies on
  its preform to close the stone would measure as the preform's blank instead of
  the stone, so it is left out. A design that deliberately uses its preform
  as part of the stone is skipped for the same reason.

The planner does not name the skipped designs; the bottom of the results column
counts them ("12 designs skipped: no usable design file"). If every chosen design is skipped,
the summary reads "None of the N selected designs has a usable design file."
Designs that could not be loaded or measured at all are named in the summary line
("; 3 designs could not be measured"), and they are retried on every run, since
nothing is stored for them. A skipped design is not an error, and it never blocks
the plan.

A design you have excluded from planning (see "Candidate designs") is a different
case. It is left out before anything is measured, so it is not counted as skipped,
costs no measuring time, and is not part of the N in "None of the N selected
designs". Instead the summary line carries the clause "; 2 designs excluded from
planning" ("; 1 design excluded from planning" for one), on every outcome,
including "No layout fits". If every candidate is excluded, **Plan** stops before
it starts and says "Every candidate design is excluded from planning. Restore one
under Candidate designs, or widen the filter."

## Next steps

Chapter 2 covers the library filter that decides which designs the planner may
use, Chapter 6 the materials and their specific gravities, and Chapter 14 the tilt
curves behind the **Optics** line. To cut one of the designs the planner suggests,
load it as in Chapter 3.
