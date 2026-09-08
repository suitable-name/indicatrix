# UI structure

`app.slint` declares `MainWindow` and wires up the top-level layout (tabs,
menu bar, keyboard shortcuts, dialog overlays). It owns only window-level
state that has nowhere more specific to live: `active_tab`/`render_view_tab`,
`live_render_detached`, the toast, and `open_user_manual`.

Everything else -- every property and callback a dialog or panel needs -- is
declared on a per-feature `export global` in `ui/models/*.slint`:

| Global | File | Owns |
| --- | --- | --- |
| `BatchModel` | `models/batch.slint` | Preview-batch and tilt-batch dialogs |
| `EditorModel` | `models/editor.slint` | The Edit tab: tiers, solve, optimize, deep solve, new-design/gear-remap dialogs |
| `ExportModel` | `models/export.slint` | The render-export dialog |
| `LibraryModel` | `models/library.slint` | The catalogue/diagram list, filters, metadata editor |
| `RemoteWorkerModel` | `models/remote_worker.slint` | Remote worker configuration and library mirroring |
| `RetargetModel` | `models/retarget.slint` | The "Retarget for material" dialog |
| `SettingsModel` | `models/settings.slint` | The render-quality settings panel |
| `SolidPreviewModel` | `models/solid_preview.slint` | The Edit tab's solid-inspection viewport |
| `TiltModel` | `models/tilt.slint` | Tilt performance graphs |
| `ViewportModel` | `models/viewport.slint` | The 3D gem viewport's camera/lighting/material controls |

A `.slint` component reads/writes a global directly (`EditorModel.solve()`,
`LibraryModel.search_text`), the same way it would read `root.` on a
window-level property -- no forwarding through `MainWindow` is needed. On the
Rust side, reach a global through the `MainWindow` handle: `ui.global::<EditorModel>().on_solve(...)`,
`ui.global::<LibraryModel>().set_search_text(...)`. `global()` comes from
`slint::ComponentHandle`, so any file that calls it needs `use slint::ComponentHandle;`
in scope.

## `changed` persistence hooks

A few properties persist their value to `AppSettings` on every edit. That
wiring is two matched halves:

- The global itself declares a `changed` block that turns the property edit
  into a callback invocation, e.g. `models/editor.slint`'s
  `changed auto_solve_budget_ms => { auto_solve_budget_ms_changed(auto_solve_budget_ms); }`.
  This lives on the global that owns the property, not on `MainWindow`.
- Rust registers a handler for that callback (`gui::mod`/`gui::startup_settings`)
  that writes the new value into `AppSettings` through the debounced
  `SettingsPersister`.

`SolidPreviewModel.view_mode_changed`, `EditorModel.auto_solve_budget_ms_changed`,
and `LibraryModel.panel_collapsed_changed` all follow this shape. `ExportModel`'s
`changed is_open` block is the one exception: it drives two of `ExportModel`'s
own callbacks (`check_remote_availability`, `populate_fanout_presets`) entirely
in `.slint`, with no Rust-side `_changed` handler needed.

## Adding a dialog or panel

1. Pick the global that owns the feature (or add a new `export global` in
   `ui/models/<feature>.slint` if none fits), and declare its properties and
   callbacks there -- not on `MainWindow`.
2. Import the global into `app.slint` (or whichever `.slint` file hosts the
   dialog) with `import { YourModel } from "models/your_model.slint";`, and
   re-export it from `app.slint`'s `export { ... }` block so generated Rust
   code (`crate::YourModel`) can name it.
3. Reference the global's properties/callbacks directly from the component
   tree (`YourModel.some_property`), rather than forwarding them through a
   chain of `in-out property`/`callback` pairs on intermediate components.
4. On the Rust side, register callback handlers and push property updates
   through `ui.global::<YourModel>()`, and add `use slint::ComponentHandle;`
   to any file that is new to calling `.global()`.
5. If a property needs to persist across restarts, add the `changed` block on
   the global itself (see above) and register the matching handler in
   `gui::startup_settings`/`gui::mod`.
