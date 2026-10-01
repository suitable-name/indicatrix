//! Pushing the Retarget dialog's readouts (the desktop's
//! `retarget_actions::proposal_view`): the proposal rows and notes, the errors that stand
//! in for them, and the target line.

use crate::{RetargetModel, RetargetRow};
use indicatrix_cut_core::{MaterialSelection, ResolvedMaterial};
use indicatrix_editor::retarget::view::{RetargetView, target_display_name};
use slint::{Color, ModelRc, SharedString, VecModel};

/// Shows `view`: its rows and notes, or the error that replaced them. Clears whatever
/// target error an earlier push left.
pub(super) fn push_view(model: &RetargetModel<'_>, view: &RetargetView) {
    let rows: Vec<RetargetRow> = view
        .rows
        .iter()
        .map(|row| RetargetRow {
            tier_index: i32::try_from(row.tier_index).unwrap_or(i32::MAX),
            block: row.block.into(),
            name: row.name.as_str().into(),
            old_angle: row.old_angle.as_str().into(),
            new_angle: row.new_angle.as_str().into(),
            margin: row.margin.as_str().into(),
            risk_label: row.risk_label.into(),
            risk_color: Color::from_rgb_u8(row.risk_rgb.0, row.risk_rgb.1, row.risk_rgb.2),
        })
        .collect();
    model.set_rows(ModelRc::new(VecModel::from(rows)));
    model.set_notes(ModelRc::new(VecModel::from(
        view.notes
            .iter()
            .map(|note| SharedString::from(note.as_str()))
            .collect::<Vec<_>>(),
    )));
    model.set_anchored_errors(ModelRc::new(VecModel::from(
        view.anchored_errors
            .iter()
            .map(|error| SharedString::from(error.as_str()))
            .collect::<Vec<_>>(),
    )));
    model.set_solve_error(view.solve_error.as_str().into());
    model.set_target_error(SharedString::new());
}

/// The target line: name, refractive index and critical angle.
pub(super) fn push_target_readout(
    model: &RetargetModel<'_>,
    selection: &MaterialSelection,
    target: &ResolvedMaterial,
) {
    model.set_target_readout(
        format!(
            "Target: {} (n_D {:.4}, critical angle {:.2}\u{b0})",
            target_display_name(selection),
            target.n_d,
            target.critical_angle_deg
        )
        .into(),
    );
}

/// An unresolvable target (an unparseable RI): says so and clears the proposal, since
/// none describes what was typed.
pub(super) fn push_target_error(model: &RetargetModel<'_>, message: &str) {
    push_view(model, &RetargetView::default());
    model.set_target_readout(SharedString::new());
    model.set_target_error(message.into());
    model.set_can_apply(false);
}
