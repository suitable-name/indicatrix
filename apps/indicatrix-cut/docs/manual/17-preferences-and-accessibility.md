# 17. Preferences and Accessibility

## What you will do

Some choices are about you and your screen, not about one design: how many controls
you want to see, how big everything is, whether the colors are easy to read, and
whether the drag handles are easy to grab with a finger or a pen. They all live in
one place, the **Preferences** dialog. This chapter covers each one, and the two
viewport buttons the app now remembers between sessions. Every choice is saved
automatically and comes back the next time you start the app.

## Opening Preferences

Choose **Edit → Preferences...**, or press **Ctrl+,** (Ctrl and the comma key). The
dialog has four sections: Interface, Appearance, Direct manipulation and Tutorials.
Press **Escape**, click **Done**, click the × in the corner, or click outside the
dialog to close it. The shortcut does nothing while you are typing in a text field,
so a comma in a name stays a comma.

While a guide or tutorial step has locked them (the worked example in Chapter 7, the
tutorials in Chapter 22), the menu item, the shortcut and the Simple | Advanced pill
are locked, because a step locks the controls it does not need. A step about the
Simple | Advanced switch or about Preferences unlocks its own control; otherwise close
the guide first.

## Simple and Advanced

The app can show every control, or only the ones most designs need.

- **Simple** shows the controls most designs need.
- **Advanced** shows every control.

There are two ways to switch, and they are the same switch: the small
**Simple | Advanced** pill at the top left of the window, next to the library
badge, and the two choices at the top of the Preferences dialog. Hover the pill to
see a reminder. You can change it at any time; switching hides or shows controls and
nothing else, so your designs, your settings and your files are never affected.

A brand-new installation starts in **Simple**. If you already used the app before
this switch existed, you stay in **Advanced** and see exactly what you saw before.

In the two viewports and their dialogs, Simple mode hides the **Tilt Curve** and
**Save View Preset** buttons; in Settings, Preview Image Size, Preview Samples, Motion
Preview Resolution, Live Compute and Live Transfer, Local Compute, Max Ray Bounces and
the Tilt Performance Curve panel (Chapter 2); and in the tilt video export, Max Ray
Bounces, Color Space, Transfer and the folder-name template (Chapter 14). A hidden
setting keeps its value and still applies. When one of the hidden render settings is
not at its default, Settings says "Some advanced settings are in use. Switch to
Advanced to see them."

In the editor, Simple mode hides these (each one is listed again in the chapter
where it is described):

- **Tier table and Tier tab** (Chapters 3 and 4): the Schedule tab; the MAST, SOLVE,
  ORBIT and IMPORTED columns; Pin, Detach and Adopt all; Steps and Mirror; the
  table filter; the imported-meet line; and "Exact scale value" in the Meets list
  (it is shown when the tier already uses it). The Optimize tab's advanced controls
  are hidden too. In History and Variants, the per-row save-as-variant icon and
  "Compare text" are hidden. The Tier tab's "+ Add Concave Tier" and Offset stay
  visible.
- **Command bar** (Chapter 3): Deep Solve, Snapshot, Compare, Tilt Curves and
  + Concave. The Export button's menu drops its Gem Cut Studio (.gcs) entry.
- **Design settings panel** (Chapter 6): RI Override, Symmetry Order, Mirror and
  Apply Symmetry, Extra Header Lines, Footnotes, Gear Ref. Angle and the
  printed-proportions row; in the gear remap dialog, Rounding. When a hidden control
  holds a value, the panel says "Some advanced settings are in use. Switch to
  Advanced to see them."
- **Status strip** (Chapter 5): the solve-duration figure, and the Pin column of
  Deep Solve's results.
- **Menus**: File > Export Gem Cut Studio (.gcs); Edit > Edit as Text and Angle
  Sweep; and the three Regenerate entries of the Library menu.
- **Top toolbar** (Chapter 1): the Remote button, until a remote worker is set up
  or a remote library is showing.
- **Library** (Chapter 2): the Advanced Filters panel's "Compute missing tilt
  curves" button and its "Regenerate for filtered set" section, and the detail
  header's Export .gcs button.

- **Dialogs**: the Export Render Image dialog (Chapter 9) leaves out Compute, Max Ray
  Bounces, Color Space, Transfer and the lighting-preset fan-out. The remote worker
  dialog (Chapter 10) leaves out the live-stream mode, the update interval, the
  preview scale, the default export transfer and the number of lanes a batch keeps
  busy. Retarget (Chapter 14) leaves out Crown Handling and the Optimize objective,
  range and effort. The material editor (Chapter 6) leaves out the birefringence
  slider, the crystal system and optical character lists, and the coefficients
  switch (it stays when the material already uses coefficients). When a hidden
  setting is not at its default, the dialog says "Some advanced settings are in use.
  Switch to Advanced to see them." New Design, the Compare window, Edit as Text, Angle
  Sweep, Cutting Mode and the Rough Planner show everything in both modes; the Edit as
  Text and Angle Sweep commands themselves are hidden from the menus and the command
  bar in Simple mode.

All of these only hide the control. A tier, setting or file that already uses one
keeps working exactly as before.

## Interface scale

**Interface scale** makes everything in the windows larger or smaller. The choices
are **Automatic** (follow your operating system's display scaling, which is what the
app always did before), 75 %, 90 %, 100 %, 110 %, 125 %, 150 %, 175 % and 200 %.

The scale is applied when the app starts, so a change **takes effect after a
restart**. Close and reopen the app to see it. Picking and dragging in the viewport
uses the same scale, so a scaled window still hits what it shows.

If you have set the `SLINT_SCALE_FACTOR` environment variable yourself, your value is
used and this choice is ignored.

## High contrast

The **High contrast** switch changes the colors to near-black backgrounds, white text,
clearly visible borders and bright accent colors, for easier reading in bright rooms
or with low vision. It takes effect **immediately**, in the main window and in the
Compare and Rough Planner windows, including ones you open later.

Everything the app draws itself follows the switch: panels, dialogs, chips, toggles,
the tilt graph with its grid and hover card, the hint badges over the Live Render
picture and the warning banners. The standard buttons, text boxes, drop-down lists,
sliders, number boxes and scroll bars are always drawn in the dark look with white
text, in normal mode and in high contrast, whatever your system theme is (the app
has no light theme, so a light system theme does not turn them light). In high
contrast their borders stay thin and subtle, because the app cannot change how those
controls draw them; the controls the app draws itself get strong borders and a clear
focus ring.

A few colors keep their own value on purpose, because they stand for something:

- the curve colors of the tilt graph (cyan for brilliance, amber for windowing, red for
  extinction) and the red and green of the Compare overlay legend;
- color swatches that show a real color, such as the material colors and the gem
  colors in the material editor;
- the rendered pictures themselves. The picture in the Compare window and the Rough
  Planner keeps its own dark background and edge colors; the frame around it follows
  the switch.

The backgrounds and labels around these colors do follow the switch.

## Direct manipulation

**Larger handles** makes the angle, depth and index handles on the Solid viewport
(Chapter 13) about 60 % bigger, and makes the area you can grab around each one just
as much bigger. It is meant for touch screens and pens, where a fingertip is far
less precise than a mouse. A touch screen also has no "hover", so with larger
handles on, the line under the toolbar tells you what the three handles do as soon
as you select a facet, instead of waiting for you to point at one.

**Snap to gear steps when dragging** is the same setting as the **Snap** button in the
Solid view's toolbar: when it is on, dragging the angle and depth handles snaps to
round steps, and when it is off the handles move freely. Change it in either place
and the other follows.

### What the app remembers

- **Snap** (above) is remembered, so a session starts the way you left it.
- The Slice tool's **Symmetric** button is remembered: whether a new facet is
  repeated around the whole symmetric set or is a single index.
- The Slice tool itself is **not** remembered. While it is on, a left drag draws a
  line instead of turning the stone, so every session starts with it off.

## Tutorials

- **Reset tutorial progress** forgets which tutorials you have already finished, so
  they offer themselves again.
- **Show the welcome tour again** brings back the first-run tour that new
  installations get once.

Each button confirms what it did with a short line under it.

## Keyboard and screen readers

Every button, switch, list and field of the dialogs and windows can be reached with
**Tab** and **Shift+Tab**, and pressed with **Space** or **Enter**; the control that has
the keyboard shows a bright ring. **Esc** closes the dialog (or backs out of its
comparison or confirmation first). Hover any control for a one-sentence tooltip that
says what it does, and the reason when it is greyed out. A round **?** button in the
header of a dialog opens the manual page for it.

- **Lists** (the tutorial browser, Retarget's options, the angle table, the manual's
  contents) are one Tab stop; **Up** and **Down** move through them. The command palette
  keeps the keyboard in its search box, and Up and Down choose a row.
- **Pictures you can turn.** In the Compare window and in the Rough Planner's 3D view,
  Tab to the picture, then the **arrow keys** turn it, **+** and **-** zoom and **Home**
  goes back to the starting view. The Compare window's split divider moves with
  **Left** and **Right**. In Angle Sweep, Tab to the chart and use **Left** and
  **Right** to choose an angle; the table follows.
- **Cutting mode** is driven by its own keys (Left and Right, Space or D, Esc) and keeps
  Tab out on purpose, so that nothing under the screen can be touched by mistake. Its
  buttons and index tick boxes are reachable by a screen reader's own actions.
- Icon buttons are at least 20 pixels square, and a screen reader hears every button's
  name, whether a switch is on, and which list row is selected.

## Where your choices are stored

All of these go into the same settings file as your render and remote settings
(Chapter 1 shows where it lives). The keys are listed in
[settings.md](../settings.md). Deleting the file puts every preference back to its
default, and the app then treats the next start as a brand-new installation.

## Limitations

- The interface scale needs a restart.
- High contrast leaves the colors that carry meaning and the rendered pictures as they
  are (listed above), and the standard controls keep their own thin borders.
- Simple mode only hides controls; it does not change what a design contains or how
  it is solved, rendered or saved.
