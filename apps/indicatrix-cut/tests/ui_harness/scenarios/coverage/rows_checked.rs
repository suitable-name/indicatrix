//! Hand-written rows of the direct-invoke table: callbacks called with VALID arguments and an
//! asserted effect, and the cancel callbacks called with nothing running.
//!
//! A checked row replaces the generated row with the same key (`all_rows` in `coverage.rs`). The
//! generated rows pass the empty value of each parameter type, which only reaches a handler's
//! validation ("... is not a number"); these reach the success path. A check that fails is a
//! `coverage-effect` finding, the callback still counts as invoked.
//!
//! The cancel callbacks have nothing to cancel in a direct-invoke state, so their effect is the
//! absence of one: the design state (tiers, dirty flag, undo and redo availability, solve flag)
//! is the same before and after.

use super::{
    super::dialogs::design_signature,
    table::{Invoke, Row, Setup},
};
use crate::support::app::{pump, wait_until};
use indicatrix_cut::*;
use slint::{ComponentHandle, Model};
use std::time::Duration;

// ---------------------------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------------------------

/// The text of an error or warning toast that is showing, if one is.
fn toast_error(ui: &MainWindow) -> Option<String> {
    if !ui.get_toast_visible() {
        return None;
    }
    let kind = ui.get_toast_type().to_string();
    (kind == "error" || kind == "warning").then(|| ui.get_toast_message().to_string())
}

/// A toast that says an argument could not be read ("is not a number", "is empty", "must be a
/// positive whole number"): what a row with valid arguments must never raise.
fn argument_complaint(ui: &MainWindow) -> Option<String> {
    toast_error(ui).filter(|message| {
        let lower = message.to_lowercase();
        lower.contains("not a number") || lower.contains("is empty") || lower.contains("must be")
    })
}

/// Hides the toast, so the row's own toast is the only one left to read.
fn clear_toast(ui: &MainWindow) {
    ui.set_toast_visible(false);
    pump(20);
}

/// `Err` naming `what` when the handler answered with an argument complaint.
fn no_complaint(ui: &MainWindow, what: &str) -> Result<(), String> {
    argument_complaint(ui).map_or(Ok(()), |message| {
        Err(format!("{what} was refused as unreadable: {message}"))
    })
}

/// The editor's tier count.
fn tier_count(ui: &MainWindow) -> usize {
    ui.global::<EditorModel>().get_tiers().row_count()
}

/// Tier 0 as the table shows it: its angle as a number and its indices joined with commas.
fn first_tier(ui: &MainWindow) -> Result<(f64, String), String> {
    let tier = ui
        .global::<EditorModel>()
        .get_tiers()
        .row_data(0)
        .ok_or("the editor lists no tier")?;
    let digits: String = tier
        .angle_full
        .chars()
        .filter(|c| c.is_ascii_digit() || *c == '.' || *c == '-')
        .collect();
    let angle = digits
        .parse::<f64>()
        .map_err(|_| format!("tier 0 angle {:?} does not parse", tier.angle_full))?;
    let indices = tier
        .indices_full
        .replace(',', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(",");
    Ok((angle, indices))
}

/// A one-line fingerprint of the design state a cancel with nothing running must not touch.
fn design_state(ui: &MainWindow) -> String {
    let editor = ui.global::<EditorModel>();
    format!(
        "{} | dirty={} undo={} redo={} solving={}",
        design_signature(ui),
        editor.get_is_dirty(),
        editor.get_can_undo(),
        editor.get_can_redo(),
        editor.get_solve_running()
    )
}

/// Runs `act` and expects the design state to be the same afterwards.
fn leaves_state_alone(ui: &MainWindow, act: impl FnOnce()) -> Result<(), String> {
    let before = design_state(ui);
    act();
    pump(80);
    let after = design_state(ui);
    if before == after {
        Ok(())
    } else {
        Err(format!(
            "a cancel with nothing running changed the design state: {before} -> {after}"
        ))
    }
}

/// The library entry the Library and Editor groups selected.
fn selected_entry(ui: &MainWindow) -> i32 {
    ui.global::<LibraryModel>().get_selected_entry_id()
}

// ---------------------------------------------------------------------------------------------
// Startup group
// ---------------------------------------------------------------------------------------------

/// A 1x1 Radiance picture (`.hdr`), the smallest file the environment-map loader reads.
const TINY_HDR: &[u8] = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 1\n\x80\x80\x80\x81";

/// Loads a real (1x1) environment map from the profile's working directory: no error toast.
fn load_env_map(ui: &MainWindow) -> Result<(), String> {
    let dir = std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?;
    let path = dir.join("harness_environment.hdr");
    std::fs::write(&path, TINY_HDR).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    clear_toast(ui);
    let text = path.to_string_lossy().into_owned();
    ui.global::<SettingsModel>()
        .invoke_load_env_map(text.as_str().into());
    pump(200);
    toast_error(ui).map_or(Ok(()), |message| {
        Err(format!("loading a valid 1x1 HDR file raised: {message}"))
    })
}

/// The Empty form's Create with a readable form: the editor holds a design afterwards.
fn new_design_create(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    ui.global::<EditorModel>().invoke_new_design_create(
        1,
        "1.5".into(),
        "1.0".into(),
        "1.5".into(),
        0,
        "96".into(),
        "8".into(),
        true,
        0,
        0,
    );
    if wait_until(Duration::from_secs(15), || {
        ui.global::<EditorModel>().get_has_design()
    }) {
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!(
            "no design was created from a readable form ({toast})"
        ))
    }
}

const STARTUP_CHECKED: &[Row] = checked_rows!(Startup, true;
    SettingsModel.load_env_map => load_env_map;
    EditorModel.new_design_create => new_design_create;
);

// ---------------------------------------------------------------------------------------------
// Library group
// ---------------------------------------------------------------------------------------------

/// Toggles the "ignored" flag of the selected entry: no error toast (the generated row used the
/// id 1, which the seeded library need not hold).
fn toggle_ignored(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    let id = selected_entry(ui);
    ui.global::<LibraryModel>().invoke_toggle_ignored(id);
    pump(300);
    toast_error(ui).map_or(Ok(()), |message| {
        Err(format!("toggling entry {id} raised: {message}"))
    })
}

/// Excludes the selected entry from the rough planner: no error toast.
fn toggle_planner_excluded(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    let id = selected_entry(ui);
    ui.global::<LibraryModel>()
        .invoke_toggle_planner_excluded(id);
    pump(300);
    toast_error(ui).map_or(Ok(()), |message| {
        Err(format!("excluding entry {id} raised: {message}"))
    })
}

const LIBRARY_CHECKED: &[Row] = checked_rows!(Library, true;
    LibraryModel.toggle_ignored => toggle_ignored;
    LibraryModel.toggle_planner_excluded => toggle_planner_excluded;
);

// ---------------------------------------------------------------------------------------------
// Editor group
// ---------------------------------------------------------------------------------------------

/// The inline angle box with a new angle for tier 0: the design changes.
fn inline_set_angle(ui: &MainWindow) -> Result<(), String> {
    let (angle, _) = first_tier(ui)?;
    clear_toast(ui);
    let before = design_signature(ui);
    ui.global::<EditorModel>()
        .invoke_inline_set_angle(0, format!("{:.3}", angle + 0.25).into());
    if wait_until(Duration::from_secs(5), || design_signature(ui) != before) {
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!(
            "a new angle for tier 0 left the design unchanged ({toast})"
        ))
    }
}

/// Generates three more tiers from tier 0's indices: the tier count grows.
fn generate_step_series(ui: &MainWindow) -> Result<(), String> {
    let (angle, indices) = first_tier(ui)?;
    clear_toast(ui);
    let before = tier_count(ui);
    ui.global::<EditorModel>().invoke_generate_step_series(
        "H".into(),
        format!("{:.2}", angle + 3.0).into(),
        "1.5".into(),
        3,
        indices.as_str().into(),
        "".into(),
    );
    if wait_until(Duration::from_secs(5), || tier_count(ui) > before) {
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!(
            "generating 3 tiers from indices {indices:?} left {before} tier(s) ({toast})"
        ))
    }
}

/// The same with "Keep linked": the tier count grows.
fn generate_step_series_linked(ui: &MainWindow) -> Result<(), String> {
    let (angle, indices) = first_tier(ui)?;
    clear_toast(ui);
    let before = tier_count(ui);
    ui.global::<RelationModel>()
        .invoke_generate_step_series_linked(
            "L".into(),
            format!("{:.2}", angle + 3.0).into(),
            "1.5".into(),
            3,
            indices.as_str().into(),
            "".into(),
        );
    if wait_until(Duration::from_secs(5), || tier_count(ui) > before) {
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!(
            "generating 3 linked tiers from indices {indices:?} left {before} tier(s) ({toast})"
        ))
    }
}

/// The Preform form applied as it stands: no unreadable-argument complaint.
fn apply_preform(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    clear_toast(ui);
    editor.invoke_apply_preform(
        editor.get_preform_shape_index(),
        editor.get_preform_half_width(),
        editor.get_preform_length_over_width(),
        editor.get_preform_depth(),
    );
    pump(300);
    no_complaint(ui, "the Preform form as it stands")
}

/// The Y-offset box with "0.00": no unreadable-argument complaint (a refusal because the girdle
/// has not anchored the scale is a different toast and not the point here).
fn apply_preform_y_offset(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    ui.global::<EditorModel>()
        .invoke_apply_preform_y_offset("0.00".into());
    pump(300);
    no_complaint(ui, "a Y-offset of 0.00")
}

/// The symmetry form as it stands: no unreadable-argument complaint.
fn apply_symmetry(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    let text = editor.get_symmetry_order_text();
    let text = if text.trim().is_empty() {
        "8".into()
    } else {
        text
    };
    clear_toast(ui);
    editor.invoke_apply_symmetry(text, editor.get_mirror());
    pump(300);
    no_complaint(ui, "the symmetry form as it stands")
}

/// The design-details form with a new title: the design's title follows.
fn apply_design_meta(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    clear_toast(ui);
    editor.invoke_apply_design_meta(
        "Harness Title".into(),
        editor.get_design_extra_headers(),
        editor.get_design_footnotes(),
        editor.get_design_gear_reference_angle(),
    );
    if wait_until(Duration::from_secs(5), || {
        editor.get_design_title().as_str() == "Harness Title"
    }) {
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!(
            "the design title is {:?} after applying \"Harness Title\" ({toast})",
            editor.get_design_title()
        ))
    }
}

/// The "+ Add" facet box on tier 0: some index position is accepted (the call answers `true`).
fn add_facet(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    let table = ui.global::<TierTableModel>();
    for position in 1..=12_u32 {
        if table.invoke_add_facet(0, position.to_string().into()) {
            pump(200);
            return Ok(());
        }
    }
    let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
    Err(format!(
        "no position 1..12 was accepted as a new facet of tier 0 ({toast})"
    ))
}

/// The multi-select Offset box with "0.5": the call answers `true`.
fn offset_selected(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    if ui
        .global::<TierTableModel>()
        .invoke_offset_selected(0, "0.5".into())
    {
        pump(200);
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!("an offset of 0.5 was refused ({toast})"))
    }
}

/// The Rotate button with one tooth: the call answers `true`.
fn rotate_tier(ui: &MainWindow) -> Result<(), String> {
    clear_toast(ui);
    if ui
        .global::<TierTableModel>()
        .invoke_rotate_tier(0, "1".into(), 1)
    {
        pump(200);
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!("rotating tier 0 by 1 tooth was refused ({toast})"))
    }
}

/// Removes the first index-wheel member of tier 0 (a position the tier really has): no
/// "out of range" refusal.
fn facet_remove(ui: &MainWindow) -> Result<(), String> {
    let (_, indices) = first_tier(ui)?;
    let position: f32 = indices
        .split(',')
        .next()
        .and_then(|first| first.trim().parse().ok())
        .ok_or_else(|| format!("tier 0 indices {indices:?} hold no position"))?;
    clear_toast(ui);
    ui.global::<EditorModel>().invoke_facet_remove(0, position);
    pump(300);
    match toast_error(ui) {
        Some(message) if message.contains("out of range") => Err(format!(
            "removing position {position} of tier 0 was refused: {message}"
        )),
        _ => Ok(()),
    }
}

/// "Add to Queue" with a 1920 x 1080, 64-sample still: the queue lists one more job (nothing
/// starts it). The handler first needs an export folder (a folder dialog while none is
/// configured: `render_export::wiring::resolve_export_dir_then`), so the row answers that dialog
/// with a folder in the profile's working directory instead of the generic cancels, and then
/// waits for the finished stone (prepared on a worker thread) to arrive.
fn add_still_jobs(ui: &MainWindow) -> Result<(), String> {
    let jobs = ui.global::<RenderJobsModel>();
    let before = jobs.get_rows().row_count();
    let dir = std::env::current_dir()
        .map_err(|e| format!("no working directory: {e}"))?
        .join("harness_queue_exports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    clear_toast(ui);
    harness_support::clear_picker_answers();
    harness_support::queue_picker_path(dir);
    jobs.invoke_add_still_jobs(1920, 1080, 64, 0, 0, 2);
    if wait_until(Duration::from_secs(20), || {
        jobs.get_rows().row_count() > before
    }) {
        Ok(())
    } else {
        let toast = toast_error(ui).unwrap_or_else(|| "no toast".to_owned());
        Err(format!("the queue still lists {before} job(s) ({toast})"))
    }
}

/// The pure name check behind the Table / Girdle / Culet buttons of the tier toolbar: a name
/// that a tier of the design carries is taken (any letter case), an unused name and the empty
/// name are not.
fn tier_name_taken(ui: &MainWindow) -> Result<(), String> {
    let editor = ui.global::<EditorModel>();
    let tiers = editor.get_tiers();
    let named = tiers
        .iter()
        .map(|tier| tier.name.to_string())
        .find(|name| !name.trim().is_empty() && !name.contains('/'));
    let unused = "No Such Tier Name 7";
    if editor.invoke_tier_name_taken(tiers.clone(), unused.into()) {
        return Err(format!("{unused:?} was reported as taken"));
    }
    if editor.invoke_tier_name_taken(tiers.clone(), "".into()) {
        return Err("the empty name was reported as taken".to_owned());
    }
    let Some(name) = named else {
        // The template design names none of its tiers: only the negative answers can be checked.
        return Ok(());
    };
    for spelling in [name.clone(), name.to_uppercase(), name.to_lowercase()] {
        if !editor.invoke_tier_name_taken(tiers.clone(), spelling.as_str().into()) {
            return Err(format!(
                "tier name {name:?} was reported as free when asked as {spelling:?}"
            ));
        }
    }
    Ok(())
}

// The cancel callbacks: nothing is running, so nothing may change.

fn solve_cancel(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || ui.global::<EditorModel>().invoke_solve_cancel())
}

fn cancel_export(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || ui.global::<ExportModel>().invoke_cancel_export())
}

fn drag_cancel(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || ui.global::<ManipulateModel>().invoke_drag_cancel())
}

fn slice_cancel(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || ui.global::<ManipulateModel>().invoke_slice_cancel())
}

fn sweep_cancel(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || ui.global::<SweepModel>().invoke_cancel())
}

fn cancel_video_export(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || {
        ui.global::<TiltVideoExportModel>()
            .invoke_cancel_video_export();
    })
}

fn cancel_optimize(ui: &MainWindow) -> Result<(), String> {
    leaves_state_alone(ui, || ui.global::<RetargetModel>().invoke_cancel_optimize())
}

const EDITOR_CHECKED: &[Row] = checked_rows!(Editor, true;
    EditorModel.inline_set_angle => inline_set_angle;
    EditorModel.generate_step_series => generate_step_series;
    RelationModel.generate_step_series_linked => generate_step_series_linked;
    EditorModel.apply_preform => apply_preform;
    EditorModel.apply_preform_y_offset => apply_preform_y_offset;
    EditorModel.apply_symmetry => apply_symmetry;
    EditorModel.apply_design_meta => apply_design_meta;
    RenderJobsModel.add_still_jobs => add_still_jobs;
    EditorModel.tier_name_taken => tier_name_taken;
    TierTableModel.add_facet => add_facet;
    TierTableModel.offset_selected => offset_selected;
    TierTableModel.rotate_tier => rotate_tier;
    EditorModel.facet_remove => facet_remove;
    EditorModel.solve_cancel => solve_cancel;
    ExportModel.cancel_export => cancel_export;
    ManipulateModel.drag_cancel => drag_cancel;
    ManipulateModel.slice_cancel => slice_cancel;
    SweepModel.cancel => sweep_cancel;
    TiltVideoExportModel.cancel_video_export => cancel_video_export;
    RetargetModel.cancel_optimize => cancel_optimize;
);

/// Every checked row, in run order.
pub fn rows() -> Vec<&'static Row> {
    [STARTUP_CHECKED, LIBRARY_CHECKED, EDITOR_CHECKED]
        .into_iter()
        .flatten()
        .collect()
}
