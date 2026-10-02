# Indicatrix Cut — User Manual

Indicatrix Cut is a desktop program for lapidaries: a catalogue
browser for faceting designs, a physically based spectral renderer that
shows any design in a chosen gem material and reports its optical
performance, and — on the standard build — a cutting-design editor that
lets you build a design from scratch, edit it tier by tier, solve it for
real cutting depths, and check or improve its performance before you touch
a lap. This manual covers all three, in the order you are likely to need
them: getting oriented, browsing and viewing, then editing, solving, and
exporting a design of your own.

## Contents

1. [Getting Started](01-getting-started.md) — the main window, where your
   library and settings live, and whether the editor is available on your
   build.
2. [Browsing the Catalogue and Viewing a Design](02-catalogue-and-viewing.md)
   — searching and filtering, the 3D viewport's controls, and the
   tilt-performance graph.
3. [Loading a Design Into the Editor and Understanding the Tier List](03-loading-and-tier-list.md)
   — cutting-instructions terms, loading a design, starting a new one, and
   every tier-list column.
4. [Editing Tiers](04-editing-tiers.md) — the tabbed inspector, the tier
   form, per-facet and whole-tier detaching, reordering and removing
   tiers, and undo/redo.
5. [Solving](05-solving.md) — what Solve does, every status message it can
   show, and why it is a deliberate button press.
6. [Adapting a Design for Another Material](06-materials-and-refractive-index.md)
   — the built-in material list, building a custom material, RI Override
   vs. Material, the Design Settings panel, and the automated Retarget
   proposal (plus the manual procedure behind it) for re-cutting a
   design's angles for a different material.
7. [Creating a New Design From Scratch: A Worked Example](07-new-design-worked-example.md)
   — a full walkthrough building an 8-fold stone tier by tier.
8. [Deep Solve, Optimize, Adopt, and Apply](08-deep-solve-optimize-adopt.md)
   — verifying a solve against catalogue proportions, every Optimize
   control (weights, budget/seed, Polish, Only selected tiers, Preview),
   Adopt/Adopt all/Adopt sel., and fixing a tier whose solve is uncertain.
9. [Rendering and Export](09-rendering-and-export.md) — exporting a
   still image, and how local and remote computation hand off while you
   work.
10. [Remote Worker Setup](10-remote-worker-setup.md) — certificates,
    setting up the remote coordinator, testing the connection, full data vs.
    final picture transfer, how many pictures a catalogue batch keeps in
    flight on it, HDR scenes, and troubleshooting a connection gone quiet.
11. [Saving and File Formats](11-saving-and-file-formats.md) — `.asc`
    versus this app's own `.indicatrix` design file (and the older
    `.indicatrix.toml` sidecars that still open), and where your files go.
12. [Troubleshooting and Limitations](12-troubleshooting-and-limitations.md)
    — a consolidated symptom/cause/fix table, where to find the app's log
    file and how to raise its verbosity, and every current limitation in
    one place.
13. [The Solid Inspection View](13-solid-inspection-view.md) — the Edit
    tab's second viewport: view modes, orbiting, hover/click facet
    picking, dragging a facet's angle/depth/index handles, slicing a new
    facet with the mouse, the critical-angle and pending-edit overlays, and
    its own "Not solved" banner.
14. [Retarget, Snapshot, Compare, and Tilt Curves](14-retarget-snapshot-compare-tilt-curves.md)
    — a quick map of these four "compare two things" tools, and which of
    their results actually survive after you close the design.
15. [Planning a Rough](15-planning-a-rough.md) — Library → Plan Rough...:
    the Rough Planner window. Modelling a block, cylinder or pebble with
    edge, corner and face cuts (including click-to-cut in the 3D view) and the
    live weight check; the plan settings and the first-run measuring pass; the
    3D view; reading the ranked layouts, their metrics and cut plans; library
    links; saved plans, export and import; shortcuts; and the limitations.

### Appendices

- [Appendix A: Glossary](appendix-a-glossary.md) — faceting and optics
  terms used throughout this manual.
- [Appendix B: Keyboard Shortcuts](appendix-b-keyboard-shortcuts.md) —
  every global shortcut, plus the tier list's own keyboard and mouse
  shortcuts.
- [Appendix C: Built-in Render Materials](appendix-c-render-materials.md)
  — every built-in material's refractive index, birefringence, optical
  character, dispersion, and colour.

## How to read this manual

You do not need to read this front to back. Four common starting points:

- **New to the app:** read Chapters 1, 3, and 7 — the main window, how the
  tier list and its terms work, and a full worked example of building a
  design.
- **Adjusting an existing design:** read Chapters 3, 6, and 8 — loading a
  design and understanding its tiers, adapting it for a different
  material, then Deep Solve/Optimize/Adopt to verify and improve it.
- **Rendering a design:** read Chapters 9 and 10 — exporting an image, and
  setting up a remote coordinator if you want faster or higher-quality renders.
- **Planning what to cut from a piece of rough:** read Chapter 15 — how to
  model your rough and rank the library's designs by the weight they yield from
  it.

Whatever your starting point, Chapter 12 and the appendices are there when
something goes wrong or you need to look up a term, a shortcut, or a
material's numbers.
