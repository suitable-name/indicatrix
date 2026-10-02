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
  file: a small embedded diagram (a crown/pavilion/profile facet drawing,
  the same one the Edit tab's Diagram view mode draws — Chapter 13) plus a
  table of every tier in cutting order (sequence number, name, angle,
  indices, solved mast, meet instruction, and any cheater/azimuth offset).
  It is meant to be opened in a browser and printed from there. It always
  re-solves the design fresh rather than reusing a cached solve, since the
  masts it prints are what you would actually set a mast gauge to.
- **Export Diagram (PNG)...** writes the same crown/pavilion/profile
  drawing as its own standalone, print-quality image (about 1800×720
  pixels, roughly 150 DPI at A4 landscape width — larger than the sheet's
  own small embedded copy, since this is meant to be viewed or printed at
  full size on its own).

Both show "Cannot build a cutting sheet: ..." / "Cannot draw this design:
..." if the design does not currently solve, and a success toast naming
the file's path otherwise. Neither is affected by the not-closed-solid
confirmation above — a design that does not solve at all has nothing to
draw, rather than something to stamp a warning header onto.

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

## Next steps

Continue to Chapter 12 for a consolidated troubleshooting list and a
summary of the app's current limitations.
