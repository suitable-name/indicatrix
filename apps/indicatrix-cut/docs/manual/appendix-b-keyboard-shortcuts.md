# Appendix B: Keyboard Shortcuts

A set of global shortcuts work anywhere in the window (unless a text field
currently has focus and would otherwise consume the key, e.g. typing a
digit into a search box). The editor (Edit tab) ships in the standard
build; a few of these — Undo/Redo, F5 Solve — are no-ops without a design
loaded, not gated behind any Cargo feature.

**Not every action has a key.** Press **Ctrl+K** (or Ctrl+Shift+P) to open the
command palette, type a few letters of what you want, and press Enter: every
action in the app is reachable that way, by name, without the mouse. Chapter 19
describes it.

The five tables in this appendix — the global shortcuts here, then "The tier
list", "The Solid viewport", "Angle fields" and "Cutting mode" further down — are generated
from one Rust table (`gui::editor::shortcuts::SHORTCUTS`) that also feeds the
in-app overlay (Help → Keyboard Shortcuts, or press `?` with no text field
focused) and the shortcuts the command palette shows beside its commands — a
test in that module (`appendix_b_generated_blocks_match_the_shortcut_table`)
compares this file's generated blocks against that table's own output, so the
manual and the overlay cannot silently drift apart. Do not hand-edit the text
between the `<!-- SHORTCUTS_GLOBAL -->`, `<!-- SHORTCUTS_TIER_LIST -->`,
`<!-- SHORTCUTS_VIEWPORT -->`, `<!-- SHORTCUTS_ANGLE_FIELD -->` and
`<!-- SHORTCUTS_CUTTING_MODE -->` marker pairs — edit `SHORTCUTS` instead and paste the regenerated output back in.

<!-- SHORTCUTS_GLOBAL:BEGIN -->
| Shortcut | Action |
| --- | --- |
| Ctrl+N | New Design... |
| Ctrl+O | Open... |
| Ctrl+S | Save |
| Ctrl+Shift+S | Save As... |
| Ctrl+Z | Undo (Edit tab) |
| Ctrl+Y (or Ctrl+Shift+Z) | Redo (Edit tab) |
| Ctrl+K / Ctrl+Shift+P | Open the command palette: type to search every action in the app, Enter runs the highlighted one. Works even while a text field has focus. |
| Ctrl+E | Toggle the 3D Spectral Preview tab's Live Render / Edit pill. **Not** Export -- "Export Edited .asc" has no keyboard shortcut of its own. |
| Ctrl+F | Focus the catalogue search box, or, while the Edit tab's tier list is showing, the tier list's own filter box instead |
| Ctrl+1 / Ctrl+2 / Ctrl+3 | Switch to the 3D Spectral Preview tab / Cutting Instructions tab / Files & Downloads tab. There is no Ctrl+4 -- the window only has these three top-level tabs. |
| 1 / 2 / 3 / 4 | Solid / Path-traced / Both / Diagram view of the Edit tab's viewport. Only while that viewport is on screen and no text field has focus. |
| Ctrl+, (Ctrl+Comma) | Open Preferences -- the Simple/Advanced interface, UI scale, high contrast, larger handles and tutorials. Not while a text field has focus. |
| F5 | Solve (Edit tab) -- disabled while a solve is already running, so F5 cannot start a second one on top of it |
| F1 | Help for this screen -- opens the manual at the page for what is on screen (the open inspector tab in the Edit tab, Live Render, the Cutting Instructions tab). Works while a text field has focus. |
| Esc | Clear the current tier selection and dismiss whatever toast is showing |
| ? | Open this Keyboard Shortcuts overlay -- only while no text field has focus |
<!-- SHORTCUTS_GLOBAL:END -->

**Tab switching needs Ctrl** (or Cmd on macOS) held down — a bare `1`, `2`,
or `3` does nothing at the window level *except* the case described next.

## Plain 1 / 2 / 3 / 4 — Solid/Diagram viewport view modes

While the Edit sub-tab's own Solid viewport is on screen (3D Spectral Preview
tab, Edit sub-tab), the unmodified digit keys **1**, **2**, **3**, and **4** switch
that viewport's own view mode — Solid, Path-traced, Both, and Diagram
respectively (Chapter 13). This is completely separate from Ctrl+1/2/3's
top-level tab switching above: same digits, different modifier, different
target, and only active while that particular viewport is the one on
screen. Like every other shortcut here, a focused text field swallows the
plain digit for itself first.

Escape's window-level behaviour above only fires when nothing more
specific already handled the key first — the tier list's own Escape, the
inline angle cell's Escape, and the New Design dialog's Escape (below) all
take priority over it while they have focus.

A menu bar (File / Edit / Library / Help) mirrors the File-related and
Undo/Redo actions above, plus Edit → Preferences, Edit → Command Palette...,
plus Help → User Manual, which opens this manual in the program's own help window
(Back, Forward and search inside it), and Help → Glossary. The Undo/Redo menu items (and
their hover hints on the command bar) say what they will actually do, e.g.
"Undo: Set P1 angle to -41.0 degrees," once there is something to undo or
redo.

The catalogue list itself is keyboard-navigable once it has focus: Up/Down
moves a highlight ring between cards, and Enter opens the highlighted
design — a mouse click still works exactly as before and is not required.

## The tier list

The Edit tab's tier list is keyboard-navigable once it has focus (click
into it first) — see Chapter 4, "Keyboard navigation in the tier list,"
for the full walkthrough:

<!-- SHORTCUTS_TIER_LIST:BEGIN -->
| Shortcut | Action |
| --- | --- |
| Up / Down | Select the previous/next row (a real selection, not just a cursor -- it re-seeds the inspector immediately, same as clicking) |
| Home / End | Select the first/last row |
| Page Up / Page Down | Select 10 rows back/forward |
| Enter | Select the highlighted row (redundant with Up/Down, kept for habit) |
| Delete | Remove the highlighted row (**not** Backspace -- that key is left alone here, since it is the universal "delete the previous character" key in every text field elsewhere in the app) |
| Ctrl+D | Duplicate the highlighted row |
| Alt+Up / Alt+Down | Move the highlighted row up/down in the table (and so in the cutting order of its side of the stone) |
| F2 | Open the highlighted row's **angle** cell for inline editing (name and other fields have no shortcut of their own) |
| Space | Add/remove the highlighted row from the multi-select group (the keyboard twin of Ctrl+click -- Chapter 4) |
| Ctrl+click a row | Add/remove that row from the multi-select group (batched angle nudging, or batch delete -- Chapter 4) |
| Shift+click a row | Select every row between it and whichever row you selected last, replacing the current selection (Chapter 4) |
| Escape (list focused, nothing else open) | Clear the current tier selection |
<!-- SHORTCUTS_TIER_LIST:END -->

A single click on a row (or its angle cell) selects it; **double-click**
the angle cell (or press F2 on the selected row) to edit it in place.

## The Solid viewport

The Edit tab's Solid viewport (Chapter 13) has its own keys, active once you
have clicked into the viewport. The first four belong to the Slice tool:

<!-- SHORTCUTS_VIEWPORT:BEGIN -->
| Shortcut | Action |
| --- | --- |
| S | Turn the Slice tool on or off (not in the Diagram view, and not while a handle or a slice line is being dragged) |
| F | While a slice tier is shown but not yet kept: flip which side of your line is cut away |
| Enter | While a slice tier is shown but not yet kept: keep it as one undo step |
| Esc | Cancel what is in progress, innermost first: a handle drag, a slice line being drawn, a slice tier not yet kept, Slice mode itself, an enlarged Diagram panel. With none of those, clear the tier selection. |
| Up / Down | Select the previous/next tier |
| Page Up / Page Down | Select the tier 10 places back/forward |
| Shift (while dragging a handle) | Switch the angle and depth handles to their fine snapping step |
<!-- SHORTCUTS_VIEWPORT:END -->

## Angle fields

While the tier list's inline angle cell is open for editing, or the
inspector's own Angle field has focus:

<!-- SHORTCUTS_ANGLE_FIELD:BEGIN -->
| Shortcut | Action |
| --- | --- |
| Up / Down | Change the angle by 0.1 degrees |
| Shift+Up / Shift+Down | Change the angle by 1 degree |
| Ctrl+Up / Ctrl+Down | Change the angle by 0.01 degrees |
| Mouse wheel | Change the angle by 0.1 degrees. Over the tier list's angle cell this only works while the cell is open for editing, or while you hold Ctrl; otherwise the wheel scrolls the list. |
| Enter | Commit the value and close the cell (the tier list's inline cell only) |
| Escape | Close the cell without committing (the tier list's inline cell only) |
<!-- SHORTCUTS_ANGLE_FIELD:END -->

All of the tier-list shortcuts above only do anything on a build
compiled with the `editor` feature, same as the global ones above.

## Cutting mode

While cutting mode (Chapter 20) covers the window, it takes the keyboard. A key
held together with Ctrl, Alt or the Windows key does nothing there:

<!-- SHORTCUTS_CUTTING_MODE:BEGIN -->
| Shortcut | Action |
| --- | --- |
| Left / Right | Go to the previous/next cutting step |
| Page Up / Page Down | Go to the previous/next cutting step |
| Space / D | Mark the step done and go on to the next one (a step that is already done is just passed) |
| Esc | Leave cutting mode (while the Reset progress question is showing, Esc cancels the question instead) |
<!-- SHORTCUTS_CUTTING_MODE:END -->

## Elsewhere

The New Design dialog accepts **Enter** to click Create (once every field
validates) and **Escape** to Cancel.

Most other actions described in this manual have no key of their own. They
are still reachable without the mouse: press **Ctrl+K**, type part of the
action's name, and press Enter (Chapter 19). Individual dialog buttons are
the exception; the palette lists actions, not the buttons inside a dialog.

The 3D viewport's camera and light controls (Chapter 2) are mouse
gestures — left-drag to orbit, right-drag or Shift+left-drag to move the
light, scroll to zoom — not keyboard shortcuts. The Edit tab's Solid
viewport (Chapter 13) shares that same orbit/zoom camera with the Live
Render tab in its Solid/Path-traced/Both modes; its Diagram mode instead
drags to pan and scrolls to zoom its own image, with a Reset View button
to snap both back.

On a selected facet in the Solid viewport's Solid, Path-traced and Both
modes, the angle, depth and index handles are dragged with the left mouse
button (Chapter 13). While a handle is being dragged, **Shift** switches the
angle and depth snapping to its fine step, and **Escape** cancels the drag
and restores the design; with nothing being dragged, Escape clears the
selection as usual.

The Solid viewport's own keys, including the Slice tool's S, F, Enter and Esc,
are in "The Solid viewport" table above.

## Next steps

Appendix C tables every built-in render material's optical properties.
