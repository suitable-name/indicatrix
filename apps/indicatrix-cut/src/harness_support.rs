//! Test-only seams for the desktop UI harness (`tests/ui_harness`), compiled only with the
//! `ui-harness` Cargo feature.
//!
//! The harness is an integration test, so it can only reach what this crate exports. This
//! module exports the few private things it needs, and nothing else:
//!
//! * a **file-picker answer queue**, the **clipboard** and **opener** captures: one-line
//!   forwards to the published `uiprobe-hooks` crate (`picker`, `clipboard`, `opener`),
//!   kept under their old names so no production call site and no scenario changes. With the
//!   feature on, `gui::pickers::pick` never opens an OS dialog: it takes the next queued answer
//!   ([`queue_picker_answer`]) instead. A picker call with an empty queue answers "cancelled"
//!   and records an "unexpected file dialog" failure that the harness reads back with
//!   [`take_picker_failures`] and reports (the harness crate reads the same hooks queue);
//! * the **resolved settings and database paths** ([`settings_path`], [`database_path`]), so
//!   the harness can prove they lie inside its throw-away profile before any window is built;
//! * a clean **window close** ([`close_all_windows`]) that stops the render thread, which
//!   dropping a `MainWindowHandle` alone does not.
//!
//! The hooks crate keeps its queues in process-global statics (not thread-locals) because the
//! picker is asked on the UI thread but the harness may inspect it from anywhere.

use crate::gui::MainWindowHandle;
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, platform::WindowEvent};
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

pub use uiprobe_hooks::picker::Answer as PickerAnswer;

/// File name of the design library database, relative to the process working directory.
///
/// Mirrors `gui::main_window::DB_PATH`, which is private to `gui`; if that constant ever
/// changes this one must follow (the harness would otherwise assert on the wrong path).
pub const DATABASE_FILE_NAME: &str = "facet_diagrams.sqlite";

/// Whether the live viewport renders nothing (see [`disable_viewport_rendering`]).
static VIEWPORT_RENDERING_DISABLED: AtomicBool = AtomicBool::new(false);

thread_local! {
    /// A separate token per launch: an old window's scan cannot complete a newer one's.
    static STARTUP_PREVIEW_SCAN: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
}

/// Marks the startup preview scan as pending until its UI result has been delivered.
/// This observes the real scan; it does not suppress it or dismiss its offer.
pub(crate) fn begin_startup_preview_scan() -> Arc<AtomicBool> {
    let pending = Arc::new(AtomicBool::new(true));
    STARTUP_PREVIEW_SCAN.with_borrow_mut(|slot| *slot = Some(Arc::clone(&pending)));
    pending
}

/// Whether the current launch can still deliver its initial preview-generation offer.
/// Read on the UI thread, after building the main window, while pumping its event loop.
#[must_use]
pub fn startup_preview_scan_pending() -> bool {
    STARTUP_PREVIEW_SCAN.with_borrow(|slot| {
        slot.as_ref()
            .is_some_and(|pending| pending.load(Ordering::Acquire))
    })
}

/// Makes every later launch skip the live viewport's ray tracing.
///
/// No GPU adapter is acquired and no frame is traced, as if the 3D view were an invisible tab
/// (the gemological metrics still update). A harness relaunches the app hundreds of times per
/// process, in up to 16 processes at once. With the GPU, creating and dropping that many Vulkan
/// devices crashed the AMD driver (`amdvlk64.dll`, exit 0xc0000409); with the CPU tracer
/// instead, 16 viewports tracing at once starved the planner and the library loads of CPU. The
/// harness never judges rendered pixels, so nothing it checks depends on the traced image.
pub fn disable_viewport_rendering() {
    VIEWPORT_RENDERING_DISABLED.store(true, Ordering::Relaxed);
}

/// See [`disable_viewport_rendering`].
pub(crate) fn viewport_rendering_disabled() -> bool {
    VIEWPORT_RENDERING_DISABLED.load(Ordering::Relaxed)
}

/// Queues the answer for the next file dialog the app opens. Answers are consumed in order.
pub fn queue_picker_answer(answer: PickerAnswer) {
    uiprobe_hooks::picker::queue(answer);
}

/// Queues "the user picked `path`" for the next file dialog.
pub fn queue_picker_path(path: impl Into<PathBuf>) {
    uiprobe_hooks::picker::queue_path(path);
}

/// Queues "the user cancelled" for the next file dialog.
pub fn queue_picker_cancel() {
    uiprobe_hooks::picker::queue_cancel();
}

/// Number of queued answers no dialog has consumed yet (a scenario can assert it is zero).
#[must_use]
pub fn pending_picker_answers() -> usize {
    uiprobe_hooks::picker::pending()
}

/// Drops every queued answer.
pub fn clear_picker_answers() {
    uiprobe_hooks::picker::clear();
}

/// Removes and returns the "unexpected file dialog: ..." failures recorded so far.
#[must_use]
pub fn take_picker_failures() -> Vec<String> {
    uiprobe_hooks::picker::take_failures()
}

/// Called by `gui::pickers::pick` (feature `ui-harness` only): the answer for the dialog
/// described by `detail` (title, kind, suggested name). `None` means cancelled. An empty queue
/// records a failure and answers cancelled.
pub(crate) fn next_picker_answer(detail: &str) -> Option<PathBuf> {
    uiprobe_hooks::picker::next(detail)
}

/// Called by `gui::library::clipboard::copy_to_clipboard` (feature `ui-harness` only) instead of
/// piping into the system clipboard tool: remembers the text. Always returns `true` (handled).
pub(crate) fn capture_clipboard_write(text: &str) -> bool {
    uiprobe_hooks::clipboard::capture(text)
}

/// Called by `gui::external_links::launch` (feature `ui-harness` only) instead of starting the
/// OS opener: remembers the (already validated) URL or path. Always returns `true` (handled).
pub(crate) fn capture_opened(target: &str) -> bool {
    uiprobe_hooks::opener::capture(target)
}

/// Removes and returns the texts the app "copied to the clipboard" since the last call.
#[must_use]
pub fn take_clipboard_writes() -> Vec<String> {
    uiprobe_hooks::clipboard::take()
}

/// Removes and returns the URLs, folders and files the app "opened" (browser, file manager)
/// since the last call.
#[must_use]
pub fn take_opened() -> Vec<String> {
    uiprobe_hooks::opener::take()
}

/// The settings file the app would read and write right now (follows the `APPDATA` /
/// `XDG_CONFIG_HOME` / `HOME` environment).
#[must_use]
pub fn settings_path() -> PathBuf {
    crate::settings::store::default_settings_path()
}

/// The design library database the app would open right now: [`DATABASE_FILE_NAME`] in the
/// current working directory.
#[must_use]
pub fn database_path() -> PathBuf {
    std::env::current_dir().map_or_else(
        |_| PathBuf::from(DATABASE_FILE_NAME),
        |dir| dir.join(DATABASE_FILE_NAME),
    )
}

/// Stores `.asc` files in the design library database ([`database_path`]).
///
/// It does what the app's own import does (`local::import_asc_bytes`, then the measured
/// proportions, then `save_design` under the local source). `files` are `(file name, raw
/// bytes)`. Returns how many were stored.
///
/// The database file is created when it does not exist yet. Call it while the app is shut down,
/// so no other connection is writing.
///
/// # Errors
///
/// The first failure (open, parse or save), naming the file.
pub fn seed_library_asc(files: &[(&str, &[u8])]) -> Result<usize, String> {
    let path = database_path();
    let text = path.to_string_lossy().into_owned();
    let db = indicatrix_vault::db::sqlite::Database::new(Some(&text))
        .map_err(|e| format!("cannot open {text}: {e:#}"))?;
    for (name, bytes) in files {
        let mut imported = indicatrix_vault::local::import_asc_bytes(name, bytes, None)
            .map_err(|e| format!("{name}: {e}"))?;
        crate::gui::harness_apply_measured_metadata(&mut imported.detail);
        db.save_design(
            &imported.entry,
            &imported.detail,
            indicatrix_vault::local::LOCAL_SOURCE_ID,
        )
        .map_err(|e| format!("{name}: cannot save: {e:#}"))?;
    }
    Ok(files.len())
}

/// File name suffix of an exported Rough Planner plan (`gui::rough_plan::saved::naming`).
pub const ROUGH_PLAN_EXPORT_SUFFIX: &str = crate::gui::ROUGH_PLAN_EXPORT_SUFFIX;
/// The suffix older versions exported Rough Planner plans with (same module).
pub const ROUGH_PLAN_LEGACY_EXPORT_SUFFIX: &str = crate::gui::ROUGH_PLAN_LEGACY_EXPORT_SUFFIX;

/// The secondary windows the harness can reach.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecondaryWindow {
    /// The pop-out compare window.
    Compare,
    /// The Rough Planner window.
    RoughPlanner,
    /// The "Locate inclusion from photos" window.
    Locate,
    /// The camera-rig editor window.
    Rig,
    /// The rough color wizard.
    RoughColor,
    /// The help window.
    Help,
}

/// `window` when it is currently shown. Windows the app only hides when "closed" (compare,
/// planner, wizard) count as closed then; the others are dropped on close.
fn shown<T: ComponentHandle>(window: Option<T>) -> Option<T> {
    window.filter(|w| w.window().is_visible())
}

/// A clone of the live compare window, or `None` when it was never created or is closed.
#[must_use]
pub fn compare_window() -> Option<crate::CompareWindow> {
    shown(crate::gui::harness_compare_window())
}

/// A clone of the live Rough Planner window, or `None` when never created or closed.
#[must_use]
pub fn rough_planner_window() -> Option<crate::RoughPlannerWindow> {
    shown(crate::gui::harness_planner_window())
}

/// A clone of the live locate window, or `None` when never created or closed.
#[must_use]
pub fn locate_window() -> Option<crate::LocateWindow> {
    shown(crate::gui::harness_locate_window())
}

/// A clone of the live rig window, or `None` when never created or closed.
#[must_use]
pub fn rig_window() -> Option<crate::RigWindow> {
    shown(crate::gui::harness_rig_window())
}

/// A clone of the live rough color wizard window, or `None` when never created or closed.
#[must_use]
pub fn rough_color_window() -> Option<crate::RoughColourWindow> {
    shown(crate::gui::rough_colour::wizard::host::harness_window())
}

/// A clone of the live help window, or `None` when never created or closed.
#[must_use]
pub fn help_window() -> Option<crate::HelpWindow> {
    shown(crate::gui::help::harness_window())
}

/// Whether the window exists and is visible.
#[must_use]
pub fn is_window_open(which: SecondaryWindow) -> bool {
    match which {
        SecondaryWindow::Compare => compare_window().is_some(),
        SecondaryWindow::RoughPlanner => rough_planner_window().is_some(),
        SecondaryWindow::Locate => locate_window().is_some(),
        SecondaryWindow::Rig => rig_window().is_some(),
        SecondaryWindow::RoughColor => rough_color_window().is_some(),
        SecondaryWindow::Help => help_window().is_some(),
    }
}

/// Lets go of the handles the application's thread-locals still hold after its main window
/// was dropped.
///
/// Those are the database, editor state, settings writer and kept secondary windows;
/// releasing them closes the window's database file so the profile can be wiped. Call it
/// after dropping the `MainWindowHandle`; building the next window does the same first.
pub fn release_window_state() {
    crate::gui::release_window_state();
}

/// The current camera pose (yaw, pitch in radians, distance) of the active viewport.
///
/// # Panics
///
/// Panics if the viewport camera handles have not been registered yet.
#[must_use]
pub fn camera_pose() -> (f32, f32, f32) {
    crate::gui::render::camera_handles::camera_pose().expect("camera not registered")
}

/// Returns what `set_view(5)` (Fit) would set for the current view and stone size.
///
/// # Panics
///
/// Panics if the viewport camera handles have not been registered yet.
#[must_use]
pub fn fit_distance_now() -> f32 {
    crate::gui::render::camera_handles::fit_distance_now().expect("camera not registered")
}

/// What the harness remembers of the main window built last: weak handles, so remembering them
/// keeps nothing alive.
struct Registered {
    database: Weak<Mutex<Database>>,
    window: slint::Weak<crate::MainWindow>,
}

thread_local! {
    /// The main window built last (UI thread only).
    static REGISTERED: RefCell<Option<Registered>> = const { RefCell::new(None) };
}

/// Shutdowns after which the window or the database handle still lived.
static SHUTDOWNS_WITH_SURVIVORS: AtomicUsize = AtomicUsize::new(0);

/// How many survivor lines [`report_shutdown_leftovers`] prints before it only counts.
const SURVIVOR_LINES_PRINTED: usize = 3;

/// Called by `gui::build_main_window` once the design library is open: remembers weak handles
/// to the window and the shared database, so the harness can tell after a shutdown whether
/// either is still alive.
pub(crate) fn register_window(ui: &crate::MainWindow, database: &Arc<Mutex<Database>>) {
    let registered = Registered {
        database: Arc::downgrade(database),
        window: ui.as_weak(),
    };
    REGISTERED.with(|cell| *cell.borrow_mut() = Some(registered));
}

/// `(the main window still exists, strong references to the shared database)` for the window
/// built last; `(false, 0)` when none was built. UI thread only.
fn survivors() -> (bool, usize) {
    REGISTERED.with(|cell| {
        cell.borrow().as_ref().map_or((false, 0), |registered| {
            (
                registered.window.upgrade().is_some(),
                registered.database.strong_count(),
            )
        })
    })
}

/// Whether every holder of the last window's shared database handle let go.
///
/// The database file is closed once the last holder, possibly a background thread winding
/// down, is done. `true` when no window was built. UI thread only.
#[must_use]
pub fn database_released() -> bool {
    survivors().1 == 0
}

/// Call at the end of a shutdown: reports whether the window or the database handle survived.
///
/// When one did, it prints `window alive: {bool}, database Arc strong count: {n}` (the first
/// few times, then only counts) and returns `true`. Returns `false` when everything was
/// released. UI thread only.
pub fn report_shutdown_leftovers() -> bool {
    let (window_alive, strong_count) = survivors();
    if !window_alive && strong_count == 0 {
        return false;
    }
    let seen = SHUTDOWNS_WITH_SURVIVORS.fetch_add(1, Ordering::Relaxed) + 1;
    if seen <= SURVIVOR_LINES_PRINTED {
        println!(
            "ui-harness: after shutdown - window alive: {window_alive}, \
             database Arc strong count: {strong_count}"
        );
        let holders = live_holders();
        if window_alive {
            // The window's own callbacks hold most of the database handles, so a live window
            // explains the count by itself.
            println!(
                "ui-harness:   the main window itself is alive: a strong `MainWindow` handle \
                 outlived the shutdown (a `clone_strong` kept by the caller across a relaunch, or \
                 a closure that captured the window instead of `as_weak`)"
            );
        }
        if holders.is_empty() {
            println!(
                "ui-harness:   no thread-local holder is set; the survivor is a background \
                 thread, a queued event-loop closure or a timer closure"
            );
        } else {
            println!("ui-harness:   holders still set: {}", holders.join("; "));
        }
    }
    true
}

/// The thread-local holders that are still set.
///
/// Holders of the design library handle, the editor state or a kept window (see
/// `gui::window_release::live_holders`). Empty after a complete [`release_window_state`].
/// UI thread only.
#[must_use]
pub fn live_holders() -> Vec<&'static str> {
    crate::gui::live_holders()
}

/// How many shutdowns so far left the window or the database handle alive.
#[must_use]
pub fn shutdowns_with_survivors() -> usize {
    SHUTDOWNS_WITH_SURVIVORS.load(Ordering::Relaxed)
}

/// Closes the main window the way the title-bar button does, which also stops the render
/// thread, flushes the settings and closes the help, compare and planner windows.
///
/// A dirty design or running planner/export work makes the window keep itself open and raise
/// the close guard; this then presses its "Discard" button, so the call always ends with the
/// window closed. Returns `true` when the main window is no longer visible afterwards (it also
/// returns `true` for a window that was never shown).
pub fn close_all_windows(handle: &MainWindowHandle) -> bool {
    let ui = &handle.ui;
    ui.window().dispatch_event(WindowEvent::CloseRequested);
    if ui.get_close_confirm_open() {
        ui.invoke_close_confirm_discard();
    }
    crate::gui::help::close_help_window();
    !ui.window().is_visible()
}
