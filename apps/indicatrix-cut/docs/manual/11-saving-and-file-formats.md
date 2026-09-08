# 11. Saving and File Formats

## What you will do

This chapter explains the two file formats the editor writes — `.asc` and
`.indicatrix.toml` — when to use each, and how the app handles a catalogue
design's original file versus a locally edited one.

## `.asc` — the plain cutting-schedule file

`.asc` is the long-standing GemCAD-style schedule format: a plain text
file listing every facet's angle, index, and meet instruction. Two
different places in the app can produce one, and they behave slightly
differently — differently enough that the two buttons no longer share
the same label (they used to both say plain "Export .asc," which made it
easy to reach for the wrong one).

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
  match.

The `.indicatrix.toml` file is plain text (TOML format), which means you can
open it in a text editor, read it, add your own comments, and put it under
version control alongside your `.asc` files if you want to.

Click **Open Native** to load an `.indicatrix.toml` file back — this restores
everything the plain `.asc` alone cannot carry (preform, constraint kinds,
detached tiers, mm sizing, material). Older `.gemcut.toml` sidecars (from
before this app was renamed from GemCut) still open the same way.

## What happens if a native file and its `.asc` disagree

The native file remembers a fingerprint of its paired `.asc` from the
moment it was saved. If you open a native sidecar whose `.asc` has
since been changed by some other means (hand-edited, or touched in another
program), the app notices the mismatch. It does not silently trust either
file blindly: the design still loads, using the `.asc` on disk as the
authoritative geometry, but the native file's extra per-tier information
is only applied where it still safely lines up. The toast after opening
tells you plainly what happened, in the form "Loaded '&lt;path&gt;':
&lt;fingerprint note&gt;; &lt;overlay note&gt;."

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

## Next steps

Continue to Chapter 12 for a consolidated troubleshooting list and a
summary of the app's current limitations.
