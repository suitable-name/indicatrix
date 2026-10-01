# UI structure

`app.slint` declares `MainWindow` and wires up the top-level layout (tabs,
menu bar, keyboard shortcuts, dialog overlays). It owns only window-level
state that has nowhere more specific to live:

- the window title's design name (`loaded_design_name`);
- the tab selection (`active_tab`/`render_view_tab` and their `*_changed` callbacks);
- File > Open Recent (`recent_native_files`, `open_recent_native_file`);
- the window-close unsaved-changes guard (`close_confirm_open`,
  `close_confirm_save`, `close_confirm_discard`);
- the toast (`toast_message`, `toast_type`, `toast_visible`, `toast_generation`);
- `open_user_manual`.

Everything else -- every property and callback a dialog or panel needs -- is
declared on a per-feature `export global`. Almost all of them live in
`ui/models/*.slint`; two live elsewhere, noted in the table. A global is
instantiated once per top-level window (`MainWindow`, the compare window, the
rough-planner window), so Rust reaches each window's instance through that
window's own handle.

| Global | File | Owns |
| --- | --- | --- |
| `ActivityModel` | `models/activity.slint` | The one list of running long actions (auto-solve, Deep Solve, Optimize, exports, ...) |
| `BatchModel` | `models/batch.slint` | Preview-batch and tilt-batch dialogs |
| `CompareModel` | `models/compare.slint` | The visual before/after compare window, and the comparison pane embedded in the Retarget dialog (separate instances per window) |
| `EditorModel` | `models/editor.slint` | The Edit tab: tiers, solve, optimize, deep solve, new-design/gear-remap dialogs |
| `ExportModel` | `models/export.slint` | The render-export dialog, including its "Transfer" choice |
| `GuideModel` | `models/guide.slint` | The worked-example guide panel: steps, navigation, control locks |
| `LibraryModel` | `models/library.slint` | The catalogue/diagram list, filters, metadata editor |
| `ManipulateModel` | `models/manipulate.slint` | The Solid viewport's angle/depth/index drag handles, their hint line, the Snap pill and the Slice tool (mode, rubber-band line, provisional tier buttons) |
| `RemoteWorkerModel` | `models/remote_worker.slint` | The one remote endpoint ("Remote Coordinator" form), "served by", library switch and mirroring |
| `RetargetModel` | `models/retarget.slint` | The "Retarget for material" dialog |
| `RoughPlanModel` | `models/rough_plan.slint` | The Rough Planner window (Library > Plan Rough...): inputs, results, saved plans |
| `SettingsModel` | `models/settings.slint` | The render-quality settings panel, including Live Compute / Live Transfer |
| `ShortcutsModel` | `models/shortcuts.slint` | The keyboard-shortcuts overlay |
| `SolidPreviewModel` | `models/solid_preview.slint` | The Edit tab's solid-inspection viewport |
| `TemplateGalleryModel` | `models/templates.slint` | The New Design dialog's template gallery |
| `Theme` | `theme.slint` | The colour palette and other design tokens; read from `.slint` only, not exported to Rust |
| `TiltModel`, `TiltVideoExportModel` | `models/tilt.slint` | Tilt performance graphs; the tilt-video export section |
| `UndoRedoLabels` | `components/editor_command_bar.slint` | The Undo/Redo wording shown by the command bar and the Edit menu (declared beside the command bar because both import it from there) |
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
