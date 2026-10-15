//! "Preview as..." (the view-only material pick) and the live half of the Stone tab's
//! material effects (crystal-axis orientation, inclusion haze, frosted girdle, edge rounding).
//!
//! Split mechanically out of the former `material_quality.rs` (the quality half lives in
//! [`super::quality`]).
//!
//! The design is the single source of truth for the stone's material, size and effects:
//!
//! * A preview material is the ONLY way the view shows another material than the design's. It
//!   starts only from the Stone tab's "Preview as..." picker (never implicitly), and
//!   "Back to design" ends it (`editor::callbacks::tier_actions::viewport_material`).
//! * The effect controls change the picture at once (here) and a debounced commit writes the
//!   design (`viewport_material`), so a drag is smooth and costs one undo step.

use crate::{
    EditorModel, LibraryModel, MainWindow, SettingsModel, ViewportModel,
    bridge::{
        preview_render::material_ri_at_sodium_d,
        render_thread::{RenderContext, resolve_material},
    },
    gui::{
        is_c_axis_override_available,
        optics::c_axis::{angles_to_c_axis, c_axis_to_angles},
        render::{render_body_color, stone_effects},
        show_toast,
        startup_settings::find_option_index,
    },
    settings::SettingsPersister,
};
use indicatrix::{optics::materials::GemMaterial, render_setup::inclusion_scale};
use slint::{ComponentHandle, SharedString};
use std::sync::{Arc, Mutex};

/// Where an explicit "use this material" action ended up
/// ([`apply_explicit_material`]); the wire value of `EditorModel.apply-named-material`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::gui) enum ExplicitApply {
    /// The open design took it (or already had it) as a design edit.
    Design,
    /// A library design is on the viewport: the material became the library view selection.
    Library,
    /// The design was busy and nothing was applied (the editor already toasted the refusal).
    Busy,
}

impl ExplicitApply {
    /// The value the Slint callback returns.
    pub(in crate::gui) const fn code(self) -> i32 {
        match self {
            Self::Design => 0,
            Self::Library => 1,
            Self::Busy => 2,
        }
    }

    /// The inverse of [`Self::code`]; anything unknown reads as the library case.
    const fn from_code(code: i32) -> Self {
        match code {
            0 => Self::Design,
            2 => Self::Busy,
            _ => Self::Library,
        }
    }
}

/// Makes the render trace the material called `material` and refreshes the two UI values that
/// follow from it (crystal-axis availability, the RI-tolerance filter's default). Returns
/// `false`, changing nothing, when the view already traces exactly that material by name.
///
/// The shared half of "Preview as..." (a design view) and of the library view's material pick.
fn trace_named_material(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    material: &str,
) -> bool {
    let mut ctx = RenderContext::lock(render_ctx);
    if ctx.material_name == material && ctx.material_override.is_none() {
        return false;
    }
    ctx.material_name = material.to_string();
    // Cleared, not left alone: `resolve_material_with_override` PREFERS
    // `material_override` over the name just written, and the design sync sets one on
    // every refresh while the view follows the design (and a library row sets its own).
    ctx.material_override = None;
    // A design that names no material suspends tracing outright; naming one by hand
    // is the way out of that.
    ctx.material_unresolved = None;
    ctx.dirty = true;
    // Resolved once for both derived UI values below. `None` (no `materials[0]` fallback)
    // is handled defensively: Diamond stands in for these two UI defaults only, never for
    // anything that reaches the tracer.
    let resolved = resolve_material(
        &GemMaterial::all_materials(),
        &ctx.custom_materials,
        material,
    );
    let available = resolved.as_ref().is_some_and(is_c_axis_override_available);
    let ri_default = resolved.as_ref().map_or_else(
        || material_ri_at_sodium_d(&GemMaterial::diamond()),
        material_ri_at_sodium_d,
    );
    drop(ctx);
    ui.global::<SettingsModel>()
        .set_c_axis_override_available(available);
    // `ri_default` is f64; Slint's `float` is f32: a harmless cast for a
    // display/filter-default value in the 1.0-3.0 RI range.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "display/filter default in the 1.0-3.0 RI range; Slint float is f32"
    )]
    let ri_display = ri_default as f32;
    ui.global::<LibraryModel>()
        .set_ri_material_default(ri_display);
    true
}

/// Whether a "Preview as..." pick names the material the view already follows: the open design
/// is on the viewport and the pick is the design's own traced material (`design_material`, which
/// the editor refresh publishes, so it holds while a preview runs too). Such a pick starts no
/// preview -- it would only drop the design's body colour and RI override and show a badge for
/// "the same" material.
fn is_designs_own_material(shows_editor_design: bool, design_material: &str, chosen: &str) -> bool {
    shows_editor_design && design_material == chosen
}

/// Remembers the material the person chose to view library designs in (the app preference
/// `selected_material`, which also seeds the next session).
fn remember_library_material(material: &str) {
    if let Some(persister) = SettingsPersister::installed_for_this_thread() {
        persister.update(|file| file.settings.selected_material = material.to_string());
    }
}

/// An explicit "use this material" action (Save & Apply in the material editor, the planner's
/// "Use colour") applied where it belongs, never as a hidden preview:
///
/// * with the open design on the viewport, the design's material becomes `name` as ONE undoable
///   design edit (together with `stone_width_mm`, when given), and a running preview ends so the
///   picture shows the result;
/// * with a library design on the viewport there is no design to edit, so `name` becomes the
///   library view selection (persisted as `selected_material`).
///
/// Returns where it went. The caller has already pointed the render context at the material for
/// the library case; the viewport's material list is selected here unless the design was busy
/// (nothing applied, the editor already toasted: the chip must not name a material the design
/// and the picture do not have, and callers must not report a success).
pub(in crate::gui) fn apply_explicit_material(
    ui: &MainWindow,
    name: &str,
    stone_width_mm: Option<f32>,
) -> ExplicitApply {
    let result = ExplicitApply::from_code(
        ui.global::<EditorModel>()
            .invoke_apply_named_material(name.into(), stone_width_mm.unwrap_or(0.0)),
    );
    match result {
        ExplicitApply::Busy => {}
        ExplicitApply::Design | ExplicitApply::Library => {
            if result == ExplicitApply::Library {
                remember_library_material(name);
            }
            let viewport = ui.global::<ViewportModel>();
            if let Some(index) = find_option_index(&viewport.get_material_options(), name) {
                viewport.set_selected_material_index(index);
            }
        }
    }
    result
}

/// Wires up the Stone tab's two material pickers that do not touch the design:
///
/// * "Preview as..." (a design on the viewport): the view shows another material, leaving the
///   design alone. Starting a preview turns "following the design" off
///   (`viewport_material_linked`), clears any remembered view-only colour so an old tint does not
///   colour the new preview, and toasts what happened. Choosing the material already shown
///   changes nothing.
/// * "Material" of a library design on the viewport: a library design is not editable, so the
///   pick is the view selection and nothing else. It is remembered as `selected_material`, it
///   starts no preview (the badge is for a design's material only) and it ends when another
///   design takes the viewport.
pub(in crate::gui) fn setup_material_changed_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let render_ctx_mat = render_ctx.clone();
    let ui_weak_mat = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_preview_material_chosen(move |material: SharedString| {
            let Some(ui) = ui_weak_mat.upgrade() else {
                return;
            };
            let viewport = ui.global::<ViewportModel>();
            if is_designs_own_material(
                viewport.get_shows_editor_design(),
                &viewport.get_design_material_name(),
                &material,
            ) {
                // Picking the design's material again while previewing is the natural way
                // back: it ends the preview instead of starting one.
                if viewport.get_viewport_material_linked() {
                    show_toast(&ui, "That is the design's own material.", "info");
                } else {
                    viewport.invoke_preview_back_to_design();
                }
                return;
            }
            if !trace_named_material(&ui, &render_ctx_mat, &material) {
                return;
            }
            // A tint left over from an earlier preview must not colour this one.
            render_body_color::clear_view_color(&render_ctx_mat);
            // Previewing is the independent choice; the design's own material is
            // untouched and "Back to design" restores it.
            ui.global::<ViewportModel>()
                .set_viewport_material_linked(false);
            show_toast(
                &ui,
                &format!("Previewing {material}. The design keeps its own material."),
                "info",
            );
        });

    let render_ctx_lib = render_ctx.clone();
    let ui_weak_lib = ui.as_weak();
    ui.global::<ViewportModel>()
        .on_library_material_chosen(move |material: SharedString| {
            let Some(ui) = ui_weak_lib.upgrade() else {
                return;
            };
            // Only a library view has this pick; a design view previews instead.
            if ui.global::<ViewportModel>().get_shows_editor_design() {
                return;
            }
            // The view colour is a separate view-only choice and stays.
            if trace_named_material(&ui, &render_ctx_lib, &material) {
                remember_library_material(&material);
                show_toast(
                    &ui,
                    &format!(
                        "Showing this library design as {material}. The design is not changed."
                    ),
                    "info",
                );
            }
        });
}

/// Wires up the live half of the Stone tab's effects: crystal-axis orientation, inclusion
/// haze, frosted girdle and edge rounding. Each control changes the render context at once and
/// raises `SettingsModel.stone_effects_edited`, whose debounced handler writes the design (one
/// undoable edit). Nothing is written to the settings file.
pub(in crate::gui) fn setup_material_effect_override_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    // Crystal-axis angles: one callback carries both angles (the moved slider's new value plus
    // the other slider's current value), so a single-axis drag never has to guess where the
    // other axis sits. `angles_to_c_axis` is the one boundary crossing from these degree
    // sliders to the physical `Vec3` `RenderContext::c_axis_override` stores.
    let render_ctx_axis_angles = render_ctx.clone();
    let ui_weak_angles = ui.as_weak();
    ui.global::<SettingsModel>().on_c_axis_angles_changed(
        move |tilt_deg: f32, azimuth_deg: f32| {
            let tilt_deg = tilt_deg.clamp(0.0, 90.0);
            let azimuth_deg = azimuth_deg.clamp(0.0, 360.0);
            let mut ctx = RenderContext::lock(&render_ctx_axis_angles);
            ctx.c_axis_override = Some(angles_to_c_axis(tilt_deg, azimuth_deg));
            ctx.dirty = true;
            drop(ctx);
            raise_edited(&ui_weak_angles);
        },
    );

    // Crystal-axis override on/off. Turning it on seeds the two angle sliders from the selected
    // material's own `c_axis` (`c_axis_to_angles`, the inverse of `angles_to_c_axis`), so
    // enabling the override never makes the stone visibly jump.
    let render_ctx_axis_toggle = render_ctx.clone();
    let ui_weak_axis_toggle = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_c_axis_override_changed(move |enabled: bool| {
            let mut ctx = RenderContext::lock(&render_ctx_axis_toggle);
            if enabled {
                // `resolve_material` is `Option`; a `None` falls back to Diamond's own
                // (identity) axis rather than panicking (a UI default, never the tracer).
                let base = resolve_material(
                    &GemMaterial::all_materials(),
                    &ctx.custom_materials,
                    &ctx.material_name,
                )
                .unwrap_or_else(GemMaterial::diamond);
                let (tilt_deg, azimuth_deg) = c_axis_to_angles(base.c_axis);
                ctx.c_axis_override = Some(angles_to_c_axis(tilt_deg, azimuth_deg));
                ctx.dirty = true;
                drop(ctx);
                if let Some(ui) = ui_weak_axis_toggle.upgrade() {
                    ui.global::<SettingsModel>().set_c_axis_tilt_deg(tilt_deg);
                    ui.global::<SettingsModel>()
                        .set_c_axis_azimuth_deg(azimuth_deg);
                }
            } else {
                ctx.c_axis_override = None;
                ctx.dirty = true;
                drop(ctx);
            }
            raise_edited(&ui_weak_axis_toggle);
        });

    // Bruted (frosted) girdle finish: a plain on/off switch. See
    // `RenderContext::girdle_frosted`'s doc comment.
    let render_ctx_girdle = render_ctx.clone();
    let ui_weak_girdle = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_girdle_frosted_changed(move |frosted: bool| {
            let mut ctx = RenderContext::lock(&render_ctx_girdle);
            ctx.girdle_frosted = frosted;
            ctx.dirty = true;
            drop(ctx);
            raise_edited(&ui_weak_girdle);
        });

    // Facet edge rounding radius; see `RenderContext::edge_rounding_radius` for the
    // `0.0`-`0.03` range's sourcing.
    let render_ctx_edge = render_ctx.clone();
    let ui_weak_edge = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_edge_rounding_changed(move |radius: f32| {
            let clamped = radius.clamp(0.0, 0.03);
            let mut ctx = RenderContext::lock(&render_ctx_edge);
            ctx.edge_rounding_radius = clamped;
            ctx.dirty = true;
            drop(ctx);
            raise_edited(&ui_weak_edge);
        });

    // Inclusion haze. The slider carries a POSITION (0-1) and `position_to_sigma_s`
    // (`2 * position^2`, finer at the clean end) is the one boundary crossing to the
    // coefficient `scattering_sigma_s` (per model unit, independent of the stone size), which
    // is what the design stores and the label shows.
    let render_ctx_inc = render_ctx.clone();
    let ui_weak_inc = ui.as_weak();
    ui.global::<SettingsModel>()
        .on_stone_inclusion_edited(move |position: f32| {
            let sigma_s = inclusion_scale::position_to_sigma_s(position);
            let mut ctx = RenderContext::lock(&render_ctx_inc);
            ctx.inclusion_sigma_s = sigma_s;
            ctx.dirty = true;
            drop(ctx);
            if let Some(ui) = ui_weak_inc.upgrade() {
                ui.global::<SettingsModel>().set_inclusion_sigma_s(sigma_s);
            }
            raise_edited(&ui_weak_inc);
        });
}

/// A control changed the picture: hold the editor refresh off the controls until the design has
/// the new value, then ask for the debounced commit.
///
/// A library design on the viewport has no design to write: its effects are view-only session
/// values (they started from the defaults when the row was shown, the Stone tab says they are
/// not saved) and the change stays in the render context.
fn raise_edited(ui_weak: &slint::Weak<MainWindow>) {
    let Some(ui) = ui_weak.upgrade() else {
        return;
    };
    if !ui.global::<ViewportModel>().get_shows_editor_design() {
        return;
    }
    stone_effects::mark_live_edit_pending();
    ui.global::<SettingsModel>().invoke_stone_effects_edited();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Choosing the design's own material in "Preview as..." starts no preview, linked or while
    /// already previewing (the design's name is what is compared, not the traced one); any other
    /// pick, or a pick on a library design, still does its job.
    #[test]
    fn choosing_the_designs_own_material_starts_no_preview() {
        assert!(is_designs_own_material(true, "Sapphire", "Sapphire"));
        assert!(!is_designs_own_material(true, "Sapphire", "Ruby"));
        // Previewing Ruby over a Sapphire design: picking Sapphire is the design's own material.
        assert!(is_designs_own_material(true, "Sapphire", "Sapphire"));
        assert!(!is_designs_own_material(true, "Sapphire", "Ruby"));
        assert!(!is_designs_own_material(false, "Sapphire", "Sapphire"));
    }

    #[test]
    fn the_explicit_apply_code_round_trips() {
        for result in [
            ExplicitApply::Design,
            ExplicitApply::Library,
            ExplicitApply::Busy,
        ] {
            assert_eq!(ExplicitApply::from_code(result.code()), result);
        }
    }
}
