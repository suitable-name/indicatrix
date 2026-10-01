//! The Preform tab's inputs: the rough's shape and size, the girdle Y-offset, the girdle
//! diameter and the specific-gravity override, and their Apply buttons (the desktop's
//! `tier_actions::design_forms::{setup_apply_preform_callback,
//! setup_apply_yield_inputs_callback, setup_apply_preform_y_offset_callback}`).
//!
//! Each group is re-seeded from the design only when that group changed
//! (`indicatrix_editor::scratch`), so an unrelated edit or solve never discards what is
//! being typed; the Y-offset alone follows the solve, since converting millimetres needs
//! one.

use super::{PushCtx, finish_no_tier};
use crate::{
    AppWindow, InspectorModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
};
use indicatrix_cut_core::{Edit, PreformShape};
use indicatrix_editor::{
    loading::parse_preform_form, material::parse_yield_form,
    view_model::yield_report::preform_y_offset_mm_text,
};
use slint::{ComponentHandle, SharedString};

/// The side count of a cylinder preform from the form: fixed, independent of the index
/// gear (the desktop's `FIXED_CYLINDER_PREFORM_SIDES`).
const FIXED_CYLINDER_PREFORM_SIDES: usize = 96;

/// Re-seeds the preform and yield fields of the groups that changed.
pub(super) fn push(pcx: &PushCtx<'_>) {
    let model = pcx.ui.global::<InspectorModel>();
    let design = pcx.design;
    if pcx.delta.preform {
        let preform = &design.preform;
        model.set_preform_shape_index(match preform.shape {
            PreformShape::Block => 0,
            PreformShape::Cylinder { .. } => 1,
        });
        model.set_preform_half_width(format!("{:.4}", preform.half_width).into());
        model.set_preform_length_over_width(format!("{:.4}", preform.length_over_width).into());
        model.set_preform_depth(format!("{:.4}", preform.depth).into());
        model.set_preform_pending(false);
    }
    if !model.get_preform_pending() {
        model.set_preform_y_offset_mm(
            preform_y_offset_mm_text(design.preform_y_offset, mm_per_unit(pcx)).into(),
        );
    }
    if pcx.delta.girdle {
        model.set_girdle_diameter_mm(
            design
                .girdle_diameter_mm
                .map_or_else(String::new, |mm| format!("{mm:.4}"))
                .into(),
        );
        model.set_yield_pending(false);
    }
    if pcx.delta.material {
        model.set_specific_gravity_override(
            design
                .material
                .specific_gravity_override
                .map_or_else(String::new, |sg| format!("{sg:.4}"))
                .into(),
        );
        model.set_yield_pending(false);
    }
}

/// Model units per millimetre-anchored scale of the current solve, when a girdle diameter
/// anchors one.
fn mm_per_unit(pcx: &PushCtx<'_>) -> Option<f64> {
    pcx.solved
        .and_then(|solved| pcx.design.yield_report(solved).mm_per_unit)
}

/// Apply Preform: the shape and size, plus the Y-offset when it was typed. One undo step.
fn apply_preform(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let preform = match parse_preform_form(
        model.get_preform_shape_index(),
        &model.get_preform_half_width(),
        &model.get_preform_length_over_width(),
        &model.get_preform_depth(),
        FIXED_CYLINDER_PREFORM_SIDES,
    ) {
        Ok(preform) => preform,
        Err(e) => {
            show_message(ctx, MessageKind::Error, &e);
            return;
        }
    };
    let y_text = model.get_preform_y_offset_mm();
    let result = {
        let mut app = ctx.state.borrow_mut();
        // The solve that anchors millimetres is the one for the design as it stands now,
        // read before the preform edit invalidates it.
        let mm = app.current_solved().and_then(|solved| {
            app.design
                .as_ref()
                .and_then(|d| d.session.design.yield_report(solved).mm_per_unit)
        });
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        y_offset_edit(&design_state.session.design, mm, &y_text).and_then(|y_offset| {
            let mut edits = vec![Edit::SetPreform { preform }];
            edits.extend(y_offset);
            let edit = if edits.len() == 1 {
                edits.remove(0)
            } else {
                Edit::Batch(edits)
            };
            design_state
                .session
                .apply(edit)
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    };
    match result {
        Ok(()) => {
            model.set_preform_pending(false);
            finish_no_tier(ctx);
        }
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// The `SetPreformYOffset` edit for the typed millimetres, `None` (nothing to apply) when
/// the field is blank or still shows what the design has.
fn y_offset_edit(
    design: &indicatrix_cut_core::Design,
    mm_per_unit: Option<f64>,
    typed: &SharedString,
) -> Result<Option<Edit>, String> {
    let trimmed = typed.trim();
    if trimmed.is_empty()
        || trimmed == preform_y_offset_mm_text(design.preform_y_offset, mm_per_unit)
    {
        return Ok(None);
    }
    let mm: f64 = trimmed
        .parse()
        .map_err(|_| format!("Y-offset '{trimmed}' is not a number."))?;
    if !mm.is_finite() {
        return Err("Y-offset must be a finite number.".to_string());
    }
    let mm_per_unit = mm_per_unit.ok_or_else(|| {
        "Set a girdle diameter and let the design solve before setting a Y-offset in millimetres."
            .to_string()
    })?;
    Ok(Some(Edit::SetPreformYOffset {
        y_offset: mm / mm_per_unit,
    }))
}

/// Apply Yield Inputs: the girdle diameter and the specific-gravity override, one undo
/// step.
fn apply_yield(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<InspectorModel>();
    let girdle = model.get_girdle_diameter_mm();
    let sg = model.get_specific_gravity_override();
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        parse_yield_form(&girdle, 0, &sg, &design_state.session.design.material).and_then(
            |(girdle_diameter_mm, material)| {
                design_state
                    .session
                    .apply(Edit::Batch(vec![
                        Edit::SetGirdleDiameterMm { girdle_diameter_mm },
                        Edit::SetMaterial { material },
                    ]))
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            },
        )
    };
    match result {
        Ok(()) => {
            model.set_yield_pending(false);
            finish_no_tier(ctx);
        }
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Registers the Preform tab's callbacks.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<InspectorModel>();
    let c = ctx.clone();
    model.on_apply_preform(move || apply_preform(&c));
    let c = ctx.clone();
    model.on_apply_yield(move || apply_yield(&c));
}
