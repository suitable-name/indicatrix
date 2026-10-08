//! The design settings panel's refresh ([`refresh_design_settings`]), the
//! "linked to design" viewport material sync ([`sync_viewport_material_link`], over
//! `indicatrix_editor::material_lookup::traced_material_for`), the material-guess
//! badge, and the Tier tab's
//! per-facet chip row ([`selected_tier_chips`]/[`push_selected_tier_chips`]).

use super::state::{
    EditorState, ScratchDelta, builtin_preset_names, design_material_index_from_name,
    gear_index_from_teeth, index_chip_items, push_rows, ri_source_text,
};
use crate::{
    EditorModel, IndexChipItem, MainWindow, ViewportModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            body_color_combo,
            material_lookup::{
                EditorMaterialLookup, material_guess, traced_gem_material, traced_material_for,
            },
        },
        render::render_body_color::display_rgb_for_stone,
    },
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, critical_angle_deg};
use indicatrix_editor::material::{body_color_index_for_material, body_color_options};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use std::sync::{Arc, Mutex};

/// The material-guess badge's push half of [`refresh_design_settings`] --
/// split out purely to keep that function under clippy's function-length
/// lint. Inferred material is shown as a guess, never as a fact: when
/// `design.material.name` is `None`
/// (every untouched `.asc` import carries only an `I`-line refractive index,
/// no species), this looks up the nearest built-in preset within tolerance
/// (`indicatrix_editor::material_lookup::material_guess`) and pushes a guess label
/// plus the OTHER close candidates for the tooltip.
/// Once a name IS set, every one of these three properties goes back to
/// `""` -- there is nothing left to guess, and [`super::state::
/// proportion_verdicts`]-style "guess vs fact" confusion is exactly what this
/// exists to prevent.
fn push_material_guess(ui: &MainWindow, design: &Design, n_d: f64) {
    let (text, name, others) = material_guess(design, n_d).unwrap_or_default();
    ui.global::<EditorModel>()
        .set_material_guess_text(text.into());
    ui.global::<EditorModel>()
        .set_material_guess_name(name.into());
    ui.global::<EditorModel>()
        .set_material_guess_other_candidates_text(others.into());
}

/// The Live Render readout for a design's body-color variant: the traced material's
/// name with the color's label (`Sapphire (Yellow)`), or `""` when the design sets
/// no color or nothing traces at all (`traced_material_for`'s refusal) -- the same
/// name `sync_viewport_material_link` traces, so the readout can never name a
/// different stone than the one rendered. Computed whether or not the viewport is
/// linked; the toolbar only shows it while linked. A custom colour's hex is the colour at the
/// current Stone Size (`stone_width_mm`, `0.0` = not set), like the toolbar swatch; it is
/// recomputed with the next editor refresh after the size changes.
fn body_color_readout(design: &Design, custom: &[GemMaterial], stone_width_mm: f32) -> String {
    let rgb = design.material.body_color_override;
    design
        .material
        .body_color_label()
        .map_or_else(String::new, |color| {
            let (name, unresolved) = traced_material_for(design, custom);
            if unresolved.is_some() {
                String::new()
            } else if let Some(rgb) = rgb.filter(|rgb| body_color_combo::is_custom(Some(*rgb))) {
                // A custom colour is named by its on-screen hex, not just "custom color".
                let [r, g, b] = display_rgb_for_stone(rgb, stone_width_mm);
                format!("{name} (Custom #{r:02x}{g:02x}{b:02x})")
            } else {
                format!("{name} ({color})")
            }
        })
}

/// Whether the design's material is a custom material whose color is a physics recipe -- the
/// explicit `RenderContext::custom_material_physics` list, never inferred from the bands.
fn is_physics_material(design: &Design, ctx: &RenderContext) -> bool {
    design.material.name.as_deref().is_some_and(|name| {
        ctx.custom_material_physics
            .iter()
            .any(|n| n.eq_ignore_ascii_case(name))
    })
}

/// The design settings panel's color combo half of [`refresh_design_settings`] --
/// split out purely to keep that function under clippy's function-length lint. The
/// option list is "Material default", the nine presets and the trailing "Custom..." entry
/// ([`body_color_combo`]); it is pushed only while the combo does not hold it yet. The selected index is re-seeded from the design only when
/// `material_changed` (the same [`ScratchDelta::material`] gate as the material combo, so an
/// in-progress pick survives an unrelated refresh); `readout` (see [`body_color_readout`])
/// always.
fn push_body_color_fields(
    ui: &MainWindow,
    design: &Design,
    readout: String,
    physics_material: bool,
    material_changed: bool,
) {
    let model = ui.global::<EditorModel>();
    let options = body_color_options();
    if model.get_body_color_options().row_count() != options.len() {
        model.set_body_color_options(ModelRc::new(VecModel::from(
            options
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    }
    if material_changed {
        model.set_body_color_index(body_color_index_for_material(&design.material));
    }
    model.set_body_color_readout(readout.into());
    // The override replaces the whole absorption tensor, so it is unavailable (greyed out) for a
    // physics material; one that is already set gets a warning rather than a silent override.
    model.set_design_material_is_physics(physics_material);
    model.set_physics_override_conflict(
        physics_material && design.material.body_color_override.is_some(),
    );
}

/// Pushes the design settings panel's state (material combo options and index,
/// RI-override/effective-RI/critical-angle readouts, gear/symmetry/mirror) and,
/// while "linked to design" is on, syncs the shared viewport's render material
/// (and its displayed selection/index) to match -- a display override made while
/// unlinked must never be silently overridden. Shared by
/// [`super::panel::refresh_editor_panel_from_solve`]/
/// [`super::panel_stale::push_stale_content`].
///
/// `viewport_material_linked` alone is the real gate a display override needs.
/// Also requiring `render_view_tab == 1` (the Edit tab itself being shown)
/// would tie the render's material to tab history rather than to the design:
/// switching to Live Render and back would never resync anything until the
/// next edit happened to run this function again.
///
/// Returns this design's effective refractive index so
/// [`super::state::tier_items`]/[`super::state::tier_items_stale`] can reuse the
/// identical value for their per-tier margin/risk column rather than
/// re-deriving it.
///
/// `delta` (from [`EditorState::record_scratch_push`], computed once by the
/// caller) gates the material/gear/symmetry scratch pushes independently, so
/// an edit that only changed one of the three never re-seeds -- and so
/// silently discards any in-progress typing/selection in -- the other two's
/// fields.
///
/// `pub(super)`, not private: shared by `panel.rs`/`panel_stale.rs` (both
/// siblings within `view`), neither of which touches the design settings panel
/// directly otherwise.
pub(super) fn refresh_design_settings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    state: &EditorState,
    delta: &ScratchDelta,
) -> f64 {
    let design = &state.design;

    // Scoped so the `RenderContext` lock (a shared resource the render thread
    // also wants every frame) is held only for the work that actually needs
    // it, and is a real `Drop` guard released at the end of this block rather
    // than sitting locked across the `delta`-gated pushes below, which touch
    // only `design`/`ui` -- clippy::nursery's `significant_drop_tightening`.
    let (n_d, options, color_readout, physics_material) = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let color_readout = body_color_readout(design, &ctx.custom_materials, ctx.stone_width_mm);
        // Custom-catalogue-aware: unlike `effective_refractive_index`,
        // this also resolves a custom material by name before falling back to a built-in
        // or the design's `I` line -- see `ri_source_text` below for the matching
        // "where did this number come from" explanation shown in the inspector.
        let n_d = design.effective_refractive_index_with(&ctx.custom_materials);
        ui.global::<EditorModel>()
            .set_ri_source_text(ri_source_text(&design.material, &ctx.custom_materials).into());
        // Custom materials can change any time, independently of `design` -- this
        // option LIST is always refreshed; only the in-out `*_index`/`*_text`
        // selections below are gated on `delta`.
        let options = state.material_combo_options(&ctx.custom_materials);
        let physics_material = is_physics_material(design, &ctx);
        drop(ctx);
        (n_d, options, color_readout, physics_material)
    };
    push_body_color_fields(ui, design, color_readout, physics_material, delta.material);
    // A second, separate lock acquisition (rather than reusing the guard
    // above): its own last use sits right at this block's own end, so the
    // lock is never held across work -- the `delta`-gated pushes below -- that
    // does not need it either.
    let selected_material_index = {
        let mut ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        sync_viewport_material_link(ui, &mut ctx, design)
    };
    // Set only after the `render_ctx` guard above is dropped -- see
    // `sync_viewport_material_link`'s own doc comment.
    if let Some(idx) = selected_material_index {
        ui.global::<ViewportModel>()
            .set_selected_material_index(idx);
    }
    // `options` is already cached at the `Vec<String>` level
    // (`EditorState::material_combo_options`'s own `MaterialComboCache`, rebuilt
    // only when the catalogue's custom-material names change). Comparing against
    // what is already pushed skips rebuilding a brand-new `ModelRc<VecModel<_>>`
    // from that cached `Vec` (and the Slint-side model reset it triggers) unless
    // the catalogue itself actually changed -- rebuilding unconditionally on every
    // refresh would force the combo box to fully reset its rows on every angle
    // nudge just as much as on an actual material save.
    let current_options = ui.global::<EditorModel>().get_material_combo_options();
    let unchanged = current_options.row_count() == options.len()
        && current_options
            .iter()
            .zip(options.iter())
            .all(|(current, new)| current == new.as_str());
    if !unchanged {
        ui.global::<EditorModel>()
            .set_material_combo_options(ModelRc::new(VecModel::from(
                options
                    .iter()
                    .cloned()
                    .map(SharedString::from)
                    .collect::<Vec<_>>(),
            )));
    }
    // Pushed the same way, so `new_design_dialog.slint`'s New
    // Design material `ComboBox` binds its `model` to `EditorModel.new_material_options`
    // instead of hand-maintaining a literal list in the SAME order as
    // `builtin_preset_names` -- see that function's own doc comment.
    // `builtin_preset_names` never changes at runtime, so this is redundant work on
    // every refresh, not a correctness issue -- matching `material_combo_options`
    // just above rather than adding a one-shot-at-startup special case for one more
    // static list.
    ui.global::<EditorModel>()
        .set_new_material_options(ModelRc::new(VecModel::from(
            builtin_preset_names()
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
    if delta.material {
        ui.global::<EditorModel>()
            .set_material_combo_index(design_material_index_from_name(
                design.material.name.as_deref(),
                &options,
            ));
        ui.global::<EditorModel>().set_ri_override_text(
            design
                .material
                .refractive_index_override
                .map_or_else(String::new, |v| format!("{v:.4}"))
                .into(),
        );
    }
    ui.global::<EditorModel>()
        .set_effective_ri_text(format!("{n_d:.4}").into());
    ui.global::<EditorModel>()
        .set_critical_angle_text(format!("{:.2}\u{b0}", critical_angle_deg(n_d)).into());
    push_material_guess(ui, design, n_d);
    if delta.gear {
        ui.global::<EditorModel>()
            .set_gear_index(gear_index_from_teeth(design.meta.gear_teeth));
        ui.global::<EditorModel>()
            .set_gear_custom_text(design.meta.gear_teeth.to_string().into());
    }
    if delta.symmetry {
        ui.global::<EditorModel>()
            .set_symmetry_order_text(design.meta.symmetry_order.to_string().into());
        ui.global::<EditorModel>().set_mirror(design.meta.mirror);
        // Whatever the last typed-but-not-yet-applied preview
        // said is no longer meaningful once the fields are freshly reseeded from
        // the real (just-applied, or freshly loaded) design -- a value that
        // matches the live design has nothing left to preview.
        ui.global::<EditorModel>()
            .set_symmetry_preview_text("".into());
    }
    if delta.meta {
        // `design.meta.headers`' first entry is the design's
        // title (the catalogue round-trip convention -- see `EditorModel.
        // design_title`'s own doc comment, `ui/models/editor.slint`), every
        // further entry an "extra" header line, `';'`-joined back for the
        // form's single-line field -- `apply_design_meta`'s own inverse split.
        let (title, extra_headers) = design
            .meta
            .headers
            .split_first()
            .map_or((String::new(), String::new()), |(title, rest)| {
                (title.clone(), rest.join(";"))
            });
        ui.global::<EditorModel>().set_design_title(title.into());
        ui.global::<EditorModel>()
            .set_design_extra_headers(extra_headers.into());
        ui.global::<EditorModel>()
            .set_design_footnotes(design.meta.footnotes.join(";").into());
        ui.global::<EditorModel>().set_design_gear_reference_angle(
            format!("{}", design.meta.gear_reference_angle).into(),
        );
    }

    n_d
}

/// The design-settings panel's "linked to design" viewport sync -- see
/// [`refresh_design_settings`]'s doc comment for why there is no separate
/// `render_view_tab` gate. Split out purely to keep that function under clippy's
/// function-length lint.
///
/// Returns the Render Material dropdown's
/// resolved index instead of setting `ViewportModel.selected_material_index`
/// itself. That setter's `changed` handler runs a DB query
/// (`custom_materials.rs`), and every caller holds `ctx` -- the `render_ctx`
/// mutex the render thread also locks every frame -- across this function's
/// call; setting the property from inside that guard stalls the render
/// thread on every refresh. Callers must set the returned index themselves,
/// after the guard on `ctx` has been dropped.
#[must_use]
pub(in crate::gui::editor) fn sync_viewport_material_link(
    ui: &MainWindow,
    ctx: &mut RenderContext,
    design: &Design,
) -> Option<i32> {
    if !ui.global::<ViewportModel>().get_viewport_material_linked() {
        return None;
    }
    let (name, unresolved) = traced_material_for(design, &ctx.custom_materials);
    if ctx.material_unresolved != unresolved {
        ctx.material_unresolved.clone_from(&unresolved);
        ctx.dirty = true;
    }
    // The resolved material itself, not merely its name:
    // `context::resolve_material` can only look a name up in the built-in table,
    // so a catalogue custom material or a typed RI override never reached the
    // trace at all. `material_override` is what the tracer, the HUD metrics, the
    // tilt sweep and the hover preview all prefer over that name lookup.
    //
    // Keyed on `name` -- the material this design is traced AS, decided by
    // `traced_material_for` just above -- and NOT on `design.material`. Passing the
    // raw selection here sent it through `MaterialSelection::resolve`, whose
    // no-name fallback is `GemMaterial::diamond()`: every `.asc` import and every
    // new design names no material, so the override was diamond, the override beats
    // the name, and the stone rendered with diamond's (empty) absorption bands --
    // colorless whatever the schedule said. `traced_gem_material` returns `None`
    // instead of substituting, and an absent override falls through to
    // `context::resolve_material`'s own by-name lookup, so the name and the override
    // can no longer describe two different stones. The design's body-color variant
    // rides along here too (`MaterialSelection::apply_overrides`, inside
    // `traced_gem_material`): `material_name` stays the species ("Sapphire"), the
    // override carries its recolored absorption.
    let resolved = unresolved
        .is_none()
        .then(|| {
            traced_gem_material(
                &name,
                &design.material,
                &EditorMaterialLookup::new(&ctx.custom_materials),
            )
        })
        .flatten();
    if ctx.material_override != resolved {
        ctx.material_override = resolved;
        ctx.dirty = true;
    }
    if ctx.material_name != name {
        // Cloned, not moved -- `name` is looked up again just below to sync
        // the dropdown's own selected index. `clone_from` reuses the existing
        // allocation instead of dropping it for a fresh one.
        ctx.material_name.clone_from(&name);
        ctx.dirty = true;
    }
    // Keeps the Render Material dropdown's own displayed selection in sync with
    // the material actually being traced on every refresh. Writing this only
    // once, at startup (`startup_settings::apply_saved_settings`), would let the
    // dropdown go on showing e.g. "Diamond" long after `ctx.material_name` (and
    // so the trace, and the tilt dialog's staleness check, both of which read
    // `ViewportModel.selected_material_index`/`material_options`) had moved to
    // something else entirely.
    let options = ui.global::<ViewportModel>().get_material_options();
    let selected_material_index = crate::gui::startup_settings::find_option_index(&options, &name);
    // The design's own real girdle diameter, under the same link gate as the
    // material above -- without this write, `context::apply_material_overrides`
    // would always skip absorption-path scaling (treating every design as if it
    // had no physical size), and the Yield panel's millimetre carat estimate
    // would have no counterpart in the render's own color depth.
    let stone_width_mm = design.girdle_diameter_mm.unwrap_or(0.0) as f32;
    if (ctx.stone_width_mm - stone_width_mm).abs() > f32::EPSILON {
        ctx.stone_width_mm = stone_width_mm;
        ctx.dirty = true;
    }
    selected_material_index
}

/// Builds the chip row for whichever tier the inspector's Tier tab currently has
/// loaded (`EditorModel.selected_tier_index`; `None` for "Add Tier" or an
/// out-of-range value) -- the per-facet editing surface. Computed
/// fresh from `design` on the UI thread every refresh, for only the one tier
/// being edited, rather than for every row: see [`IndexChipItem`]'s own doc
/// comment for why this can never be a field on the (background-thread-built,
/// `Send`) [`crate::EditorTierItem`] instead.
#[must_use]
fn selected_tier_chips(design: &Design, selected_tier_index: i32) -> Vec<IndexChipItem> {
    usize::try_from(selected_tier_index)
        .ok()
        .and_then(|idx| design.tiers.get(idx))
        .map_or_else(Vec::new, |tier| {
            index_chip_items(&tier.indices, &tier.detached)
        })
}

/// Pushes [`selected_tier_chips`]'s result into `EditorModel.selected_tier_chips`,
/// plus the currently selected tier's own cheater-offset text
/// into `EditorModel.selected_tier_cheater_offset_text` -- shared by
/// [`super::panel::refresh_editor_panel`]/[`super::panel_stale::push_stale_content`]/
/// `callbacks::tier_actions::apply_selected_tier_change` (a plain row-click/
/// facet-click selection change, with no edit of its own) so both stay current
/// after every one of the three ways the selected tier can change, exactly like
/// `tiers` itself.
pub(in crate::gui::editor) fn push_selected_tier_chips(ui: &MainWindow, state: &EditorState) {
    let selected = ui.global::<EditorModel>().get_selected_tier_index();
    let cheater_offset_text = usize::try_from(selected)
        .ok()
        .and_then(|index| state.design.cheater_offset_deg(index))
        .map_or_else(String::new, |deg| format!("{deg:.2}"));
    ui.global::<EditorModel>()
        .set_selected_tier_cheater_offset_text(cheater_offset_text.into());
    let note_text = usize::try_from(selected)
        .ok()
        .and_then(|index| state.design.tier_note(index))
        .map_or_else(String::new, str::to_string);
    ui.global::<EditorModel>()
        .set_selected_tier_note_text(note_text.into());
    let chips = selected_tier_chips(&state.design, selected);
    push_rows(
        &ui.global::<EditorModel>().get_selected_tier_chips(),
        chips,
        |model| ui.global::<EditorModel>().set_selected_tier_chips(model),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::geometry::meet_solver::MeetConstraint;
    use indicatrix_cut_core::ConstraintTier;

    fn design_with_named_tiers(names: &[&str]) -> Design {
        let mut state = EditorState::fresh();
        for &name in names {
            state.design.tiers.push(ConstraintTier {
                angle_deg: -40.0,
                name: name.to_string(),
                indices: vec![0.0],
                constraint: MeetConstraint::MeetExisting,
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            });
        }
        state.session.design
    }

    // --- selected_tier_chips ---

    #[test]
    fn selected_tier_chips_is_empty_when_nothing_is_selected() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(selected_tier_chips(&design, -1).len(), 0);
    }

    #[test]
    fn selected_tier_chips_is_empty_for_an_out_of_range_index() {
        let design = design_with_named_tiers(&["G1"]);
        assert_eq!(selected_tier_chips(&design, 5).len(), 0);
    }

    #[test]
    fn selected_tier_chips_reads_the_selected_tiers_own_indices_and_detached_set() {
        let mut design = design_with_named_tiers(&["G1", "P1"]);
        design.tiers[1].indices = vec![0.0, 24.0];
        design.tiers[1].detached = vec![24.0];
        let chips = selected_tier_chips(&design, 1);
        assert_eq!(chips.len(), 2);
        assert!(!chips[0].detached);
        assert!(chips[1].detached);
    }
}
