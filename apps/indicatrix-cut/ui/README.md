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
| `BuildDesignModel` | `models/build_design.slint` | "Build this design": starts the generated rebuild lesson for a library design (the library list's menu, the detail header's steps button, the palette) and says whether one is being prepared. Rust: `gui::editor::guide::build_design` |
| `CompareModel` | `models/compare.slint` | The visual before/after compare window, and the comparison pane embedded in the Retarget dialog (separate instances per window) |
| `CuttingTableModel` | `models/cutting_table.slint` | One pure callback for the library cutting table's keyboard cursor (`step_row`): Slint has no loop to find the next row a Pavilion or Crown filter still shows. Rust: `gui::library::detail::row_cursor` |
| `DesignSettingsModel` | `models/design_settings.slint` | One pure callback for the Design settings panel (`advanced_in_use`): whether a control the Simple interface hides holds a non-default value, which shows the "Some advanced settings are in use" line. Rust: `gui::editor::design_settings_model` |
| `EditorModel` | `models/editor.slint` | The Edit tab: tiers, solve, optimize, deep solve, new-design/gear-remap dialogs |
| `ExportModel` | `models/export.slint` | The render-export dialog, including its "Transfer" choice |
| `ExportRunModel` | `models/export_run.slint` | "Is an export running?" (the still export, the tilt video or a queue job; every Start control is disabled while it is true) and the text of the header's export indicator (`components/export_chip.slint`), which shows a dialog export that runs in the background and reopens its popup. Rust: `gui::export_run` |
| `GuideModel` | `models/guide.slint` | The worked-example guide panel: steps, navigation, control locks |
| `HelpModel`, `HelpTopics` | `models/help.slint` | The in-app help: the manual's blocks and chapter list, search, Back/Forward, the glossary dialog. `HelpTopics` holds the topic ids a `HelpButton { topic: ...; }` opens (a Rust test keeps them equal to `gui::help::topics`). A global is per window, so the help window (`components/help_window.slint`) has its own instance, filled by `gui::help` |
| `HistoryModel` | `models/history.slint` | The Inspector's History tab: the undo history as rows (newest first, ending with "Start"), the row pictures (a pure `thumbnail` callback answered from a cache and drawn on a worker thread), and the `jump` to a step. Filled by `gui::editor::history_panel` |
| `VariantsModel` | `models/variants.slint` | The History tab's Variants view: the open design's saved variants (rows with pictures, newest first), the compare boxes, the save/rename/note/delete form, and the text comparison page. Variants live in the library database by design id. Filled by `gui::editor::variants` |
| `LibraryModel` | `models/library.slint` | The catalogue/diagram list, filters, metadata editor |
| `ManipulateModel` | `models/manipulate.slint` | The Solid viewport's angle/depth/index drag handles, their hint line, the Snap pill and the Slice tool (mode, rubber-band line, provisional tier buttons) |
| `PreferencesModel` | `models/preferences.slint` | App-wide preferences: the Simple/Advanced switch (`simple_mode`, read by other components to hide advanced controls), UI scale, high contrast, larger handles, the welcome-tour flag and the Preferences dialog's open state |
| `RelationModel` | `models/relation.slint` | Tier relations (a tier's angle follows other tiers): "Remove relation", the hint for an angle cell that cannot be edited, and the Steps panel's "Keep linked" Generate. The relation itself is typed into the Tier form's Angle field with a leading `=` |
| `RemoteWorkerModel` | `models/remote_worker.slint` | The one remote endpoint ("Remote Coordinator" form), "served by", library switch and mirroring |
| `RetargetModel` | `models/retarget.slint` | The "Retarget for material" dialog |
| `RoughPlanModel` | `models/rough_plan.slint` | The Rough Planner window (Library > Plan Rough...): inputs, results, saved plans |
| `SettingsModel` | `models/settings.slint` | The render-quality settings panel, including Live Compute / Live Transfer |
| `ShortcutsModel` | `models/shortcuts.slint` | The keyboard-shortcuts overlay |
| `SolidPreviewModel` | `models/solid_preview.slint` | The Edit tab's solid-inspection viewport |
| `SweepModel` | `models/sweep.slint` | The Angle Sweep dialog (Edit > Angle Sweep...): the form, the run's progress, and the table, chart and readout of the finished sweep; the dialog is `components/sweep_dialog.slint` |
| `TemplateGalleryModel` | `models/templates.slint` | The New Design dialog's template gallery |
| `Theme` | `theme.slint` | The color palette and other design tokens. Read from `.slint`; exported to Rust only for its `high-contrast` flag, which every color token switches on (Rust sets it per window: `gui::preferences`). Use `Theme` tokens, never hex literals, so the high-contrast palette reaches new code (see "Colours and high contrast" below) |
| `TierTableModel` | `models/tier_table.slint` | The few callbacks the tier table and the Tier form need from Rust beyond `EditorModel`'s edits: the multi-select Offset box, the "+ Add" facet box and the Rotate buttons read what was typed as a number or a small calculation (`eval_number`) and say in a toast when they cannot; `any_detached` lets the Simple interface note a detached tier. Rust: `gui::editor::tier_table_model` |
| `TiltModel`, `TiltVideoExportModel` | `models/tilt.slint` | Tilt performance graphs; the tilt-video export section |
| `UndoRedoLabels` | `components/editor_command_bar.slint` | The Undo/Redo wording shown by the command bar and the Edit menu (declared beside the command bar because both import it from there) |
| `VerdictModel` | `models/verdict.slint` | The overall Good / Check / Problem verdict of the open design: the word, the headline sentence, the reasons (each with a Show tier and an optional Fix), the stale and busy flags, and the `fix_reason` callback. Shown by `components/verdict_badge.slint` and `components/verdict_popover.slint` in the status strip. Rust: `gui::editor::verdict` over `indicatrix_editor::verdict` |
| `ViewportModel` | `models/viewport.slint` | The 3D gem viewport's camera/lighting/material controls |

A `.slint` component reads/writes a global directly (`EditorModel.solve()`,
`LibraryModel.search_text`), the same way it would read `root.` on a
window-level property -- no forwarding through `MainWindow` is needed. On the
Rust side, reach a global through the `MainWindow` handle: `ui.global::<EditorModel>().on_solve(...)`,
`ui.global::<LibraryModel>().set_search_text(...)`. `global()` comes from
`slint::ComponentHandle`, so any file that calls it needs `use slint::ComponentHandle;`
in scope.

## Colours and high contrast

Every colour in a `.slint` file comes from a `Theme` token (`theme.slint`); no hex
literal, no `white`/`black`. Each token is `Theme.high-contrast ? <hc> : <normal>`, so
the one Preferences switch recolours the whole UI. When a role has no token, add one
whose normal value is the literal you would have written (the normal look must not
move) and whose high-contrast value meets the ratios in `theme.slint`'s header.

Roles that already have a token, besides the general surface, border and text ones:

| Role | Tokens |
| --- | --- |
| Chip or field rows, unselected pill | `surface-chip`, `surface-chip-off` |
| Selected series pill (20 % tint) | `tint-cyan`, `tint-amber`, `tint-red`, `tint-purple`, `tint-neutral` |
| Tilt graph and its dialog | `surface-graph`, `surface-graph-popup`, `chart-bg`, `chart-border`, `chart-grid`, `chart-grid-mid`, `chart-guide`, `chart-crosshair`, `chart-extinction` |
| Orange attention banner | `notice-surface`, `notice-accent`, `notice-accent-hover`, `notice-text` (ink on the accent: `text-on-accent`) |
| Catalogue badges | `badge-amber-bg`, `badge-emerald-bg`, `badge-purple-bg`, `badge-orange-bg`, `badge-orange-text` |
| Overlays on the Live Render picture | `viewport-bg`, `hud-surface`, `hint-surface`, `hint-text`, `scrim-light` |
| Shadows | `modal-shadow`, `toast-shadow`, `shadow-heavy`, `shadow-medium` |
| Switch knob | `toggle-knob` (black in high contrast, so it shows on the light off-track and on every accent track) |
| Logo mark, Compare legend | `brand-mark-bg`, `compare-removed`, `compare-added` |

Data colours (a chart series, a swatch that shows a real gem or material colour, the
Compare overlay's red and green) are the same in both palettes: the token exists to name
the role, and the surfaces and labels around it are what change.

Colour maths on a token is fine (`Theme.accent-amber.with-alpha(0.14)`); on a literal it
is not. Rust-side rasters that bake a theme colour into pixels (the Compare and Rough
Planner pictures) keep the normal value on purpose; their comments say why.

The standard widgets (`Button`, `LineEdit`, `ComboBox`, `Slider`, ...) draw from Slint's
style palette, not from `Theme`. The app is dark only, so they are dark in both palettes.
Two things pin that: `build.rs` compiles the UI with the `fluent-dark` style (the compiler
then fixes the colour scheme to dark for every window from the first frame), and the
`changed high-contrast` block in `theme.slint` sets `Palette.color-scheme` to dark again
whenever the switch flips (a `changed` block does not run at start-up, which is why the
style is the main pin). That block lives in the global, so every window that has a `Theme`
is covered. The style's own control borders stay subtle in high contrast; a custom control
draws its own border from the tokens.

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

## Tooltips, keyboard reach and screen readers

A control is built from a shared piece that already has a tooltip, a focus ring, Tab, Space
and Enter, and a screen-reader label. Pass it a `hint`: one plain sentence saying what it does
(or, when it is greyed out, why).

- Hand-drawn controls: `PillButton`, `ModePill` and `SegmentPill` (`pill_button.slint`),
  `IconButton`, `ChipToggle` and `ActionButton` (`chip_toggle.slint`), `CompactButton`,
  `CompactIcon` and `TipButton` (`compact_controls.slint`), `ToggleRow`.
- The standard widgets (`LineEdit`, `ComboBox`, `SpinBox`, `Slider`) have no tooltip of their
  own and a screen reader does not take the caption beside them as their name. Wrap one in
  `TipBox { hint: "..."; }` (`tip_box.slint`) and give it an `accessible-label`. `FieldCaption`
  in the same file is a caption with a tooltip.
- A custom `TouchArea` control needs a `FocusScope` (Space and Enter), a ring in
  `Theme.accent-cyan-hover`, and `accessible-role`, `accessible-label` and the matching
  `-checked`, `-selected` or `-enabled`. Keys reach only an ancestor `FocusScope`, so a dialog
  card is a child of the scope that handles its keys, and that scope returns `reject` for
  `Key.Tab` and `Key.Backtab` or it swallows focus traversal.
- A layout cannot carry `accessible-*` properties or give a child an `x` and `y`: wrap in a
  `Rectangle`.
- Context help: `HelpButton { topic: HelpTopics.some-topic; }` in the dialog's header. The topic
  is listed in `models/help.slint` and in `gui::help::topics`; a Rust test keeps them equal. A
  separate window has its own `HelpModel`, so Rust forwards its `open_topic` to the main window.
- Simple mode: hide with `if GuideModel.shows_advanced()`, and show `AdvancedNote` when a
  hidden value is off its default. The "in use" decision is a pure Rust function with tests,
  reached through a pure callback on the dialog's global (`ExportModel.advanced_in_use`,
  `RemoteWorkerModel.advanced_in_use`, `RetargetModel.advanced_in_use`).

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
6. Give every control a tooltip and an accessible label (see "Tooltips, keyboard reach and
   screen readers"), put a `HelpButton` in the header, and decide what Simple mode hides.
   Add the new file to `CONVERTED_FILES` in `tests/theme_tokens.rs` once it holds no colour
   literal.
