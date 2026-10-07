# 11. Saving and File Formats

## What you will do

This chapter explains the two file formats the editor writes — the
`.indicatrix` design file, which is this app's own native format, and the
plain `.asc` schedule — when to use each, and how the app handles a catalogue
design's original file versus a locally edited one. It also covers opening a
file by double-clicking it or giving it on the command line, the older
`.indicatrix.toml` files that still open, the experimental Gem Cut Studio
`.gcs` export, and opening `.gem`/`.gcs` files directly in the editor.

## `.asc` — the plain cutting-instructions file

`.asc` is the long-standing GemCAD-style schedule format: a plain text
file listing every facet's angle, index, and meet instruction. Two
different places in the app can produce one, and they behave slightly
differently — the buttons have distinct labels to make their difference
clear.

### Export Edited .asc, from the editor

While a design is open in the cutting-design editor (Chapter 3), click
**Export Edited .asc**. This re-solves every tier's mast fresh from the
design's current, edited geometry and writes a plain `.asc` file wherever
you choose to save it. If some tier can't currently be solved, you'll see
"Cannot export: &lt;reason&gt;." — fix that first (Chapter 5). On success,
you'll see "Exported edited schedule to &lt;path&gt;."

### Export .asc, from the catalogue detail view

Selecting a design in the catalogue and clicking **Export .asc** there
works differently — hovering the button spells this out ("the original
file stored in the catalogue, not your edits"):

- If the design has an original `.asc` file attached, that exact file is
  exported byte-for-byte.
- If it does not, the app reconstructs one from the design's stored angle
  table. **A reconstructed file always has placeholder mast distances (all
  zero)** and is stamped with a "RECONSTRUCTED" note at the top of the
  file, saying plainly that mast distances are placeholders and were not
  part of the original stored data. You'll see a matching toast: "Exported
  reconstructed schedule to &lt;path&gt; (mast distances are placeholders --
  see the file's RECONSTRUCTED header)." Do not treat a reconstructed
  file's masts as real cutting depths — solve it properly first.

## Editing the instructions as text

Choose **Edit > Edit as Text...** (in the command palette:
**Edit Instructions as Text...**) to see the open design's cutting
instructions as `.asc` text in a box, change them by typing, and turn the
changed text back into the design. It is the quickest way to renumber a
whole crown, paste a run of tiers from another schedule, or read a design the
way a cutter's sheet reads.

**What the text is.** Exactly what **Export Edited .asc** would write: the
refractive index the design is cut for (a material's own, or the one you
typed), every tier's mast freshly solved from the design as it is now, and a
concave tier as a footnote line. If the design does not solve, the dialog says
so instead of showing depths that are not real; fix the design first.

**What the text leaves out.** A line under the intro, starting "Not in this
text:", names what an `.asc` file cannot hold for this design: tier notes,
cheater offsets, depth and width targets, tier relations, concave tiers, and
always the preform, the material and the girdle size. These are not lost by
opening the dialog. Tiers you keep by name keep them (see Apply below).

**The live check.** A moment after you stop typing, one sentence under the
box says whether the text can be used. A problem is shown in red with its line
number, for example "Line 7: ..." — a missing field, a word where a number
belongs, no gear or symmetry line, an angle outside -90 to 90 degrees, or an
index that is not on the gear. When the text is fine, the dialog lists what
**Apply** would change and what it would lose or ignore, so you see the cost
before you pay it.

**Apply** changes the design in one step. **Undo** takes the whole text edit
back at once, and the History tab shows it as one entry,
"Edit instructions as text". Because the text carries no tier ids, tiers are
matched by name; a renamed line is matched by its angle and indices, and when
the number of tiers has not changed, by position. A tier that stays keeps its
note, cheater offset, depth target, relation and meet rule. The exceptions are
stated in the summary before you apply:

- A tier whose line you delete takes its note, offset, target and relation
  with it, and a relation that read a deleted tier is dropped.
- If you change a tier's depth in the text, its depth target is dropped, and a
  changed mast or a changed `G` instruction pins the tier to what you typed,
  the same way opening that `.asc` would.
- A tier whose angle follows a relation keeps the relation's angle; an angle
  typed on its line is ignored.
- Changing the gear takes the new gear as written, and any index that falls
  off a smaller wheel is refused with its line number. The index numbers are
  not rescaled to the new gear.
- While the design has a concave tier, the gear cannot be changed in the text
  at all. The concave tiers are not in the text, so they could not follow the
  new gear: tooth 24 is 90 degrees on a 96-tooth gear and 45 degrees on a
  192-tooth one. The message names the concave tiers and sends you to the
  design settings, which change the gear and move the concave tiers with it.

The preform, the material and the concave tiers are never changed by Apply.
After it, no tier is selected, because the old tier numbers no longer mean the
same facets.

**Revert** writes the text again from the design and drops your edits. Closing
the dialog with edits that were not applied asks first.

**Compare with...** lays the text in the box against another text, line by
line: **Saved file on disk** (the `.indicatrix` file the design was last
opened from or saved to, shown as the text it gives; for a design that came
from a plain `.asc`, that file's text; a design never saved has nothing to
compare with and says so) or **Snapshot** (the design kept by Snapshot Design,
available once you have taken one). A line only in the box is marked
**added** and shows only a new line number; a line only in the other text is
marked **removed** and shows only an old line number, so the two can be told
apart without the green and red tint. Long runs of unchanged lines fold into
"N unchanged lines", and a count of added, removed and unchanged lines sits
above the list. Choose **Back to the text** to return to editing; the text is
exactly as you left it.

## `.gcs` — Gem Cut Studio export (experimental)

**Export as Gem Cut Studio (.gcs)...** writes the same cutting instructions
**Export Edited .asc** would write, as a Gem Cut Studio `.gcs` file. In the
editor it sits in the command bar's **Export...** menu and in the File menu;
it re-solves the design first and asks the same "not a closed solid"
question as the `.asc` export. In the catalogue, the small **gcs** button next
to Export .asc exports a design's original `.gcs` file unchanged when it was
imported from one, and otherwise writes one from its attached `.asc` (a design
with no attached `.asc` has no mast distances, so the app asks you to solve it
in the editor and export from there). The app computes every facet's polygon
from the facet planes and rescales the stone the way Gem Cut Studio does. The
export is marked **experimental**: it follows the published Gem Cut Studio
1.1 file description and reads back correctly in this app, but it has not yet
been checked in Gem Cut Studio itself, and chiral designs (crown and pavilion
twisted against each other) are untested. Open the file in Gem Cut Studio and
compare it with the faceting diagram before cutting from it.

## Opening `.gem` and `.gcs` files in the editor

**Open** also accepts a GemCAD `.gem` file or a Gem Cut Studio `.gcs`
file. The design is converted to `.asc` cutting instructions exactly as Import
converts it (Chapter 1) and opens like a plain `.asc` with no `.indicatrix` design file. The
window title names the file you opened, but the design is recorded under
`<name>.asc`: **Save** asks where to write a new `<name>.indicatrix` and never
overwrites the `.gem` or `.gcs`. Anything the
converter noted (a preform that is not converted, a hidden tier, a missing
refractive index) is listed in the toast.

The same conversion applies to catalogue designs. When a design's attachments
include no `.asc` but do include a `.gem` (or, failing that, a `.gcs`), **Load
Selected**, the detail view's 3D preview and a remote worker's library all
build the design from that file's real facet geometry instead of the
placeholder angle-table reconstruction, and the catalogue's **Export .asc**
writes the converted cutting instructions. A file that cannot be read falls
back to the angle table exactly as an unreadable `.asc` does.

## `.indicatrix` — this app's own native format

`.asc` has no place to record everything this editor tracks: the rough's
shape and size, exactly how each tier's meet is stated (not just a
free-text instruction), which tiers are deliberately detached from their
symmetric family, the real-world girdle diameter in millimetres, and your
chosen material/specific-gravity settings.

The **`.indicatrix` design file** keeps all of that, and it is **one
self-contained file**: every tier in full (angle, indices, meet
constraint, name, notes, cheater offset, target), the rough, the material
(including a custom material's own numbers), the schedule's header, gear,
symmetry and refractive index, the girdle size, and a short history of
your edits. Nothing else has to sit beside it, and nothing else is written
when you save. It is plain text (TOML), so you can open it in a text
editor, read it, and put it under version control. Its first two lines
name the format and its version, which is how the app tells it apart from
every other file.

Click **Save** (or press Ctrl+S) to write it. The first time, a Save As
dialog offers `<name>.indicatrix` (the extension is added if you leave it
off); after that, Save writes straight to the same file. **Save As**
always asks, and from then on the design belongs to the new file.

- Saving keeps the file's previous version as `<name>.indicatrix.bak`
  alongside it — one generation back, so a second save overwrites the
  previous `.bak` in turn. The new file is written to a temporary name
  first and swapped into place, so a failed or interrupted save never
  leaves a half-written design.
- The file never replaces your `.asc`. To hand a plain schedule to another
  cutter or program, use **Export Edited .asc** (above). Such an export
  re-solves the design and writes the tier names with every run of
  whitespace turned into a single `_` (so "Crown Main" reads "Crown_Main"
  in the `.asc`); the `.indicatrix` file keeps the name you typed.
- `.asc` has no field of its own for a cheater/azimuth offset (Chapter 4),
  so an exported `.asc` carries a tier you have offset with its index-wheel
  position(s) shifted by a fraction of a tooth instead — `offset_deg / 360 *
  gear teeth`, added to each of that tier's indices and rounded to three
  decimal places. The `.indicatrix` file stores the offset itself.
- `.asc` also has no way to say that one tier's angle *follows* another's
  (a relation — Chapter 4). An exported `.asc` writes every such tier's
  current angle as a plain number; the relation stays in the `.indicatrix`
  file only.

A design with no anchor tiers, or no tiers at all, can still be saved --
**Save** writes it as a **draft**: the file records that the design did not
solve, and every tier's real constraint is in it. The toast says so plainly,
e.g. "Saved '&lt;path&gt;' as a draft (no scale-reference tier yet). Add a
scale-reference tier to finish it." Opening a draft back up rebuilds its
tiers exactly, so nothing is lost by parking a design mid-thought.

A design with more facet planes than the solver can verify (currently more
than 400 total planes) is saved the same way, as a draft, rather than being
rejected outright. Solve down to 400 planes or fewer (or split the design)
to get real cutting instructions out of it.

### Opening a design

Click **Open** and choose a `.indicatrix` file (the first filter in the
dialog), or pick it from the File menu's **Open Recent** submenu. The app
looks at what the file holds, not at its name, so a design file opens as one
whatever it is called. A file saved by a **newer version** of the app is
refused with a message saying so ("saved by a newer version of
Indicatrix ... Update Indicatrix to open it"), rather than opened wrongly.

You can also open a design straight from the operating system. Starting the
program with a file as its argument — `indicatrix-cut "C:\designs\round.indicatrix"`
— opens that file once the window is up; a `.asc` (or `.gem`/`.gcs`) given
the same way opens like **Open** on that file. This is what a double-click on a
`.indicatrix` file does once the file type is registered with the system
(`docs/file-association.md` in the app folder gives the registry and desktop
entries). A design named this way is opened instead of the "Recover unsaved
work?" or "Reopen last design?" offer; a leftover autosave stays where it is
and is offered at the next normal start. Each launch opens its own window: a
second double-click does not hand its file to a window that is already open.

### Older `.indicatrix.toml` and `.gemcut.toml` files

Before the `.indicatrix` design file existed, **Save Native** wrote a plain
`.asc` plus a small `.indicatrix.toml` sidecar that held only what `.asc`
cannot (the rough, the meet constraints, the material). Those pairs — and the
still older `.gemcut.toml` sidecars, from before this app was renamed from
GemCut — **still open**: choose the sidecar (or its `.asc`) in **Open**, or
import the `.asc` into the catalogue, exactly as before. Opening one does not
change or delete any of its files. The next **Save** writes a new
`<name>.indicatrix` — the dialog opens in the same folder and suggests the
same name — and leaves the old `.asc` and sidecar alone. Open Recent keeps old
entries working, and drops an old entry once you have saved that design in
the new format.

The File menu's **Open Recent** submenu lists the design files you have saved
or opened, newest first, so you do not have to hunt for a file picker to get
back to one.

## What happens if an older sidecar and its `.asc` disagree

This applies to the older paired files only: a `.indicatrix` design file
has no `.asc` to disagree with. An older `.indicatrix.toml` sidecar remembers a
fingerprint of its paired `.asc` from the moment it was saved. If you open a
sidecar whose `.asc` has since been changed by some other means (hand-edited,
or touched in another program), the app notices the mismatch and asks you what
to do, rather than silently picking one file over the other: a dialog headed
"Older Sidecar Out of Sync" explains that the per-tier meet constraints and
detached facets in the sidecar can no longer be trusted to line up by
position with the changed `.asc`, and offers **Apply Sidecar Anyway**
(hidden if the two files no longer even agree on how many tiers there
are), **Use .asc Only** (load the geometry with none of the sidecar's
extra information), or Cancel.

## What Save does to your catalogue

**Export Edited .asc** genuinely never touches the catalogue at all — file
only, no database write of any kind. But **Save itself does register the
design in your catalogue**, every time it succeeds (the button's hover hint
says so too). The row is given the
design's cutting instructions as an `.asc` — preserved byte-for-byte when
the schedule has not changed since it was loaded, regenerated when it has — and
the `.indicatrix` file beside it; the editor opens the `.indicatrix` file
whenever a row has one:

- If this design already has a catalogue row (you loaded it from there, or
  a previous Save already created one), that row's schedule,
  attached files, and every geometry-derived column are updated in place.
- If that row has since been deleted from the catalogue by some other
  means, a new row is inserted instead, and you get a toast saying so:
  "This design's catalogue row no longer exists (it may have been
  deleted) -- saved as a new catalogue entry instead of updating it."
- If this design has never been saved to the catalogue before (including a
  design loaded from a **remote** library — see below), a brand-new local
  row is inserted, and every later Save updates that same row.

The catalogue write happens only *after* the `.indicatrix` file is
safely written to disk, and only ever adds to your success — a
failure to update the catalogue is reported as its own toast ("Catalogue
not updated: ...") without undoing or invalidating the file save you just
made, since the file on disk is already the design of record either
way. This holds even for an unexpected internal error while the catalogue
write-back re-measures the design's geometry: that step is guarded so it
cannot bring down the save in progress, and instead reports "...updating
the catalogue failed unexpectedly... the file itself is safe; try Save
again to retry the catalogue update." The ordinary success toast itself
only mentions the file ("Saved '...'."), not the catalogue
update, since the update is the normal case.

### What a Save update deletes

When Save updates an **existing** catalogue row (not when it
inserts a brand-new one), it also deletes that row's cached preview
images and tilt-performance curves. This is deliberate, not a bug: both
describe the geometry as it was *before* this save, and would otherwise
silently show stale data next to the design's new angles. They are not
regenerated immediately — the catalogue card shows placeholder thumbnails
again, and any Tilt Performance filter (Chapter 2) recomputes on demand —
right-click the card and choose Generate Previews / Compute Tilt Curves
when you want them back.

## What a `.indicatrix` file carries besides the design

A design file is meant to be handed to someone else and still make sense, so
besides the tiers it keeps the facts about the design that cannot be worked out
again from them:

- **Descriptive metadata** (the `[meta]` table): a stable id, the title,
  designer and the designer/citation line, where it was published and the
  source's own design id, shape and shape category, competition label, the
  names of its PDF and `.gem` file, your notes, tags, licence and copyright
  text, the creation and last-save times, and the *ignored* and *excluded from
  the Rough Planner* marks.
- **Attached files** (the `[[attachments]]` array): the design's PDF, the
  original `.gem` and `.asc`, the diagram image and any other file, stored
  byte for byte (up to 64 MiB in total, so a design file stays one self-contained
  text file).

Everything the file's own tiers and schedule determine is **not** stored and is
recomputed whenever the file is imported: the length, height, table, pavilion
and crown ratios, the volume, the facet count, the angle-settings table, the
solid hull and extents, preview images, tilt curves, and any Rough Planner
results. An imported design therefore always shows figures that match its
geometry, never a stale copy.

On **Save**, the metadata comes from the design's library row when it has one
(so edits you made in the library are written into the file; changing a row's
metadata also marks the open design as having unsaved changes), and from the
file you opened otherwise. Fields the library has no column for (notes,
licence, copyright, the id, the creation time and any keys a newer version
wrote) are kept exactly as they were loaded, and the id and creation time stay
the same from one Save to the next. Tags are written sorted. If the attachments
add up to more than the limit, the Save stops with a message instead of leaving
a file out.

On **Import**, the title, designer, source, shape, competition, PDF and `.gem`
names, tags and marks fill the new library row, the diagram image becomes the
row's diagram image, and every attachment is attached to the row. The design file
itself stays attached too, so the fields the library has no column for travel with
the row. Re-importing a file with the same name replaces the earlier row, exactly
as for any other import.

## What the library keeps about a design besides the file

The design file holds the design and nothing else. Some things you make *around*
a design are kept in the library database on this computer instead, so they never
change the design file:

- the **variants** you save (named copies of the design that you can come back to),
- the cutting steps you have marked **done**, and
- the **lighting** you chose for the design.

The library knows which design these belong to by the design's **id**: the `id` line
in the file's `[meta]` table. Every design has an id from the moment it opens:

- A `.indicatrix` file that has an id keeps it. Opening and saving never changes it.
- A `.indicatrix` file without one, a `.asc`, `.gem` or `.gcs` file opened from disk and
  an older `.indicatrix.toml` pair are given an id worked out from where the file is. Open
  the same file again and you get the same id, so its variants, progress and lighting are
  still there even if you never saved it. The next **Save** (or autosave) writes the id
  into the `.indicatrix` file, and from then on the file keeps it wherever you move it.
  If you move or rename a file that has no id yet, it counts as a different design until
  you save it.
- A design from a remote library, a new design and a design recovered from an autosave
  snapshot get a new id each time they open. The next **Save** (or autosave) writes it
  into the `.indicatrix` file. If you never save such a design, the id is gone when you
  close it and anything kept under it can no longer be found.
- A design you load from your catalogue whose attached file has no id is given one worked
  out from its catalogue entry, so loading the same entry again finds the same variants,
  progress and lighting. Once you save it to a file, that id is written into the file.
- **Save As** keeps the id. The copy is the same design, so it shows the same variants,
  progress and lighting as the original.

All of this stays on this computer. Copying a `.indicatrix` file to another computer
takes the design, but not its variants, progress or lighting. Deleting a design from the
catalogue does not delete them either, because the design may still exist as a file.

## Confirming a save when the design is not a closed solid

If you click Export Edited .asc or Save (or Save As) while
the design does not currently solve to a closed solid, the app does not
silently write a broken file. A dialog headed **"This design is
not a closed solid"** appears, showing the same problem text the status
strip already shows (Chapter 5) followed by "Save anyway? The written file
will note this in its own header." (or "Export anyway?" from the export
path).

- Clicking **Cancel** cancels the save/export entirely — nothing is written,
  and there is no toast, since this was your own deliberate cancel.
- Clicking **Save Anyway** (or **Export Anyway**) writes the file anyway, with a leading header line
  stamped into it: `NOT A CLOSED SOLID -- <the same problem text>`. This
  stamp is idempotent (saving an already-stamped file again does not add
  a second copy) and is a separate marker from the `RECONSTRUCTED` header
  a catalogue export without an original file gets (above) — the two
  never both apply to the same export.

## Autosave and recovery

An **autosave** runs automatically every two minutes while there are
unsaved changes, to its own recovery file — `<design-name-or-"untitled">
.autosave.indicatrix`, stored next to this app's own settings file
(Chapter 1), never beside your real design files. It is a complete
`.indicatrix` file. It never overwrites the design you last deliberately
saved, and it does nothing at all while the design has no unsaved changes,
or while a background solve is in flight. The moment you do Save for real,
that autosave file is deleted. Recovery files left by older versions
(`<name>.indicatrix.autosave.toml`) are still found and still open.

If the app finds a leftover autosave file the next time it starts (a sign
the previous session did not shut down cleanly), it offers to recover it
**before** offering to reopen your last design — a dialog headed
**"Recover unsaved work?"** with the message "The previous session ended
without saving. A recovery file is still on disk: `<path>`", and buttons
**Recover** / **Not now** / **Delete**. Declining ("Not now") leaves the
file exactly where it is; it is only ever cleared by a later successful
save, not by declining the offer. Choosing **Delete** instead removes
exactly that one offered autosave file immediately, without opening it.
(With no leftover autosave, the same dialog instead offers to reopen your
most recently used design file, headed **"Reopen last design?"** with only
**Reopen** / **Not now** — no Delete, since that file is your own real
save, not a recovery snapshot.)

A recovered autosave is never a place Save writes back to: the first Save
asks for a file name and suggests the design's own. An autosave is
self-contained: it carries the full tier list, material, offsets and
notes, so recovery never asks for a paired `.asc`.

## What a design loaded from a remote worker does on first save

Loading a design from a remote worker's library (Chapter 10) never
attaches it to any catalogue row — a remote entry's own ID and a local
catalogue row's ID are unrelated numbers, so there is nothing to update
even if a matching number happened to exist locally. The **first** Save
of a remotely-loaded design therefore always inserts a **new,
local** catalogue row, tagged as belonging to your local library, exactly
like saving a brand-new design. Every Save after that updates that
same new local row — the original remote entry is never touched.

## Other exports: Cutting Sheet and Diagram

Two further exports live in the **File** menu rather than the command
bar:

- **Export Cutting Sheet (HTML)...** writes a single, self-contained HTML
  file laid out like a printed cutting-instructions page: a header, the
  facet counts, the size ratios, the design data, four small facet drawings
  (the same drawing the Edit tab's Diagram view mode makes — Chapter 13),
  your comments, and then two tables of every tier in cutting order, one for
  the pavilion and one for the crown. The next section describes each block.
  It is meant to be opened in a browser and printed from there. It always
  re-solves the design fresh rather than reusing a cached solve, since the
  masts it prints are what you would actually set a mast gauge to.
- **Export Diagram (PNG)...** writes the crown/pavilion/profile drawing as
  its own standalone, print-quality image (about 1800×720 pixels, roughly
  150 DPI at A4 landscape width). It is one wide picture with the three
  panels side by side, not the four views of the sheet, and the layout
  change of the sheet does not touch it.

Both show "Cannot build a cutting sheet: ..." / "Cannot draw this design:
..." if the design does not currently solve, and a success toast naming
the file's path otherwise. Neither is affected by the not-closed-solid
confirmation above — a design that does not solve at all has nothing to
draw, rather than something to stamp a warning header onto.

### How the cutting sheet is laid out

The sheet follows the layout of a printed fantasy-cut page. From top to
bottom it has these blocks. A line or block with nothing to print is left out
rather than filled with a guess.

- **Header.** The design's title is the heading. If the design has no title
  the heading reads "Cutting instructions". Under it come a subtitle (when
  one is given), "by" and the designer, and the date. The title comes from
  the design's file details, the title, designer, shape and notes stored in
  its `.indicatrix` file (as they were when the design was opened or last
  saved, so save after you change them in the library). When a file
  detail is empty, the sheet falls back to the `.asc` header lines of the
  design: the first line that does not start with "by" is the title, and a
  line starting with "by " gives the designer. A design opened from a plain
  `.asc` file has only those header lines. The date is the month and year of
  the export, such as "October 2026", taken from your computer's clock when
  you export. Nothing else on the sheet depends on the clock.
- **Facet Data.** How many tiers and how many facets the stone has in the
  pavilion, the girdle and the crown, and the totals. A tier with no index
  list is one facet; any other tier counts one facet for each index. A
  concave tier counts one facet for each place the tool is used, on the side
  of the stone it belongs to. The culet counts as a pavilion facet. When the
  design has a table, the crown reads "32+1": the crown facets, plus the
  table.
- **Size Data.** The six ratios you can check against a printed diagram, to
  three decimals: L/W, H/W, V/W³, P/W, C/W and P/C. They are the same figures
  the status bar and the yield report show (W is the width of the stone), and
  P/C is the pavilion depth over the crown height. A dash means the design
  does not measure (it does not close, or it has no girdle to take the crown
  and pavilion from).
- **Design Data.** The refractive index with its material, for example
  "1.540 (Quartz?)" — a question mark means the material is a guess from the
  index, not a name you set. When the design gives a refractive-index range
  it is printed as "RI range" instead, followed by the size range in
  millimetres when one is given. Then the symmetry ("8-fold, mirror" or
  "8-fold"), the index gear ("96 index") and the shape. The girdle diameter
  and the carat estimate follow, when you have set a girdle diameter.
- **Views.** Four drawings in two rows, each labelled with the direction you
  look from: "down +Z" is the crown seen from above, "down -Z" is the
  pavilion seen from below, "down +X" is the side (the profile), and "down -Y"
  is the end, the side view turned a quarter turn. The labels use the stone's
  own axes, with Z along the axis of the stone.
- **Comments.** Your comment lines, one under the other: the design's notes
  and its `.asc` footnotes. The block is left out when there are none.
- **Pavilion and Crown.** Two tables with the same columns: the step number,
  the label (the tier's code), the angle, the index list, the instruction,
  then the mast, the mast in millimetres and the cheater offset when the
  sheet carries them. The girdle rows are in the Pavilion table. The Crown
  table ends with the table facet. The step numbers run on from the Pavilion
  table into the Crown table, so they are the order you cut in. A concave
  tier keeps its tool line under its facet line.

The plain-text version of the sheet has the same blocks in the same order
under plain headings, without the four drawings.

### How the cutting sheet is ordered and labelled

The sheet lists the tiers in the order they are cut, and that one order is
the same everywhere in the app: the cutting sheet, Cutting mode (Chapter 20),
the Cut slider (Chapter 13) and a Build this design lesson (Chapter 22) all
follow it. The plain-text version of the sheet follows the same rules.

- **Order.** The pavilion tiers and the girdle tiers come first, then the
  crown tiers, and the table is always last. Inside each part the tiers keep
  the order of the tier table, so a design stored in any order prints in the
  same cutting order. A concave tier (Chapter 16) is cut after the flat tiers
  of its side, so its row comes after them.
- **A tier that meets a later tier.** A facet cannot meet a facet that is not
  cut yet, so a tier that meets a facet listed lower in the same part of the
  table moves to just after the last facet it meets. If tiers meet each other
  in a circle, or the facet it meets is on the other side of the girdle, the
  tier keeps its place in the table.
- **Codes.** Each row is labelled with the tier's code: P1, P2, ... for the
  pavilion, G1, G2, ... for the girdle, C1, C2, ... for the crown, T for the
  table and Culet for the culet. The codes are numbered separately for each
  letter, in the order the tiers are cut, so P and G count on their own even
  when they alternate (P1, P2, G1, G2, P3). A concave tier carries on the
  count of its side: after the flat tiers P1 and P2, a concave pavilion tier
  is P3. The tier table's CODE column (Chapter 3) shows the same codes.
- **Instruction.** The text on each row starts with the tier's own name when
  it is more than an old-style name such as 1 or A: "Crown Main: Meet P1, P2".
  A tier that meets named facets prints the codes of those facets, in the order
  its Meets box lists them. A note recorded in an imported `.asc` file is
  printed exactly as the designer wrote it.
- **Index lists.** Positions are written with dashes and two digits each:
  96-08-16-24. A position between two teeth keeps two decimals (03.50), and a
  tier with no index list shows a single dash.

## Unsaved changes

While the design has edits that have not been saved, the window title
shows a leading "* " and the status strip (Chapter 5) shows a permanent
amber **UNSAVED** badge, so you are never in doubt about whether the file
on disk matches what is on screen. Clicking **New**, **Load Selected**,
**Open**, or closing the window while there are unsaved changes
asks first, rather than discarding them outright.

Separately, an autosave runs every two minutes while there are unsaved
changes and offers to recover itself on the next launch if the app did
not shut down cleanly — see "Autosave and recovery" above for the file
location and the recovery dialog's exact wording.

## A note on catalogue re-sync

The catalogue's own reconstruction mechanism (used when exporting a
design with no original file attached, described above) is separate from
the older-sidecar fingerprint check — they are two independent ways the app
flags "this file is not guaranteed to be the verified original," one for
catalogue designs with no attached file, and one for an older sidecar whose
paired `.asc` has drifted. Neither one silently pretends a reconstructed
or drifted file is the trusted original.

## Which format to use

| You want to... | Use |
|---|---|
| Hand a plain schedule to another cutter or another program | **Export Edited .asc** (editor) |
| Keep working on this design later in this app, with everything preserved | **Save** (a `.indicatrix` file) |
| Get the design's original file back out unchanged | **Export .asc** from the catalogue (when an original is attached) |
| Hand the design to a Gem Cut Studio user | **Export as Gem Cut Studio (.gcs)...** (experimental) |

Save is the safer everyday choice while you are actively working on a
design: the `.indicatrix` file holds the whole design, including everything
a plain `.asc` has no field for, and you can produce a fresh `.asc` from it
at any time with **Export Edited .asc**. Keep an `.asc` export if another
program or cutter needs the schedule — a `.indicatrix` file is only for this
app.

## Where exported and saved files go

Export Edited .asc and Export .asc open a normal Save As dialog every time
you click them, and so does the first Save of a design (and every Save As) —
you choose the destination folder and filename yourself, and
the app does not remember a fixed export folder the way the high-resolution
image export does (Chapter 9). Cancelling either dialog writes nothing at
all; there is no fallback location it quietly writes to instead.

The one folder the app suggests without being asked is `./exports/`,
relative to wherever you started the program (Chapter 1) — this only comes
up the first time you export something and only as a starting point for the
dialog, never as a silent write destination.

## Design file schema additions (tier ids, targets, authored RI)

The `.indicatrix` file's `[[tiers]]` array holds a stable per-tier id
(`tier_id`) and an authoring-level target (`target` — see Chapter 4), and
the document as a whole holds the authored refractive index kept
apart from the effective one (see Chapter 6). An older `.indicatrix.toml`
sidecar with none of them still opens exactly as before, reading each as
absent.

All three survive a real Save / reopen round trip: `Design::tier_id_at`/
`Design::tier_target` populate `tier_id`/`target` on save, and `Design::tiers`/
`Design::tier_ids`/`Design::tier_targets` are rebuilt from them on load — so a
tier's stable identity (what a manufacturability warning badges directly, per
the row identity note in Chapter 4) and any target you authored on it persist
the next time you open the file, rather than being reconstructed fresh from
an `.asc`.

### Relations and the file version

A tier whose angle follows a relation (`P2 = P1 - 2`, Chapter 4) is stored
with its relation, written against the tiers' stable ids rather than their
names, so renaming a tier never breaks one. The tier's own angle is saved as
well, as the angle the relation gives. Opening the file restores every
relation and checks it again against the angles.

The file's second line, `version`, becomes **3** **only when the design has at
least one relation**. A design without relations keeps the version it always
had (1, or 2 when it has concave tiers) and its file is byte-for-byte what an
older build would have written. Version 3 is the point of no return in one
direction: an **older build of the app cannot open a version 3 file**. It
refuses it with a message saying the file was saved by a newer version,
rather than opening it and quietly dropping the relations the next time it
saves. If you need to hand such a design to someone with an older build,
remove the relations first (**Remove relation** on each tier, Chapter 4) or
export an `.asc`, which carries plain angles.

## Next steps

Continue to Chapter 12 for a consolidated troubleshooting list and a
summary of the app's current limitations.
