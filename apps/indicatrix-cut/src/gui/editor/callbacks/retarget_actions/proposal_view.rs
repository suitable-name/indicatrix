//! The `push_*` renderers that turn the Slint-free retarget proposal view model
//! (`indicatrix_editor::retarget::view`, shared with the web app and re-exported
//! here at its old paths) into `MainWindow`'s `editor_retarget_*`/`RetargetModel`
//! properties -- see this group's own `mod.rs` doc comment ("Slint-free view-model
//! split").

use crate::{MainWindow, RetargetModel, RetargetRowItem};
use indicatrix_cut_core::{MaterialSelection, ResolvedMaterial};
use slint::{Color, ComponentHandle, ModelRc, SharedString, VecModel};

pub(super) use indicatrix_editor::retarget::view::{
    RetargetRowView, RetargetView, retarget_view, row_view,
};

/// Pushes [`RetargetView`] into `MainWindow`'s `editor_retarget_rows`/`_notes`/
/// `_anchored_errors`/`_solve_error` -- the only place a `RetargetRowView` becomes
/// a real `RetargetRowItem`.
pub(super) fn push_retarget_view(ui: &MainWindow, view: RetargetView) {
    let rows: Vec<RetargetRowItem> = view
        .rows
        .into_iter()
        .map(|r| RetargetRowItem {
            tier_index: r.tier_index as i32,
            block: r.block.into(),
            name: r.name.into(),
            old_angle: r.old_angle.into(),
            new_angle: r.new_angle.into(),
            margin: r.margin.into(),
            risk_label: r.risk_label.into(),
            risk_color: Color::from_rgb_u8(r.risk_rgb.0, r.risk_rgb.1, r.risk_rgb.2),
        })
        .collect();
    ui.global::<RetargetModel>()
        .set_rows(ModelRc::new(VecModel::from(rows)));
    ui.global::<RetargetModel>()
        .set_notes(ModelRc::new(VecModel::from(
            view.notes
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_anchored_tier_errors(ModelRc::new(VecModel::from(
            view.anchored_errors
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    ui.global::<RetargetModel>()
        .set_solve_error(view.solve_error.into());
}

/// Pushes the target material readout -- fixed for the lifetime of one dialog
/// session, but re-pushed on every rebuild anyway since it costs nothing. Also clears
/// `RetargetModel.target_material_error` (see [`push_target_error`]): reaching this
/// function at all means `material::resolve_target_selection` just succeeded, so any
/// earlier parse error no longer applies.
pub(super) fn push_target_readout(
    ui: &MainWindow,
    selection: &MaterialSelection,
    target: &ResolvedMaterial,
) {
    ui.global::<RetargetModel>()
        .set_target_material_name(super::material::target_display_name(selection).into());
    ui.global::<RetargetModel>()
        .set_target_material_ri(format!("{:.4}", target.n_d).into());
    ui.global::<RetargetModel>()
        .set_target_critical_angle(format!("{:.2}\u{b0}", target.critical_angle_deg).into());
    ui.global::<RetargetModel>()
        .set_target_material_error("".into());
}

/// `ri_override_text` failed to parse -- pushes `message` into
/// `RetargetModel.target_material_error` (shown above the proposal table,
/// `retarget_dialog.slint`) and clears `rows`/`notes` so nothing on screen implies a
/// proposal was actually built against this text. The readout above the error
/// (`target_material_name`/`_ri`/`_critical_angle`) is deliberately left as-is --
/// whatever the last valid target was -- rather than reset, matching the error
/// banner's own "showing the last valid target" wording.
pub(super) fn push_target_error(ui: &MainWindow, message: &str) {
    ui.global::<RetargetModel>()
        .set_target_material_error(message.into());
    push_retarget_view(
        ui,
        RetargetView {
            rows: Vec::new(),
            notes: Vec::new(),
            anchored_errors: Vec::new(),
            solve_error: String::new(),
        },
    );
}
