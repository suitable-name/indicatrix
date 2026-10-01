//! Pushing the design settings dialog's state (the desktop's
//! `view::inspector::refresh_design_settings`): the combo option lists, the RI
//! readouts and the material guess every push; the editable fields of one group (material,
//! gear, symmetry, details) only when that group changed in the design.

use crate::{AppWindow, DesignSettingsModel, app::state::DesignState, editor::inspector::PushCtx};
use indicatrix_cut_core::critical_angle_deg;
use indicatrix_editor::{
    material::{
        body_colour_index_for, body_colour_options, design_material_index_from_name,
        design_material_options, gear_index_from_teeth, ri_source_text,
    },
    material_lookup::material_guess,
};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// Sets `model`'s option list to `options`, only when it differs (so a combo is not reset
/// on every refresh).
fn set_options_if_changed(
    current: &ModelRc<SharedString>,
    options: &[String],
    set: impl FnOnce(ModelRc<SharedString>),
) {
    let unchanged = current.row_count() == options.len()
        && current
            .iter()
            .zip(options)
            .all(|(existing, new)| existing == new.as_str());
    if !unchanged {
        set(ModelRc::new(VecModel::from(
            options
                .iter()
                .cloned()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    }
}

/// Seeds the printed-proportions fields from the design's stored figures.
pub(super) fn seed_printed(model: &DesignSettingsModel<'_>, design: Option<&DesignState>) {
    let text = |value: Option<f64>| -> SharedString {
        value.map_or_else(SharedString::new, |v| format!("{v}").into())
    };
    let props = design.and_then(|d| d.printed_proportions.as_ref());
    model.set_printed_vol_w3(text(props.and_then(|p| p.vol_w3)));
    model.set_printed_lw(text(props.and_then(|p| p.lw)));
    model.set_printed_cw(text(props.and_then(|p| p.cw)));
    model.set_printed_pw(text(props.and_then(|p| p.pw)));
    model.set_printed_hw(text(props.and_then(|p| p.hw)));
}

/// Blank: no design is loaded.
pub fn clear(ui: &AppWindow) {
    let model = ui.global::<DesignSettingsModel>();
    model.set_open(false);
    model.set_gear_remap_open(false);
    model.set_guess_text(SharedString::new());
    model.set_effective_ri_text(SharedString::new());
    model.set_critical_angle_text(SharedString::new());
    model.set_ri_source_text(SharedString::new());
    model.set_symmetry_preview_text(SharedString::new());
    seed_printed(&model, None);
}

/// One refresh of the dialog.
pub fn push(pcx: &PushCtx<'_>) {
    let model = pcx.ui.global::<DesignSettingsModel>();
    let design = pcx.design;

    // The material combo (built-ins, then custom materials, then "Custom RI...") follows
    // the session's custom materials; the colour list is static.
    let options = design_material_options(pcx.custom);
    set_options_if_changed(&model.get_material_options(), &options, |m| {
        model.set_material_options(m);
    });
    set_options_if_changed(&model.get_colour_options(), &body_colour_options(), |m| {
        model.set_colour_options(m);
    });
    if pcx.delta.material {
        model.set_material_index(design_material_index_from_name(
            design.material.name.as_deref(),
            &options,
        ));
        model.set_ri_override_text(
            design
                .material
                .refractive_index_override
                .map_or_else(String::new, |v| format!("{v:.4}"))
                .into(),
        );
        model.set_colour_index(body_colour_index_for(design.material.body_colour_override));
        model.set_material_pending(false);
    }
    model.set_effective_ri_text(format!("{:.4}", pcx.n_d).into());
    model.set_critical_angle_text(format!("{:.2}\u{b0}", critical_angle_deg(pcx.n_d)).into());
    model.set_ri_source_text(ri_source_text(&design.material, pcx.custom).into());
    let (guess_text, guess_name, guess_others) =
        material_guess(design, pcx.n_d).unwrap_or_default();
    model.set_guess_text(guess_text.into());
    model.set_guess_name(guess_name.into());
    model.set_guess_others(guess_others.into());

    if pcx.delta.gear {
        model.set_gear_index(gear_index_from_teeth(design.meta.gear_teeth));
        model.set_gear_custom_text(design.meta.gear_teeth.to_string().into());
        model.set_gear_pending(false);
    }
    if pcx.delta.symmetry {
        model.set_symmetry_order_text(design.meta.symmetry_order.to_string().into());
        model.set_mirror(design.meta.mirror);
        // Whatever the last typed-but-unapplied preview said no longer applies.
        model.set_symmetry_preview_text(SharedString::new());
        model.set_symmetry_pending(false);
    }
    if pcx.delta.meta {
        // The first header line is the title; the rest are "extra" lines, joined for the
        // single-line field (`apply_meta`'s inverse).
        let (title, extra) = design
            .meta
            .headers
            .split_first()
            .map_or((String::new(), String::new()), |(title, rest)| {
                (title.clone(), rest.join(";"))
            });
        model.set_title(title.into());
        model.set_extra_headers(extra.into());
        model.set_footnotes(design.meta.footnotes.join(";").into());
        model.set_gear_reference_angle(format!("{}", design.meta.gear_reference_angle).into());
        model.set_meta_pending(false);
    }
    if pcx.replaced || pcx.previous_generation.is_none() {
        seed_printed(&model, Some(pcx.design_state));
        model.set_printed_pending(false);
    }
}
