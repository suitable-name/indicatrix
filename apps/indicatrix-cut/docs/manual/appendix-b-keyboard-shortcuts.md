# Appendix B: Keyboard Shortcuts

A small set of global shortcuts work anywhere in the window (unless a text
field currently has focus and would otherwise consume the key, e.g. typing
a digit into a search box):

| Shortcut | Action |
| --- | --- |
| Ctrl+S | Save Native (the Edit tab's `.indicatrix.toml` sidecar + `.asc`) |
| Ctrl+Z | Undo (Edit tab) |
| Ctrl+Y (or Ctrl+Shift+Z) | Redo (Edit tab) |
| Ctrl+F | Focus the catalogue search box |
| Ctrl+1 / Ctrl+2 / Ctrl+3 | Switch to 3D Spectral Preview / Cutting Schedule / Files & Downloads |
| F5 | Solve (Edit tab) |
| Esc | Clear the current tier selection and dismiss whatever toast is showing |

**Tab switching needs Ctrl** (or Cmd on macOS) held down — a bare `1`, `2`,
or `3` does nothing at the window level (and would type into a text field
that has focus instead). Escape's window-level behaviour above only fires
when nothing more specific already handled the key first — the inline
angle cell (below) and the New Design dialog both have their own, different
Escape behaviour.

The Ctrl+S/Ctrl+Z/Ctrl+Y/F5 shortcuts only do anything on a build compiled
with the `editor` feature (the Edit tab) — on a build without it they're
simply no-ops, since there is no undo/redo/solve state to act on.

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

| Shortcut | Action |
| --- | --- |
| Up / Down | Select the previous/next row (a real selection, not just a cursor — it re-seeds the inspector immediately, same as clicking) |
| Home / End | Select the first/last row |
| Page Up / Page Down | Select 10 rows back/forward |
| Enter | Select the highlighted row (redundant with Up/Down, kept for habit) |
| Delete | Remove the highlighted row (**not** Backspace — that key is left alone here) |
| Ctrl+D | Duplicate the highlighted row |
| Alt+Up / Alt+Down | Move the highlighted row up/down in cutting order |
| F2 | Open the highlighted row's angle cell for inline editing |
| Ctrl+click a row | Add/remove that row from the multi-select group (batched angle nudging, or batch delete — Chapter 4) |
| Shift+click a row | Select every row between it and whichever row you selected last, replacing the current selection (Chapter 4) |

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
