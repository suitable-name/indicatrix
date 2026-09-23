# Appendix B: Keyboard Shortcuts

A set of global shortcuts work anywhere in the window (unless a text field
currently has focus and would otherwise consume the key, e.g. typing a
digit into a search box). The editor (Edit tab) ships in the standard
build; a few of these — Undo/Redo, F5 Solve — are no-ops without a design
loaded, not gated behind any Cargo feature.

The table below, and "The tier list" section's own table further down, are
generated from one Rust table (`gui::editor::shortcuts::SHORTCUTS`) that also
feeds the in-app overlay (Help → Keyboard Shortcuts, or press `?` with no
text field focused) — a test in that module
(`appendix_b_generated_blocks_match_the_shortcut_table`) compares this file's
generated blocks against that table's own output, so the manual and the
overlay cannot silently drift apart. Do not hand-edit the text between the
`<!-- SHORTCUTS_GLOBAL -->`/`<!-- SHORTCUTS_TIER_LIST -->` marker pairs —
edit `SHORTCUTS` instead and paste the regenerated output back in.

<!-- SHORTCUTS_GLOBAL:BEGIN -->
| Shortcut | Action |
| --- | --- |
| Ctrl+N | New Design... |
| Ctrl+O | Open Native... |
| Ctrl+S | Save Native |
| Ctrl+Shift+S | Save Native As... |
| Ctrl+Z | Undo (Edit tab) |
| Ctrl+Y (or Ctrl+Shift+Z) | Redo (Edit tab) |
| Ctrl+D | Duplicate the selected tier (tier list must have keyboard focus -- see "The tier list" below) |
| Ctrl+E | Toggle the 3D Gem tab's Live Render / Edit pill. **Not** Export -- "Export Edited .asc" has no keyboard shortcut of its own. |
| Ctrl+F | Focus the catalogue search box, or, while the Edit tab's tier list is showing, the tier list's own filter box instead |
| Ctrl+1 / Ctrl+2 / Ctrl+3 | Switch to the 3D Gem tab / Cutting Table tab / Attachments tab. There is no Ctrl+4 -- the window only has these three top-level tabs. |
| F5 | Solve (Edit tab) -- disabled while a solve is already running, so F5 cannot start a second one on top of it |
| Esc | Clear the current tier selection and dismiss whatever toast is showing |
| ? | Open this Keyboard Shortcuts overlay -- only while no text field has focus |
<!-- SHORTCUTS_GLOBAL:END -->

**Tab switching needs Ctrl** (or Cmd on macOS) held down — a bare `1`, `2`,
or `3` does nothing at the window level *except* the case described next.

## Plain 1 / 2 / 3 / 4 — Solid/Diagram viewport view modes

While the Edit sub-tab's own Solid viewport is on screen (3D Gem tab, Edit
sub-tab), the unmodified digit keys **1**, **2**, **3**, and **4** switch
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

A menu bar (File / Edit / Help) mirrors the File-related and Undo/Redo
actions above, plus Help → User Manual, which opens this manual's own
`README.md` in your system's default viewer. The Undo/Redo menu items (and
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
| Alt+Up / Alt+Down | Move the highlighted row up/down in cutting order |
| F2 | Open the highlighted row's **angle** cell for inline editing (name and other fields have no shortcut of their own) |
| Ctrl+click a row | Add/remove that row from the multi-select group (batched angle nudging, or batch delete -- Chapter 4) |
| Shift+click a row | Select every row between it and whichever row you selected last, replacing the current selection (Chapter 4) |
| Escape (list focused, nothing else open) | Clear the current tier selection |
<!-- SHORTCUTS_TIER_LIST:END -->

A single click on a row (or its angle cell) selects it; **double-click**
the angle cell (or press F2 on the selected row) to edit it in place.

While the tier list's inline angle cell is open for editing, or the
inspector's own Angle field has focus:

| Shortcut | Step |
| --- | --- |
| Up / Down | ±0.1° |
| Shift+Up / Shift+Down | ±1° |
| Ctrl+Up / Ctrl+Down | ±0.01° |
| Enter | Commit the value and close the cell (inline cell only) |
| Escape | Close the cell **without** committing (inline cell only) |

The scroll wheel does the same ±0.1° step over the tier list's inline angle
cell, but **only** while that cell is already open for editing, or while
you hold **Ctrl** — an ordinary scroll over it otherwise just scrolls the
list. All of the tier-list shortcuts above only do anything on a build
compiled with the `editor` feature, same as the global ones above.

## Elsewhere

The New Design dialog accepts **Enter** to click Create (once every field
validates) and **Escape** to Cancel.

Every other action described in this manual — Save/Add Tier, Import,
individual dialog buttons, and so on — is still a button or menu click
with no keyboard equivalent of its own.

The 3D viewport's camera and light controls (Chapter 2) are mouse
gestures — left-drag to orbit, right-drag or Shift+left-drag to move the
light, scroll to zoom — not keyboard shortcuts. The Edit tab's Solid
viewport (Chapter 13) shares that same orbit/zoom camera with the Live
Render tab in its Solid/Path-traced/Both modes; its Diagram mode instead
drags to pan and scrolls to zoom its own image, with a Reset View button
to snap both back.

## Next steps

Appendix C tables every built-in render material's optical properties.
