# Appendix B: Keyboard Shortcuts

A small set of global shortcuts now work anywhere in the window (unless a
text field currently has focus and would otherwise consume the key, e.g.
typing a digit into a search box):

| Shortcut | Action |
| --- | --- |
| Ctrl+S | Save Native (the Edit tab's `.indicatrix.toml` sidecar + `.asc`) |
| Ctrl+Z | Undo (Edit tab) |
| Ctrl+Y (or Ctrl+Shift+Z) | Redo (Edit tab) |
| Ctrl+F | Focus the catalogue search box |
| F5 | Solve (Edit tab) |
| Esc | Cancel an in-progress export, or close the export dialog |
| 1 / 2 / 3 | Switch to 3D Spectral Preview / Cutting Schedule / Files & Downloads |

The Ctrl+S/Ctrl+Z/Ctrl+Y/F5 shortcuts only do anything on a build compiled
with the `editor` feature (the Edit tab) — on a build without it they're
simply no-ops, since there is no undo/redo/solve state to act on.

A menu bar (File / Edit / Help) mirrors the File-related and Undo/Redo
actions above, plus Help → User Manual, which opens this manual's own
`README.md` in your system's default viewer.

The catalogue list itself is keyboard-navigable once it has focus: Up/Down
moves a highlight ring between cards, and Enter opens the highlighted
design — a mouse click still works exactly as before and is not required.

The Edit tab's tier list is likewise keyboard-navigable once it has focus
(click into it first) — see Chapter 4, "Keyboard navigation in the tier
list," for the full walkthrough:

| Shortcut | Action |
| --- | --- |
| Up / Down | Move the tier list's keyboard cursor (scrolls the row into view) |
| Enter | Load the highlighted tier into the form |
| Delete / Backspace | Remove the highlighted tier |
| Ctrl+D | Duplicate the highlighted tier |
| F2 | Open the highlighted tier's angle cell for inline editing |
| Ctrl+click a row | Add/remove that tier from the multi-select group (batched angle nudging) |

While the tier list's inline angle cell is open for editing, or the tier
form's own Angle field has focus:

| Shortcut | Step |
| --- | --- |
| Up / Down | ±0.1° |
| Shift+Up / Shift+Down | ±1° |
| Ctrl+Up / Ctrl+Down | ±0.01° |

The scroll wheel does the same ±0.1° step whenever the cursor is over the
tier list's inline angle cell. All of the tier-list shortcuts above only do
anything on a build compiled with the `editor` feature, same as the
Ctrl+S/Ctrl+Z/Ctrl+Y/F5 shortcuts below.

Every other action described in this manual — the tier form's Save Tier,
Import, individual dialog buttons, and so on — is still a button or menu
click with no keyboard equivalent of its own.

The 3D viewport's camera and light controls (Chapter 2) are mouse
gestures — left-drag to orbit, right-drag or Shift+left-drag to move the
light, scroll to zoom — not keyboard shortcuts.

## Next steps

Appendix C tables every built-in render material's optical properties.
