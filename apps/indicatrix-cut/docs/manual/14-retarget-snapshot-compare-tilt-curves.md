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
to the native sidecar file, and it is not the same thing as Deep Solve's
comparison against the catalogue's printed proportions (Chapter 8) — a
snapshot only ever compares the current design against your own earlier
click of Snapshot Design, never against catalogue data.

## Tilt curves: when a computed result actually persists

The **Compute Tilt Curves** button (and the catalogue-wide batch tool in
Chapter 2's Advanced Filters panel) always computes the full sweep and
shows it to you regardless of whether the design is saved. Whether that
result is then **kept** depends on one thing: does this design already
have a row in your catalogue?

- **Design already in the catalogue** (you loaded it from there, or have
  already run Save Native at least once): the curves are written back to
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
catalogue-proportion-based comparison; Chapter 11 covers Save Native and
what putting a design in your catalogue actually means.
