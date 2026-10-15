//! Dialog and panel states of the click-through passes that run 6 and `states_more.rs` still left
//! uncovered (round 6, lane R6B): the material editor's sub-pages, the import form of the preview
//! batch, the Preferences tutorial buttons, the empty state's recent file, saved lightings, the
//! live-compute link, the Angle Sweep results and the Guide panel's Back button.
//!
//! Every state reaches its page the way a user would: model flags first, then the opener button.
//! Everything below is an `in` / `in-out` property or a callback of a global in `ui/models/*.slint`.
//! A page that only exists after a long job (a finished sweep) is prepared here, outside the pass,
//! with the smallest input that makes it (three angles); the pass itself keeps the long-job
//! controls (Run Sweep, Full Render, Generate) denied.

use super::{
    super::{Piece, dialogs, existing_install},
    overlay_states::{Base, NONE, State, deny, edit_tab, parameters, press},
    overlays, state_pieces,
};
use crate::support::{
    HarnessApp,
    app::{pump, wait_until},
    smoke::{DenyMatch, DenyRule},
};
use indicatrix_cut::{
    BatchModel, BodyColorEditorModel, DesignLightingModel, DispersionEditorModel, EditorModel,
    GuideModel, PreferencesModel, RemoteAvailabilityItem, RemoteWorkerModel, SettingsModel,
    SweepModel, ViewportModel, harness_support,
};
use indicatrix_formats::native::design::DESIGN_EXTENSION;
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel, platform::WindowEvent};
use std::time::Duration;

/// The pieces of this file: one `overlay:dialogs-*:<state>` piece per state.
pub fn pieces() -> Vec<Piece> {
    let groups: [super::StateGroup; 5] = [
        ("dialogs-material-editor", material_editor_states),
        ("dialogs-batch-preferences", batch_and_preferences_states),
        ("dialogs-empty-guide", empty_state_and_guide_states),
        ("dialogs-lighting-compute", lighting_and_compute_states),
        ("dialogs-sweep", sweep_states),
    ];
    let mut all = Vec::new();
    for (group, states) in groups {
        all.extend(state_pieces(group, existing_install, states));
    }
    all
}

fn state(
    name: &'static str,
    base: Base,
    scope: Option<&'static str>,
    deny: &'static [DenyRule],
    open: fn(&HarnessApp),
) -> State {
    State {
        name,
        base,
        scope,
        deny,
        open,
    }
}

fn strings(items: &[&str]) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|item| SharedString::from(*item))
            .collect::<Vec<_>>(),
    ))
}

// ---------------------------------------------------------------------------------------------
// Deny lists
// ---------------------------------------------------------------------------------------------

/// The preview batch offered after an import: the two render buttons start a batch on the GPU.
const DENY_IMPORT_BATCH: &[DenyRule] = &[
    deny(
        DenyMatch::Contains,
        "full render",
        "long job: BatchModel.preview_generate_confirmed renders previews for the whole set",
    ),
    deny(
        DenyMatch::Contains,
        "quick (solid)",
        "long job: renders solid previews for the whole set",
    ),
    deny(
        DenyMatch::Contains,
        "generate",
        "long job: renders previews for the whole set",
    ),
];

/// Preferences: switching the update check on is a GitHub request.
const DENY_PREFERENCES: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "check for new versions",
    "network: switching it on starts an update check (UpdateModel.enabled_changed)",
)];

const DENY_SWEEP_RUN: &[DenyRule] = &[deny(
    DenyMatch::Contains,
    "run sweep",
    "long job: solves the design at every angle of the range",
)];

// ---------------------------------------------------------------------------------------------
// Material editor
// ---------------------------------------------------------------------------------------------

/// Opens the Gem Material Editor over the Edit view.
fn material_open(a: &HarnessApp) {
    edit_tab(a);
    a.ui()
        .global::<ViewportModel>()
        .set_material_editor_open(true);
    pump(300);
}

/// Saves a custom material through the editor's own callback ("Save & Apply") and selects it, so
/// the dialog opens on it: Delete is enabled and the optics fields hold its values. `false` when
/// the material never appeared in the list.
fn saved_material(
    a: &HarnessApp,
    name: &str,
    crystal: i32,
    character: i32,
    birefringence: f32,
    delta: f32,
    color_json: &str,
) -> bool {
    edit_tab(a);
    let viewport = a.ui().global::<ViewportModel>();
    viewport.invoke_save_custom_material(
        name.into(),
        1.9,
        0.02,
        birefringence,
        0,
        crystal,
        character,
        delta,
        3.5,
        true,
        color_json.into(),
    );
    let position = || {
        viewport
            .get_material_options()
            .iter()
            .position(|option| option.as_str() == name)
    };
    if !wait_until(Duration::from_secs(10), || position().is_some()) {
        return false;
    }
    if let Some(index) = position().and_then(|index| i32::try_from(index).ok()) {
        viewport.set_selected_material_index(index);
    }
    pump(200);
    wait_until(Duration::from_secs(5), || {
        viewport.get_selected_custom_material_valid()
    })
}

/// A saved biaxial material: Delete (the name is an existing custom one), the Overwrite question
/// behind both Save buttons and the "Biaxial n beta - n alpha" slider exist.
fn biaxial_material(a: &HarnessApp) {
    // Orthorhombic, Biaxial (+), birefringence 0.01, n_beta - n_alpha 0.004.
    saved_material(a, "Harness Biaxial", 4, 3, 0.01, 0.004, "");
    material_open(a);
}

/// The Dispersion section opened in Coefficients mode on `kind` (0 Sellmeier 1 term, 1 Sellmeier
/// 3 terms, 2 Cauchy) with these field texts, as Rust pushes them for a stored material.
fn coefficients(a: &HarnessApp, kind: i32, texts: &[&str; 6]) {
    let section = a.ui().global::<DispersionEditorModel>();
    section.set_prefill_mode(1);
    section.set_prefill_kind(kind);
    section.set_prefill_texts(strings(texts));
    material_open(a);
}

/// The body-colour editor ("Pick...") with its hue / saturation / brightness picker open ("Use
/// Picker..."); the plain editor is covered by `states_reach.rs`. Apply Color needs the first
/// solve of the background thread, so the state waits for it.
fn lch_editor(a: &HarnessApp) {
    material_open(a);
    press(a, "Pick\u{2026}", None);
    let colour = a.ui().global::<BodyColorEditorModel>();
    let _ = wait_until(Duration::from_secs(15), || {
        colour.get_has_solution() && !colour.get_busy()
    });
    // The pressed element is the TipButton's inner Button, whose id is not the site's: record the
    // site once the press took.
    if press(a, "Use Picker...", None) {
        crate::support::coverage::record_site_id("LchEditor::tip-button-picker-open");
    }
    pump(250);
}

/// The physics colour section the way a user meets "Update to Current Data" and "Redo": a saved
/// material whose recipe was built with older catalogue data is opened (the badge comes from
/// `ColorMode::data_outdated`, nobody sets it), one edit is made and undone. The edit refreshes the
/// recipe to the current data version, so the undo, which restores the stored recipe exactly, is
/// what brings the badge back; it is also the last action, so Redo stays enabled.
#[cfg(feature = "physical-color")]
fn physics_recipe(a: &HarnessApp) {
    use indicatrix::optics::chromophore::{
        ChromophoreCatalogue, ColorRecipe, ResolvedBands, resolve,
    };
    use indicatrix_cut::PhysicscolorModel;
    use indicatrix_cut_core::material::ColorMode;
    let catalogue = ChromophoreCatalogue::global();
    let mut recipe = ColorRecipe::new("corundum", catalogue.data_version);
    recipe.set_amount("Cr", 0.3);
    if let Ok((tensor, _)) = resolve(&recipe, catalogue) {
        recipe.resolved_bands = ResolvedBands::from_tensor(&tensor);
    }
    recipe.data_version = catalogue.data_version.saturating_sub(1);
    let json = ColorMode::physics(recipe, [0.0; 3]).to_json();
    // Trigonal, uniaxial negative: a ruby-like corundum.
    saved_material(a, "Harness Aged Recipe", 3, 2, -0.008, 0.0, &json);
    material_open(a);
    let model = a.ui().global::<PhysicscolorModel>();
    // The dialog's own open rebuilds the state from the selected material; wait for its push.
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_active() && model.get_data_updated()
    });
    // The controls are pressed here, as a user does: touching the fields afterwards edits the recipe
    // and the badge, the treatment chips and the undo state would be gone before the click pass.
    // 1. The badge's button recomputes the colour from the current data.
    if model.get_data_updated() && !press(a, "Update to Current Data", None) {
        println!(
            "physics_recipe: Update to Current Data not pressed (data_updated {}, active {})",
            model.get_data_updated(),
            model.get_active()
        );
    }
    let _ = wait_until(Duration::from_secs(10), || !model.get_solving());
    // 2. Iron and titanium make a corundum offer its treatments.
    for id in ["Fe", "Ti"] {
        let option = model
            .get_add_options()
            .iter()
            .find(|option| option.enabled && option.id.as_str() == id);
        match option {
            Some(option) => {
                model.invoke_add_item(option.id);
                let _ = wait_until(Duration::from_secs(10), || {
                    !model.get_solving() && model.get_rows().iter().any(|row| row.id.as_str() == id)
                });
            }
            None => println!("physics_recipe: no enabled add option '{id}'"),
        }
    }
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_treatments().row_count() > 0 && !model.get_solving()
    });
    match model.get_treatments().row_data(0) {
        Some(treatment) => {
            if !press(a, treatment.name.as_str(), None) {
                println!(
                    "physics_recipe: treatment chip '{}' not pressed (treatments {}, solving {})",
                    treatment.name,
                    model.get_treatments().row_count(),
                    model.get_solving()
                );
            }
        }
        None => println!(
            "physics_recipe: no treatment offered (rows {}, solving {})",
            model.get_rows().row_count(),
            model.get_solving()
        ),
    }
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_can_undo() && !model.get_solving()
    });
    // 3. Undo; nothing between it and the Redo below may push a new edit.
    if model.get_can_undo() && !press(a, "Undo", Some("Chip")) {
        println!(
            "physics_recipe: Undo not pressed (can_undo {}, solving {})",
            model.get_can_undo(),
            model.get_solving()
        );
    }
    let _ = wait_until(Duration::from_secs(10), || {
        model.get_can_redo() && !model.get_solving()
    });
    // 4. Redo, pressed here too: the host's one field-touch pass edits the recipe after this
    // function, which empties the redo stack before the pass lists its controls.
    if model.get_can_redo() && !press(a, "Redo", Some("Chip")) {
        println!(
            "physics_recipe: Redo not pressed (can_redo {}, solving {})",
            model.get_can_redo(),
            model.get_solving()
        );
    }
    pump(250);
}

/// A zoned material: the zone list and its "relative to stone size" check box.
fn zoned_material(a: &HarnessApp) {
    use indicatrix_cut::{MaterialZoneRow, Zoning};
    let zoning = a.ui().global::<Zoning>();
    zoning.set_material_zone_rows(ModelRc::new(VecModel::from(vec![
        MaterialZoneRow {
            title: "Base zone".into(),
            params: "Everywhere no zone covers".into(),
            swatch: slint::Color::from_rgb_u8(190, 120, 220),
        },
        MaterialZoneRow {
            title: "Zone 2: Cylinder".into(),
            params: "outer radius 3.2".into(),
            swatch: slint::Color::from_rgb_u8(230, 190, 60),
        },
    ])));
    zoning.set_material_relative_to_stone(true);
    material_open(a);
}

fn material_editor_states() -> Vec<State> {
    #[cfg_attr(not(feature = "physical-color"), allow(unused_mut))]
    let mut states = vec![
        state(
            "Material editor, saved biaxial material",
            Base::Design,
            Some("MaterialEditorDialog"),
            NONE,
            biaxial_material,
        ),
        state(
            "Material editor, Sellmeier 3-term coefficients",
            Base::Design,
            Some("MaterialEditorDialog"),
            NONE,
            |a| {
                coefficients(
                    a,
                    1,
                    &[
                        "0.6961663",
                        "0.4079426",
                        "0.8974794",
                        "0.0046791",
                        "0.0135121",
                        "97.934",
                    ],
                );
            },
        ),
        state(
            "Material editor, Sellmeier 1-term coefficients",
            Base::Design,
            Some("MaterialEditorDialog"),
            NONE,
            |a| coefficients(a, 0, &["1.0", "0.01", "", "", "", ""]),
        ),
        state(
            "Material editor, Cauchy coefficients",
            Base::Design,
            Some("MaterialEditorDialog"),
            NONE,
            |a| coefficients(a, 2, &["1.70", "0.01", "0", "", "", ""]),
        ),
        state(
            "Material editor, body-colour editor with the picker",
            Base::Design,
            Some("MaterialEditorDialog"),
            NONE,
            lch_editor,
        ),
    ];
    #[cfg(feature = "physical-color")]
    states.push(state(
        "Material editor, physics colour recipe",
        Base::Design,
        Some("MaterialEditorDialog"),
        NONE,
        physics_recipe,
    ));
    states.push(state(
        "Material editor, zoned material",
        Base::Design,
        Some("MaterialEditorDialog"),
        NONE,
        zoned_material,
    ));
    states
}

// ---------------------------------------------------------------------------------------------
// Preview batch from an import, Preferences tutorials
// ---------------------------------------------------------------------------------------------

/// Scrolls the scroll area inside `scope` with the mouse wheel until a control labelled `label`
/// is enumerable (a control scrolled out of a `ScrollView` is not). Tries both wheel directions.
fn scroll_until_visible(a: &HarnessApp, scope: &str, label: &str) {
    for delta in [-3000.0_f32, 3000.0] {
        if a.main_view().has(label) {
            return;
        }
        let Some(view) = overlays::find_scope(a, scope) else {
            return;
        };
        let area = view
            .root
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first();
        let element = area.as_ref().unwrap_or(&view.root);
        let origin = element.absolute_position();
        let size = element.size();
        let at =
            slint::LogicalPosition::new(origin.x + size.width / 2.0, origin.y + size.height / 2.0);
        let window = a.ui().window();
        window.dispatch_event(WindowEvent::PointerMoved { position: at });
        window.dispatch_event(WindowEvent::PointerScrolled {
            position: at,
            delta_x: 0.0,
            delta_y: delta,
        });
        pump(200);
    }
}

fn batch_and_preferences_states() -> Vec<State> {
    vec![
        state(
            "Preview batch, offered after an import",
            Base::Library,
            Some("PreviewBatchDialog"),
            DENY_IMPORT_BATCH,
            |a| {
                // The import offer adds "Quick (Solid)", "Full Render", "Skip" and the "Remember
                // My Choice" box to the confirm step.
                let batch = a.ui().global::<BatchModel>();
                batch.set_preview_confirming(true);
                batch.set_preview_offer_count(2);
                batch.set_preview_offer_from_import(true);
                batch.set_preview_visible(true);
            },
        ),
        state(
            "Preferences, tutorials section",
            Base::Startup,
            Some("PreferencesDialog"),
            DENY_PREFERENCES,
            |a| {
                // "Reset Tutorial Progress" and "Show the Welcome Tour Again" are the last rows
                // of the dialog's scroll area, below the 600 px card.
                a.ui().global::<PreferencesModel>().set_dialog_open(true);
                pump(250);
                scroll_until_visible(a, "PreferencesDialog", "Reset Tutorial Progress");
            },
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Empty state with a recent file, Guide panel after one step
// ---------------------------------------------------------------------------------------------

/// A saved design file listed under "Open Recent" while no design is showing: the design is made
/// and saved into the profile (which records it as a recent file), then the editor is put back to
/// "no design", so the empty state's recent entry opens a real file when it is pressed.
fn empty_with_recent_file(a: &HarnessApp) {
    edit_tab(a);
    let ui = a.ui();
    if !dialogs::make_template_design(ui) {
        return;
    }
    let Ok(dir) = std::env::current_dir() else {
        return;
    };
    let path = dir.join(format!("harness_recent.{DESIGN_EXTENSION}"));
    harness_support::queue_picker_path(path.clone());
    let editor = ui.global::<EditorModel>();
    editor.invoke_save_native_as();
    let wanted = path.display().to_string();
    let saved = wait_until(Duration::from_secs(15), || {
        path.exists() && editor.get_last_saved_path().as_str() == wanted
    });
    harness_support::clear_picker_answers();
    pump(300);
    if !saved {
        return;
    }
    if ui.get_recent_native_files().row_count() == 0 {
        ui.set_recent_native_files(strings(&[wanted.as_str()]));
    }
    editor.set_has_design(false);
    pump(300);
}

/// The Welcome Tour's second step: Back is enabled.
fn guide_second_step(a: &HarnessApp) {
    let guide = a.ui().global::<GuideModel>();
    guide.invoke_start_guide("welcome-tour".into());
    if wait_until(Duration::from_secs(10), || guide.get_open()) {
        guide.invoke_next();
    }
    pump(300);
}

fn empty_state_and_guide_states() -> Vec<State> {
    vec![
        state(
            "Empty state, recent file listed",
            Base::Startup,
            Some("EmptyState"),
            NONE,
            empty_with_recent_file,
        ),
        state(
            "Guide panel, second step",
            Base::Startup,
            Some("GuidePanel"),
            NONE,
            guide_second_step,
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Saved lightings, live compute with a remote
// ---------------------------------------------------------------------------------------------

/// The Lighting tab with one lighting of the user's own saved and remembered for the design, so
/// its rename and delete buttons and "Forget for This Design" exist. `renaming` presses the
/// pencil, which opens the name field and the "Keep the new name" button.
fn saved_lighting(a: &HarnessApp, renaming: bool) {
    let viewport = a.ui().global::<ViewportModel>();
    viewport.invoke_save_lighting_preset("Harness lighting".into());
    let _ = wait_until(Duration::from_secs(5), || {
        viewport
            .get_lighting_presets()
            .iter()
            .any(|preset| !preset.built_in)
    });
    let design = a.ui().global::<DesignLightingModel>();
    design.invoke_use_for_design();
    let _ = wait_until(Duration::from_secs(5), || design.get_has_saved());
    parameters(a, 1);
    if renaming {
        press(a, "Rename this saved lighting", None);
    }
}

fn lighting_and_compute_states() -> Vec<State> {
    vec![
        state(
            "Parameters: Lighting, saved lighting and design lighting",
            Base::Design,
            Some("ParametersPanel"),
            NONE,
            |a| saved_lighting(a, false),
        ),
        state(
            "Parameters: Lighting, renaming a saved lighting",
            Base::Design,
            Some("ParametersPanel"),
            NONE,
            |a| saved_lighting(a, true),
        ),
        state(
            "Parameters: Render and Output, live compute on the remote",
            Base::Design,
            Some("ParametersPanel"),
            NONE,
            |a| {
                // "Open Remote Settings..." shows with a remote in the loop (Remote Only or
                // Local + Remote). The index is set directly: its change callback would start
                // the remote path.
                a.ui()
                    .global::<RemoteWorkerModel>()
                    .set_availability(RemoteAvailabilityItem {
                        configured: true,
                        reachable_render_capable: true,
                    });
                a.ui()
                    .global::<SettingsModel>()
                    .set_live_compute_target_index(2);
                parameters(a, 2);
            },
        ),
    ]
}

// ---------------------------------------------------------------------------------------------
// Angle Sweep
// ---------------------------------------------------------------------------------------------

/// The Angle Sweep dialog after a finished three-angle sweep with a usable row selected: the
/// figure pills, the chart, the table and the CSV / "Use This Angle" buttons need results. The
/// sweep is run here, outside the pass, over one degree in half-degree steps (a few solves).
fn sweep_with_results(a: &HarnessApp) {
    edit_tab(a);
    let sweep = a.ui().global::<SweepModel>();
    sweep.invoke_open_dialog();
    pump(300);
    if !sweep.get_dialog_open() {
        return;
    }
    let from: f64 = sweep
        .get_from_text()
        .as_str()
        .trim()
        .parse()
        .unwrap_or(30.0);
    sweep.set_to_text(format!("{:.1}", from + 1.0).into());
    sweep.set_step_text("0.5".into());
    sweep.invoke_form_changed();
    pump(100);
    if !sweep.get_form_ok() {
        return;
    }
    sweep.invoke_run();
    let _ = wait_until(Duration::from_secs(90), || {
        sweep.get_has_results() && !sweep.get_running()
    });
    let usable = sweep
        .get_rows()
        .iter()
        .position(|row| row.is_valid && !row.is_current);
    if let Some(row) = usable.and_then(|row| i32::try_from(row).ok()) {
        sweep.invoke_select_row(row);
    }
    pump(250);
}

fn sweep_states() -> Vec<State> {
    vec![
        state(
            "Angle sweep, form fields",
            Base::Design,
            Some("SweepDialog"),
            DENY_SWEEP_RUN,
            |a| {
                edit_tab(a);
                a.ui().global::<SweepModel>().invoke_open_dialog();
                pump(300);
            },
        ),
        state(
            "Angle sweep, finished sweep",
            Base::Design,
            Some("SweepDialog"),
            DENY_SWEEP_RUN,
            sweep_with_results,
        ),
    ]
}
