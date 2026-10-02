# 14. Retarget, Snapshot, Compare, and Tilt Curves

## What you will do

This chapter is a quick map of four related "compare two things" tools
scattered across the editor, plus one rule worth knowing before you rely
on any of them: which results actually survive after you close the
design.

## Retarget

Covered fully in Chapter 6. In one line: **Retarget...** proposes new
crown/pavilion angles for a different material and shows you a table of
what would change before you apply anything. It compares your current
design against a *hypothetical* re-angled version of itself, not against
another design or an earlier state.

## Snapshot Design and Compare to Snapshot

Two buttons on the command bar's second row let you diff your current
angles against an earlier point in the same editing session:

- **Snapshot Design** — hover hint: "Capture the current design and its
  solved masts, to compare against later." Click it to capture the
  design's tiers and their currently solved masts under a label built
  from the design's own name.
- **Compare to Snapshot** — hover hint: "Diff the current design against
  the last snapshot." Click it to open a dialog titled **Compare to
  Snapshot**, listing one row per tier with its name, its angle at
  snapshot time versus now, the signed change in solved mast, and a status
  badge: **Same**, **Changed**, **Added** (a tier that did not exist at
  snapshot time), or **Removed** (a tier the snapshot had that is now
  gone). Click **Done** to close it.

If you click Compare to Snapshot before ever taking one, you get a toast:
"No snapshot taken yet -- use Snapshot Design first."

**A snapshot is not saved anywhere.** It lives only in memory for the rest
of this session — closing the design, closing the app, or clicking
Snapshot Design again (which replaces it) all lose it. It is not written
to the `.indicatrix` file, and it is not the same thing as Deep Solve's
comparison against the catalogue's printed proportions (Chapter 8) — a
snapshot only ever compares the current design against your own earlier
click of Snapshot Design, never against catalogue data.

## Visual before/after comparison

The tables above tell you which angles move; the **compare window** shows
you what the stone looks like before and after. Open it with **Compare…**
next to "Preview in viewport" in the Retarget dialog (once a proposal is
ready), with **Compare…** next to "Preview" in the Optimize tab (once a
result is waiting for Apply), or with **Compare visually…** at the bottom
of the Compare to Snapshot table. It is a separate window you can move and
resize freely; opening it again replaces whatever it was showing. Both
sides are solved when it opens, so a large design may read "Solving both
sides…" for a moment.

Two layouts sit at the top left. **Side by side** shows the two stones next
to each other; **Split slider** shows one stone with a draggable divider,
the "before" design to its left and the "after" design to its right, both
rendered at exactly the same pose and size. Dragging in either image (or
anywhere in the split view) turns both stones together, the mouse wheel
zooms both, and a double-click resets them to the front view. **Solid**
(the default) is the Edit tab's flat grey view and updates instantly;
**Traced** runs the path tracer at a modest fixed quality in the design's
own material (for Retarget, the "after" side uses the target material) and
starts once you stop turning the stone, showing the solid view meanwhile —
the status line reads "Tracing… 1 of 2" while it works. A side that does
not solve shows a hatched placeholder and the status line says why.

**Keep after** applies the change exactly as the Retarget dialog's or the
Optimize tab's own Apply button does (one undo step, same checks), then
closes the window; it is disabled unless both sides solve, and if the
proposal changed after you opened the window you get a toast asking you to
open Compare again instead. **Discard** closes the window without applying
anything and switches that feature's viewport preview off, leaving the
proposal or result pending so you can adjust it and compare again.
**Close** (or the window's own close button) just closes it. A snapshot
comparison only offers Close.

## Tilt curves: when a computed result actually persists

The **Compute Tilt Curves** button (and the catalogue-wide batch tool in
Chapter 2's Advanced Filters panel) always computes the full sweep and
shows it to you regardless of whether the design is saved. Whether that
result is then **kept** depends on one thing: does this design already
have a row in your catalogue?

- **Design already in the catalogue** (you loaded it from there, or have
  already run Save at least once): the curves are written back to
  that catalogue row, and you get a success toast, e.g. "Saved
  tilt-performance curves for this design." A later Tilt Performance
  filter (Chapter 2) can then use them without recomputing.
- **Design not yet in the catalogue** (a brand-new design, or one loaded
  as a placeholder reconstruction with no attached file): the curves are
  computed and shown in the dialog, but nothing is written anywhere. The
  toast says so directly: "Computed tilt-performance curves -- save this
  design to the catalogue to keep them."
- **Design edited while the sweep was running**: the result is discarded
  either way, with an info toast asking you to re-run it: "The design
  changed while computing tilt curves -- re-run to save an up-to-date
  result."

In short: compute tilt curves on a design you intend to keep only after
you have saved it into your catalogue at least once (Chapter 11), or plan
to recompute them once you have.

The catalogue-wide batch is different from the single-design button: it does
not show a result, it computes and saves the sweep of every design you chose.
With a remote coordinator configured and Live Compute including it, the batch
sends several designs to the remote at once (one request per design, so a
design's four axes are never split) — **Remote lanes for batches**, Chapter
10 — while your own computer works through the rest. A design the remote could
not sweep is computed on your own computer under **Local + Remote**, and
counted as failed under **Remote only**.

## Exporting a tilt performance video

The Tilt Performance dialog's **Export tilt video** section renders one
high-quality frame per swept angle (posed exactly like the dialog's own
hover preview), optionally overlays the swept brilliance/windowing/
extinction/angle values into each frame, then muxes the numbered PNG
sequence into an MP4 (or an animated GIF if `ffmpeg` is not on your PATH).

Every frame renders through the SAME compute setup as a still-image export
(Chapter 9): whichever local CPU / CPU+GPU / GPU mode you have chosen in
Settings, plus the remote coordinator if one is configured, with the same
graceful fallback to local-only rendering if the remote is unreachable or none
is configured. There is no separate "Compute" choice for the video itself,
but with a remote configured the section shows the same **Transfer** row as
the export dialog (Full data, or Final picture only: one finished PNG per
frame from the remote -- Chapter 10).
Because a video now shares the app's single GPU adapter and the remote
with the rest of the app, the live viewport pauses for the whole
video's duration — exactly as it does during a still-image export — and
resumes automatically once the video finishes, is cancelled, or fails.

## Stale results

Retarget's held proposal and the Tilt Performance curves both carry a small
amber **Stale: design changed** badge whenever the result on screen no
longer matches what the design currently looks like:

- **Retarget**: badged the instant a further edit lands on top of an
  already-built proposal (Shift mode's own rebuild, or a finished Optimize
  search) — the table stays visible (it is still useful context), but the
  badge says plainly that it may no longer describe the design you are
  holding. Click **Recompute** on the badge to rebuild the proposal against
  the mode/crown/target controls exactly as they stand now, the same as
  changing any of those controls does.
- **Tilt curves**: badged whenever the geometry, material, or light the
  curves were swept against has moved since the last sweep actually
  finished. Click **Recompute** to re-sweep against the current inputs —
  the same action the existing "this curve was rendered with a different
  material" prompt already offers for a material change specifically; this
  badge also catches a geometry edit, which that one does not.

Deep Solve and Optimize carry the identical badge and Recompute action in
their own panels and in the status strip's Details popup — see Chapter 8.
The path-traced viewport (Chapter 5) carries the same badge too, over the
live render, with its "Recompute" action running a full Solve — the only
thing that can ever refresh a trace.

## Next steps

Chapter 6 covers Retarget in full; Chapter 8 covers Deep Solve's own,
catalogue-proportion-based comparison; Chapter 11 covers Save and
what putting a design in your catalogue actually means.
