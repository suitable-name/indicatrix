//! More click-through passes (the part of run 6's uncovered elements that live in the secondary
//! windows, plus the registration of `states_more.rs`).
//!
//! Proves: in the Rough Planner (a finished plan, saved plans, the question rows, the excluded
//! list, a mesh-like rough), the Rig window (Rig and Calibrate tabs with a stored rig), the Locate
//! window (with a stored rig), the Compare window (split view, reference choice, tilt button) and
//! the Rough Colour wizard (each of its eight steps) every enabled control is pressed once and
//! every field touched, without a panic, an unexpected file dialog or an error toast.
//!
//! The engine here is the overlay one: `OVERLAY_DENY` instead of the generic smoke list, so the
//! destructive pills (Delete, Remove, Replace, Discard) are pressed too: the profile is rebuilt
//! after every visible change. A press that closes the window is the control doing its job
//! (`scope_may_close`, views named `Window/<state>`).

use super::{
    super::{Piece, existing_install},
    overlay_states::{OVERLAY_DENY, deny},
    overlays, setups, state_pieces, states_more,
};
use crate::support::{
    Ctx, HarnessApp, Profile, SecondaryWindow, SettingsSeed, SweepOptions, WindowView,
    app::{pump, wait_until},
    coverage,
    query::is_clickable_role,
    smoke::{self, DenyMatch, DenyRule, SmokeHost, indicatrix_smoke},
    sweep_options,
};
use indicatrix_cut::{CompareMetricRow, CompareModel, RoughPlanModel, harness_support};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use slint_harness::{backend::wait_until_labeled, report::Report};
use std::time::Duration;

/// The pieces of this file: one `overlay:more-*:<state>` piece per main-window state and one
/// `window:<name>:<state>` piece per secondary-window state.
pub fn pieces() -> Vec<Piece> {
    let mut all = Vec::new();
    all.extend(state_pieces(
        "more-dialogs",
        existing_install,
        states_more::dialog_states,
    ));
    all.extend(state_pieces(
        "more-library",
        existing_install,
        states_more::library_states,
    ));
    all.extend(state_pieces(
        "more-panels",
        existing_install,
        states_more::panel_states,
    ));
    for (index, label) in PLANNER_STEP_LABELS.iter().enumerate() {
        all.push(Piece::new(
            format!("window:planner:{label}"),
            existing_install,
            move |ctx| planner_step_pass(ctx, index),
        ));
    }
    all.push(Piece::new(
        format!("window:planner:{PLANNER_MESH_LABEL}"),
        existing_install,
        planner_mesh_pass,
    ));
    for (index, (label, _, _, _)) in RIG_PASSES.iter().enumerate() {
        all.push(Piece::new(
            format!("window:rig-locate:{label}"),
            existing_install,
            move |ctx| rig_pass(ctx, index),
        ));
    }
    all.push(Piece::new(
        "window:compare:split view",
        existing_install,
        compare_split,
    ));
    for (index, label) in ROUGH_COLOUR_LABELS.iter().enumerate() {
        all.push(Piece::new(
            format!("window:rough-colour:{label}"),
            existing_install,
            move |ctx| rough_colour_pass(ctx, index),
        ));
    }
    all
}

// ---------------------------------------------------------------------------------------------
// The secondary-window host with the overlay deny list
// ---------------------------------------------------------------------------------------------

/// [`SmokeHost`] for one secondary window: restore = relaunch + `prepare`; the window must be
/// open afterwards. Its fields are touched once, on the first restore. `relaunch` is false when
/// `prepare` launches the app itself (through `overlays::launch_library`).
struct WindowHost<'a> {
    which: SecondaryWindow,
    label: &'static str,
    relaunch: bool,
    app: &'a mut HarnessApp,
    profile: &'a Profile,
    seed: &'a SettingsSeed,
    prepare: &'a mut dyn FnMut(&mut HarnessApp),
    report: &'a mut Report,
    fields_done: bool,
}

impl WindowHost<'_> {
    /// The window's view, named `<Window>/<state>` so a press that closes it is expected.
    fn view(&self) -> Option<WindowView> {
        let mut view = overlays::secondary_view(self.which)?;
        view.name = format!("{}/{}", view.name, self.label);
        Some(view)
    }
}

impl SmokeHost for WindowHost<'_> {
    fn restore(&mut self) -> Result<Vec<WindowView>, String> {
        if self.relaunch {
            self.app.relaunch(self.profile, self.seed)?;
        }
        (self.prepare)(self.app);
        overlays::grow(self.which);
        pump(100);
        let Some(view) = self.view() else {
            return Err(format!(
                "the window of the state {:?} is not open after its prepare step",
                self.label
            ));
        };
        if !self.fields_done {
            self.fields_done = true;
            let touched = overlays::touch_fields(&view, &|_| true);
            println!(
                "ui-harness:   window {}: {touched} field(s) touched",
                self.label
            );
            self.app.ui().set_toast_visible(false);
        }
        Ok(self.view().into_iter().collect())
    }

    fn views(&self) -> Vec<WindowView> {
        self.view().into_iter().collect()
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

/// One pass over `which` in the state `prepare` builds.
fn run_window(
    ctx: &mut Ctx<'_>,
    which: SecondaryWindow,
    label: &'static str,
    relaunch: bool,
    extra_deny: &'static [DenyRule],
    prepare: &mut dyn FnMut(&mut HarnessApp),
) {
    let mut config = indicatrix_smoke(format!("window: {label}"));
    config.deny = OVERLAY_DENY.to_vec();
    config.extra_deny = extra_deny.to_vec();
    config.scope_may_close = true;
    let stats = {
        let mut host = WindowHost {
            which,
            label,
            relaunch,
            app: &mut *ctx.app,
            profile: ctx.profile,
            seed: &ctx.seed,
            prepare,
            report: &mut *ctx.report,
            fields_done: false,
        };
        smoke::run(&mut host, &config)
    };
    println!(
        "ui-harness:   smoke {}: {} control(s), {} activated, {} denied, {} gone, {} closed the window, {} restore(s)",
        config.state,
        stats.enumerated,
        stats.activated,
        stats.denied,
        stats.gone,
        stats.closed,
        stats.restores
    );
    ctx.check_health(&config.state);
}

/// Presses the first clickable element of `which` labelled `label`, as the engine would.
fn press_in(which: SecondaryWindow, label: &str) -> bool {
    let Some(view) = overlays::secondary_view(which) else {
        return false;
    };
    let target = view
        .by_label(label)
        .into_iter()
        .find(|e| e.accessible_role().is_some_and(is_clickable_role));
    let Some(element) = target else {
        return false;
    };
    coverage::record_activation(&element);
    coverage::record_wrappers(&view.root, &element, false);
    element.invoke_accessible_default_action();
    pump(250);
    true
}

fn strings(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|s| SharedString::from(*s))
            .collect::<Vec<_>>(),
    ))
}

// ---------------------------------------------------------------------------------------------
// Rough Planner
// ---------------------------------------------------------------------------------------------

const DENY_PLANNER_MESH: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "remove",
    "removing an inclusion needs an imported mesh with an added inclusion",
)];

/// The library, the planner open with its materials listed; `false` when it did not open.
fn planner_ready(app: &mut HarnessApp, profile: &Profile, library: bool) -> bool {
    let seed = SettingsSeed::ExistingInstall;
    if library && let Err(message) = overlays::launch_library(app, profile, &seed) {
        println!("ui-harness:   planner state: {message}");
        if !app.is_open() && app.relaunch(profile, &seed).is_err() {
            return false;
        }
    }
    if !overlays::open_planner(app) {
        return false;
    }
    harness_support::rough_planner_window().is_some_and(|w| {
        let model = w.global::<RoughPlanModel>();
        wait_until_labeled("planner-open", Duration::from_secs(10), || {
            model.get_materials().row_count() > 0
        })
    })
}

/// A 40 x 30 x 25 mm block planned over the whole library; waits (up to 60 s) for the layouts.
fn plan_block(max_designs: i32) -> bool {
    let Some(window) = harness_support::rough_planner_window() else {
        return false;
    };
    let model = window.global::<RoughPlanModel>();
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.set_max_designs(max_designs);
    model.set_use_filter(false);
    model.invoke_base_field_committed();
    pump(250);
    model.invoke_plan();
    let _ = wait_until_labeled("planner-plan", Duration::from_secs(60), || {
        !model.get_running()
            && (model.get_results().row_count() > 0 || !model.get_error_text().is_empty())
    });
    model.get_results().row_count() > 0
}

/// Saves every result under the suggested name; answers the replace question when it comes.
fn save_plan() -> bool {
    let Some(window) = harness_support::rough_planner_window() else {
        return false;
    };
    let model = window.global::<RoughPlanModel>();
    model.invoke_begin_save(false);
    pump(150);
    if model.get_save_name().is_empty() {
        return false;
    }
    model.invoke_confirm_save();
    let _ = wait_until_labeled("planner-save", Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > 0 || model.get_confirm_overwrite_open()
    });
    if model.get_confirm_overwrite_open() {
        model.invoke_confirm_overwrite(true);
    }
    let _ = wait_until_labeled("planner-save", Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > 0
            && !model.get_confirm_overwrite_open()
            && !model.get_save_name_open()
    });
    model.get_saved_plans().row_count() > 0
}

/// What `step` of the planner states does after the block is planned.
fn planner_step(app: &mut HarnessApp, profile: &Profile, step: u8) {
    if !planner_ready(app, profile, true) || !plan_block(2) {
        return;
    }
    let Some(window) = harness_support::rough_planner_window() else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    match step {
        // The result list with one layout selected (the view toggles, Open in Editor).
        1 => {
            model.invoke_select_result(0);
            model.set_view_mode(1);
        }
        // Cuts of every kind, one undone so Redo is enabled.
        2 => {
            for kind in 0..4 {
                model.invoke_add_cut(kind);
                pump(150);
            }
            model.invoke_undo_shape();
        }
        // Planning again over results that were not saved asks whether to replace them.
        3 => model.invoke_plan(),
        // The name row of a save.
        4 => model.invoke_begin_save(false),
        // The replace-saved-plan question.
        5 => overwrite_question(&window),
        // The saved list, plain / with Rename open / with Delete asking.
        6..=8 if save_plan() => {
            model.set_saved_open(true);
            pump(250);
            if step == 7 {
                press_in(SecondaryWindow::RoughPlanner, "Rename");
            } else if step == 8 {
                press_in(SecondaryWindow::RoughPlanner, "Delete");
            }
        }
        _ => {}
    }
    pump(300);
}

/// Saves a plan, then saves again under the same name until the replace question is open.
fn overwrite_question(window: &indicatrix_cut::RoughPlannerWindow) {
    if !save_plan() {
        return;
    }
    let model = window.global::<RoughPlanModel>();
    let Some(taken) = model.get_saved_plans().row_data(0).map(|row| row.name) else {
        return;
    };
    model.invoke_begin_save(false);
    pump(150);
    model.set_save_name(taken);
    model.invoke_confirm_save();
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_confirm_overwrite_open()
    });
}

/// The mesh-like rough: the controls of an imported mesh (inclusions, the scan time limit,
/// locate), set directly because Rust pushes these flags after the mesh file dialog, which has no
/// file here. The excluded list and the weighed cylinder are states of `windows_reach.rs`.
fn planner_mesh(app: &mut HarnessApp, profile: &Profile) {
    if !planner_ready(app, profile, true) {
        return;
    }
    let Some(window) = harness_support::rough_planner_window() else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    model.set_inclusions_possible(true);
    model.set_locate_possible(true);
    model.set_inclusion_rows(strings(&["Inclusion 1: 4.0 x 4.0 x 4.0 mm, 64.0 mm3"]));
    harness_support::queue_picker_cancel();
    harness_support::queue_picker_cancel();
    pump(300);
}

/// The planner states that need a finished plan (`planner_step`), by step number.
const PLANNER_STEP_LABELS: [&str; 9] = [
    "result list",
    "layout selected",
    "cuts of every kind",
    "replace question",
    "save name row",
    "replace saved plan question",
    "saved plans",
    "saved plan, rename",
    "saved plan, delete",
];

/// The planner state that needs no plan (`planner_mesh`).
const PLANNER_MESH_LABEL: &str = "mesh-like rough";

/// The pass of `PLANNER_STEP_LABELS[index]`.
fn planner_step_pass(ctx: &mut Ctx<'_>, index: usize) {
    let profile: &Profile = ctx.profile;
    let step = u8::try_from(index).expect("the label list is far shorter than 256 entries");
    let mut prepare = |app: &mut HarnessApp| planner_step(app, profile, step);
    run_window(
        ctx,
        SecondaryWindow::RoughPlanner,
        PLANNER_STEP_LABELS[index],
        false,
        &[],
        &mut prepare,
    );
}

/// The pass of the mesh-like rough.
fn planner_mesh_pass(ctx: &mut Ctx<'_>) {
    let profile: &Profile = ctx.profile;
    let mut prepare = |app: &mut HarnessApp| planner_mesh(app, profile);
    run_window(
        ctx,
        SecondaryWindow::RoughPlanner,
        PLANNER_MESH_LABEL,
        false,
        DENY_PLANNER_MESH,
        &mut prepare,
    );
}

// ---------------------------------------------------------------------------------------------
// Rig and Locate
// ---------------------------------------------------------------------------------------------

/// Locate window from the planner, then the Rig window from Locate with one rig stored.
fn rig_prepare(app: &HarnessApp, tab: i32, ortho: bool) {
    if !overlays::open_planner(app) {
        return;
    }
    pump(300);
    if let Some(window) = harness_support::rough_planner_window() {
        window.global::<RoughPlanModel>().invoke_locate_inclusion();
    }
    if !overlays::wait_open(SecondaryWindow::Locate) {
        return;
    }
    if let Some(window) = harness_support::locate_window() {
        window.invoke_edit_rigs();
    }
    if !overlays::wait_open(SecondaryWindow::Rig) {
        return;
    }
    let Some(rig) = harness_support::rig_window() else {
        return;
    };
    rig.invoke_new_rig();
    pump(200);
    rig.set_name_text("Harness rig".into());
    rig.invoke_save_rig();
    let _ = wait_until(Duration::from_secs(10), || {
        rig.get_rig_names().row_count() > 0
    });
    rig.set_tab(tab);
    rig.set_gen_ortho(ortho);
    harness_support::queue_picker_cancel();
    harness_support::queue_picker_cancel();
    pump(250);
}

/// The Rig / Locate passes: label, window, Rig tab, parallel projection.
const RIG_PASSES: [(&str, SecondaryWindow, i32, bool); 4] = [
    ("rig tab, one rig", SecondaryWindow::Rig, 0, false),
    (
        "rig tab, parallel projection",
        SecondaryWindow::Rig,
        0,
        true,
    ),
    ("calibrate tab", SecondaryWindow::Rig, 1, false),
    ("one rig stored", SecondaryWindow::Locate, 0, false),
];

/// The pass of `RIG_PASSES[index]`.
fn rig_pass(ctx: &mut Ctx<'_>, index: usize) {
    let (label, which, tab, ortho) = RIG_PASSES[index];
    let mut prepare = |app: &mut HarnessApp| rig_prepare(app, tab, ortho);
    run_window(ctx, which, label, true, &[], &mut prepare);
}

// ---------------------------------------------------------------------------------------------
// Compare
// ---------------------------------------------------------------------------------------------

fn compare_split(ctx: &mut Ctx<'_>) {
    let profile: &Profile = ctx.profile;
    let none: &'static [DenyRule] = &[];
    let mut prepare = |app: &mut HarnessApp| {
        let seed = SettingsSeed::ExistingInstall;
        if overlays::launch_library(app, profile, &seed).is_err() {
            return;
        }
        let ui = app.ui().clone_strong();
        let editor = ui.global::<indicatrix_cut::EditorModel>();
        editor.invoke_load_selected();
        if !wait_until(Duration::from_secs(10), || editor.get_has_design()) {
            return;
        }
        // Waits for the snapshot to be held first: the design just loaded is snapshotted only
        // when its background solve lands.
        if let Err(message) = setups::snapshot_and_open_compare(&ui) {
            println!("ui-harness:   compare split state: {message}");
            return;
        }
        if let Some(window) = harness_support::compare_window() {
            let compare = window.global::<CompareModel>();
            // The background solve lands later and rewrites `can_keep`, the figures and the tilt
            // flags; the properties below are set only after its figures arrived, or they would be
            // overwritten (the strip, hence the tilt button, shows only with figure rows).
            let landed = wait_until_labeled("compare-solved", Duration::from_secs(20), || {
                compare.get_metric_rows().row_count() > 0
            });
            if !landed {
                compare.set_metric_rows(ModelRc::new(VecModel::from(vec![CompareMetricRow {
                    label: "Windowing".into(),
                    before: "1.0".into(),
                    after: "1.0".into(),
                    change: "0.0".into(),
                    verdict: "about the same".into(),
                    tone: 0,
                }])));
            }
            // A snapshot comparison offers only Close; Retarget and Optimize also offer Discard
            // and Keep After, which these two properties show and enable.
            compare.set_show_keep_discard(true);
            compare.set_can_keep(true);
            compare.set_tilt_done(false);
            compare.set_mode_index(1);
            compare.set_reference_options(strings(&["Current design", "Harness variant"]));
            compare.set_reference_available(true);
            compare.set_tilt_available(true);
            compare.set_tilt_running(true);
            compare.set_tilt_progress(0.4);
        }
        pump(300);
    };
    run_window(
        ctx,
        SecondaryWindow::Compare,
        "split view, reference choice, tilt running",
        false,
        none,
        &mut prepare,
    );
}

// ---------------------------------------------------------------------------------------------
// Rough Colour wizard
// ---------------------------------------------------------------------------------------------

const DENY_ROUGH_COLOUR: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "prepare photos",
        "long job: loads and aligns eight backlit photos",
    ),
    deny(
        DenyMatch::Contains,
        "start fit",
        "long job: traces light through the stone for every view",
    ),
    deny(
        DenyMatch::Contains,
        "refine geometry",
        "long job: repeats the fit",
    ),
    deny(
        DenyMatch::Contains,
        "fit zone",
        "long job: fits a primitive to the marks",
    ),
    deny(
        DenyMatch::Contains,
        "suggest from the photos",
        "long job: looks for color boundaries in the photos",
    ),
    deny(
        DenyMatch::Contains,
        "export report",
        "writes a report and pictures to a folder",
    ),
];

/// The wizard opened from the planner, put on `step` with the lists a finished earlier step would
/// have filled (rigs, cameras, backlights, shapes); step 5 also adds a zone so the zone list and
/// its parameters exist. `busy` shows the Cancel pill.
fn rough_colour_prepare(app: &HarnessApp, step: i32, busy: bool) {
    use indicatrix_cut::{ViewRow, Zoning};
    if !overlays::open_planner(app) {
        return;
    }
    pump(300);
    if let Some(window) = harness_support::rough_planner_window() {
        window.global::<Zoning>().invoke_open_rough_colour();
    }
    if !overlays::wait_open(SecondaryWindow::RoughColor) {
        return;
    }
    let Some(w) = harness_support::rough_color_window() else {
        return;
    };
    w.set_step_titles(strings(&[
        "Checklist",
        "Photos",
        "Calibration",
        "Masks",
        "Surfaces",
        "Fit",
        "Compare",
        "Accept",
    ]));
    w.set_step_ok(ModelRc::new(VecModel::from(vec![true; 8])));
    w.set_rig_names(strings(&["Harness rig"]));
    w.set_camera_names(strings(&["Harness camera"]));
    w.set_backlight_names(strings(&["Harness panel"]));
    w.set_led_names(strings(&["White LED"]));
    w.set_host_names(strings(&["Unknown", "Quartz"]));
    w.set_show_host_picker(true);
    w.set_shape_names(strings(&["Plane", "Sphere", "Box", "Prism"]));
    w.set_view_rows(ModelRc::new(VecModel::from(vec![
        ViewRow {
            name: "+X".into(),
            status: "found".into(),
            active: true,
        },
        ViewRow {
            name: "-X".into(),
            status: "found".into(),
            active: false,
        },
    ])));
    harness_support::queue_picker_cancel();
    harness_support::queue_picker_cancel();
    // `add_zone` ends in `push_steps`, which writes the wizard's own step back (0): the zone is
    // added before the step is set.
    if step == 5 {
        w.invoke_add_zone(0);
        pump(400);
    }
    w.set_step(step);
    if busy {
        w.set_busy(true);
        w.set_busy_text("Working...".into());
    }
    pump(300);
}

/// The eight wizard steps, then the busy state (the last entry).
const ROUGH_COLOUR_LABELS: [&str; 9] = [
    "step 1, checklist",
    "step 2, photos",
    "step 3, calibration",
    "step 4, masks",
    "step 5, surfaces",
    "step 6, fit with a zone",
    "step 7, compare",
    "step 8, accept",
    "a step running",
];

/// The pass of `ROUGH_COLOUR_LABELS[index]`.
fn rough_colour_pass(ctx: &mut Ctx<'_>, index: usize) {
    let busy = index >= 8;
    let step = if busy {
        0
    } else {
        i32::try_from(index).expect("the label list is far shorter than i32::MAX entries")
    };
    let mut prepare = |app: &mut HarnessApp| rough_colour_prepare(app, step, busy);
    run_window(
        ctx,
        SecondaryWindow::RoughColor,
        ROUGH_COLOUR_LABELS[index],
        true,
        DENY_ROUGH_COLOUR,
        &mut prepare,
    );
}
