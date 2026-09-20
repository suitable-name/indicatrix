# 1. Getting Started

## What you will do

This chapter introduces the application, its main window, and where it keeps
your files and settings, so you know your way around before you load your
first design.

## What the application is

Facet Diagrams Studio (this manual calls it "the diagram editor" or "the
app") is a desktop program for lapidaries — gem cutters — that does three
things:

1. It is a **catalogue browser**: it stores a library of faceting designs
   (cutting diagrams) and lets you search, filter, and inspect them.
2. It is a **spectral renderer**: it can show a physically based 3D render of
   any design in the library, in a chosen gem material and lighting, and
   report optical performance figures such as brilliance and fire.
3. It is a **cutting-design editor**: on the standard build (see below) it
   also lets you build a new design from scratch, edit an existing one facet
   tier by facet tier, solve it (work out the actual cutting depths), and
   check or improve its optical performance before you ever touch a lap.

The window title bar reads "Facet Diagrams Studio — Physically Based Gem
Explorer", and the top-left corner of the window carries the same name next
to a small gem icon.

## The main window

A **menu bar** (File / Edit / Help) runs above everything else. File
mirrors the editor's New / Open Native / Save Native / Export Edited .asc,
plus an **Open Recent** submenu of native files you have saved or opened
(Chapters 3 and 11; greyed out on a build without the editor — see the
Limitation note below); Edit mirrors Undo/Redo, each now saying what it
will actually do once there is something to act on; and Help → User Manual
opens this manual's own `README.md` in your system's default viewer.
Appendix B lists the keyboard shortcuts that work alongside it. While the
Edit tab has unsaved changes, the window title carries a leading "* " so
you always know at a glance whether the file on disk matches what's on
screen (Chapter 11).

Below the menu bar, the window is divided into three areas:

- A **top toolbar** running the full width of the window: the app name and a
  status line on the left, then a search box, a Shape filter, a Gear filter,
  a Filters button (for the advanced range filters), an Import button, and a
  Remote button on the right.
- A **catalogue panel** on the left, listing every design that matches your
  current search and filters. You can collapse this panel to a thin rail to
  give the render viewport more room, using the collapse control at its edge;
  click the rail to bring the panel back.
- A **detail area** on the right, which shows the design you have selected.
  This area has its own header (title, designer, shape, and a menu of
  per-design actions), and below it three tabs:
  - **3D Spectral Preview** — the render viewport, described in Chapter 2.
    On the standard build, this tab has its own inner pair of sub-tabs,
    **Live Render** and **Edit**, plus a **Pop Out** button that moves the
    render into its own always-on-top window (useful on a second monitor).
    The Edit sub-tab is the cutting-design editor covered in Chapters 3–8.
  - **Cutting Schedule** — a plain table of every facet's angle, index, and
    notes, filterable to All Steps / Pavilion / Crown, with a "Copy
    Schedule" button and a click-to-copy on any row.
  - **Files & Downloads** — any original file(s) attached to the design in
    the catalogue (typically the source `.asc`), each with a "Save / Export"
    button and a "Copy URL" button.

If a design has no cutting schedule recorded, the Cutting Schedule tab shows
"No cutting schedule recorded for this diagram." If it has no attached
files, Files & Downloads shows "No files or GemCAD data attached to this
diagram," and notes that anything you do export lands in `./exports/`
relative to wherever you launched the program.

## Whether the editor is available to you

The cutting-design editor (the Edit sub-tab, and everything in Chapters
3–8 of this manual) ships in the **standard build** of the application. It
is compiled in by default. If someone has built the program with the editor
deliberately switched off, the Edit sub-tab and its button simply do not
appear — only Live Render is offered under the 3D tab. There is no in-app
setting to turn it on or off; it is decided when the program is built.

Similarly, GPU-accelerated rendering (Chapter 2 and `docs/gpu.md`-derived
material) is **off in an ordinary build** and only present in a build
compiled with GPU support turned on. If your copy has it, an extra "Local
Compute" choice appears in Settings; if not, rendering always uses the CPU.

## The design library and where it lives

The catalogue you browse and search is a local database file,
`facet_diagrams.sqlite`. It is opened relative to the folder the program is
started from (its current working directory), not a fixed application-data
folder.

**Limitation.** Because of this, launching the program from a different
folder than usual opens (or silently creates) a *different*, empty-looking
library. If your designs seem to have vanished, check that you are starting
the program the same way you did before. Exporting is not affected by this:
every export asks you where to save through a normal file-save window, and
`./exports/` is only ever offered as a suggested starting folder.

You can also point the app at a **remote worker's** library instead of your
local one — see Chapter 10. A small badge next to the status line always
shows which library you are currently browsing ("Local library" or the
remote worker's name), so you can tell at a glance.

## Bringing your own designs in

Click **Import** in the top toolbar to open the import panel. From there you
can:

- Click **Choose file...** to import one `.asc` file, or
- Click **Choose folder...** to import every `.asc` file in a folder.

If you want subfolders included in a folder import, turn on **Include
subfolders (set this first)** *before* clicking "Choose folder..." — the
setting is read at the moment you click, not afterwards, so switching it
on and then choosing the folder is the only order that works. Cancelling
either file/folder picker does nothing; the panel just stays open with
nothing imported.

An import runs in the background, so the rest of the app stays usable. For
a folder import, the panel shows "file N / M" and a progress bar as each
file is picked up. When a design's geometry is measured during import (its
proportions, and a best-guess shape classification), that information fills
in automatically rather than being left blank.

## Settings and where they are stored

Your rendering and remote-worker preferences (sample count, bounce cap,
exposure, lighting, remote workers, lighting presets, and more — Chapters
2, 9, and 10 cover what each one does) are saved automatically to a settings
file, so they come back the next time you start the app. Its location
depends on your operating system:

| Platform | Settings file |
|---|---|
| Windows | `%APPDATA%\indicatrix-cut\settings.toml` |
| macOS | `~/Library/Application Support/indicatrix-cut/settings.toml` |
| Linux/Unix | `$XDG_CONFIG_HOME/indicatrix-cut/settings.toml`, or `~/.config/indicatrix-cut/settings.toml` |

You do not need to edit this file by hand — every control that changes it
has an on-screen equivalent — but if the file is ever missing, unreadable,
or damaged, the app quietly falls back to its defaults rather than failing
to start; nothing you do in the app can corrupt your settings file, since
writes are saved safely (to a temporary file first, then swapped in).

## Next steps

Continue to Chapter 2 to search the catalogue and view a design in 3D, or
jump to Chapter 3 if you already have a design open and want to start
editing it.
