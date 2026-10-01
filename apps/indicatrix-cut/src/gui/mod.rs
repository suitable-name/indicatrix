// Only `search` is `pub`: `refresh_diagram_list` is reused by a downstream binary's
// sync-complete handler (see `search::refresh_diagram_list`'s doc comment). The rest
// are wiring `build_main_window` uses internally.
mod batch;
// Converting a startup custom-materials row read into the render context's
// material/specific-gravity lists -- see the module's own doc comment.
mod custom_material_startup;
// The `indicatrix-cut-core`-backed Edit sub-tab.
mod editor;
// The Edit sub-tab's resizable-dock/collapsible-section layout persistence -- lives
// in its own module purely to keep this file from growing further; see this
// module's own doc comment.
mod editor_layout;
// Opening the bundled user manual and revealing the Edit tab's last-saved folder --
// see the module's own doc comment.
mod external_links;
pub(crate) mod latest_worker;
mod library;
// Building/wiring the `MainWindow` and its `MainWindowHandle` RAII guard -- see the
// module's own doc comment. Moved out of this file purely to keep it from growing
// further.
mod main_window;
mod optics;
// Resolving a native picker's starting directory from a text field's current value --
// see the module's own doc comment.
mod picker_field;
// The single shared 5x7 bitmap-font glyph lookup for `solid_preview::diagram2d`'s
// panel labels and `tilt::video_export::overlay`'s readout text -- see the
// module's own doc comment. Moved into `indicatrix-solid` (`solid_preview::
// diagram2d`'s own crate) and re-exported here at its old path so this app's own
// `tilt::video_export::overlay` keeps resolving `gui::pixel_font` unchanged.
pub(in crate::gui) use indicatrix_solid::pixel_font;
// The one place the `rfd` crate's file dialog is ever constructed in this app
// -- an off-UI-thread native picker/save-as/pick-folder worker plus the
// UI-thread rendezvous and test hook every other module's own picker needs
// use through this. See the module's own doc comment.
mod pickers;
mod remote;
pub(crate) mod render;
// The Library menu's "Plan Rough..." dialog: up to K stones of library designs out of
// one rough block -- see the module's own doc comment.
mod rough_plan;
pub mod solid_preview;
// The real `PreviewSink` that hops a finished solid-preview frame back onto the UI
// thread -- see the module's own doc comment. Moved out of this file purely to keep
// it from growing further.
mod solid_sink;
// Startup settings-application and the settings-value <-> UI-pill-index conversions
// that go with it -- moved out of this file purely to keep it from growing further.
// Several of its items are used well beyond startup (e.g. by callbacks that need the
// same index<->value mapping later), so this module re-exports them flatly below
// rather than requiring every caller to spell out `gui::startup_settings::`.
mod startup_settings;
mod tilt;
// Fits the main window to its monitor on first show -- see the module doc for why the
// .slint preferred size alone is not enough on Full HD displays.
mod window_sizing;
// The window-close unsaved-changes guard's Save/Discard callbacks -- see the
// module's own doc comment.
mod window_close;
// A rolling, regression-based time-remaining estimator shared by the still-image
// export queue (`render::render_export::queue`) and the tilt-video export
// (`tilt::video_export::run`) -- see the module's own doc comment for why a
// regression over a window rather than an instantaneous rate.
mod progress_eta;

// `sync_range_bounds_to_ui` is defined in `library::diagram_list` but re-exported here
// so its public path (`gui::sync_range_bounds_to_ui`) is unchanged for a downstream
// binary's sync-complete handler and this crate's own `gui::library`, both of which
// call it at that spelling.
pub use library::diagram_list::sync_range_bounds_to_ui;
// `search` is grouped into `gui::library` with the rest of the library UI (see
// that module's own doc comment), and re-exported here under this name so
// `gui::search` (the path a downstream binary's sync-complete handler is
// documented to use -- see `search::refresh_diagram_list`'s doc comment) keeps
// resolving.
pub use library::search;
// `MainWindowHandle`/`build_main_window` are re-exported here (rather than only at
// `gui::main_window::`) so their public path (`gui::MainWindowHandle`/
// `gui::build_main_window`) is unchanged for the downstream binary
// `lib.rs`'s own doc comment describes.
pub use main_window::{MainWindowHandle, build_main_window};
// `gui::library::search::fetch` opens a read-only connection to the same on-disk
// design library for background search -- see `DB_PATH`'s own doc comment.
pub(in crate::gui) use main_window::DB_PATH;
// Flat re-exports so every existing call site elsewhere in `gui` (which all spell
// these as `gui::X`, never `gui::startup_settings::X`) keeps resolving unchanged.
pub(in crate::gui) use startup_settings::{
    color_space_from_index, env_map_status_text, is_c_axis_override_available,
    local_compute_target_from_index, local_preview_scale_from_index,
    refresh_lighting_preset_options, refresh_material_options,
};
// Same reasoning as the `startup_settings` re-export above -- every existing call
// site spells this as `gui::starting_dir_from_picker_field`.
use picker_field::starting_dir_from_picker_field;

use crate::MainWindow;
use slint::ComponentHandle;

/// This crate's binary entry point (`apps/indicatrix-cut/src/main.rs` calls
/// straight through to this).
///
/// # Errors
///
/// See [`run_gui`].
pub fn main() -> anyhow::Result<()> {
    run_gui()
}

/// Runs this crate's window standalone: builds it (see [`build_main_window`])
/// and runs it to completion.
///
/// # Errors
///
/// Returns an error if [`build_main_window`] or the window's own event loop
/// (`MainWindow::run`) fails -- see [`build_main_window`]'s doc comment for what that
/// covers.
pub fn run_gui() -> anyhow::Result<()> {
    let handle = build_main_window()?;
    // Whether the user has ever touched the Edit sub-tab's layout -- read from the
    // settings store's own in-memory snapshot (already seeded from disk by
    // `editor_layout::apply_editor_layout_from_settings` above) rather than the UI's
    // `EditorModel.dock_width`/etc. directly, since `fit_initial_window_size` only
    // needs this one flag, not the whole layout. See `window_sizing`'s own doc
    // comment for what it does with it.
    //
    // Read BEFORE `show()` below, not after: `apply_editor_layout_from_settings`'s
    // own startup push only QUEUES its property writes, and `show()` is what flushes them -- which, on a
    // truly fresh settings file, raises `EditorModel.layout_changed` purely
    // because `editor_layout::FIRST_RUN_INSPECTOR_HEIGHT` differs from the
    // compiled default (340px vs. 260px), not because of anything the user did.
    // Reading the snapshot after `show()` used to see whatever that spurious
    // flush had already written back (`editor_layout::setup_editor_layout_
    // callbacks`'s own `on_layout_changed` sets `editor_layout_touched = true`
    // unconditionally), so the small-screen default below never actually saw a
    // fresh install as fresh. Reading it here instead captures the true on-disk
    // value, from before that flush can happen.
    let editor_layout_touched = handle
        .settings_store
        .snapshot()
        .settings
        .editor_layout_touched;
    // Spelled out instead of `MainWindow::run` so the monitor fit can be queued
    // between `show` and the event loop (the winit window only exists once the loop
    // turns -- see `window_sizing`).
    handle.ui.show()?;
    window_sizing::fit_initial_window_size(&handle.ui, editor_layout_touched);
    slint::run_event_loop()?;
    handle.ui.hide()?;
    Ok(())
}

/// Shows a toast message.
///
/// Informational/success toasts auto-dismiss after 3.5s; `"error"` and `"warning"`
/// stay on screen until the user dismisses them (via
/// `Toast.dismiss` -> `root.toast_visible = false` in `app.slint`), since a
/// critical outcome the user needed to act on could otherwise disappear before
/// they read it. `"warning"` is for a critical-but-not-failed outcome
/// (e.g. "your masts are placeholders", "the saved meet constraints were not
/// restored") that deserves the same persistence as an error without implying the
/// action itself failed; `ui/components/toast.slint`'s `Toast` component has
/// its own amber `"warning"` branch on background/border/icon colour, so a
/// `"warning"` toast renders distinctly from both `"info"` and `"error"` rather
/// than falling back to the info-style default.
///
/// A plain function (not a closure) since it captures nothing from `run_gui` --
/// every `ui.on_X` callback across this module's submodules that needs it just calls
/// it directly on its own `ui: &MainWindow`. `pub` because a downstream binary
/// reusing this window needs to surface its own results through the same toast.
pub fn show_toast(ui: &MainWindow, msg: &str, toast_type: &str) {
    ui.set_toast_message(msg.into());
    ui.set_toast_type(toast_type.into());
    ui.set_toast_visible(true);

    // Every toast bumps this counter; a scheduled dismiss below only acts if it's
    // still THIS call's generation by the time it fires -- otherwise a second toast
    // shown while the first one's timer is still running would get hidden early by
    // that stale timer (see `MainWindow.toast_generation`'s own doc comment).
    let generation = ui.get_toast_generation() + 1;
    ui.set_toast_generation(generation);

    // Errors and warnings stay until the user dismisses them -- see this
    // function's own doc comment.
    if toast_type == "error" || toast_type == "warning" {
        return;
    }

    let ui_weak = ui.as_weak();
    slint::Timer::single_shot(std::time::Duration::from_millis(3500), move || {
        if let Some(ui) = ui_weak.upgrade()
            && ui.get_toast_generation() == generation
        {
            ui.set_toast_visible(false);
        }
    });
}
