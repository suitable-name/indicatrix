//! App-wide preferences: the Simple/Advanced switch, the UI scale, high contrast, larger
//! drag handles and the tutorial resets, plus the Snap/Slice pills the Solid viewport
//! remembers.
//!
//! The Slint side is `ui/models/preferences.slint` (`PreferencesModel`), the Preferences
//! dialog (`ui/components/preferences_dialog.slint`, Edit > Preferences..., Ctrl+Comma) and
//! the header's "Simple | Advanced" pill. This module is the Rust half:
//!
//! - [`apply_loaded`] seeds the models from the loaded `AppSettings` at start-up;
//! - [`setup_preferences_callbacks`] saves each change through the `SettingsPersister` and
//!   applies it;
//! - high contrast reaches EVERY window: each window's own `Theme` instance (a Slint
//!   global exists once per window) is registered as a sink, so the main window, the
//!   compare window and the rough planner all switch together, however late they open;
//! - [`saved_ui_scale_factor`] turns the saved UI scale into `SLINT_SCALE_FACTOR`; `main`
//!   sets it before any window exists, so a change needs a restart.

use crate::{
    CompareWindow, LocateWindow, MainWindow, ManipulateModel, PreferencesModel, RigWindow,
    RoughPlannerWindow, Theme,
    gui::tutorial_events::{raise, raise_weak},
    settings::{
        SettingsPersister,
        model::{AppSettings, UI_SCALE_CHOICES, UiMode, ui_scale_factor_text},
        store::peek_ui_scale_percent,
    },
};
use indicatrix_editor::guide::viewing_events as events;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
};

/// How much larger the Solid viewport's handles are drawn (and grabbed) with the "Larger
/// handles" preference: markers, strokes and the hit radius all scale by this.
const LARGE_HANDLE_SCALE: f32 = 1.6;

/// `ManipulateModel.handle_scale` for the "Larger handles" preference.
const fn handle_scale(large_handles: bool) -> f32 {
    if large_handles {
        LARGE_HANDLE_SCALE
    } else {
        1.0
    }
}

// --- UI scale -----------------------------------------------------------------------

/// The scale combo's first entry: follow the operating system.
const AUTOMATIC_LABEL: &str = "Automatic";

/// The labels of the scale combo: "Automatic", then one per [`UI_SCALE_CHOICES`] entry
/// ("75 %", ...), in that order.
fn ui_scale_labels() -> Vec<String> {
    std::iter::once(AUTOMATIC_LABEL.to_owned())
        .chain(
            UI_SCALE_CHOICES
                .iter()
                .map(|percent| format!("{percent} %")),
        )
        .collect()
}

/// The percentage combo entry `index` stands for: `0` (Automatic) for the first entry and
/// for anything out of range.
fn ui_scale_percent_for_index(index: i32) -> u16 {
    usize::try_from(index)
        .ok()
        .and_then(|entry| entry.checked_sub(1))
        .and_then(|choice| UI_SCALE_CHOICES.get(choice))
        .copied()
        .unwrap_or(0)
}

/// The combo entry for a stored `percent`: the entry of that percentage, else Automatic.
fn ui_scale_index_for_percent(percent: u16) -> i32 {
    UI_SCALE_CHOICES
        .iter()
        .position(|&choice| choice == percent)
        .and_then(|choice| i32::try_from(choice + 1).ok())
        .unwrap_or(0)
}

/// What `SLINT_SCALE_FACTOR` should be set to for the saved `percent`. `None` leaves it
/// alone: Automatic, a percentage that is not offered, or a variable the user has set
/// themselves (their value wins).
const fn scale_factor_override(percent: u16, user_set_variable: bool) -> Option<&'static str> {
    if user_set_variable {
        None
    } else {
        ui_scale_factor_text(percent)
    }
}

/// The value to put in `SLINT_SCALE_FACTOR` for the saved UI scale, or `None` to leave the
/// operating system's scale in charge.
///
/// Reads the settings file directly (see `settings::store::peek_ui_scale_percent`) because
/// the scale has to be in place before the first window exists, which is earlier than the
/// normal settings load. `main` calls this first thing, before any thread starts.
#[must_use]
pub fn saved_ui_scale_factor() -> Option<&'static str> {
    let user_set = std::env::var_os("SLINT_SCALE_FACTOR").is_some_and(|value| !value.is_empty());
    scale_factor_override(peek_ui_scale_percent(), user_set)
}

// --- High contrast across windows ----------------------------------------------------

/// Applies the high-contrast flag to one window's `Theme`; returns `false` once that
/// window is gone, so the registry can forget it.
type ThemeSink = Box<dyn Fn(bool) -> bool>;

thread_local! {
    /// The current high-contrast state, so a window created later starts in it. UI-thread
    /// only, like every Slint handle.
    static HIGH_CONTRAST: Cell<bool> = const { Cell::new(false) };
    /// One sink per window that shows the app's colours.
    static THEME_SINKS: RefCell<Vec<ThemeSink>> = const { RefCell::new(Vec::new()) };
}

/// Whether the high-contrast palette is on right now.
fn high_contrast_enabled() -> bool {
    HIGH_CONTRAST.get()
}

/// Registers `sink` and brings it up to the current state at once. A sink that reports its
/// window gone (`false`) is not kept.
fn register_theme_sink(sink: impl Fn(bool) -> bool + 'static) {
    if sink(high_contrast_enabled()) {
        THEME_SINKS.with_borrow_mut(|sinks| sinks.push(Box::new(sink)));
    }
}

/// Switches the palette in every registered window, dropping the sinks of closed ones.
fn apply_high_contrast(on: bool) {
    HIGH_CONTRAST.set(on);
    THEME_SINKS.with_borrow_mut(|sinks| sinks.retain(|sink| sink(on)));
}

/// Makes the main window follow the high-contrast preference.
fn bind_main_window_theme(ui: &MainWindow) {
    let weak = ui.as_weak();
    register_theme_sink(move |on| {
        let Some(ui) = weak.upgrade() else {
            return false;
        };
        ui.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the compare window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_compare_window_theme(window: &CompareWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the rough planner window follow the high-contrast preference. Called once, when
/// the window is created.
pub(super) fn bind_planner_window_theme(window: &RoughPlannerWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the locate-inclusion window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_locate_window_theme(window: &LocateWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the Rough colour wizard window follow the high-contrast preference. Called once, when
/// the window is created (`zoning` feature only).
#[cfg(feature = "zoning")]
pub(super) fn bind_rough_colour_window_theme(window: &crate::RoughColourWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

/// Makes the camera-rig window follow the high-contrast preference. Called once, when the
/// window is created.
pub(super) fn bind_rig_window_theme(window: &RigWindow) {
    let weak = window.as_weak();
    register_theme_sink(move |on| {
        let Some(window) = weak.upgrade() else {
            return false;
        };
        window.global::<Theme>().set_high_contrast(on);
        true
    });
}

// --- Start-up and callbacks ----------------------------------------------------------

/// Seeds the preference models, the Solid viewport's remembered Snap/Slice pills and the
/// main window's palette from the loaded `settings`. Called once from
/// `startup_settings::apply_loaded_ui_mirrors`, before any callback is wired.
pub(super) fn apply_loaded(ui: &MainWindow, settings: &AppSettings) {
    let prefs = ui.global::<PreferencesModel>();
    prefs.set_simple_mode(settings.ui_mode.is_simple());
    prefs.set_ui_scale_percent(i32::from(settings.ui_scale_percent));
    prefs.set_ui_scale_options(ModelRc::new(VecModel::from(
        ui_scale_labels()
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
    prefs.set_ui_scale_index(ui_scale_index_for_percent(settings.ui_scale_percent));
    prefs.set_high_contrast(settings.high_contrast);
    prefs.set_large_handles(settings.large_handles);
    // The welcome tour itself reads this flag; here it only says whether one is due.
    prefs.set_first_run_tour_pending(!settings.first_run_tour_done);

    let manipulate = ui.global::<ManipulateModel>();
    manipulate.set_snap_off(settings.manipulate_snap_off);
    manipulate.set_slice_symmetric(settings.slice_symmetric);
    manipulate.set_handle_scale(handle_scale(settings.large_handles));

    HIGH_CONTRAST.set(settings.high_contrast);
    bind_main_window_theme(ui);
}

/// Wires every `PreferencesModel` callback: each change is saved through the debounced
/// `SettingsPersister` and applied at once (all but the UI scale, which needs a restart).
pub(super) fn setup_preferences_callbacks(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let prefs = ui.global::<PreferencesModel>();

    // Each change below is also reported to an open tutorial: a step may wait for it.
    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_simple_mode_changed(move |simple: bool| {
        store.update(|s| s.settings.ui_mode = UiMode::from_simple(simple));
        raise_weak(&ui_weak, events::INTERFACE_MODE_CHANGED);
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_ui_scale_changed(move |index: i32| {
        let percent = ui_scale_percent_for_index(index);
        store.update(|s| s.settings.set_ui_scale_percent(i32::from(percent)));
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<PreferencesModel>()
                .set_ui_scale_percent(i32::from(percent));
            raise(&ui, events::UI_SCALE_CHANGED);
        }
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_high_contrast_changed(move |on: bool| {
        store.update(|s| s.settings.high_contrast = on);
        apply_high_contrast(on);
        raise_weak(&ui_weak, events::HIGH_CONTRAST_CHANGED);
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_large_handles_changed(move |on: bool| {
        store.update(|s| s.settings.large_handles = on);
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<ManipulateModel>()
                .set_handle_scale(handle_scale(on));
            raise(&ui, events::LARGE_HANDLES_CHANGED);
        }
    });

    let store = Arc::clone(settings_store);
    prefs.on_reset_tutorials(move || {
        store.update(|s| s.settings.reset_tutorials());
    });

    let store = Arc::clone(settings_store);
    let ui_weak = ui.as_weak();
    prefs.on_show_tour_again(move || {
        store.update(|s| s.settings.show_tour_again());
        if let Some(ui) = ui_weak.upgrade() {
            ui.global::<PreferencesModel>()
                .set_first_run_tour_pending(true);
        }
    });
}

// --- The Solid viewport's remembered pills -------------------------------------------

/// Saves the Snap pill (`ManipulateModel.snap_off`) through the ambient persister. Called
/// from the pill's `snap_toggled` handler, which has no settings handle of its own.
pub(super) fn persist_snap_off(snap_off: bool) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|s| s.settings.manipulate_snap_off = snap_off);
    }
}

/// Saves the Slice tool's Symmetric pill (`ManipulateModel.slice_symmetric`). The Slice
/// mode itself is deliberately never saved: it changes what a left-drag does.
pub(super) fn persist_slice_symmetric(symmetric: bool) {
    if let Some(store) = SettingsPersister::installed_for_this_thread() {
        store.update(|s| s.settings.slice_symmetric = symmetric);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::SettingsFile;
    use std::rc::Rc;

    #[test]
    fn the_scale_combo_starts_with_automatic_and_lists_every_choice_in_order() {
        let labels = ui_scale_labels();
        assert_eq!(labels.len(), UI_SCALE_CHOICES.len() + 1);
        assert_eq!(labels[0], "Automatic");
        assert_eq!(labels[1], "75 %");
        assert_eq!(labels[3], "100 %");
        assert_eq!(labels.last().map(String::as_str), Some("200 %"));
    }

    #[test]
    fn every_combo_entry_maps_to_a_percentage_and_back() {
        assert_eq!(ui_scale_percent_for_index(0), 0);
        for (position, &percent) in UI_SCALE_CHOICES.iter().enumerate() {
            let index = i32::try_from(position + 1).expect("a small index");
            assert_eq!(ui_scale_percent_for_index(index), percent);
            assert_eq!(ui_scale_index_for_percent(percent), index);
        }
        assert_eq!(ui_scale_index_for_percent(0), 0);
    }

    #[test]
    fn an_out_of_range_entry_or_percentage_means_automatic() {
        assert_eq!(ui_scale_percent_for_index(-1), 0);
        assert_eq!(ui_scale_percent_for_index(99), 0);
        assert_eq!(ui_scale_index_for_percent(133), 0);
        assert_eq!(ui_scale_index_for_percent(u16::MAX), 0);
    }

    #[test]
    fn the_environment_override_follows_the_saved_scale() {
        assert_eq!(scale_factor_override(125, false), Some("1.25"));
        assert_eq!(scale_factor_override(75, false), Some("0.75"));
        assert_eq!(scale_factor_override(200, false), Some("2"));
        assert_eq!(scale_factor_override(0, false), None, "Automatic");
        assert_eq!(scale_factor_override(133, false), None, "not offered");
    }

    #[test]
    fn a_scale_the_user_set_in_the_environment_wins_over_the_saved_one() {
        assert_eq!(scale_factor_override(125, true), None);
        assert_eq!(scale_factor_override(0, true), None);
    }

    #[test]
    fn larger_handles_are_one_point_six_times_bigger() {
        assert!((handle_scale(false) - 1.0).abs() < 1e-6);
        assert!((handle_scale(true) - 1.6).abs() < 1e-6);
    }

    /// A sink that records every state it is given and stays registered while `alive`.
    fn recording_sink(
        seen: &Rc<RefCell<Vec<bool>>>,
        alive: &Rc<Cell<bool>>,
    ) -> impl Fn(bool) -> bool + 'static {
        let seen = Rc::clone(seen);
        let alive = Rc::clone(alive);
        move |on| {
            seen.borrow_mut().push(on);
            alive.get()
        }
    }

    #[test]
    fn a_new_window_starts_in_the_current_palette_and_follows_later_changes() {
        apply_high_contrast(true);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        register_theme_sink(recording_sink(&seen, &alive));
        assert_eq!(*seen.borrow(), vec![true], "starts in the current state");

        apply_high_contrast(false);
        apply_high_contrast(true);
        assert_eq!(*seen.borrow(), vec![true, false, true]);
        assert!(high_contrast_enabled());
        apply_high_contrast(false);
    }

    #[test]
    fn every_registered_window_switches_together() {
        apply_high_contrast(false);
        let first = Rc::new(RefCell::new(Vec::new()));
        let second = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        register_theme_sink(recording_sink(&first, &alive));
        register_theme_sink(recording_sink(&second, &alive));
        apply_high_contrast(true);
        assert_eq!(*first.borrow(), vec![false, true]);
        assert_eq!(*second.borrow(), vec![false, true]);
        apply_high_contrast(false);
    }

    #[test]
    fn a_closed_window_is_forgotten_and_never_called_again() {
        apply_high_contrast(false);
        let seen = Rc::new(RefCell::new(Vec::new()));
        let alive = Rc::new(Cell::new(true));
        register_theme_sink(recording_sink(&seen, &alive));
        alive.set(false);
        apply_high_contrast(true); // reported gone: dropped here
        apply_high_contrast(false);
        assert_eq!(*seen.borrow(), vec![false, true]);

        // A sink that is already gone when it registers is never kept at all.
        let gone = Rc::new(RefCell::new(Vec::new()));
        register_theme_sink(recording_sink(&gone, &alive));
        apply_high_contrast(true);
        assert_eq!(*gone.borrow(), vec![false]);
        apply_high_contrast(false);
    }

    /// A persister on a scratch path, installed for this test's thread.
    fn install_scratch_persister(tag: &str) -> (Arc<SettingsPersister>, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-cut-preferences-test-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("scratch directory");
        let path = dir.join("settings.toml");
        let persister = Arc::new(SettingsPersister::spawn(
            path.clone(),
            SettingsFile::default(),
        ));
        SettingsPersister::install_for_this_thread(&persister);
        (persister, path)
    }

    #[test]
    fn the_snap_pill_is_saved_through_the_ambient_persister() {
        let (persister, path) = install_scratch_persister("snap");
        assert!(!persister.snapshot().settings.manipulate_snap_off);
        persist_snap_off(true);
        assert!(persister.snapshot().settings.manipulate_snap_off);
        persist_snap_off(false);
        assert!(!persister.snapshot().settings.manipulate_snap_off);

        persist_snap_off(true);
        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("manipulate_snap_off = true"), "{text}");
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn the_symmetric_pill_is_saved_and_the_slice_mode_never_is() {
        let (persister, path) = install_scratch_persister("symmetric");
        assert!(persister.snapshot().settings.slice_symmetric);
        persist_slice_symmetric(false);
        assert!(!persister.snapshot().settings.slice_symmetric);

        persister.flush();
        let text = std::fs::read_to_string(&path).expect("flush wrote the file");
        assert!(text.contains("slice_symmetric = false"), "{text}");
        assert!(
            !text.contains("slice_mode"),
            "the Slice tool must not be remembered: {text}"
        );
        let _ = std::fs::remove_dir_all(path.parent().expect("a parent directory"));
    }

    #[test]
    fn saving_a_pill_without_a_persister_does_nothing() {
        // No persister is installed on this test's own thread.
        persist_snap_off(true);
        persist_slice_symmetric(false);
    }
}
