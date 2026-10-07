//! Design settings (the desktop's `editor_design_settings.slint` and
//! `tier_actions::{materials_symmetry,gear_remap,design_forms,viewport_material}`): the
//! schedule-wide material, RI override and color, index gear, symmetry and mirror, the
//! title / header / footnote lines, and the printed proportions -- a modal dialog opened
//! by Design > Design settings...
//!
//! Every Apply is one undoable `EditorSession` edit (a gear change is one batch of the
//! index remap and the schedule change, after a confirmation that previews the remap).
//! [`push`] seeds the dialog's fields from the design group by group
//! (`indicatrix_editor::scratch`), so an unrelated edit never discards what is being
//! typed.

mod custom;
mod gear;
mod lch;
mod push;

pub use push::{clear, push};

use crate::{
    AppWindow, DesignSettingsModel,
    app::{
        Ctx,
        persist::schedule_save,
        push::{MessageKind, show_message},
    },
    editor::{
        edit::Dirty,
        inspector::{finish, finish_no_tier, refresh},
    },
};
use indicatrix_cut_core::Edit;
use indicatrix_editor::{
    loading::ri_override_for_material_pick,
    material::{design_material_options, parse_design_material_form, with_body_color_choice},
    printed_proportions::parse_printed_proportions_form,
    view_model::row_format::tiers_incomplete_under_proposed_symmetry,
};
use slint::ComponentHandle;

/// Apply Material: the combo, the typed RI and the color, as one `SetMaterial`. A plain
/// pick with no typed RI must not silently change what the exported `I` line reads, nor
/// pin the OUTGOING material's RI onto the incoming one
/// (`ri_override_for_material_pick`).
fn apply_material(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let combo_index = model.get_material_index();
    let ri_text = model.get_ri_override_text();
    let color_index = model.get_color_index();
    let result = {
        let mut app = ctx.state.borrow_mut();
        let options = design_material_options(&app.custom_materials);
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let current = design_state.session.design.material.clone();
        let legacy_ri = design_state.session.design.meta.refractive_index;
        parse_design_material_form(combo_index, &ri_text, &options, &current).and_then(|material| {
            let mut material = with_body_color_choice(material, color_index, &current);
            if material.refractive_index_override.is_none() {
                material.refractive_index_override = ri_override_for_material_pick(
                    material.name.as_deref(),
                    current.name.as_deref(),
                    legacy_ri,
                );
            }
            design_state
                .session
                .apply(Edit::SetMaterial { material })
                .map(|_| ())
                .map_err(|e| e.to_string())
        })
    };
    match result {
        Ok(()) => {
            model.set_material_pending(false);
            finish_no_tier(ctx);
        }
        Err(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// "Set material" on the inferred-material guess: only the NAME changes ("confirm the
/// guess", not "reconfigure the material").
fn set_material_from_guess(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let name = ui
        .global::<DesignSettingsModel>()
        .get_guess_name()
        .to_string();
    if name.is_empty() {
        return;
    }
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let mut material = design_state.session.design.material.clone();
        material.name = Some(name.clone());
        design_state
            .session
            .apply(Edit::SetMaterial { material })
            .map(|_| ())
    };
    match result {
        Ok(()) => {
            finish_no_tier(ctx);
            show_message(
                ctx,
                MessageKind::Success,
                &format!("Material set to {name}."),
            );
        }
        Err(e) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The symmetry order the field holds, if it is a positive whole number.
fn parse_symmetry(text: &str) -> Option<u32> {
    text.trim().parse::<u32>().ok().filter(|&order| order >= 1)
}

/// Apply Symmetry: the order and mirror, keeping the design's current gear (only a gear
/// change goes through the remap confirmation).
fn apply_symmetry(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let Some(symmetry_order) = parse_symmetry(&model.get_symmetry_order_text()) else {
        show_message(
            ctx,
            MessageKind::Error,
            "Symmetry order must be a positive whole number.",
        );
        return;
    };
    let mirror = model.get_mirror();
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let gear_teeth = design_state.session.design.meta.gear_teeth;
        design_state
            .session
            .apply(Edit::SetSchedule {
                gear_teeth,
                symmetry_order,
                mirror,
            })
            .map(|_| ())
    };
    match result {
        Ok(()) => {
            model.set_symmetry_pending(false);
            // Symmetry can move every tier's index positions, untracked: trust no mast.
            finish(ctx, Dirty::All);
        }
        Err(e) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// The symmetry field or the mirror switch changed: previews how many tiers would be left
/// with an incomplete orbit.
fn symmetry_edited(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let text = parse_symmetry(&model.get_symmetry_order_text()).map_or_else(String::new, |order| {
        let app = ctx.state.borrow();
        let incomplete = app.design.as_ref().map_or(0, |d| {
            tiers_incomplete_under_proposed_symmetry(&d.session.design, order, model.get_mirror())
        });
        match incomplete {
            0 => "No tiers would become incomplete orbits.".to_string(),
            1 => "1 tier would become an incomplete orbit.".to_string(),
            n => format!("{n} tiers would become incomplete orbits."),
        }
    });
    model.set_symmetry_preview_text(text.into());
}

/// `text` split on `;` into trimmed, non-empty lines.
fn split_lines(text: &str) -> Vec<String> {
    text.split(';')
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect()
}

/// Apply Details: the title (the first header line), the further header lines, the
/// footnotes and the gear reference angle, as one `SetMeta`.
fn apply_meta(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let gear_ref = model.get_gear_reference_angle();
    let gear_ref_trimmed = gear_ref.trim();
    let gear_reference_angle = match gear_ref_trimmed.parse::<f64>() {
        Ok(angle) if angle.is_finite() => angle,
        Ok(_) => {
            show_message(
                ctx,
                MessageKind::Error,
                "Gear reference angle must be a finite number.",
            );
            return;
        }
        Err(_) => {
            show_message(
                ctx,
                MessageKind::Error,
                &format!("Gear reference angle '{gear_ref_trimmed}' is not a number."),
            );
            return;
        }
    };
    let mut headers = Vec::new();
    let title = model.get_title();
    if !title.trim().is_empty() {
        headers.push(title.trim().to_string());
    }
    headers.extend(split_lines(&model.get_extra_headers()));
    let footnotes = split_lines(&model.get_footnotes());
    let result = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        design_state
            .session
            .apply(Edit::SetMeta {
                headers,
                footnotes,
                gear_reference_angle,
            })
            .map(|_| ())
    };
    match result {
        Ok(()) => {
            model.set_meta_pending(false);
            finish_no_tier(ctx);
        }
        Err(e) => show_message(ctx, MessageKind::Error, &e.to_string()),
    }
}

/// Apply Printed: the five figures a design file carries in its `[source]` table.
fn apply_printed(ctx: &Ctx) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let props = match parse_printed_proportions_form(
        &model.get_printed_vol_w3(),
        &model.get_printed_lw(),
        &model.get_printed_cw(),
        &model.get_printed_pw(),
        &model.get_printed_hw(),
    ) {
        Ok(props) => props,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
    };
    let has_any = props.vol_w3.is_some()
        || props.lw.is_some()
        || props.cw.is_some()
        || props.pw.is_some()
        || props.hw.is_some();
    {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        design_state.printed_proportions = has_any.then_some(props);
    }
    push::seed_printed(&model, ctx.state.borrow().design.as_ref());
    model.set_printed_pending(false);
    // Not an undoable edit (it never touches the `Design`), but it is saved with the design file.
    schedule_save(ctx);
    refresh(ctx);
    show_message(
        ctx,
        MessageKind::Success,
        if has_any {
            "Printed proportions saved with the design."
        } else {
            "Printed proportions cleared."
        },
    );
}

/// Registers the dialog's callbacks.
pub fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<DesignSettingsModel>();
    let c = ctx.clone();
    model.on_apply_material(move || apply_material(&c));
    lch::wire(ui, ctx);
    let c = ctx.clone();
    model.on_set_material_from_guess(move || set_material_from_guess(&c));
    let c = ctx.clone();
    model.on_apply_symmetry(move || apply_symmetry(&c));
    let c = ctx.clone();
    model.on_symmetry_edited(move || symmetry_edited(&c));
    let c = ctx.clone();
    model.on_apply_meta(move || apply_meta(&c));
    let c = ctx.clone();
    model.on_apply_printed(move || apply_printed(&c));
    gear::wire(&model, ctx);
    custom::wire(ui, ctx);
}
