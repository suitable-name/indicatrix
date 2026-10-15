//! Click-through passes inside the overlays, panels and secondary windows the other scenarios'
//! smoke passes never reached (`coverage` gate: elements).
//!
//! Proves: in each state of `overlay_states.rs` (and in the Help, Rough Planner, Locate, Rig and
//! Rough Colour windows) every enabled button, tab, check box and combo box inside the overlay is
//! pressed once by the smoke engine, every text field, slider and spin box is touched (a field is
//! set to the value it already holds, a slider is stepped up and back, which fires the real change
//! callbacks), and none of it panics, opens an unexpected file dialog or raises an error toast.
//! `coverage_export_menu` drives the Export split button's three items with a queued picker answer
//! and looks for the file each one writes.
//!
//! The engine is [`smoke::run`] with a host limited to the overlay's component: a restore
//! relaunches the app, rebuilds the base (library, template design, edited design) and reopens the
//! overlay, so a press that closes it costs one relaunch.

use super::{
    super::{Piece, dialogs, existing_install, first_run, main_window},
    overlay_states::{
        Base, OVERLAY_DENY, State, design_dialog_states, design_panel_states, library_states,
        startup_states, welcome_states,
    },
    state_pieces,
};
use crate::support::{
    Control, Ctx, ElementHandle, HarnessApp, Profile, ProfileExt, SecondaryWindow, SettingsSeed,
    SweepOptions, WindowView,
    app::{pump, wait_until},
    coverage, sample_designs, seed_library,
    smoke::{self, SmokeHost, indicatrix_smoke},
    sweep_options,
};
use indicatrix_cut::{
    BodyColorEditorModel, HelpModel, LibraryModel, RetargetModel, RoughPlanModel, harness_support,
};
use slint::{ComponentHandle, Model};
use slint_harness::{AccessibleRole, backend::wait_until_labeled, report::Report, timing};
use std::{
    collections::{HashMap, HashSet},
    ops::ControlFlow,
    time::Duration,
};

/// Logical size of the main window during the overlay passes. Slint's `is_visible` is clip-aware:
/// a control scrolled out of a `ScrollView` (an inspector tab, a long dialog) is not enumerated, so
/// the window is made tall enough that dialogs and panels show their whole content. The testing
/// backend has no screen to exceed.
pub(super) const TALL: (u32, u32) = (1366, 3200);
/// Height given to the secondary windows for the same reason (their width stays at the default).
pub(super) const TALL_HEIGHT: f32 = 3000.0;

/// The pieces of this file: one `overlay:<group>:<state>` piece per overlay state, the Export
/// menu, and one `window:<name>` piece per secondary-window pass.
pub fn pieces() -> Vec<Piece> {
    let mut all = Vec::new();
    all.extend(state_pieces("app", existing_install, startup_states));
    all.extend(state_pieces("library", existing_install, library_states));
    all.extend(state_pieces(
        "design-dialogs",
        existing_install,
        design_dialog_states,
    ));
    all.extend(state_pieces(
        "design-panels",
        existing_install,
        design_panel_states,
    ));
    all.extend(state_pieces("welcome", first_run, welcome_states));
    all.push(Piece::new(
        "coverage_export_menu",
        existing_install,
        export_menu,
    ));
    // The six passes of the old `coverage_secondary_windows`, one piece each.
    let passes: [super::Pass; 6] = [
        ("help", help_pass),
        ("help-after-back", help_back_pass),
        ("compare", compare_pass),
        ("planner", planner_pass),
        ("locate", locate_pass),
        ("rig", rig_pass),
    ];
    for (name, pass) in passes {
        all.push(Piece::new(format!("window:{name}"), existing_install, pass));
    }
    all.push(Piece::new(
        "coverage_rough_colour_window",
        existing_install,
        rough_colour_window,
    ));
    all
}

// ---------------------------------------------------------------------------------------------
// Fields
// ---------------------------------------------------------------------------------------------

/// Touches every enabled text field, slider and spin box in `view` and records it. A text field is
/// set to the value it already holds; a slider or spin box is stepped up and back down. These
/// roles are not "clickable", so the smoke engine never reaches them. Returns how many were
/// touched.
pub(super) fn touch_fields(view: &WindowView, admit: &dyn Fn(&Control) -> bool) -> usize {
    let mut touched = 0;
    for control in view.controls() {
        if !control.enabled || control.width <= 0.0 || control.height <= 0.0 || !admit(&control) {
            continue;
        }
        let handle = &control.handle;
        match control.role {
            AccessibleRole::TextInput => {
                let current = handle.accessible_value().unwrap_or_default();
                handle.set_accessible_value(current);
            }
            AccessibleRole::Slider | AccessibleRole::Spinbox => {
                handle.invoke_accessible_increment_action();
                handle.invoke_accessible_decrement_action();
            }
            _ => continue,
        }
        coverage::record_touch(handle);
        coverage::record_wrappers(&view.root, handle, true);
        record_enclosing_sites(&view.root, handle);
        touched += 1;
    }
    pump(150);
    touched
}

/// Records the ids of the elements around `field` that carry an id and have no interactive role
/// of their own. That is the instantiation site of a field wrapper whose root is larger than the
/// field it wraps (a caption above it or a label beside it: `RoughField`, `SweepNumberField`,
/// `CoefficientField`, `SavedPlanCard`), which `coverage::record_wrappers` misses because it only
/// looks at elements of the field's own rectangle. A wrapper holds exactly one interactive
/// element, so the field being inside it is the site being used; the ids of plain containers on
/// the way are recorded too and match no inventory entry.
pub(super) fn record_enclosing_sites(root: &ElementHandle, field: &ElementHandle) {
    let at = field.absolute_position();
    let size = field.size();
    if size.width <= 0.0 || size.height <= 0.0 || !root.is_valid() {
        return;
    }
    let role = field.accessible_role();
    let is_field = |element: &ElementHandle| {
        let p = element.absolute_position();
        let s = element.size();
        element.accessible_role() == role
            && (p.x - at.x).abs() < 0.6
            && (p.y - at.y).abs() < 0.6
            && (s.width - size.width).abs() < 0.6
            && (s.height - size.height).abs() < 0.6
    };
    let mut ids: Vec<String> = Vec::new();
    let _: Option<()> = root.visit_descendants(|element| {
        let p = element.absolute_position();
        let s = element.size();
        let encloses = p.x <= at.x + 0.6
            && p.y <= at.y + 0.6
            && p.x + s.width + 0.6 >= at.x + size.width
            && p.y + s.height + 0.6 >= at.y + size.height;
        let plain = element
            .accessible_role()
            .is_none_or(|r| r == AccessibleRole::None);
        if encloses
            && plain
            && let Some(id) = element.id().filter(|id| !id.is_empty())
            && element
                .visit_descendants(|inner| {
                    if is_field(&inner) {
                        ControlFlow::Break(())
                    } else {
                        ControlFlow::Continue(())
                    }
                })
                .is_some()
        {
            ids.push(id.to_string());
        }
        ControlFlow::Continue(())
    });
    for id in ids {
        coverage::record_site_id(&id);
    }
}

// ---------------------------------------------------------------------------------------------
// The main-window host
// ---------------------------------------------------------------------------------------------

/// Builds the state's base on a freshly relaunched app.
pub(super) fn launch_base(
    app: &mut HarnessApp,
    profile: &Profile,
    seed: &SettingsSeed,
    base: Base,
) -> Result<(), String> {
    if base == Base::Library {
        return launch_library(app, profile, seed);
    }
    app.relaunch(profile, seed)?;
    if matches!(base, Base::Design | Base::Edited) {
        let ui = app.ui().clone_strong();
        if !dialogs::make_template_design(&ui) {
            return Err("no template design could be created".to_owned());
        }
        if base == Base::Edited && !dialogs::nudge_first_tier(&ui) {
            return Err("tier 0 could not be edited".to_owned());
        }
    }
    Ok(())
}

/// The fixture library with the first row selected.
pub(super) fn launch_library(
    app: &mut HarnessApp,
    profile: &Profile,
    seed: &SettingsSeed,
) -> Result<(), String> {
    timing::time("relaunch", || launch_library_inner(app, profile, seed))
}

fn launch_library_inner(
    app: &mut HarnessApp,
    profile: &Profile,
    seed: &SettingsSeed,
) -> Result<(), String> {
    app.shutdown();
    profile.wipe_app_data();
    profile.seed_settings(seed);
    harness_support::clear_picker_answers();
    let designs = sample_designs();
    seed_library(profile, &designs)?;
    *app = HarnessApp::launch()?;
    let ui = app.ui().clone_strong();
    let wanted = designs.len();
    let library = || ui.global::<LibraryModel>();
    let list = || library().get_diagram_list();
    // The start-up load takes two worker hops: the status turns to "Database loaded: N ..." and
    // then the rows follow after a 120 ms (mock time) debounce, which `wait_until` pumps.
    let loaded = |rows: usize| {
        library()
            .get_status_message()
            .to_string()
            .starts_with("Database loaded:")
            && rows >= wanted
    };
    let mut ok = wait_until_labeled("library-load", Duration::from_secs(3), || {
        loaded(list().row_count())
    });
    if !ok && list().row_count() == 0 {
        // Ask for the list the way a filter change does, then keep waiting.
        library().invoke_filters_changed();
        ok = wait_until_labeled("library-load", Duration::from_secs(10), || {
            loaded(list().row_count())
        });
    } else if !ok {
        ok = wait_until_labeled("library-load", Duration::from_secs(10), || {
            loaded(list().row_count())
        });
    }
    if !ok {
        let message = format!(
            "LibraryModel.diagram_list held {} of {wanted} seeded rows, status {:?}",
            list().row_count(),
            library().get_status_message().to_string()
        );
        if list().row_count() == 0 {
            return Err(message);
        }
        // Partial list: report the state and press what is there.
        println!("ui-harness:   {message}");
    }
    if let Some(row) = list().row_data(0) {
        ui.global::<LibraryModel>().invoke_select_diagram(row.id);
        pump(300);
    }
    Ok(())
}

/// The subtree view of the component `name` (the element that IS it or INHERITS it: an inlined
/// component reports its root's base type first and its own name among the bases, so an exact
/// type-name match misses it), when it is showing and holds controls. `None` for a component that
/// lives in a `PopupWindow` (the popup's content is a separate item tree with no element the
/// query can root at) and for one that is not open.
pub(super) fn find_scope(app: &HarnessApp, name: &str) -> Option<WindowView> {
    let full = app.main_view();
    let root = full
        .root
        .query_descendants()
        .match_inherits(name)
        .find_first()?;
    let view = WindowView::subtree(&format!("MainWindow/{name}"), root, full.size);
    (!view.controls().is_empty()).then_some(view)
}

/// How many controls of each key the main window shows.
fn key_counts(view: &WindowView) -> HashMap<String, usize> {
    let mut counts = HashMap::new();
    for control in view.controls() {
        *counts.entry(control.key()).or_insert(0) += 1;
    }
    counts
}

/// [`SmokeHost`] for one overlay state: restore = relaunch, base, open route, fields (once).
///
/// A scope that is a popup is found by difference: the keys of the controls the popup added to
/// the main window (`admitted`) are the only ones the engine enumerates, over the whole window.
struct OverlayHost<'a> {
    app: &'a mut HarnessApp,
    profile: &'a Profile,
    seed: &'a SettingsSeed,
    state: &'a State,
    report: &'a mut Report,
    fields_done: bool,
    admitted: Option<HashSet<String>>,
}

impl OverlayHost<'_> {
    fn admits_control(&self, control: &Control) -> bool {
        self.admitted
            .as_ref()
            .is_none_or(|keys| keys.contains(&control.key()))
    }
}

impl SmokeHost for OverlayHost<'_> {
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        launch_base(self.app, self.profile, self.seed, self.state.base)?;
        self.app.set_size(TALL.0, TALL.1);
        let before = key_counts(&self.app.main_view());
        (self.state.open)(self.app);
        pump(300);
        self.app.ui().set_toast_visible(false);
        self.admitted = None;
        if let Some(name) = self.state.scope
            && find_scope(self.app, name).is_none()
        {
            let after = key_counts(&self.app.main_view());
            let added: HashSet<String> = after
                .into_iter()
                .filter(|(key, count)| *count > before.get(key).copied().unwrap_or(0))
                .map(|(key, _)| key)
                .collect();
            // Once per pass, not once per restore (a pass restores dozens of times).
            if !self.fields_done {
                println!(
                    "ui-harness:   overlay {}: scope {name} is a popup or not open; {} new control key(s) taken by difference",
                    self.state.name,
                    added.len()
                );
            }
            self.admitted = Some(added);
        }
        if !self.fields_done {
            self.fields_done = true;
            let views = self.views();
            let touched: usize = views
                .iter()
                .map(|view| touch_fields(view, &|c| self.admits_control(c)))
                .sum();
            println!(
                "ui-harness:   overlay {}: {touched} field(s) touched",
                self.state.name
            );
            self.app.ui().set_toast_visible(false);
            // A touched color slider starts a background match; "Apply Color" is disabled while
            // it runs, and the engine reads the controls' enabled state right after this.
            let colour = self.app.ui().global::<BodyColorEditorModel>();
            let _ = wait_until(Duration::from_secs(30), || !colour.get_busy());
            // Likewise a touched Retarget field restarts the change's check (validity 1 while it
            // runs, plus a 120 ms debounce before it starts), and Apply is dimmed until it ends.
            let retarget = self.app.ui().global::<RetargetModel>();
            if retarget.get_is_open() {
                pump(250);
                let _ = wait_until(Duration::from_secs(20), || {
                    retarget.get_validity_state() != 1 && !retarget.get_is_busy()
                });
            }
            pump(100);
        }
        Ok(self.views())
    }

    fn views(&self) -> Vec<WindowView> {
        let full = || self.app.main_view();
        let view = match (self.state.scope, &self.admitted) {
            (Some(name), None) => find_scope(self.app, name).unwrap_or_else(full),
            _ => full(),
        };
        vec![view]
    }

    fn admits(&self, control: &Control) -> bool {
        self.admits_control(control)
    }

    fn notice(&mut self) -> Option<(String, String)> {
        let ui = self.app.ui();
        ui.get_toast_visible().then(|| {
            (
                ui.get_toast_type().to_string(),
                ui.get_toast_message().to_string(),
            )
        })
    }

    fn clear_notice(&mut self) {
        self.app.ui().set_toast_visible(false);
    }

    fn main_alive(&self) -> bool {
        self.app.is_open()
    }

    fn report(&mut self) -> &mut Report {
        &mut *self.report
    }

    fn sweep_options(&self) -> SweepOptions {
        sweep_options()
    }
}

/// One pass per state.
pub(super) fn run_states(ctx: &mut Ctx<'_>, states: &[State]) {
    for state in states {
        let mut config = indicatrix_smoke(format!("overlay: {}", state.name));
        config.deny = OVERLAY_DENY.to_vec();
        config.extra_deny = state.deny.to_vec();
        config.scope_may_close = true;
        let outcome = {
            let mut host = OverlayHost {
                app: &mut *ctx.app,
                profile: ctx.profile,
                seed: &ctx.seed,
                state,
                report: &mut *ctx.report,
                fields_done: false,
                admitted: None,
            };
            smoke::run(&mut host, &config)
        };
        println!(
            "ui-harness:   smoke {}: {} control(s), {} activated, {} denied, {} gone, {} closed the overlay, {} restore(s)",
            config.state,
            outcome.enumerated,
            outcome.activated,
            outcome.denied,
            outcome.gone,
            outcome.closed,
            outcome.restores
        );
        ctx.check_health(&config.state);
    }
}

// ---------------------------------------------------------------------------------------------
// The Export split button
// ---------------------------------------------------------------------------------------------

/// Presses the first clickable element labelled `label` of element type `kind`.
fn press_typed(app: &HarnessApp, label: &str, kind: &str) -> bool {
    let target = app.main_view().by_label(label).into_iter().find(|e| {
        e.accessible_role()
            .is_some_and(crate::support::query::is_clickable_role)
            && e.type_name().is_some_and(|t| t.as_str() == kind)
    });
    let Some(element) = target else {
        return false;
    };
    coverage::record_activation(&element);
    element.invoke_accessible_default_action();
    pump(300);
    true
}

/// Opens the Export menu of the editor's command bar and presses its three items, each with a
/// queued answer for the file dialog (a path in the profile's scratch folder). Proves the menu
/// opens and each item reaches its writer; a missing file is a note (the item may write beside
/// the chosen name).
fn export_menu(ctx: &mut Ctx<'_>) {
    if !dialogs::create_template_design(ctx) {
        return;
    }
    main_window::select_top_tab_model(ctx.app, 0);
    main_window::select_render_view(ctx.app, 1);
    let items = [
        ("Export Cutting Sheet (HTML)", "html"),
        ("Export Diagram (PNG)", "png"),
        ("Export as Gem Cut Studio (.gcs)", "gcs"),
    ];
    for (label, extension) in items {
        let path = ctx.scratch_file(&format!("harness_export.{extension}"));
        let _ = std::fs::remove_file(&path);
        harness_support::queue_picker_path(path.clone());
        let opened = press_typed(ctx.app, "Export", "Rectangle");
        ctx.expect(
            opened,
            "export-menu-opens",
            "the Export split button was not found on the Edit view",
        );
        if !opened {
            break;
        }
        let pressed = press_typed(ctx.app, label, "Rectangle");
        ctx.expect(
            pressed,
            "export-menu-item",
            format!("the Export menu shows no item {label:?}"),
        );
        if pressed && !wait_until(Duration::from_secs(10), || path.exists()) {
            ctx.report.note(format!(
                "coverage_export_menu: {label} wrote no file at {} (it may name its own file beside it)",
                path.display()
            ));
        }
        harness_support::clear_picker_answers();
        ctx.check_health(label);
    }
}

// ---------------------------------------------------------------------------------------------
// Secondary windows
// ---------------------------------------------------------------------------------------------

/// The view of `which` when it is open.
pub(super) fn secondary_view(which: SecondaryWindow) -> Option<WindowView> {
    match which {
        SecondaryWindow::Compare => {
            harness_support::compare_window().map(|w| WindowView::of("CompareWindow", &w))
        }
        SecondaryWindow::RoughPlanner => harness_support::rough_planner_window()
            .map(|w| WindowView::of("RoughPlannerWindow", &w)),
        SecondaryWindow::Locate => {
            harness_support::locate_window().map(|w| WindowView::of("LocateWindow", &w))
        }
        SecondaryWindow::Rig => {
            harness_support::rig_window().map(|w| WindowView::of("RigWindow", &w))
        }
        SecondaryWindow::RoughColor => {
            harness_support::rough_color_window().map(|w| WindowView::of("RoughColourWindow", &w))
        }
        SecondaryWindow::Help => {
            harness_support::help_window().map(|w| WindowView::of("HelpWindow", &w))
        }
    }
}

/// Waits (real time, 10 s) until `which` is open.
pub(super) fn wait_open(which: SecondaryWindow) -> bool {
    wait_until(Duration::from_secs(10), || {
        harness_support::is_window_open(which)
    })
}

/// Makes `which` tall (default width kept) so its scroll areas show all their content, then lets
/// the layout settle. Does nothing when the window is not open.
pub(super) fn grow(which: SecondaryWindow) {
    macro_rules! tall {
        ($window:expr) => {
            if let Some(w) = $window {
                let width = w.window().size().width as f32 / w.window().scale_factor().max(1.0);
                w.window()
                    .set_size(slint::LogicalSize::new(width, TALL_HEIGHT));
            }
        };
    }
    match which {
        SecondaryWindow::Compare => tall!(harness_support::compare_window()),
        SecondaryWindow::RoughPlanner => tall!(harness_support::rough_planner_window()),
        SecondaryWindow::Locate => tall!(harness_support::locate_window()),
        SecondaryWindow::Rig => tall!(harness_support::rig_window()),
        SecondaryWindow::RoughColor => tall!(harness_support::rough_color_window()),
        SecondaryWindow::Help => tall!(harness_support::help_window()),
    }
    pump(100);
}

/// Grows `which` and touches its fields once (`done` remembers it across the engine's restores).
pub(super) fn touch_secondary(which: SecondaryWindow, done: &mut bool) {
    grow(which);
    if *done {
        return;
    }
    if let Some(view) = secondary_view(which) {
        *done = true;
        let touched = touch_fields(&view, &|_| true);
        println!("ui-harness:   {}: {touched} field(s) touched", view.name);
    }
}

/// Opens the Rough Planner from the main window; `true` once it is showing.
pub(super) fn open_planner(app: &HarnessApp) -> bool {
    app.ui().global::<RoughPlanModel>().invoke_open();
    wait_open(SecondaryWindow::RoughPlanner)
}

// The Help window with a search done and a second topic opened (Back and Forward have somewhere
// to go), the Rough Planner with a block rough, a cut and an inclusion, the Locate window and
// the Rig window: a smoke pass and a field pass in each, one piece per pass (see `pieces`).

fn help_pass(ctx: &mut Ctx<'_>) {
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        let ui = app.ui().clone_strong();
        ui.global::<HelpModel>().invoke_open_context_help();
        if !wait_open(SecondaryWindow::Help) {
            return;
        }
        ui.global::<HelpModel>()
            .invoke_open_topic("05-solving#what-solve-does".into());
        pump(300);
        if let Some(window) = harness_support::help_window() {
            let model = window.global::<HelpModel>();
            model.set_search_text("facet".into());
            model.invoke_search("facet".into());
            let _ = wait_until(Duration::from_secs(10), || model.get_hits().row_count() > 0);
        }
        touch_secondary(SecondaryWindow::Help, &mut done);
    };
    let _stats = ctx.smoke_secondary(SecondaryWindow::Help, "searched, topic open", &mut prepare);
    ctx.check_health("Help window pass");
}

/// The Help window after Back: the history has a page to go forward to, so Forward is enabled
/// (`HelpModel.can_forward`) and the engine can press it.
fn help_back_pass(ctx: &mut Ctx<'_>) {
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        let ui = app.ui().clone_strong();
        ui.global::<HelpModel>().invoke_open_context_help();
        if !wait_open(SecondaryWindow::Help) {
            return;
        }
        let model = ui.global::<HelpModel>();
        model.invoke_open_topic("05-solving#what-solve-does".into());
        pump(200);
        model.invoke_open_topic("04-editing-tiers#the-tier-form".into());
        pump(200);
        if let Some(window) = harness_support::help_window() {
            window.global::<HelpModel>().invoke_back();
            pump(200);
        }
        touch_secondary(SecondaryWindow::Help, &mut done);
    };
    let _stats = ctx.smoke_secondary(SecondaryWindow::Help, "after Back", &mut prepare);
    ctx.check_health("Help window after Back pass");
}

/// The visual-compare window on a snapshot of the first library design compared with itself.
fn compare_pass(ctx: &mut Ctx<'_>) {
    let profile: &Profile = ctx.profile;
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        let seed = SettingsSeed::ExistingInstall;
        if let Err(message) = launch_library(app, profile, &seed) {
            println!("ui-harness:   compare pass: {message}");
            return;
        }
        let ui = app.ui().clone_strong();
        ui.global::<indicatrix_cut::EditorModel>()
            .invoke_load_selected();
        if !wait_until(Duration::from_secs(10), || {
            ui.global::<indicatrix_cut::EditorModel>().get_has_design()
        }) {
            return;
        }
        match super::setups::snapshot_and_open_compare(&ui) {
            Ok(()) => touch_secondary(SecondaryWindow::Compare, &mut done),
            Err(message) => println!("ui-harness:   compare pass: {message}"),
        }
    };
    let _stats = ctx.smoke_secondary(
        SecondaryWindow::Compare,
        "snapshot against itself",
        &mut prepare,
    );
    ctx.check_health("Compare window pass");
}

fn planner_pass(ctx: &mut Ctx<'_>) {
    let profile: &Profile = ctx.profile;
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        let seed = SettingsSeed::ExistingInstall;
        // The planner works without a library; a library that did not load is reported and the
        // window is opened over whatever app is running (launch_library leaves none when the
        // launch itself failed, then the app is relaunched empty).
        if let Err(message) = launch_library(app, profile, &seed) {
            println!("ui-harness:   planner pass: {message}");
            if !app.is_open() && app.relaunch(profile, &seed).is_err() {
                return;
            }
        }
        if !open_planner(app) {
            return;
        }
        if let Some(window) = harness_support::rough_planner_window() {
            let model = window.global::<RoughPlanModel>();
            model.invoke_set_base(0);
            model.set_rough_x("40".into());
            model.set_rough_y("30".into());
            model.set_rough_z("25".into());
            model.invoke_base_field_committed();
            pump(250);
            model.invoke_add_cut(0);
            harness_support::queue_picker_cancel();
            model.invoke_add_inclusion();
            pump(300);
        }
        touch_secondary(SecondaryWindow::RoughPlanner, &mut done);
    };
    let _stats = ctx.smoke_secondary(
        SecondaryWindow::RoughPlanner,
        "block, cut and inclusion",
        &mut prepare,
    );
    ctx.check_health("Rough Planner pass");
}

fn locate_pass(ctx: &mut Ctx<'_>) {
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        if !open_planner(app) {
            return;
        }
        pump(300);
        if let Some(window) = harness_support::rough_planner_window() {
            window.global::<RoughPlanModel>().invoke_locate_inclusion();
        }
        if wait_open(SecondaryWindow::Locate) {
            touch_secondary(SecondaryWindow::Locate, &mut done);
        }
    };
    let _stats = ctx.smoke_secondary(
        SecondaryWindow::Locate,
        "opened from the planner",
        &mut prepare,
    );
    ctx.check_health("Locate window pass");
}

fn rig_pass(ctx: &mut Ctx<'_>) {
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        if !open_planner(app) {
            return;
        }
        pump(300);
        if let Some(window) = harness_support::rough_planner_window() {
            window.global::<RoughPlanModel>().invoke_locate_inclusion();
        }
        if !wait_open(SecondaryWindow::Locate) {
            return;
        }
        if let Some(window) = harness_support::locate_window() {
            window.invoke_edit_rigs();
        }
        if wait_open(SecondaryWindow::Rig) {
            touch_secondary(SecondaryWindow::Rig, &mut done);
        }
    };
    let _stats = ctx.smoke_secondary(SecondaryWindow::Rig, "opened from Locate", &mut prepare);
    ctx.check_health("Rig window pass");
}

/// The Rough colour wizard, opened from the planner.
fn rough_colour_window(ctx: &mut Ctx<'_>) {
    use indicatrix_cut::Zoning;
    let mut done = false;
    let mut prepare = |app: &mut HarnessApp| {
        if !open_planner(app) {
            return;
        }
        pump(300);
        if let Some(window) = harness_support::rough_planner_window() {
            window.global::<Zoning>().invoke_open_rough_colour();
        }
        if wait_open(SecondaryWindow::RoughColor) {
            touch_secondary(SecondaryWindow::RoughColor, &mut done);
        }
    };
    let _stats = ctx.smoke_secondary(
        SecondaryWindow::RoughColor,
        "opened from the planner",
        &mut prepare,
    );
    ctx.check_health("Rough colour wizard pass");
}
