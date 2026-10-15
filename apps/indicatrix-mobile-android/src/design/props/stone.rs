//! The Stone tab: its pages (settings, material picker, color picker, custom-material
//! form), the picks, the effect sliders and the custom-material form.

use indicatrix_mobile_core::{
    app_state::AppState,
    design_props::{
        CustomMaterialInput, MaterialRow, add_custom_material, body_color_names, material_rows,
        select_material, set_body_color, set_crystal_axis, set_edge_rounding, set_frosted_girdle,
        set_inclusion_haze, stone_view,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Pushed, bind0, bind1, finish, string_model, with_pushed};
use crate::{AppWindow, MaterialItem, PropsModel, Shell, design::Shared, push::toast};

/// How far a slider must have moved from the design's value to count as a change. Anything
/// closer is the float round trip of an untouched control and must not write the design.
const SLIDER_EPSILON: f32 = 1e-6;

/// Connects the Stone tab's callbacks.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    props.on_open_page(bind1(window, state, open_page));
    props.on_material_query_changed(bind1(window, state, material_query_changed));
    props.on_pick_material(bind1(window, state, pick_material));
    props.on_pick_color(bind1(window, state, pick_color));
    props.on_effects_released(bind0(window, state, effects_released));
    props.on_save_custom(bind0(window, state, save_custom));
}

/// The Slint row of a material.
fn material_item(row: &MaterialRow) -> MaterialItem {
    MaterialItem {
        name: row.name.as_str().into(),
        detail: row.detail_text().into(),
        custom: row.is_custom,
        selected: row.is_selected,
    }
}

/// Builds the rows of the page the tab shows (the material or the color picker), when
/// they are out of date.
pub(super) fn push_pickers(props: &PropsModel, state: &AppState, pushed: &mut Pushed) {
    match props.get_stone_page() {
        1 => {
            let query = props.get_material_query().to_string();
            let key = (
                query,
                state.session.design.material.name.clone(),
                state.props.custom_materials.len(),
            );
            if pushed.materials_key.as_ref() != Some(&key) {
                let items: Vec<MaterialItem> = material_rows(state, &key.0)
                    .iter()
                    .map(material_item)
                    .collect();
                props.set_materials(ModelRc::new(VecModel::from(items)));
                pushed.materials_key = Some(key);
            }
        }
        2 => {
            let current = stone_view(state).color_label;
            let items: Vec<MaterialItem> = body_color_names()
                .into_iter()
                .map(|name| {
                    let selected = name == current;
                    MaterialItem {
                        name: name.into(),
                        detail: SharedString::new(),
                        custom: false,
                        selected,
                    }
                })
                .collect();
            props.set_colors(ModelRc::new(VecModel::from(items)));
        }
        _ => {}
    }
}

/// Whether a stored value differs from the one last pushed. Both are copies of the same
/// stored setting, so exact inequality is the change test.
#[expect(
    clippy::float_cmp,
    reason = "change detection between two copies of the same stored value; any bit difference means it moved"
)]
fn moved(old: f32, new: f32) -> bool {
    old != new
}

/// Pushes the Stone tab's values and the page it shows.
pub(super) fn push(props: &PropsModel, state: &AppState, pushed: &mut Pushed) {
    let view = stone_view(state);
    let was = pushed.stone.as_ref();

    props.set_material_name(
        state
            .session
            .design
            .material
            .name
            .clone()
            .unwrap_or_default()
            .into(),
    );
    props.set_material_title(view.material_title.as_str().into());
    props.set_material_detail(view.material_detail.as_str().into());
    props.set_effective_ri(view.effective_ri_text.as_str().into());
    props.set_color_label(view.color_label.as_str().into());
    props.set_color_enabled(view.color_enabled);
    props.set_width_note(view.width_note.as_str().into());
    props.set_axis_available(view.axis_available);
    if was.is_none() {
        props.set_color_names(string_model(body_color_names()));
    }

    // The editable fields follow the design only when the design's value moved.
    if was.is_none_or(|old| old.ri_text != view.ri_text) {
        props.set_ri_text(view.ri_text.as_str().into());
    }
    if was.is_none_or(|old| old.width_text != view.width_text) {
        props.set_width_text(view.width_text.as_str().into());
    }
    if was.is_none_or(|old| moved(old.haze_position, view.haze_position)) {
        props.set_haze(view.haze_position);
    }
    if was.is_none_or(|old| old.frosted != view.frosted) {
        props.set_frosted(view.frosted);
    }
    if was.is_none_or(|old| moved(old.edge_rounding, view.edge_rounding)) {
        props.set_rounding(view.edge_rounding);
    }
    if was.is_none_or(|old| old.axis_on != view.axis_on) {
        props.set_axis_on(view.axis_on);
    }
    if was.is_none_or(|old| moved(old.axis_tilt_deg, view.axis_tilt_deg)) {
        props.set_axis_tilt(view.axis_tilt_deg);
    }
    if was.is_none_or(|old| moved(old.axis_azimuth_deg, view.axis_azimuth_deg)) {
        props.set_axis_azimuth(view.axis_azimuth_deg);
    }
    pushed.stone = Some(view);
    push_pickers(props, state, pushed);
}

/// Shows page `page` of the tab (0 settings, 1 materials, 2 colors, 3 custom form). The
/// sub-pages ask the sheet for its full height.
fn open_page(window: &AppWindow, state: &Shared, page: i32) {
    let props = window.global::<PropsModel>();
    let page = page.clamp(0, 3);
    if page == 1 {
        props.set_material_query(SharedString::new());
    }
    if page == 3 {
        props.set_custom_error(SharedString::new());
    }
    if page != 0 {
        window.global::<Shell>().set_sheet_detent(2);
    }
    props.set_stone_page(page);
    with_pushed(|pushed| {
        pushed.materials_key = None;
        push_pickers(&props, &state.borrow(), pushed);
    });
}

/// The search text changed: rebuild the material rows.
fn material_query_changed(window: &AppWindow, state: &Shared, _query: SharedString) {
    let props = window.global::<PropsModel>();
    with_pushed(|pushed| {
        pushed.materials_key = None;
        push_pickers(&props, &state.borrow(), pushed);
    });
}

/// A material was picked: it becomes the design's material and the picker closes.
#[expect(
    clippy::needless_pass_by_value,
    reason = "bind1 takes fn(&AppWindow, &Shared, A) with the Slint callback payload passed by value"
)]
fn pick_material(window: &AppWindow, state: &Shared, name: SharedString) {
    let result = select_material(&mut state.borrow_mut(), &name);
    if result.is_ok() {
        window.global::<PropsModel>().set_stone_page(0);
    }
    finish(window, state, result);
}

/// A body color was picked.
fn pick_color(window: &AppWindow, state: &Shared, index: i32) {
    let result = set_body_color(&mut state.borrow_mut(), index);
    if result.is_ok() {
        window.global::<PropsModel>().set_stone_page(0);
    }
    finish(window, state, result);
}

/// An effect slider was released or a switch flipped: apply whichever of the four
/// controls differs from the design.
fn effects_released(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    let (haze, frosted, rounding) = (props.get_haze(), props.get_frosted(), props.get_rounding());
    let (axis_on, tilt, azimuth) = (
        props.get_axis_on(),
        props.get_axis_tilt(),
        props.get_axis_azimuth(),
    );
    let results = {
        let mut guard = state.borrow_mut();
        let app = &mut *guard;
        let view = stone_view(app);
        let differs = |shown: f32, wanted: f32| (shown - wanted).abs() > SLIDER_EPSILON;
        let mut results = Vec::new();
        if differs(view.haze_position, haze) {
            results.push(set_inclusion_haze(app, haze));
        }
        if view.frosted != frosted {
            results.push(set_frosted_girdle(app, frosted));
        }
        if differs(view.edge_rounding, rounding) {
            results.push(set_edge_rounding(app, rounding));
        }
        if view.axis_on != axis_on
            || differs(view.axis_tilt_deg, tilt)
            || differs(view.axis_azimuth_deg, azimuth)
        {
            results.push(set_crystal_axis(app, axis_on, tilt, azimuth));
        }
        results
    };
    let mut changed = false;
    let mut failure = None;
    for result in results {
        match result {
            Ok(did_change) => changed |= did_change,
            Err(message) => failure = failure.or(Some(message)),
        }
    }
    finish(window, state, failure.map_or(Ok(changed), Err));
}

/// "Save and Use" of the custom-material form.
fn save_custom(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    let input = CustomMaterialInput {
        name: props.get_custom_name().to_string(),
        ri: props.get_custom_ri().to_string(),
        dispersion: props.get_custom_dispersion().to_string(),
        birefringence: props.get_custom_birefringence().to_string(),
        specific_gravity: props.get_custom_sg().to_string(),
        color_index: props.get_custom_color(),
    };
    let name = input.name.trim().to_owned();
    let result = add_custom_material(&mut state.borrow_mut(), &input);
    match result {
        Err(message) => props.set_custom_error(message.as_str().into()),
        Ok(changed) => {
            props.set_custom_name(SharedString::new());
            props.set_custom_ri(SharedString::new());
            props.set_custom_dispersion(SharedString::new());
            props.set_custom_birefringence(SharedString::new());
            props.set_custom_sg(SharedString::new());
            props.set_custom_color(0);
            props.set_custom_error(SharedString::new());
            props.set_stone_page(0);
            toast::show(window, &format!("{name} is now this design's material."));
            finish(window, state, Ok(changed));
        }
    }
}
