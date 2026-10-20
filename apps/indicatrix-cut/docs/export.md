# indicatrix-cut — Import / Export

How `.asc` files move in and out of the local library. For everything else see
[the README](../README.md).

For rendered pictures, the export dialog selects **Path-Traced** or **Ray-Traced**.
Ray-Traced freezes 1/4/8/16 AA taps and Fire with the job, finishes at that tap count,
and bypasses denoising; it does not use a progressive sample target. Path-Traced
keeps its spectral sampling and denoising controls. Frozen Ray-Traced jobs use
render-job format 2; historical format-1 jobs remain Path-Traced. Remote peers must
all use protocol 29 to carry this choice. See the
[rendering manual](manual/09-rendering-and-export.md) for the approximation's limits.

**Import**: the Import Designs panel has **Choose File...** (one `.asc`, `.gem` or
`.gcs` file) and **Choose Folder...** (every `.asc`, `.gem` and `.gcs` file in it,
and any stand-alone `.isdd` or legacy `.indicatrix` design file; the extension is
compared case-insensitively). The **Include Subfolders (Set This First)** switch
makes a folder import descend into the folders inside it; set it before pressing
**Choose Folder...**. Choosing runs the import immediately, with no path field and no
separate confirm step (apart from the replace question below); cancelling the picker
does nothing at all. The work happens on a worker thread, reporting `Importing N / M`
as it goes, so a large folder never freezes the window. A `.gem` (GemCAD) or `.gcs`
(Gem Cut Studio) file is converted to `.asc` cutting instructions, and the original
file stays attached to the design; an `.asc` is parsed via
`indicatrix_formats::asc::parse_asc` and saved through
`indicatrix_vault::local::import_asc`. Per-file failures are collected and reported
in a summary rather than aborting the batch. When the import has finished, the
program asks whether to generate catalogue previews (see `import_preview_choice` in
[settings.md](settings.md)).

**Designs that replace an existing entry are asked about first.** A locally imported
design is keyed by `local://<file_name>`, and `diagram_entries.url` is unique, so
re-importing the same *filename* updates that design in place — which is what you
want for a genuine re-import, but means two different designs that happen to share
a filename collapse into one. Before anything is imported, a "Replace Existing
Design(s)?" question says how many of the files share a name with a design already
in the catalogue; **Continue** goes ahead and declining leaves the catalogue
untouched. The question cannot be switched off.

### Metadata the `.asc` File Doesn't Carry

A GemCAD schedule stores the cutting instructions, not the finished stone's
proportions. Title, refractive index, index gear, facet count, symmetry order and
mirror symmetry come straight from the file; the rest is **measured from the
design's own geometry** at import, using the same reconstructed facet planes the
viewport renders:

| Field | Source |
|---|---|
| `lw_ratio`, `hw_ratio`, `cw_ratio`, `pw_ratio`, `volume` | `indicatrix::geometry::stone_metrics::measure_solid`, stored as ratios over width (volume as `Vol/W³`, dimensionless) |
| `shape` | Classified from the girdle outline — see below |
| everything else | Left empty rather than guessed |

Nothing here is ever fabricated: when `measure_solid` cannot measure a design, the
fields stay empty.

**Shape classification is deliberately conservative.** It counts the design's
girdle facets (`indicatrix::geometry::girdle::classify_girdle_plane_indices`), because
the girdle outline *is* the silhouette a shape name describes. Twelve or more
sides at a near-unit length/width ratio reads as Round; exactly 3, 4, 6 or 8 sides
reads as Triangle, Square, Hexagon or Octagon. **Anything else is left blank.**
Oval, Rectangle, Marquise and Pear are never guessed among — telling them apart
needs outline curvature this does not measure, and a confidently wrong shape looks
authoritative in a way an empty one does not.

The count is used rather than the schedule's declared symmetry order for a
concrete reason: a round cut built from six repeats declares 6-fold symmetry, and
a fold-count rule calls it a hexagon. Its girdle has far more than six facets.

Shape is editable afterwards from the detail view, next to the title, populated
from `Database::get_unique_shapes()` — which serves the seeded canonical
vocabulary unioned with whatever shapes your library already contains.

**Export .asc**: if the selected diagram has an original `.asc` among its
attached files, that's exported byte-for-byte. Otherwise, a schedule is
reconstructed from the stored angle-settings table
(`indicatrix_vault::local::reconstruct_asc_schedule` → `indicatrix_formats::asc::to_asc_string`)
— the success message explicitly warns that mast distances in a reconstructed
file are placeholders, and the file itself carries a `RECONSTRUCTED` header
marker (see `indicatrix-formats`'s README on `mark_reconstructed`). Exported filenames are
sanitized (`\ / : * ? " < > |` stripped) and offered as the default name in a
native Save As dialog. Cancelling that dialog writes nothing at all — an export
the user declined to name is not quietly written somewhere else.

**Export as Gem Cut Studio File**, next to it in the detail header (Advanced interface
only), writes the catalogue's cutting instructions as a Gem Cut Studio `.gcs` file. It
is experimental; a design with concave tiers is first asked about with the "Export
Anyway" question (manual Chapter 16).

**Choosing paths.** Export destinations, the HDR environment map and the
remote-worker certificate folder all open a native OS dialog (`rfd`), seeded from
the field's current value when it names an existing path and the process working
directory otherwise. Those dialogs fill the adjacent text field rather than
replacing it, so pasting a path still works, and cancelling leaves the field
exactly as it was. Import is the exception: it has no field to fill, because
choosing *is* the action.

## High-Resolution Still Export

Independent of the viewport and of the `.asc` export above: renders the current
scene at its own resolution and sample count on its own worker thread, so a
multi-minute 4K render neither blocks nor corrupts the live viewport's
accumulation. Output is PNG, at 1080p, 4K, or a custom size (16..8192 px per
side), at 1..32768 samples per pixel, in sRGB, Display P3, or Rec.2020 — the two
wide-gamut spaces get a generated ICC profile embedded so the file is never
silently misread as sRGB.

**Live preview.** While the export runs, the dialog shows a thumbnail of the
render in progress, refreshed about twice a second alongside the progress bar.
It is box-downsampled straight from the live accumulation buffers (both of them,
on a hybrid CPU+GPU export) and normalised by the samples actually completed, so
it looks correct from the first tick rather than dark until the end. It is
always tone-mapped through the sRGB path regardless of the export's chosen color
space, because it is displayed on screen by a widget that cannot carry an ICC
profile — the preview is a guide to composition and convergence, not a proof of
the final file's color.

### Remote Compute: Timeouts and Liveness

A remote chunk dispatch (calibration or a share of the main render) is judged
against the same two-tier liveness deadline the live viewport's connection
uses (see [remote-rendering.md](remote-rendering.md)'s own "Timeouts and
Liveness" section): a generous 30s first-event grace for the wait for that
chunk's very first update (a worker's calibration/warm-up at a high resolution
with a coarse cadence can legitimately take that long), then a tighter 8s
steady-state deadline once at least one update has arrived. This prevents a false
timeout: exporting at 4K with a worker cadence of 20 samples/tick would otherwise
incorrectly report "worker silent" for a calibration probe that is, in fact,
still computing its first tick.

When a chunk's dispatch does time out (or fails for any other reason) partway
through, the export does not silently finish without that chunk's remaining
samples: with a local fallback available (`Compute: Both`), the unfinished
remainder is handed to the local CPU lane, a toast names how long the
worker was silent, and remote keeps trying fresh chunks (a fresh connection
each time) for whatever of the export's sample budget is still unclaimed —
remote is only given up on entirely after two consecutive chunk failures.
With no local fallback (`Compute: Remote`), a chunk that comes back short
fails the whole export outright, loudly, rather than writing an image that is
silently missing samples.

All of the above is the dialog's default **Transfer: Full Data** path. **Final
Picture Only** instead sends one `FinalImageRequest` for the whole image and writes
the finished PNG the remote returns. Local lanes are not idle by default, though:
with `Compute: Both`, the `contribute_to_final_picture` setting (default on,
Render & Output tab: "Final-Picture Exports: This Machine Renders a Share Too") traces
a share of the samples locally and uploads it as one `CONTRIBUTION`, which the
coordinator folds in before tone-mapping; turn the setting off to leave this
machine idle during the export instead. See
[remote-rendering.md](remote-rendering.md)'s "Transfer: Full Data or Final Picture" section for its fallbacks. A
scene lit by an HDR map is only sent to a remote that advertises HDR support; any
other remote leaves that export local-only, with a note, so the whole image is lit by
one environment.
