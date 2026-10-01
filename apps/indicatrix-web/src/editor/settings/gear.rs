//! The gear change and its remap confirmation (the desktop's
//! `tier_actions::gear_remap`): Apply Gear previews what every tier's indices would become
//! on the new gear and waits for Confirm; Confirm applies the remap and the new schedule as
//! ONE undo step, and refuses a preview the design has since moved past.

use crate::{
    DesignSettingsModel, GearRemapRow,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::{edit::Dirty, inspector::finish},
};
use indicatrix_cut_core::{Edit, RemapRounding};
use indicatrix_editor::material::{
    gear_choice_to_teeth, gear_index_from_teeth, gear_remap_preview,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::cell::RefCell;

/// The remap waiting for Confirm.
struct PendingRemap {
    from_gear: i32,
    to_gear: i32,
    symmetry_order: u32,
    mirror: bool,
    rounding: RemapRounding,
    /// The design generation the preview was built against.
    generation: u64,
}

thread_local! {
    static PENDING: RefCell<Option<PendingRemap>> = const { RefCell::new(None) };
}

/// Pushes the preview rows of `pending` against the current design.
fn push_rows(ctx: &Ctx, model: &DesignSettingsModel<'_>, pending: &PendingRemap) {
    let rows: Vec<GearRemapRow> = ctx
        .state
        .borrow()
        .design
        .as_ref()
        .map_or_else(Vec::new, |d| {
            gear_remap_preview(
                &d.session.design,
                pending.from_gear,
                pending.to_gear,
                pending.rounding,
            )
            .into_iter()
            .map(|row| GearRemapRow {
                name: row.name.into(),
                old_indices: row.old_indices.into(),
                new_indices: row.new_indices.into(),
                non_integral: row.non_integral,
            })
            .collect()
        });
    model.set_gear_remap_rows(ModelRc::new(VecModel::from(rows)));
}

/// Apply Gear: previews the remap and opens the confirmation.
fn apply_gear(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let to_gear = match gear_choice_to_teeth(model.get_gear_index(), &model.get_gear_custom_text())
    {
        Ok(teeth) => teeth,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let pending = {
        let app = ctx.state.borrow();
        let Some(design_state) = app.design.as_ref() else {
            return;
        };
        let meta = &design_state.session.design.meta;
        PendingRemap {
            from_gear: meta.gear_teeth,
            to_gear,
            symmetry_order: meta.symmetry_order,
            mirror: meta.mirror,
            rounding: RemapRounding::Nearest,
            generation: design_state.session.current_generation(),
        }
    };
    if pending.from_gear == pending.to_gear {
        model.set_gear_pending(false);
        show_message(ctx, MessageKind::Info, "Already using this gear.");
        return;
    }
    push_rows(ctx, &model, &pending);
    PENDING.with(|cell| *cell.borrow_mut() = Some(pending));
    model.set_gear_remap_open(true);
}

/// The Rounding combo of the confirmation changed.
fn set_rounding(ctx: &Ctx, index: i32) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let rounding = match index {
        1 => RemapRounding::Floor,
        2 => RemapRounding::Ceil,
        _ => RemapRounding::Nearest,
    };
    PENDING.with(|cell| {
        if let Some(pending) = cell.borrow_mut().as_mut() {
            pending.rounding = rounding;
            push_rows(ctx, &model, pending);
        }
    });
}

/// Confirm: the remap and the new schedule as one undo step.
fn confirm(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    model.set_gear_remap_open(false);
    let Some(pending) = PENDING.with(|cell| cell.borrow_mut().take()) else {
        return;
    };
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        if design_state.session.current_generation() == pending.generation {
            design_state
                .session
                .apply(Edit::Batch(vec![
                    Edit::RemapIndices {
                        from_gear: pending.from_gear,
                        to_gear: pending.to_gear,
                        rounding: pending.rounding,
                    },
                    Edit::SetSchedule {
                        gear_teeth: pending.to_gear,
                        symmetry_order: pending.symmetry_order,
                        mirror: pending.mirror,
                    },
                ]))
                .map(|_| ())
                .map_err(|e| e.to_string())
        } else {
            Err(
                "The design changed while Apply Gear was open, so this preview no longer \
                 matches it. Apply the gear again to remap against the current design."
                    .to_string(),
            )
        }
    };
    match result {
        Ok(()) => {
            model.set_gear_pending(false);
            // A remap rewrites every tier's index positions: trust no mast.
            finish(ctx, Dirty::All);
        }
        Err(message) => show_message(ctx, MessageKind::Warning, &message),
    }
}

/// Cancel: drops the pending remap; the gear combo shows the design's gear again.
fn cancel(ctx: &Ctx) {
    PENDING.with(|cell| *cell.borrow_mut() = None);
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    model.set_gear_remap_open(false);
    model.set_gear_pending(false);
    // Re-seed the combo from the design.
    if let Some(design_state) = ctx.state.borrow().design.as_ref() {
        let teeth = design_state.session.design.meta.gear_teeth;
        model.set_gear_index(gear_index_from_teeth(teeth));
        model.set_gear_custom_text(teeth.to_string().into());
    }
}

/// Registers the gear callbacks.
pub(super) fn wire(model: &DesignSettingsModel<'_>, ctx: &Ctx) {
    let c = ctx.clone();
    model.on_apply_gear(move || apply_gear(&c));
    let c = ctx.clone();
    model.on_remap_set_rounding(move |index| set_rounding(&c, index));
    let c = ctx.clone();
    model.on_remap_confirm(move || confirm(&c));
    let c = ctx.clone();
    model.on_remap_cancel(move || cancel(&c));
}
