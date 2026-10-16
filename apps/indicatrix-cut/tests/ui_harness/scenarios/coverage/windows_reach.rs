//! Secondary-window states for the elements run 7 still left uncovered in the Rough Planner and
//! the Rough Colour wizard (lane E of the 2026-10-15 findings).
//!
//! Proves: in each state every enabled control of the window is pressed once and every field
//! touched (the overlay engine with `OVERLAY_DENY`), without a panic, an unexpected file dialog
//! or an error toast.
//!
//! What kept the elements unreached, by state:
//!
//! * the planner's excluded list was opened before Rust had pushed the exclusion, and the losses
//!   fields sit in a disclosure no state opened;
//! * "Save Selected" needs a ticked result, "Re-Plan" an opened saved plan, the pose pills and
//!   "Use Color" a stored rough color (`Zoning.colour_available`), "Fit to Weight" the weight
//!   check the shape worker pushes after the weighed carat is committed;
//! * the wizard's zone list: `add_zone` ends in `push_steps`, which writes the wizard's own step
//!   back (0), so adding the zone after `set_step(5)` threw the Fit page away; the camera file,
//!   LED and frosted-skin controls need their combo or pill choice made.

use super::{
    super::{Piece, existing_install},
    overlay_states::OVERLAY_DENY,
    overlays,
};
use crate::support::{
    Ctx, HarnessApp, Profile, SecondaryWindow, SettingsSeed, SweepOptions, WindowView,
    app::{pump, wait_until},
    smoke::{self, DenyRule, SmokeHost, indicatrix_smoke},
    sweep_options,
};
use indicatrix_cut::{LibraryModel, RoughPlanModel, harness_support};
use slint::{ComponentHandle, Model};
use slint_harness::{backend::wait_until_labeled, report::Report};
use std::time::Duration;

/// The pieces of this file: one `window:reach-*:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    let mut all = Vec::new();
    for (index, label) in PLANNER_STATES.iter().enumerate() {
        all.push(Piece::new(
            format!("window:reach-planner:{label}"),
            existing_install,
            move |ctx| planner_pass(ctx, index),
        ));
    }
    for (index, (label, _)) in ROUGH_COLOUR_STATES.iter().enumerate() {
        all.push(Piece::new(
            format!("window:reach-rough-colour:{label}"),
            existing_install,
            move |ctx| rough_colour_pass(ctx, index),
        ));
    }
    all
}

// ---------------------------------------------------------------------------------------------
// The host (the overlay deny list over one secondary window)
// ---------------------------------------------------------------------------------------------

/// [`SmokeHost`] for one secondary window: restore = relaunch + `prepare`; the window must be
/// open afterwards. Its fields are touched once, on the first restore. `relaunch` is false when
/// `prepare` shuts the app down and launches it again itself (the planner states, through
/// `overlays::launch_library`): a relaunch before it would launch the app twice per restore.
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

// ---------------------------------------------------------------------------------------------
// Rough Planner
// ---------------------------------------------------------------------------------------------

/// The planner states, by step number (see [`planner_prepare`]).
const PLANNER_STATES: [&str; 4] = [
    "excluded designs and losses open",
    "a kept result with a stored rough color",
    "a saved plan opened",
    "cylinder weighed",
];

/// A 40 x 30 x 25 mm block planned over the whole library; waits (up to 60 s) for the layouts.
fn plan_block(model: &RoughPlanModel<'_>) -> bool {
    model.invoke_set_base(0);
    model.set_rough_x("40".into());
    model.set_rough_y("30".into());
    model.set_rough_z("25".into());
    model.set_max_designs(2);
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
fn save_plan(model: &RoughPlanModel<'_>) -> bool {
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
    wait_until_labeled("planner-save", Duration::from_secs(15), || {
        model.get_saved_plans().row_count() > 0 && !model.get_save_name_open()
    })
}

/// The library, then the planner, then what `step` needs:
///
/// * 0: the first design excluded from planning through the library (its "Exclude from
///   Planner"), the planner's Excluded list and Losses disclosure open once Rust listed it;
/// * 1: a plan with result 0 selected and ticked, and a stored rough color;
/// * 2: a plan saved and opened again from the saved list (the "Re-Plan" banner);
/// * 3: a cylinder with a weighed carat, once the shape worker pushed its weight check.
fn planner_prepare(app: &mut HarnessApp, profile: &Profile, step: u8) {
    let seed = SettingsSeed::ExistingInstall;
    if let Err(message) = overlays::launch_library(app, profile, &seed) {
        println!("ui-harness:   planner state: {message}");
        if !app.is_open() && app.relaunch(profile, &seed).is_err() {
            return;
        }
    }
    if step == 0 {
        let library = app.ui().global::<LibraryModel>();
        if let Some(row) = library.get_diagram_list().row_data(0) {
            library.invoke_toggle_planner_excluded(row.id);
            let _ = wait_until(Duration::from_secs(8), || {
                library
                    .get_diagram_list()
                    .row_data(0)
                    .is_some_and(|row| row.planner_excluded)
            });
        }
    }
    if !overlays::open_planner(app) {
        return;
    }
    let Some(window) = harness_support::rough_planner_window() else {
        return;
    };
    let model = window.global::<RoughPlanModel>();
    let _ = wait_until_labeled("planner-open", Duration::from_secs(10), || {
        model.get_materials().row_count() > 0
    });
    match step {
        0 => {
            let _ = wait_until(Duration::from_secs(10), || model.get_excluded_count() > 0);
            model.set_excluded_open(true);
            model.set_losses_open(true);
        }
        1 if plan_block(&model) => {
            model.invoke_select_result(0);
            model.invoke_toggle_keep(0);
            let _ = wait_until(Duration::from_secs(5), || model.get_keep_count() > 0);
            window
                .global::<indicatrix_cut::Zoning>()
                .set_colour_available(true);
        }
        2 if plan_block(&model) && save_plan(&model) => {
            // `open_saved` takes the plan's database id, not its row: id 0 matches no plan and
            // the banner (and its "Re-Plan" pill) never came.
            if let Some(row) = model.get_saved_plans().row_data(0) {
                model.invoke_open_saved(row.id);
                let _ = wait_until(Duration::from_secs(15), || {
                    !model.get_loaded_banner().is_empty()
                });
            }
        }
        3 => {
            model.invoke_set_base(1);
            model.set_cyl_diameter("12".into());
            model.set_cyl_length("20".into());
            model.set_weighed_ct("2.5".into());
            model.invoke_base_field_committed();
            let _ = wait_until(Duration::from_secs(10), || {
                model.get_weight_check_level() != 0
            });
        }
        _ => {}
    }
    pump(300);
}

/// The pass of `PLANNER_STATES[index]`.
fn planner_pass(ctx: &mut Ctx<'_>, index: usize) {
    let profile: &Profile = ctx.profile;
    let step = u8::try_from(index).expect("the state list is far shorter than 256 entries");
    let mut prepare = |app: &mut HarnessApp| planner_prepare(app, profile, step);
    run_window(
        ctx,
        SecondaryWindow::RoughPlanner,
        PLANNER_STATES[index],
        false,
        &[],
        &mut prepare,
    );
}

// ---------------------------------------------------------------------------------------------
// Rough Colour wizard
// ---------------------------------------------------------------------------------------------

use super::overlay_states::deny;
use crate::support::smoke::DenyMatch;

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

/// The wizard states: label and wizard step (0-based).
const ROUGH_COLOUR_STATES: [(&str, i32); 3] = [
    ("calibration, camera file and LED backlight", 2),
    ("surfaces, frosted skin", 4),
    ("fit, a zone selected", 5),
];

/// The wizard opened from the planner on `step`, with the lists an earlier step would have
/// filled, and the choice that shows the step's hidden controls: a camera with a curves file and
/// an LED backlight (step 2), a frosted skin with a fixed roughness (step 4), a zone added and the
/// prism primitive picked (step 5). The zone is added BEFORE the step is set: `add_zone` ends in
/// `push_steps`, which writes the wizard's own step back.
fn rough_colour_prepare(app: &HarnessApp, step: i32) {
    use indicatrix_cut::{ViewRow, Zoning};
    use slint::{ModelRc, SharedString, VecModel};
    let strings = |items: &[&str]| -> ModelRc<SharedString> {
        ModelRc::new(VecModel::from(
            items
                .iter()
                .map(|item| SharedString::from(*item))
                .collect::<Vec<_>>(),
        ))
    };
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
    if step == 5 {
        w.invoke_add_zone(0);
        pump(400);
    }
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
    w.set_camera_names(strings(&["Generic camera", "Harness camera"]));
    w.set_backlight_names(strings(&["Harness panel", "LED", "Measured"]));
    w.set_led_names(strings(&["White LED"]));
    w.set_host_names(strings(&["Unknown", "Quartz"]));
    w.set_show_host_picker(true);
    w.set_shape_names(strings(&["Plane", "Sphere", "Box", "Prism"]));
    w.set_view_rows(ModelRc::new(VecModel::from(vec![ViewRow {
        name: "+X".into(),
        status: "found".into(),
        active: true,
    }])));
    harness_support::queue_picker_cancel();
    harness_support::queue_picker_cancel();
    w.set_step(step);
    match step {
        2 => {
            w.set_camera_index(1);
            w.set_backlight_index(1);
        }
        4 => w.set_surface_index(1),
        5 => w.set_mark_kind_index(3),
        _ => {}
    }
    pump(300);
}

/// The pass of `ROUGH_COLOUR_STATES[index]`.
fn rough_colour_pass(ctx: &mut Ctx<'_>, index: usize) {
    let (label, step) = ROUGH_COLOUR_STATES[index];
    let mut prepare = |app: &mut HarnessApp| rough_colour_prepare(app, step);
    run_window(
        ctx,
        SecondaryWindow::RoughColor,
        label,
        true,
        DENY_ROUGH_COLOUR,
        &mut prepare,
    );
}
