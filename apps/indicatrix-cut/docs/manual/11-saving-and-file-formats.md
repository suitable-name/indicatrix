# 11. Saving and File Formats

## What you will do

This chapter explains the two file formats the editor writes — `.asc` and
`.indicatrix.toml` — when to use each, and how the app handles a catalogue
design's original file versus a locally edited one.

## `.asc` — the plain cutting-schedule file

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

## `.indicatrix.toml` — this app's own native format

`.asc` has no place to record everything this editor tracks: the rough's
shape and size, exactly how each tier's meet is stated (not just a
free-text instruction), which tiers are deliberately detached from their
symmetric family, the real-world girdle diameter in millimetres, and your
chosen material/specific-gravity settings.

Click **Save Native** to save all of that. This always writes **two**
files together — a real `.asc` and a paired `.indicatrix.toml` sidecar file —
never the native file alone. That means an `.indicatrix.toml` file is never the
*only* copy of your design; there is always a plain `.asc` alongside it
that any other GemCAD-compatible tool can open.

- If your schedule hasn't actually changed since it was loaded, the
  paired `.asc` is preserved exactly, byte-for-byte.
- The moment any edit changes the schedule, a fresh `.asc` is written to
  match. Only an **Exact scale value** tier's original note (the source
  file's `G` field) survives this regeneration, and only as long as that
  specific tier's own constraint has not itself changed since import. A
  **Named facet(s)** or **Unspecified vertex** tier's `G` field is always
  regenerated on any regeneration (blank, or `Meet <names>`) — even for a
  tier that was itself never touched — since only the scale-reference kind
  currently checks the imported note before falling back to a generated
  one. A tier you actually re-authored, or one that was never imported at
  all, also gets a generated note, same as before.
- Saving keeps the file's previous version as a `.bak` backup alongside
  it, for both the `.asc` and the `.toml` — one generation back, so a
  second save overwrites the previous `.bak` in turn.

A design with no anchor tiers, or no tiers at all, can still be saved --
Save Native writes it as a **draft**: a placeholder `.asc` (real angles and
indices, but no real masts yet) plus a native sidecar carrying every tier's
real constraint. The toast says so plainly, e.g. "Saved '&lt;path&gt;' as a
draft (no scale-reference tier yet). Add a scale-reference tier to finish
it." Opening a draft back up rebuilds its tiers from the sidecar, ignoring
the placeholder `.asc` masts entirely, so nothing is lost by parking a
design mid-thought.

The `.indicatrix.toml` file is plain text (TOML format), which means you can
open it in a text editor, read it, add your own comments, and put it under
version control alongside your `.asc` files if you want to.

Click **Open Native** to load an `.indicatrix.toml` file back — this restores
everything the plain `.asc` alone cannot carry (preform, constraint kinds,
detached tiers, mm sizing, material). Older `.gemcut.toml` sidecars (from
before this app was renamed from GemCut) still open the same way. The File
menu also keeps an **Open Recent** submenu of native files you have saved
or opened, so you don't have to hunt for a file picker to get back to one.

## What happens if a native file and its `.asc` disagree

The native file remembers a fingerprint of its paired `.asc` from the
moment it was saved. If you open a native sidecar whose `.asc` has since
been changed by some other means (hand-edited, or touched in another
program), the app notices the mismatch and asks you what to do, rather
than silently picking one file over the other: a dialog headed "Native
Sidecar Out of Sync" explains that the per-tier meet constraints and
detached facets in the sidecar can no longer be trusted to line up by
position with the changed `.asc`, and offers **Apply Sidecar Anyway**
(hidden if the two files no longer even agree on how many tiers there
are), **Use .asc Only** (load the geometry with none of the sidecar's
extra information), or Cancel.

## What Save Native does to your catalogue

Hovering **Save Native** shows the hint "Write the native .asc plus its
.indicatrix.toml sidecar to disk; your library is not updated." **This is
only half true.** It is exactly correct for **Export Edited .asc**, which
genuinely never touches the catalogue at all — file only, no database
write of any kind. But **Save Native itself does register the design in
your catalogue**, every time it succeeds:

- If this design already has a catalogue row (you loaded it from there, or
  a previous Save Native already created one), that row's schedule,
  attached files, and every geometry-derived column are updated in place.
- If that row has since been deleted from the catalogue by some other
  means, a new row is inserted instead, and you get a toast saying so:
  "This design's catalogue row no longer exists (it may have been
  deleted) -- saved as a new catalogue entry instead of updating it."
- If this design has never been saved to the catalogue before (including a
  design loaded from a **remote** library — see below), a brand-new local
  row is inserted, and every later Save Native updates that same row.

The catalogue write happens only *after* the `.asc`/`.indicatrix.toml`
pair is safely written to disk, and only ever adds to your success — a
failure to update the catalogue is reported as its own toast ("Catalogue
not updated: ...") without undoing or invalidating the file save you just
made, since the files on disk are already the design of record either
way. The ordinary success toast itself only mentions the files ("Saved
'...' and '...' ..."), not the catalogue update, since the update is the
normal case.

### What a Save Native update deletes

When Save Native updates an **existing** catalogue row (not when it
inserts a brand-new one), it also deletes that row's cached preview
images and tilt-performance curves. This is deliberate, not a bug: both
describe the geometry as it was *before* this save, and would otherwise
silently show stale data next to the design's new angles. They are not
regenerated immediately — the catalogue card shows placeholder thumbnails
again, and any Tilt Performance filter (Chapter 2) recomputes on demand —
right-click the card and choose Generate Previews / Compute Tilt Curves
when you want them back.

## Confirming a save when the design is not a closed solid

If you click Export Edited .asc or Save Native (or Save Native As) while
the design does not currently solve to a closed solid, the app does not
silently write a broken file. A system dialog titled **"This design is
not a closed solid"** appears, showing the same problem text the status
strip already shows (Chapter 5) followed by "Save anyway? The written file
will note this in its own header." (or "Export anyway?" from the export
path).

- Clicking **No** cancels the save/export entirely — nothing is written,
  and there is no toast, since this was your own deliberate cancel.
- Clicking **Yes** writes the file anyway, with a leading header line
  stamped into it: `NOT A CLOSED SOLID -- <the same problem text>`. This
  stamp is idempotent (saving an already-stamped file again does not add
  a second copy) and is a separate marker from the `RECONSTRUCTED` header
  a catalogue export without an original file gets (above) — the two
  never both apply to the same export.

## Autosave and recovery

An **autosave** runs automatically every two minutes while there are
unsaved changes, to its own recovery file — `<design-name-or-"untitled">
.indicatrix.autosave.toml`, stored next to this app's own settings file
(Chapter 1), never beside your real `.asc`/`.indicatrix.toml` files. It
never overwrites the design you last deliberately saved, and it does
nothing at all while the design has no unsaved changes, or while a
background solve is in flight. The moment you do Save Native for real,
that autosave file is deleted.

If the app finds a leftover autosave file the next time it starts (a sign
the previous session did not shut down cleanly), it offers to recover it
**before** offering to reopen your last design — a dialog headed
**"Recover unsaved work?"** with the message "The previous session ended
without saving. A recovery file is still on disk: `<path>`", and buttons
**Recover** / **Not now**. Declining leaves the file exactly where it is;
it is only ever cleared by a later successful save, not by declining the
offer. (With no leftover autosave, the same dialog instead offers to
reopen your most recently used native file, headed **"Reopen last
design?"** with **Reopen** / **Not now**.)

An autosave is self-contained: it carries the full tier list, material,
offsets and notes, so recovery never asks for a paired `.asc`.

## What a design loaded from a remote worker does on first save

Loading a design from a remote worker's library (Chapter 10) never
attaches it to any catalogue row — a remote entry's own ID and a local
catalogue row's ID are unrelated numbers, so there is nothing to update
even if a matching number happened to exist locally. The **first** Save
Native of a remotely-loaded design therefore always inserts a **new,
local** catalogue row, tagged as belonging to your local library, exactly
like saving a brand-new design. Every Save Native after that updates that
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
**Open Native**, or closing the window while there are unsaved changes
asks first, rather than discarding them outright.

Separately, an autosave runs every two minutes while there are unsaved
changes and offers to recover itself on the next launch if the app did
not shut down cleanly — see "Autosave and recovery" above for the file
location and the recovery dialog's exact wording.

## A note on catalogue re-sync

The catalogue's own reconstruction mechanism (used when exporting a
design with no original file attached, described above) is separate from
this native-file fingerprint check — they are two independent ways the app
flags "this file is not guaranteed to be the verified original," one for
catalogue designs with no attached file, and one for a native file whose
paired `.asc` has drifted. Neither one silently pretends a reconstructed
or drifted file is the trusted original.

## Which format to use

| You want to... | Use |
|---|---|
| Hand a plain schedule to another cutter or another program | **Export Edited .asc** (editor) |
| Keep working on this design later in this app, with everything preserved | **Save Native** |
| Get the design's original file back out unchanged | **Export .asc** from the catalogue (when an original is attached) |

Save Native is the safer everyday choice while you are actively working on
a design, since it never leaves you with only a format this app can open —
a real `.asc` always comes with it.

## Where exported and saved files go

Export Edited .asc, Export .asc, and Save Native each open a normal Save As dialog every time you
click them — you choose the destination folder and filename yourself, and
the app does not remember a fixed export folder the way the high-resolution
image export does (Chapter 9). Cancelling either dialog writes nothing at
all; there is no fallback location it quietly writes to instead.

The one folder the app suggests without being asked is `./exports/`,
relative to wherever you started the program (Chapter 1) — this only comes
up the first time you export something and only as a starting point for the
dialog, never as a silent write destination.

## Native schema additions (tier ids, targets, authored RI)

The native sidecar's `[[tiers]]` array has room for a stable per-tier id
(`tier_id`) and an authoring-level target (`target` — see Chapter 4), and
the document as a whole has room for the authored refractive index kept
apart from the effective one (see Chapter 6). All three are
`#[serde(default)]`: an older sidecar with none of them still opens exactly
as before, reading each as absent.

All three survive a real Save Native / reopen round trip: `Design::tier_id_at`/
`Design::tier_target` populate `tier_id`/`target` on save, and `Design::tiers`/
`Design::tier_ids`/`Design::tier_targets` are rebuilt from them on load — so a
tier's stable identity (what a manufacturability warning badges directly, per
the row identity note in Chapter 4) and any target you authored on it persist
the next time you open the file, rather than being reconstructed fresh from
the `.asc` alone.

## Next steps

Continue to Chapter 12 for a consolidated troubleshooting list and a
summary of the app's current limitations.
