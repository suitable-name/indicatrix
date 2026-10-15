//! The Light tab: the lighting rig list, the light sliders and the backdrop.
//!
//! Everything here is an app setting (`MobileSettings::lighting`), saved by the core and
//! restored at the next start. The renderer reads it back from the settings, so a change
//! only needs a refresh.

use indicatrix_mobile_core::{
    app_state::AppState,
    design_props::{
        LightValues, LightingView, backdrop_from_index, lighting_rows, lighting_view, set_backdrop,
        set_light_values, set_lighting,
    },
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use super::{Pushed, bind0, bind1, with_pushed};
use crate::{
    AppWindow, LightingItem, PropsModel,
    design::{Shared, refresh},
    push::toast,
};

/// Connects the Light tab's callbacks.
pub(super) fn wire(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    props.on_preset_picked(bind1(window, state, preset_picked));
    props.on_light_released(bind0(window, state, light_released));
    props.on_backdrop_picked(bind1(window, state, backdrop_picked));
}

/// A lighting rig was picked.
#[expect(
    clippy::needless_pass_by_value,
    reason = "bind1 takes fn(&AppWindow, &Shared, A) with the Slint callback payload passed by value"
)]
fn preset_picked(window: &AppWindow, state: &Shared, name: SharedString) {
    let result = set_lighting(&mut state.borrow_mut(), &name);
    if let Err(message) = result {
        toast::show(window, &message);
    }
    refresh(window, state);
}

/// A light slider was released: store all five values.
fn light_released(window: &AppWindow, state: &Shared) {
    let props = window.global::<PropsModel>();
    let values = LightValues {
        exposure: props.get_exposure(),
        azimuth_deg: props.get_azimuth(),
        elevation_deg: props.get_elevation(),
        head_shadow_deg: props.get_head_shadow(),
        surface_glare_pct: props.get_glare(),
    };
    set_light_values(&mut state.borrow_mut(), values);
    // The core clamps and snaps; show what it kept.
    with_pushed(|pushed| pushed.lighting = None);
    refresh(window, state);
}

/// A backdrop was picked.
fn backdrop_picked(window: &AppWindow, state: &Shared, index: i32) {
    set_backdrop(&mut state.borrow_mut(), backdrop_from_index(index));
    refresh(window, state);
}

/// The Slint rows of the lighting list, with the chosen rig marked.
fn preset_items(view: &LightingView) -> Vec<LightingItem> {
    lighting_rows()
        .into_iter()
        .map(|row| {
            // While the HDR map lights the stone, no rig is the chosen one.
            let selected = row.name == view.preset_name && !view.hdr.selected;
            LightingItem {
                name: row.name.into(),
                title: row.title.into(),
                description: row.description.into(),
                selected,
            }
        })
        .collect()
}

/// Whether a stored slider value differs from the one last pushed. The values are copied
/// verbatim from the settings, so exact inequality is the change test.
#[expect(
    clippy::float_cmp,
    reason = "change detection between two copies of the same stored value; any bit difference means it moved"
)]
fn moved(old: f32, new: f32) -> bool {
    old != new
}

/// Pushes the Light tab; sliders are rewritten only when their stored value moved.
pub(super) fn push(props: &PropsModel, state: &AppState, pushed: &mut Pushed) {
    let view = lighting_view(state);
    let was = pushed.lighting.as_ref();
    if was == Some(&view) {
        return;
    }
    if was.is_none_or(|old| {
        old.preset_name != view.preset_name || old.hdr.selected != view.hdr.selected
    }) {
        props.set_presets(ModelRc::new(VecModel::from(preset_items(&view))));
    }
    if was.is_none_or(|old| old.hdr != view.hdr) {
        props.set_hdr_available(view.hdr.available);
        props.set_hdr_selected(view.hdr.selected);
        props.set_hdr_description(view.hdr.description.as_str().into());
    }
    if was.is_none_or(|old| moved(old.values.exposure, view.values.exposure)) {
        props.set_exposure(view.values.exposure);
    }
    if was.is_none_or(|old| moved(old.values.azimuth_deg, view.values.azimuth_deg)) {
        props.set_azimuth(view.values.azimuth_deg);
    }
    if was.is_none_or(|old| moved(old.values.elevation_deg, view.values.elevation_deg)) {
        props.set_elevation(view.values.elevation_deg);
    }
    if was.is_none_or(|old| moved(old.values.head_shadow_deg, view.values.head_shadow_deg)) {
        props.set_head_shadow(view.values.head_shadow_deg);
    }
    if was.is_none_or(|old| moved(old.values.surface_glare_pct, view.values.surface_glare_pct)) {
        props.set_glare(view.values.surface_glare_pct);
    }
    props.set_backdrop(view.backdrop_index);
    props.set_uses_light_direction(view.uses_light_direction);
    props.set_uses_head_shadow(view.uses_head_shadow);
    pushed.lighting = Some(view);
}
