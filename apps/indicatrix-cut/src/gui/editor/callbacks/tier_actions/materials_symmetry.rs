//! The design settings panel's material combo/RI-override, the inferred-material
//! guess "Set material" action, and Symmetry/Mirror Apply (with its live preview).

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, Edit, MaterialSelection, PreformSpec};
use indicatrix_editor::loading::eval_number;
use slint::{ComponentHandle, Model, SharedString};

use crate::{
    EditorModel, MainWindow, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            body_color_combo, guide,
            state::{EditorState, tiers_incomplete_under_proposed_symmetry},
            view::{SolidLastSolved, refresh_editor_panel_stale, submit_preview_replan},
        },
        render::{material::ExplicitApply, render_body_color},
        show_toast,
        solid_preview::preview_state::SolidPreviewState,
    },
};

// The RI-preservation rule for a plain material pick moved to `indicatrix_editor::loading`
// (shared with the web design settings); re-exported at its old path, which
// `super::tests` exercises.
pub(super) use indicatrix_editor::loading::ri_override_for_material_pick;

/// The Symmetry Order field: a positive whole number, or arithmetic that works out to one
/// (`4*2`). `None` for anything else (text that is no number, a fraction, zero, a negative
/// number). A plain whole number reads exactly as it always did.
fn symmetry_order_from_text(text: &str) -> Option<u32> {
    let trimmed = text.trim();
    if let Ok(order) = trimmed.parse::<u32>() {
        return (order >= 1).then_some(order);
    }
    let value = eval_number(trimmed, None).ok()?;
    let whole = value.round();
    if !((value - whole).abs() < 1e-9 && (1.0..=f64::from(u32::MAX)).contains(&whole)) {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "whole is an integer checked to lie in 1..=u32::MAX just above"
    )]
    let order = whole as u32;
    Some(order)
}

/// The design settings panel's material combo + RI override field + color combo --
/// applies [`Edit::SetMaterial`] via [`body_color_combo::material_for_apply`] (the
/// material parse of `parse_design_material_form`), reading the combo's
/// current option list from `editor_material_combo_options` (pushed fresh every
/// refresh, so this always parses against the SAME list the user actually saw). The
/// color combo is authoritative: its index becomes the selection's
/// `body_color_override` (`0` = the material's own color; the trailing "Custom" entry,
/// present only for a triple that matches no preset, keeps the design's own triple).
pub(in crate::gui::editor) fn setup_apply_design_material_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    setup_apply_named_material_callback(ui, state, render_ctx, preview_state, solid_last_solved);
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_design_material(
        move |combo_index: i32, ri_override_text: SharedString, body_color_index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let options: Vec<String> = ui
                .global::<EditorModel>()
                .get_material_combo_options()
                .iter()
                .map(|s| s.to_string())
                .collect();
            let mut st = state.borrow_mut();
            match body_color_combo::material_for_apply(
                combo_index,
                &ri_override_text,
                &options,
                &st.design.material,
                body_color_index,
            ) {
                Ok(mut material) => {
                    // A plain material
                    // pick (no typed RI override -- that path is left alone, it
                    // is an explicit choice) must not silently change what the
                    // exported I line reads, but must also not pin the OUTGOING
                    // material's RI onto the incoming one. See
                    // `ri_override_for_material_pick`'s own doc comment.
                    if material.refractive_index_override.is_none() {
                        material.refractive_index_override = ri_override_for_material_pick(
                            material.name.as_deref(),
                            st.design.material.name.as_deref(),
                            st.design.meta.refractive_index,
                        );
                    }
                    // A zoned material's colour is its zones: the colour combo is greyed
                    // beside it, so its (stale) pick must not be saved on the design.
                    {
                        let zoned = render_ctx.lock().is_ok_and(|ctx| {
                            names_zoned_material(&ctx.custom_materials, material.name.as_deref())
                        });
                        if zoned {
                            clear_colour_fields(&mut material);
                        }
                    }
                    match st.apply(Edit::SetMaterial { material }) {
                        Ok(()) => {
                            // A material change never moves a tier's own mast.
                            refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                            // a Load Selected suggestion banner
                            // ("Set material to X (RI Y)?") left unanswered describes
                            // the material the design HAD at load time -- an explicit
                            // material pick through this combo (a different path than
                            // the banner's own accept/dismiss) must not leave that
                            // stale offer sitting there to later overwrite this pick
                            // if clicked.
                            ui.global::<EditorModel>()
                                .set_material_suggestion_name("".into());
                            ui.global::<EditorModel>()
                                .set_material_suggestion_text("".into());
                            // The guide's "Pick a real material" step completes here.
                            guide::check_progress(&ui, &st);
                            submit_preview_replan(
                                &ui,
                                &render_ctx,
                                &preview_state,
                                &solid_last_solved,
                                &st,
                                BTreeSet::new(),
                                false,
                            );
                        }
                        Err(e) => show_toast(&ui, &e.to_string(), "error"),
                    }
                }
                Err(e) => show_toast(&ui, &e, "error"),
            }
        },
    );
}

/// The selection `current` becomes when the material called `name` is chosen by an explicit
/// action (Save & Apply, the planner's "Use colour"): the new name, the RI rule of a plain
/// material pick (`ri_override_for_material_pick`), and no body-colour override, since the
/// chosen material brings its own colour. The specific-gravity override stays.
fn named_material(current: &MaterialSelection, legacy_ri: f64, name: &str) -> MaterialSelection {
    let mut material = current.clone();
    material.name = Some(name.to_string());
    material.refractive_index_override =
        ri_override_for_material_pick(Some(name), current.name.as_deref(), legacy_ri);
    clear_colour_fields(&mut material);
    material
}

/// Drops every body-colour field of `material` (triple, bands, path scale): the chosen material
/// brings its own colour. Shared by the explicit "use this material" path and, with the
/// the Stone tab's material combo when it picks a zoned material.
fn clear_colour_fields(material: &mut MaterialSelection) {
    material.body_color_override = None;
    material.body_color_bands_override = None;
    material.absorption_path_scale_override = None;
}

/// Whether `name` is a custom material with colour zones in `materials`. Its colour is its
/// zones, so a body-colour override picked beside it must not be stored on the design.
fn names_zoned_material(materials: &[GemMaterial], name: Option<&str>) -> bool {
    name.is_some_and(|name| {
        materials
            .iter()
            .any(|m| m.zoning.is_some() && m.name.eq_ignore_ascii_case(name))
    })
}

/// What one explicit "use this" action asks of the design on the bench
/// ([`apply_explicit_edits`]); a field left `None` asks nothing of that part.
#[derive(Clone, Copy)]
pub(super) struct ExplicitChoice<'a> {
    /// The material to use, by name (the design's material becomes `named_material` of it).
    pub(super) name: Option<&'a str>,
    /// The design's girdle diameter in millimetres; only a positive, finite width counts.
    pub(super) width_mm: Option<f64>,
    /// The preform to give the design (the Rough Planner's planned piece of rough).
    pub(super) preform: Option<&'a PreformSpec>,
}

/// The edits one explicit "use this material / width / preform" action makes to `design`, in
/// the order material, girdle diameter, preform, preform offset; empty when the design has all
/// of it already.
///
/// * the material when `name` is given and the selection [`named_material`] makes of it differs
///   from the design's;
/// * the girdle diameter when `width_mm` is given, positive and finite, and differs (bit for
///   bit) from the design's;
/// * the preform when given and different, and, when a preform is given at all, the preform's
///   vertical offset reset to `0` if the design has one: a planned piece is positioned by its
///   own planes, so an offset left over from the design's old preform would shift it.
pub(super) fn explicit_edits(
    design: &Design,
    name: Option<&str>,
    width_mm: Option<f64>,
    preform: Option<&PreformSpec>,
) -> Vec<Edit> {
    let mut edits = Vec::new();
    if let Some(name) = name {
        let material = named_material(&design.material, design.meta.refractive_index, name);
        if material != design.material {
            edits.push(Edit::SetMaterial { material });
        }
    }
    let width = width_mm.filter(|width| width.is_finite() && *width > 0.0);
    if width.is_some() && width.map(f64::to_bits) != design.girdle_diameter_mm.map(f64::to_bits) {
        edits.push(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: width,
        });
    }
    if let Some(preform) = preform {
        if *preform != design.preform {
            edits.push(Edit::SetPreform {
                preform: preform.clone(),
            });
        }
        if design.preform_y_offset != 0.0 {
            edits.push(Edit::SetPreformYOffset { y_offset: 0.0 });
        }
    }
    edits
}

/// An explicit "use this material / width / preform" action as ONE undoable design edit, never a
/// hidden preview: the material when `choice.name` is given and differs, the girdle diameter when
/// `choice.width_mm` is given, positive and differs, the preform when given and differs (with the
/// preform's vertical offset reset to 0, since a planned piece is positioned by its own planes).
/// See [`explicit_edits`].
///
/// Returns the same [`ExplicitApply`] codes as `EditorModel.apply_named_material`: `Library` when
/// the editor holds no design, `Busy` when the design could not be borrowed (refusal toasted,
/// nothing applied), else `Design` (also when the design already had everything).
///
/// Whether the viewport shows the editor's design is the caller's question, not asked here: the
/// Rough Planner applies a stone to a design it has just loaded, before the load's own refresh has
/// claimed the viewport.
pub(super) fn apply_explicit_edits(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
    choice: &ExplicitChoice<'_>,
) -> ExplicitApply {
    let Ok(mut st) = state.try_borrow_mut() else {
        show_toast(
            ui,
            "The design is busy right now. Try again in a moment.",
            "error",
        );
        return ExplicitApply::Busy;
    };
    if !st.has_design {
        return ExplicitApply::Library;
    }
    let mut edits = explicit_edits(&st.design, choice.name, choice.width_mm, choice.preform);
    // An explicit apply ends a running preview: the picture shows the design again.
    let was_previewing = !ui.global::<ViewportModel>().get_viewport_material_linked();
    if was_previewing {
        ui.global::<ViewportModel>()
            .set_viewport_material_linked(true);
        render_body_color::clear_view_color(render_ctx);
    }
    if edits.is_empty() {
        // The design already has it; only a preview that just ended needs the picture
        // put back on the design's material.
        if was_previewing {
            refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::new());
        }
        return ExplicitApply::Design;
    }
    let edit = if edits.len() == 1 {
        edits.remove(0)
    } else {
        Edit::Batch(edits)
    };
    match st.apply(edit) {
        Ok(()) => {
            // Neither a material, a girdle diameter nor a preform moves a tier's own mast.
            refresh_editor_panel_stale(ui, render_ctx, &st, &BTreeSet::new());
            if choice.name.is_some() {
                // A Load Selected suggestion banner describes the material the design HAD;
                // this explicit pick must not leave that offer to overwrite it.
                ui.global::<EditorModel>()
                    .set_material_suggestion_name("".into());
                ui.global::<EditorModel>()
                    .set_material_suggestion_text("".into());
            }
            guide::check_progress(ui, &st);
            submit_preview_replan(
                ui,
                render_ctx,
                preview_state,
                solid_last_solved,
                &st,
                BTreeSet::new(),
                false,
            );
        }
        Err(e) => show_toast(ui, &e.to_string(), "error"),
    }
    ExplicitApply::Design
}

/// `EditorModel.apply_named_material`: an explicit "use this material" action as ONE undoable
/// design edit (the material, plus the girdle diameter when the caller gives one), never as a
/// hidden preview. Returns an [`ExplicitApply`] code: `Library` when no editable design is on the
/// viewport (a library design is shown, so the caller uses the library view selection instead),
/// `Busy` when the design could not be borrowed (refusal toasted, nothing applied). The work is
/// [`apply_explicit_edits`].
fn setup_apply_named_material_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_named_material(
        move |name: SharedString, width_mm: f32| -> i32 {
            let Some(ui) = ui_weak.upgrade() else {
                return ExplicitApply::Library.code();
            };
            if !ui.global::<ViewportModel>().get_shows_editor_design() {
                return ExplicitApply::Library.code();
            }
            let choice = ExplicitChoice {
                name: Some(name.as_str()),
                width_mm: (width_mm > 0.0).then(|| f64::from(width_mm)),
                preform: None,
            };
            apply_explicit_edits(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
                &choice,
            )
            .code()
        },
    );
}

/// "Set material" on an inferred-material guess (the "Inferred material shown as
/// a guess, never as a fact" principle) -- writes `name` into the design's
/// [`indicatrix_cut_core::MaterialSelection`]
/// as an ordinary, undoable `Edit::SetMaterial`, keeping the design's own
/// current specific-gravity/RI-override fields untouched (only the NAME
/// changes -- this is "confirm the guess", not "reconfigure the material").
/// Once a name is set, `view::refresh_design_settings` stops computing a
/// guess at all (`design.material.name.is_some()`), so the guess label
/// disappears and the next native save carries the confirmed name through
/// `design.material`.
pub(in crate::gui::editor) fn setup_material_guess_set_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_set_material_from_guess(move |name: SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let mut st = state.borrow_mut();
            let mut material = st.design.material.clone();
            material.name = Some(name.to_string());
            match st.apply(Edit::SetMaterial { material }) {
                Ok(()) => {
                    // A material name change never moves a tier's own mast.
                    refresh_editor_panel_stale(&ui, &render_ctx, &st, &BTreeSet::new());
                    // same reasoning as `setup_apply_design_material_callback`'s
                    // matching clear -- this is a second path that sets the design's
                    // material directly, so a stale Load Selected suggestion banner
                    // must not survive it either.
                    ui.global::<EditorModel>()
                        .set_material_suggestion_name("".into());
                    ui.global::<EditorModel>()
                        .set_material_suggestion_text("".into());
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        false,
                    );
                    show_toast(&ui, &format!("Material set to {name}."), "success");
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        });
}

/// The design settings panel's Symmetry/Mirror "Apply" -- wholesale
/// [`Edit::SetSchedule`], keeping the design's CURRENT gear (this control never
/// changes gear -- that's [`setup_gear_apply_callback`]'s job, since only a gear
/// change needs the remap confirmation). Also registers
/// [`EditorModel::on_request_symmetry_preview`]: a live
/// dry-run preview of the SAME proposed change, computed as the Symmetry Order
/// field is edited or Mirror is toggled, so switching (say) 8-fold to 6-fold no
/// longer turns rows amber with no warning and no chance to reconsider before
/// clicking Apply -- mirroring the gear-remap path's own dry-run preview
/// (`gear_remap_preview`/[`setup_gear_apply_callback`]). Registered here rather
/// than as its own `setup_*` function so it can share this function's own
/// `state` clone instead of this module's registration point (`mod.rs`, a
/// different lane's file right now) needing a new call site.
pub(in crate::gui::editor) fn setup_apply_symmetry_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    //    // `on_apply_symmetry` below moves the ORIGINAL `state` binding into its
    // own closure, not after (that closure is `move`, so `state` is gone once
    // it is constructed).
    let state_preview = Rc::clone(&state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_apply_symmetry(
        move |symmetry_order_text: SharedString, mirror: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(symmetry_order) = symmetry_order_from_text(&symmetry_order_text) else {
                show_toast(
                    &ui,
                    "Symmetry order must be a positive whole number.",
                    "error",
                );
                return;
            };
            let mut st = state.borrow_mut();
            let gear_teeth = st.design.meta.gear_teeth;
            match st.apply(Edit::SetSchedule {
                gear_teeth,
                symmetry_order,
                mirror,
            }) {
                Ok(()) => {
                    // Symmetry/mirror can move every tier's index-wheel position, not
                    // tracked precisely here, so force a full (non-blocking) solve --
                    // and trust no cached mast either.
                    refresh_editor_panel_stale(
                        &ui,
                        &render_ctx,
                        &st,
                        &(0..st.design.tiers.len()).collect(),
                    );
                    submit_preview_replan(
                        &ui,
                        &render_ctx,
                        &preview_state,
                        &solid_last_solved,
                        &st,
                        BTreeSet::new(),
                        true,
                    );
                }
                Err(e) => show_toast(&ui, &e.to_string(), "error"),
            }
        },
    );

    //    // comment above.
    let ui_weak_preview = ui.as_weak();
    ui.global::<EditorModel>().on_request_symmetry_preview(
        move |symmetry_order_text: SharedString, mirror: bool| {
            let Some(ui) = ui_weak_preview.upgrade() else {
                return;
            };
            // An unparsable/zero symmetry order can never be applied (see the
            // real `on_apply_symmetry` handler's own validation above) -- no
            // preview to show rather than a stale or misleading one.
            let Some(symmetry_order) = symmetry_order_from_text(&symmetry_order_text) else {
                ui.global::<EditorModel>()
                    .set_symmetry_preview_text(String::new().into());
                return;
            };
            let st = state_preview.borrow();
            let incomplete =
                tiers_incomplete_under_proposed_symmetry(&st.design, symmetry_order, mirror);
            let text = if incomplete == 0 {
                "No tiers would become incomplete orbits.".to_string()
            } else {
                let plural = if incomplete == 1 { "" } else { "s" };
                format!("{incomplete} tier{plural} would become incomplete orbits.")
            };
            ui.global::<EditorModel>()
                .set_symmetry_preview_text(text.into());
        },
    );
}

#[cfg(test)]
mod tests {
    use super::{named_material, symmetry_order_from_text};
    use crate::gui::editor::{body_color_combo::material_for_apply, state::EditorState};
    use indicatrix_cut_core::{Edit, MaterialSelection};

    /// Every way the Stone tab and the explicit actions choose a material is a design edit:
    /// the material combo (every entry) and the named-material path (Save & Apply, Use
    /// colour). Each changes `design.material` through `apply`, and one `undo` puts the design
    /// back, so nothing ever changes only the picture (a "silent unlink"). The one remaining
    /// non-edit path, "Preview as...", is pinned by the source scan in
    /// `tests/parameter_panels.rs`.
    #[test]
    fn every_path_that_chooses_a_design_material_is_an_undoable_edit() {
        let mut state = EditorState::fresh();
        let options = state.material_combo_options(&[]);
        assert!(options.len() > 2, "the combo lists the built-in materials");
        let original = state.design.material.clone();

        let mut applied = 0;
        for index in 0..options.len() {
            let index = i32::try_from(index).expect("a small index");
            let Ok(material) = material_for_apply(index, "", &options, &original, 0) else {
                continue;
            };
            if material == original {
                continue;
            }
            state
                .apply(Edit::SetMaterial {
                    material: material.clone(),
                })
                .expect("a combo pick applies");
            assert!(state.design.material == material, "combo entry {index}");
            assert!(state.undo().expect("undoes"), "combo entry {index} undoes");
            assert!(state.design.material == original, "combo entry {index}");
            applied += 1;
        }
        assert!(applied > 0, "at least one entry changes the material");

        let named = named_material(&original, state.design.meta.refractive_index, "Ruby");
        state
            .apply(Edit::SetMaterial { material: named })
            .expect("an explicit apply applies");
        assert_eq!(state.design.material.name.as_deref(), Some("Ruby"));
        assert!(state.undo().expect("undoes"));
        assert_eq!(state.design.material, original);
    }

    #[test]
    fn an_explicit_material_replaces_the_name_and_the_colour_and_keeps_the_gravity() {
        let current = MaterialSelection {
            name: Some("Sapphire".to_string()),
            specific_gravity_override: Some(4.0),
            refractive_index_override: Some(1.77),
            body_color_override: Some([0.2, 0.4, 2.8]),
            body_color_bands_override: Some(vec![[460.0, 45.0, 0.25]]),
            absorption_path_scale_override: Some(2.0),
        };
        let next = named_material(&current, 1.77, "Ruby colour");
        assert_eq!(next.name.as_deref(), Some("Ruby colour"));
        assert_eq!(next.specific_gravity_override, Some(4.0));
        assert_eq!(next.body_color_override, None);
        assert_eq!(next.body_color_bands_override, None);
        assert_eq!(next.absorption_path_scale_override, None);
        // The outgoing material's RI is never pinned onto the incoming one.
        assert_eq!(next.refractive_index_override, None);
    }

    /// Picking a zoned material from the combo drops the colour combo's override (the same
    /// helper `named_material` uses); a plain material of the same list does not count as zoned.
    #[test]
    fn a_zoned_material_pick_clears_the_colour_fields() {
        use super::{clear_colour_fields, names_zoned_material};
        use indicatrix::optics::{
            absorption::AbsorptionTensor,
            materials::GemMaterial,
            zoning::{ZoneAbsorption, ZonedAbsorption},
        };
        let zoning = ZonedAbsorption::new(ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(
            Vec::new(),
        )));
        let mut zoned = GemMaterial::diamond().with_zoning(zoning);
        zoned.name = "Rough colour".to_string();
        let mut plain = GemMaterial::diamond();
        plain.name = "Plain custom".to_string();
        let list = vec![zoned, plain];
        assert!(names_zoned_material(&list, Some("rough COLOUR")));
        assert!(!names_zoned_material(&list, Some("Plain custom")));
        assert!(!names_zoned_material(&list, Some("Sapphire")));
        assert!(!names_zoned_material(&list, None));

        let mut material = MaterialSelection {
            name: Some("Rough colour".to_string()),
            specific_gravity_override: Some(4.0),
            refractive_index_override: None,
            body_color_override: Some([0.2, 0.4, 2.8]),
            body_color_bands_override: Some(vec![[460.0, 45.0, 0.25]]),
            absorption_path_scale_override: Some(2.0),
        };
        clear_colour_fields(&mut material);
        assert_eq!(material.body_color_override, None);
        assert_eq!(material.body_color_bands_override, None);
        assert_eq!(material.absorption_path_scale_override, None);
        assert_eq!(material.specific_gravity_override, Some(4.0));
    }

    #[test]
    fn a_plain_whole_number_reads_as_before() {
        assert_eq!(symmetry_order_from_text("8"), Some(8));
        assert_eq!(symmetry_order_from_text("  4 "), Some(4));
        assert_eq!(symmetry_order_from_text("1"), Some(1));
    }

    #[test]
    fn arithmetic_that_works_out_to_a_whole_number_is_accepted() {
        assert_eq!(symmetry_order_from_text("4*2"), Some(8));
        assert_eq!(symmetry_order_from_text("(3 + 1) * 2"), Some(8));
        assert_eq!(symmetry_order_from_text("24 / 4"), Some(6));
    }

    #[test]
    fn zero_fractions_negatives_and_text_are_refused() {
        for text in ["0", "-4", "4.5", "9 / 2", "0 * 5", "abc", "", "4 +", "1e12"] {
            assert_eq!(symmetry_order_from_text(text), None, "{text:?}");
        }
    }
}
