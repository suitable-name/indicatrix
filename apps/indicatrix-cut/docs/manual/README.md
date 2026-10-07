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
   control (weights, budget/starts/seed, Polish, Only selected tiers, Preview),
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
16. [Concave Tiers](16-concave-tiers.md) — tool-cut facets (cylinder, cone,
    circle, disc, sphere): adding and editing them in the tier table and the
    inspector's concave form, how they print on the cutting sheet and in the
    schedule, how they show in the solid view and the render, what `.asc`,
    `.gcs` and `.indicatrix` keep of them, and the limitations.
17. [Preferences and Accessibility](17-preferences-and-accessibility.md) — Edit →
    Preferences...: the Simple/Advanced interface, UI scale, high contrast, larger
    drag handles for touch screens and pens, the Snap and Slice buttons the app
    remembers, and resetting the tutorials.
18. [History and Variants](18-history-and-variants.md) — the inspector's list of every
    change to the open design, with a small picture of each step: reading it, going
    back to a step with one click or the keyboard, why a jump is not an undo step, and
    what a new change after going back does. Then variants, named copies of a design
    kept in your library: saving one (from now or from any step), the list, opening one
    as a single undo step, which variant a design came from, comparing two designs as
    pictures or as text, and the limitations.
19. [The Command Palette and the Keyboard](19-command-palette-and-keyboard.md) —
    Ctrl+K: finding and running any action by typing part of its name, how the search
    ranks, recent commands, why some rows are dim and what their reason says, the
    commands on offer, and the shortcuts worth learning.
20. [Cutting Mode](20-cutting-mode.md) — Edit → Cutting Mode... and the Cut Mode
    button: the cutting steps one page at a time, the angle and indices in large type,
    the stone after each step, the index wheel, done marks and index ticks that are
    kept for the design in your library, "Changed since you marked it", and Reset
    progress.
21. [Angle Sweeps](21-angle-sweeps.md) — Edit → Angle Sweep...: trying one tier's
    angle over a range, choosing the tier, range and step, the optional tilt
    averages, reading the table and the chart, making an angle the tier's real angle
    in one undo step, copying or saving the rows as CSV, and the limitations.
22. [Tutorials and the Welcome Tour](22-tutorials.md) — Help → Tutorials...:
    short guided lessons inside the program, the tutorial browser (sections, search,
    Done marks, Reset progress), how a tutorial starts, the guide panel, the welcome
    dialog a new user sees first, and where progress is kept.
23. [Render Jobs](23-render-jobs.md) — File → Render Jobs...: collecting still
    pictures and tilt videos with Add to Queue, the Render Jobs window, Start Queue
    and Pause Queue, what Pause keeps, closing the app while a job renders, exporting
    a PowerShell or shell script, running a job file with `indicatrix-cli`, and the
    limitations.

### Appendices

- [Appendix A: Glossary](appendix-a-glossary.md) — faceting and optics
  terms used throughout this manual.
- [Appendix B: Keyboard Shortcuts](appendix-b-keyboard-shortcuts.md) —
  every global shortcut, plus the tier list's, the Solid viewport's and the
  angle fields' own keyboard and mouse shortcuts.
- [Appendix C: Built-in Render Materials](appendix-c-render-materials.md)
  — every built-in material's refractive index, birefringence, optical
  character, dispersion, and color.

## How to read this manual

You do not need to read this front to back. Four common starting points:

- **New to the app:** read Chapters 1, 3, and 7 — the main window, how the
  tier list and its terms work, and a full worked example of building a
  design.
- **Adjusting an existing design:** read Chapters 3, 6, and 8 — loading a
  design and understanding its tiers, adapting it for a different
  material, then Deep Solve/Optimize/Adopt to verify and improve it.
- **Rendering a design:** read Chapters 9, 10 and 23 — exporting an image,
  setting up a remote coordinator if you want faster or higher-quality renders,
  and collecting many renders as jobs that run one after another.
- **Planning what to cut from a piece of rough:** read Chapter 15 — how to
  model your rough and rank the library's designs by the weight they yield from
  it.

Whatever your starting point, Chapter 12 and the appendices are there when
something goes wrong or you need to look up a term, a shortcut, or a
material's numbers.

## Help inside the app

You do not need to leave the program to read this manual. **Help → User Manual**
opens it in a window of its own, built into the program, so it is the same text as
this folder and works on an installed copy too. The window has a chapter list on the
left, a search box that looks through every chapter, **Back** and **Forward** buttons
that follow the links you have clicked, and it scrolls straight to a section when a
link names one. Links between chapters work inside the window, and a web address
opens in your browser.

Many panels carry a small **?** button. It opens the manual at the section about that
panel. **Help → Glossary** opens a short dialog with every term of Appendix A and a
search box; its **Open in the manual** button goes to the appendix itself. **Open
manual folder** in the help window's toolbar shows the Markdown files on disk, if you
want to print them or keep a copy.
