//! The Rough Planner window: created on the first open, hidden (never destroyed) when
//! it is closed so its results and model survive, and destroyed together with the main
//! window.

use super::{
    counts::{is_remote, refresh_counts, refresh_counts_if_filter_changed, sync_remote_message},
    cut_faces::{corner_names, edge_names, options_model},
    editing,
    exclusions::{self, ExclusionWorker},
    format::to_i32,
    inputs::{built_in_choices, material_choices},
    library_link, run, saved,
    session::{DEFAULT_MATERIAL, MaterialChoice, Session},
    shape_worker::ShapeWorker,
    view,
};
use crate::{
    MainWindow, RoughPlanModel, RoughPlannerWindow,
    bridge::library::source::LibrarySource,
    gui::{show_toast, tutorial_events::raise},
};
use indicatrix_editor::guide::viewing_events::ROUGH_PLANNER_OPENED;
use indicatrix_vault::db::sqlite::Database;
use slint::{
    CloseRequestResponse, ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel, Weak,
    winit_030::WinitWindowAccessor,
};
use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::{Arc, Mutex, atomic::Ordering},
    time::Duration,
};
use tracing::warn;

/// How often the library filter is compared with the one the design counts were asked
/// for, while the window is open.
const FILTER_POLL: Duration = Duration::from_secs(1);

/// The share of the monitor's width the window may take when it is first shown.
const MAX_WIDTH_FRACTION: f32 = 0.94;

/// The share of the monitor's height the window may take when it is first shown. Lower
/// than the width's because the task bar and the title bar take height, and `winit` has
/// no cross-platform work-area query.
const MAX_HEIGHT_FRACTION: f32 = 0.88;

/// Everything the planner window's callbacks reach on the UI thread.
pub(super) struct Host {
    /// The window itself.
    pub(super) window: RoughPlannerWindow,
    /// Notices a change of the library filter while the window is open and asks for fresh
    /// design counts (they are a display hint; a plan resolves its candidates itself).
    filter_watch: Timer,
    /// Whether the window was fitted to its monitor, which happens once, on the first show.
    size_fitted: Cell<bool>,
    /// The main window (library links and the filter snapshot need it).
    pub(super) main: Weak<MainWindow>,
    /// The catalogue database.
    pub(super) db: Arc<Mutex<Database>>,
    /// The active library source.
    pub(super) source: Arc<Mutex<LibrarySource>>,
    /// The edited model, its history, the results and the selection.
    pub(super) session: Rc<RefCell<Session>>,
    /// The live model evaluation.
    pub(super) shape: ShapeWorker,
    /// The reads and writes of the designs excluded from the planner, off the UI thread.
    pub(super) exclusions: ExclusionWorker,
}

thread_local! {
    /// The one planner window of the app session, once it was opened.
    static PLANNER: RefCell<Option<Rc<Host>>> = const { RefCell::new(None) };
}

/// Runs `f` with the planner host, or returns `None` when the window does not exist
/// (never opened, or already closed with the main window). The host is cloned out first,
/// so `f` may call `with_host` again.
pub(super) fn with_host<R>(f: impl FnOnce(&Rc<Host>) -> R) -> Option<R> {
    let host = PLANNER.with(|cell| cell.borrow().clone())?;
    Some(f(&host))
}

/// [`with_host`] for callbacks that have nothing to return.
pub(super) fn on_host(f: impl FnOnce(&Rc<Host>)) {
    let _ = with_host(f);
}

/// [`on_host`] for the callbacks that edit the model: they are ignored while a plan runs,
/// because the plan works on a snapshot and the window must keep showing the model it
/// is planning.
pub(super) fn on_idle_host(f: impl FnOnce(&Rc<Host>)) {
    on_host(|host| {
        if !host.window.global::<RoughPlanModel>().get_running() {
            f(host);
        }
    });
}

/// The index to select in `choices`: the material called `previous` when it is still
/// listed, else the default material, else the first.
#[must_use]
fn selected_material_index(choices: &[MaterialChoice], previous: Option<&str>) -> usize {
    previous
        .and_then(|name| choices.iter().position(|c| c.name == name))
        .or_else(|| choices.iter().position(|c| c.name == DEFAULT_MATERIAL))
        .unwrap_or(0)
}

/// Shows `choices` as the material list, keeping the material picked before when it
/// still exists.
fn apply_choices(host: &Rc<Host>, choices: Vec<MaterialChoice>) {
    let model = host.window.global::<RoughPlanModel>();
    let previous = usize::try_from(model.get_material_index())
        .ok()
        .and_then(|i| host.session.borrow().choices.get(i).map(|c| c.name.clone()));
    let index = selected_material_index(&choices, previous.as_deref());
    let labels: Vec<SharedString> = choices
        .iter()
        .map(|c| format!("{} (SG {:.2})", c.name, c.specific_gravity).into())
        .collect();
    model.set_materials(ModelRc::new(VecModel::from(labels)));
    model.set_material_index(to_i32(index));
    host.session.borrow_mut().choices = choices;
}

/// The catalogue's own materials arrived from the background read: the list is replaced
/// (and the model re-evaluated for the selected material) unless nothing changed.
fn merge_choices(host: &Rc<Host>, choices: Vec<MaterialChoice>) {
    if host.session.borrow().choices == choices {
        return;
    }
    apply_choices(host, choices);
    editing::readout_inputs_changed(host);
}

/// Reads the catalogue materials on a thread of their own (the database lock may be held
/// by a long search) and merges them into the list when they are in.
fn load_catalogue_materials(host: &Rc<Host>) {
    let db = Arc::clone(&host.db);
    let window = host.window.as_weak();
    let spawned = std::thread::Builder::new()
        .name("rough-materials".to_string())
        .spawn(move || {
            let choices = material_choices(&db);
            let _ = window.upgrade_in_event_loop(move |_window| {
                on_host(|host| merge_choices(host, choices));
            });
        });
    if let Err(error) = spawned {
        warn!("Rough planner: could not start the materials thread: {error}");
    }
}

/// Fills the material list: the built-in materials at once (on the first open), then the
/// catalogue's from a background read, so the UI thread never waits on the database.
fn fill_materials(host: &Rc<Host>) {
    if host.session.borrow().choices.is_empty() {
        apply_choices(host, built_in_choices());
    }
    load_catalogue_materials(host);
}

/// Pushes whether library links work: they do not for a remote library, which also shows
/// the "switch to the local library" line (and takes it away again for a local one).
fn push_links_enabled(host: &Rc<Host>) {
    let model = host.window.global::<RoughPlanModel>();
    let remote = is_remote(&host.source);
    model.set_links_enabled(!remote);
    sync_remote_message(&model, remote);
}

/// Asks for fresh design counts for the current library filter (unless a plan runs).
fn recount(host: &Rc<Host>) {
    if host.window.global::<RoughPlanModel>().get_running() {
        return;
    }
    if let Some(main) = host.main.upgrade() {
        refresh_counts(&main, &host.window, &host.db, &host.source, &host.session);
    }
}

/// The library source was switched (local or remote): the window's library links and
/// design counts follow. Does nothing while the planner window has never been opened.
/// Called from `gui::library::remote` when the source changes.
pub(in crate::gui) fn refresh_links_enabled() {
    on_host(|host| {
        push_links_enabled(host);
        recount(host);
    });
}

/// Timer callback: fresh counts when the library filter changed since they were asked
/// for, while the window is showing.
fn recount_on_filter_change(host: &Rc<Host>) {
    if !host.window.window().is_visible() {
        return;
    }
    if let Some(main) = host.main.upgrade() {
        refresh_counts_if_filter_changed(
            &main,
            &host.window,
            &host.db,
            &host.source,
            &host.session,
        );
    }
}

/// The size a window of `current` logical size is brought to on a monitor of the given
/// logical size: capped at a share of the monitor, never enlarged. `None` when it fits
/// already (or the monitor size is unusable).
fn fitted_size(
    monitor_width: f32,
    monitor_height: f32,
    current: slint::LogicalSize,
) -> Option<slint::LogicalSize> {
    if monitor_width <= 0.0 || monitor_height <= 0.0 {
        return None;
    }
    let width = current.width.min(monitor_width * MAX_WIDTH_FRACTION);
    let height = current.height.min(monitor_height * MAX_HEIGHT_FRACTION);
    let changed = (width - current.width).abs() > 0.5 || (height - current.height).abs() > 0.5;
    changed.then(|| slint::LogicalSize::new(width.floor(), height.floor()))
}

/// Shrinks the shown window to its monitor when its preferred size does not fit. The
/// window may still be made smaller (down to its content's minimum) or larger by hand.
/// Needs the window to be shown: its platform window exists from the first event-loop
/// turn on.
fn fit_to_monitor(window: &RoughPlannerWindow) {
    let monitor = window
        .window()
        .with_winit_window(|winit_window| {
            winit_window
                .current_monitor()
                .or_else(|| winit_window.primary_monitor())
                .map(|monitor| {
                    let physical = monitor.size();
                    let scale = monitor.scale_factor();
                    (
                        f64::from(physical.width) / scale,
                        f64::from(physical.height) / scale,
                    )
                })
        })
        .flatten();
    let Some((monitor_width, monitor_height)) = monitor else {
        warn!("Rough planner: could not determine the monitor size; keeping the window size.");
        return;
    };
    let current = window
        .window()
        .size()
        .to_logical(window.window().scale_factor());
    if let Some(size) = fitted_size(monitor_width as f32, monitor_height as f32, current) {
        window.window().set_size(size);
    }
}

/// Fits the window to its monitor once, on the first show.
fn fit_on_first_show(host: &Rc<Host>) {
    if host.size_fitted.replace(true) {
        return;
    }
    let weak = host.window.as_weak();
    Timer::single_shot(Duration::ZERO, move || {
        if let Some(window) = weak.upgrade() {
            fit_to_monitor(&window);
        }
    });
}

/// What opening (or re-opening) the window refreshes: the library links, and, unless a
/// plan is running, the material list, the designs excluded from the planner and the two
/// design counts (which leave those out and read the marks themselves). The material list
/// and the exclusions are read on threads of their own; the window shows them when they
/// arrive, so opening never waits for the library database.
fn refresh_on_open(main: &MainWindow, host: &Rc<Host>) {
    push_links_enabled(host);
    if host.window.global::<RoughPlanModel>().get_running() {
        return;
    }
    fill_materials(host);
    exclusions::reload(host);
    refresh_counts(main, &host.window, &host.db, &host.source, &host.session);
    editing::readout_inputs_changed(host);
}

/// Creates the window and wires every callback once.
fn create_host(
    main: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) -> Option<Rc<Host>> {
    let window = match RoughPlannerWindow::new() {
        Ok(window) => window,
        Err(error) => {
            warn!("Could not create the rough planner window: {error}");
            return None;
        }
    };
    // Starts in the current palette (high contrast) and follows later changes.
    crate::gui::preferences::bind_planner_window_theme(&window);
    // The `zoning` feature's UI switch (a global is per window).
    crate::gui::zoning_ui::apply_to_planner(&window);
    let host = Rc::new(Host {
        shape: ShapeWorker::new(window.as_weak()),
        exclusions: ExclusionWorker::new(window.as_weak(), Arc::clone(db)),
        window,
        filter_watch: Timer::default(),
        size_fitted: Cell::new(false),
        main: main.as_weak(),
        db: Arc::clone(db),
        source: Arc::clone(source),
        session: Rc::new(RefCell::new(Session::default())),
    });
    push_links_enabled(&host);
    // Closing hides the window; the host and the session stay.
    host.window
        .window()
        .on_close_requested(|| CloseRequestResponse::HideWindow);
    let model = host.window.global::<RoughPlanModel>();
    model.set_time_limit_secs(crate::plan_limit::stored_limit_secs().to_string().into());
    model.set_edge_options(options_model(edge_names()));
    model.set_corner_options(options_model(corner_names()));
    editing::setup_edit_callbacks(&host);
    #[cfg(feature = "zoning")]
    super::colour_link::setup(&host);
    view::setup_view_callbacks(&host);
    run::setup_run_callbacks(&host);
    saved::setup_saved_callbacks(&host);
    library_link::setup_link_callbacks(&host);
    exclusions::setup_callbacks(&host);
    host.filter_watch
        .start(TimerMode::Repeated, FILTER_POLL, || {
            on_host(recount_on_filter_change);
        });
    PLANNER.with(|cell| *cell.borrow_mut() = Some(Rc::clone(&host)));
    Some(host)
}

/// Opens the planner window: creates it on the first call, otherwise shows and raises
/// the existing one with everything it held.
pub(in crate::gui) fn open_planner(
    main: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let Some(host) = with_host(Rc::clone).or_else(|| create_host(main, db, source)) else {
        show_toast(main, "Could not open the Rough Planner window.", "error");
        return;
    };
    refresh_on_open(main, &host);
    if let Err(error) = host.window.show() {
        warn!("Could not show the rough planner window: {error}");
        show_toast(main, "Could not open the Rough Planner window.", "error");
        return;
    }
    host.window.window().set_minimized(false);
    fit_on_first_show(&host);
    // The 3D view's size: its own change notifications miss the size of the first layout.
    view::sync_view_size(&host);
    // A tutorial step may wait for the planner to open.
    raise(main, ROUGH_PLANNER_OPENED);
}

/// The sentence naming the planner work that closing the application would lose, given
/// how many results are unsaved (zero when none, or when the shown results are saved)
/// and whether a plan is running. `None` when nothing would be lost.
fn risk_message(unsaved_results: usize, plan_running: bool) -> Option<String> {
    let mut parts = Vec::new();
    if unsaved_results > 0 {
        let noun = if unsaved_results == 1 {
            "result"
        } else {
            "results"
        };
        parts.push(format!(
            "The Rough Planner has {unsaved_results} unsaved {noun}."
        ));
    }
    if plan_running {
        parts.push("A rough plan is still running.".to_string());
    }
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// What closing the main window would lose in the planner: a message when the results on
/// screen were never saved or a plan is still running, `None` when nothing is at risk or
/// the planner window was never opened. Closing the planner window itself only hides it,
/// so this is only asked when the whole application closes.
pub(in crate::gui) fn planner_work_at_risk() -> Option<String> {
    with_host(|host| {
        // A session that is being written to right now cannot be read; it then counts as
        // holding no results, and a running plan is still reported.
        let unsaved = host.session.try_borrow().map_or(0, |session| {
            if session.run.results_unsaved {
                session.run.layouts.len()
            } else {
                0
            }
        });
        let running = host.window.global::<RoughPlanModel>().get_running();
        risk_message(unsaved, running)
    })
    .flatten()
}

/// Closes the planner window for good: asks a running plan to stop and drops the host.
/// Called wherever the main window hides, so the planner never outlives it.
pub(in crate::gui) fn close_planner_window() {
    let Some(host) = PLANNER.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    if let Some(flag) = &host.session.borrow().cancel {
        flag.store(true, Ordering::Relaxed);
    }
    let _ = host.window.hide();
    // The locate and rig windows belong to the planner and go with it.
    super::locate::close_windows();
    #[cfg(feature = "zoning")]
    super::colour_link::close();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn choice(name: &str) -> MaterialChoice {
        MaterialChoice {
            name: name.to_string(),
            specific_gravity: 3.0,
        }
    }

    #[test]
    fn the_material_picked_before_stays_selected_when_it_is_still_listed() {
        let choices = [choice("Corundum"), choice("Quartz"), choice("Beryl")];
        assert_eq!(selected_material_index(&choices, Some("Beryl")), 2);
        assert_eq!(selected_material_index(&choices, Some("Corundum")), 0);
    }

    #[test]
    fn a_missing_material_falls_back_to_the_default_then_to_the_first() {
        let choices = [choice("Corundum"), choice("Quartz"), choice("Beryl")];
        assert_eq!(selected_material_index(&choices, Some("Gone")), 1);
        assert_eq!(selected_material_index(&choices, None), 1);
        let without_default = [choice("Corundum"), choice("Beryl")];
        assert_eq!(selected_material_index(&without_default, None), 0);
        assert_eq!(selected_material_index(&[], Some("Quartz")), 0);
    }

    #[test]
    fn a_window_is_shrunk_to_a_share_of_a_small_monitor_and_never_enlarged() {
        // 1280 x 820 on a 1536 x 864 monitor: the width fits (0.94 x 1536 = 1443.84 >
        // 1280) and the height is capped at 0.88 x 864 = 760.32, floored to 760.
        let current = slint::LogicalSize::new(1280.0, 820.0);
        let size = fitted_size(1536.0, 864.0, current).expect("the height must shrink");
        assert_eq!(size.width, 1280.0);
        assert_eq!(size.height, (864.0_f32 * MAX_HEIGHT_FRACTION).floor());
        // A monitor with room changes nothing, however large it is.
        assert!(fitted_size(2560.0, 1440.0, current).is_none());
        assert!(fitted_size(7680.0, 4320.0, current).is_none());
        // A window the platform already made smaller than the cap is left as it is.
        let small = slint::LogicalSize::new(900.0, 600.0);
        assert!(fitted_size(1536.0, 864.0, small).is_none());
        // A degenerate monitor size is ignored.
        assert!(fitted_size(0.0, 864.0, current).is_none());
        assert!(fitted_size(1536.0, -1.0, current).is_none());
    }

    #[test]
    fn closing_warns_about_unsaved_results_and_a_running_plan() {
        assert_eq!(risk_message(0, false), None);
        assert_eq!(
            risk_message(3, false).as_deref(),
            Some("The Rough Planner has 3 unsaved results.")
        );
        assert_eq!(
            risk_message(1, false).as_deref(),
            Some("The Rough Planner has 1 unsaved result.")
        );
        assert_eq!(
            risk_message(0, true).as_deref(),
            Some("A rough plan is still running.")
        );
        assert_eq!(
            risk_message(10, true).as_deref(),
            Some("The Rough Planner has 10 unsaved results. A rough plan is still running.")
        );
    }

    #[test]
    fn the_planner_host_does_not_exist_before_the_window_is_opened() {
        // No window in a unit test: the callbacks that reach for the host do nothing.
        assert!(with_host(|_| ()).is_none());
        assert_eq!(planner_work_at_risk(), None);
        let mut ran = false;
        on_host(|_| ran = true);
        on_idle_host(|_| ran = true);
        assert!(!ran);
        // A change of the exclusions made in the library window reaches no planner.
        crate::gui::rough_plan::exclusions_changed();
    }
}
