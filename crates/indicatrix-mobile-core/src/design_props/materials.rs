//! The material catalogue and the edits that choose a material: the rows the picker
//! lists, picking one, the refractive-index override, the body color and the custom
//! materials made in the compact form.
//!
//! A material pick is one undoable `Edit::SetMaterial` on the design, and it also records
//! the pick as the render material name in the settings (the render follows the design
//! while the link is on, see the parent module). Everything here reads and writes through
//! the shared editor helpers, so the rules (what an RI pick keeps, when a typed color
//! survives a species change) are the desktop's.

use std::sync::OnceLock;

use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{
    Edit, MaterialLookup, MaterialSelection, built_in_specific_gravity, critical_angle_deg,
};
use indicatrix_editor::{
    loading::{eval_number, ri_override_for_material_pick},
    material::{body_color_from_index, body_color_index_for_material, body_color_options},
    material_lookup::{EditorMaterialLookup, traced_material_for},
};
use indicatrix_web_core::custom_material::{
    BuiltCustomMaterial, CustomMaterialForm, build_custom_material,
};

use super::trimmed;
use crate::app_state::AppState;

/// How many body colors the phone offers: `Material Default` and the nine presets. The
/// desktop's trailing "Custom..." entry needs the L*C*h editor and is not offered.
const PICKABLE_COLORS: usize = 10;

/// The index of the last body color the phone offers.
const LAST_PICKABLE_COLOR: i32 = 9;

/// One line of the material picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialRow {
    /// The material's name, exactly as the design stores it.
    pub name: String,
    /// The refractive index at the sodium D line, three decimals.
    pub ri_text: String,
    /// The dispersion (`n_F - n_C`), three decimals.
    pub dispersion_text: String,
    /// The specific gravity, two decimals; empty when none is on file.
    pub sg_text: String,
    /// Whether this is a custom material of the session.
    pub is_custom: bool,
    /// Whether this is the design's material.
    pub is_selected: bool,
}

impl MaterialRow {
    /// The supporting line of the row: `RI 2.417 · Dispersion 0.025 · SG 3.52` (no SG part
    /// when none is on file).
    #[must_use]
    pub fn detail_text(&self) -> String {
        let mut text = format!(
            "RI {} \u{b7} Dispersion {}",
            self.ri_text, self.dispersion_text
        );
        if !self.sg_text.is_empty() {
            text.push_str(" \u{b7} SG ");
            text.push_str(&self.sg_text);
        }
        text
    }
}

/// What the Stone tab's material row shows for the design's current material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialSummary {
    /// The material's name, or `Not Set`.
    pub title: String,
    /// Its optics in one line, or the sentence saying what to do when none is set.
    pub detail: String,
}

/// The catalogue facts of one built-in material.
struct BuiltIn {
    name: String,
    ri: f32,
    dispersion: f32,
    specific_gravity: Option<f64>,
}

/// Every built-in material, alphabetical (ignoring case). Built once: the table builders
/// allocate, and the picker reads it on every keystroke of the search field.
fn built_ins() -> &'static [BuiltIn] {
    static TABLE: OnceLock<Vec<BuiltIn>> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table: Vec<BuiltIn> = GemMaterial::all_materials()
            .iter()
            .map(|gem| BuiltIn {
                name: gem.name.clone(),
                ri: gem.dispersion.n_d(),
                dispersion: gem.dispersion.delta_f_c(),
                specific_gravity: built_in_specific_gravity(&gem.name)
                    .map(|specific_gravity| specific_gravity.representative),
            })
            .collect();
        table.sort_by_key(|material| material.name.to_ascii_lowercase());
        table
    })
}

/// The picker rows whose name contains `query` (ignoring case; a blank query lists
/// everything): the built-in materials alphabetically, then the session's custom ones.
#[must_use]
pub fn material_rows(state: &AppState, query: &str) -> Vec<MaterialRow> {
    let needle = query.trim().to_lowercase();
    let selected = state.session.design.material.name.as_deref();
    let is_selected = |name: &str| selected.is_some_and(|own| own.eq_ignore_ascii_case(name));
    let matches = |name: &str| needle.is_empty() || name.to_lowercase().contains(&needle);

    let built_in = built_ins()
        .iter()
        .filter(|material| matches(&material.name))
        .map(|material| MaterialRow {
            name: material.name.clone(),
            ri_text: format!("{:.3}", material.ri),
            dispersion_text: format!("{:.3}", material.dispersion),
            sg_text: material
                .specific_gravity
                .map_or_else(String::new, |value| format!("{value:.2}")),
            is_custom: false,
            is_selected: is_selected(&material.name),
        });
    let mut customs: Vec<&GemMaterial> = state
        .props
        .custom_materials
        .iter()
        .filter(|gem| matches(&gem.name))
        .collect();
    customs.sort_by_key(|gem| gem.name.to_ascii_lowercase());
    let custom = customs.into_iter().map(|gem| MaterialRow {
        name: gem.name.clone(),
        ri_text: format!("{:.3}", gem.dispersion.n_d()),
        dispersion_text: format!("{:.3}", gem.dispersion.delta_f_c()),
        sg_text: state
            .props
            .snapshot_for(&gem.name)
            .and_then(|snapshot| snapshot.specific_gravity)
            .map_or_else(String::new, |value| format!("{value:.2}")),
        is_custom: true,
        is_selected: is_selected(&gem.name),
    });
    built_in.chain(custom).collect()
}

/// The name of the built-in or custom material called `wanted` (ignoring case), spelled
/// the way the catalogue spells it.
fn known_material_name(state: &AppState, wanted: &str) -> Option<String> {
    built_ins()
        .iter()
        .map(|material| material.name.as_str())
        .chain(
            state
                .props
                .custom_materials
                .iter()
                .map(|gem| gem.name.as_str()),
        )
        .find(|name| name.eq_ignore_ascii_case(wanted))
        .map(str::to_owned)
}

/// Applies `next` as the design's material: one undoable edit, none when it equals the
/// current selection.
///
/// # Errors
///
/// The editor's refusal as a sentence.
pub(super) fn apply_selection(
    state: &mut AppState,
    next: MaterialSelection,
) -> Result<bool, String> {
    if next == state.session.design.material {
        return Ok(false);
    }
    state
        .session
        .apply(Edit::SetMaterial { material: next })
        .map(|_| true)
        .map_err(|error| error.to_string())
}

/// Makes `name` the design's material (one undoable edit) and the render material name of
/// the settings.
///
/// A plain pick keeps the design's specific-gravity override, keeps a typed color only
/// while the species stays the same, and clears the RI override -- except that a design
/// that had no material yet keeps its exported refractive index when the pick's own
/// differs (`ri_override_for_material_pick`), so a pick never silently rewrites the
/// schedule's `I` line.
///
/// # Errors
///
/// A sentence when `name` is not a built-in or session material, or the editor refuses.
pub fn select_material(state: &mut AppState, name: &str) -> Result<bool, String> {
    let wanted = name.trim();
    let canonical = known_material_name(state, wanted)
        .ok_or_else(|| format!("'{wanted}' is not a known material."))?;
    let current = state.session.design.material.clone();
    let legacy_ri = state.session.design.meta.refractive_index;
    let same = current
        .name
        .as_deref()
        .is_some_and(|own| own.eq_ignore_ascii_case(&canonical));
    let next = MaterialSelection {
        name: Some(canonical.clone()),
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override: if same {
            current.refractive_index_override
        } else {
            ri_override_for_material_pick(Some(&canonical), current.name.as_deref(), legacy_ri)
        },
        body_color_override: current.body_color_override.filter(|_| same),
        body_color_bands_override: current.body_color_bands_override.clone().filter(|_| same),
        absorption_path_scale_override: current.absorption_path_scale_override.filter(|_| same),
    };
    let changed = apply_selection(state, next)?;
    state.update_settings(|settings| settings.material_name = canonical);
    Ok(changed)
}

/// Sets the design's refractive-index override from `text`; blank clears it (the
/// material's own index applies). Arithmetic is allowed (`1.5+0.25`).
///
/// # Errors
///
/// A sentence when the text is not a finite number above 1.0, or the editor refuses.
pub fn set_refractive_index_override(state: &mut AppState, text: &str) -> Result<bool, String> {
    let value = if text.trim().is_empty() {
        None
    } else {
        let value =
            eval_number(text, None).map_err(|error| error.message("Refractive index", text))?;
        if !value.is_finite() || value <= 1.0 {
            return Err(
                "Refractive index override must be a finite number greater than 1.0.".to_owned(),
            );
        }
        Some(value)
    };
    let mut next = state.session.design.material.clone();
    next.refractive_index_override = value;
    apply_selection(state, next)
}

/// The names of the body colors the phone offers, `Material Default` first; the index of
/// a name is what [`set_body_color`] takes.
#[must_use]
pub fn body_color_names() -> Vec<String> {
    body_color_options()
        .into_iter()
        .take(PICKABLE_COLORS)
        .collect()
}

/// The design material as the tracer resolves it: a custom material first, then a
/// built-in; a material-less design is resolved by its refractive index when a built-in is
/// close enough. `None` when nothing resolves.
pub(super) fn resolved_gem(state: &AppState) -> Option<GemMaterial> {
    let design = &state.session.design;
    let name = design
        .material
        .name
        .clone()
        .unwrap_or_else(|| traced_material_for(design, &state.props.custom_materials).0);
    if name.is_empty() {
        return None;
    }
    EditorMaterialLookup::new(&state.props.custom_materials).lookup(&name)
}

/// Whether a body color can be set: not for a material whose color comes from its zones.
#[must_use]
pub fn body_color_available(state: &AppState) -> bool {
    resolved_gem(state).is_none_or(|gem| gem.zoning.is_none())
}

/// The label of the design's body color: `Material Default`, a preset's name or `Custom`.
#[must_use]
pub(super) fn body_color_label(state: &AppState) -> String {
    let index = body_color_index_for_material(&state.session.design.material);
    usize::try_from(index)
        .ok()
        .and_then(|position| body_color_options().into_iter().nth(position))
        .filter(|_| index <= LAST_PICKABLE_COLOR)
        .unwrap_or_else(|| "Custom".to_owned())
}

/// Sets the design's body color to entry `index` of [`body_color_names`] (`0` restores
/// the material's own color).
///
/// # Errors
///
/// A sentence when `index` is out of range, the material's color comes from its zones, or
/// the editor refuses.
pub fn set_body_color(state: &mut AppState, index: i32) -> Result<bool, String> {
    if !(0..=LAST_PICKABLE_COLOR).contains(&index) {
        return Err("That color is not available.".to_owned());
    }
    if !body_color_available(state) {
        return Err(
            "This material's color comes from its zones, so it cannot be recolored.".to_owned(),
        );
    }
    let next = state
        .session
        .design
        .material
        .clone()
        .with_body_color(body_color_from_index(index));
    apply_selection(state, next)
}

/// The Stone tab's material row for the design as it stands.
#[must_use]
pub fn material_summary(state: &AppState) -> MaterialSummary {
    let design = &state.session.design;
    let Some(name) = design.material.name.clone() else {
        let (guess, _) = traced_material_for(design, &state.props.custom_materials);
        let detail = if guess.is_empty() {
            "Pick a material to score and render this design.".to_owned()
        } else {
            format!(
                "Looks like {guess} (refractive index {}).",
                trimmed(design.effective_refractive_index(), 4)
            )
        };
        return MaterialSummary {
            title: "Not Set".to_owned(),
            detail,
        };
    };
    let custom_sg = state.props.custom_specific_gravity();
    let lookup =
        EditorMaterialLookup::new(&state.props.custom_materials).with_specific_gravity(&custom_sg);
    let specific_gravity = design.material.effective_specific_gravity_with(&lookup);
    let detail = lookup.lookup(&name).map_or_else(
        || "This material is not in the catalogue.".to_owned(),
        |gem| {
            let optics = format!(
                "RI {:.3} \u{b7} Dispersion {:.3}",
                gem.dispersion.n_d(),
                gem.dispersion.delta_f_c()
            );
            specific_gravity.map_or_else(
                || optics.clone(),
                |value| format!("{optics} \u{b7} SG {value:.2}"),
            )
        },
    );
    MaterialSummary {
        title: name,
        detail,
    }
}

/// The effective refractive index and critical angle line of the Stone tab, empty when the
/// design has no usable index.
#[must_use]
pub(super) fn effective_ri_text(state: &AppState) -> String {
    let index = state
        .session
        .design
        .effective_refractive_index_with(&state.props.custom_materials);
    if index.is_finite() && index > 1.0 {
        format!(
            "Effective RI {index:.4} \u{b7} Critical angle {:.2}\u{b0}",
            critical_angle_deg(index)
        )
    } else {
        String::new()
    }
}

/// The compact custom-material form as typed (all fields text, as on the web).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CustomMaterialInput {
    /// The material's name.
    pub name: String,
    /// Refractive index (sodium D).
    pub ri: String,
    /// Dispersion, `n_F - n_C`; blank reads as 0.
    pub dispersion: String,
    /// Birefringence (signed); blank reads as 0.
    pub birefringence: String,
    /// Specific gravity for the carat estimate; blank means not recorded.
    pub specific_gravity: String,
    /// The color: an index into [`body_color_names`] (`0` is clear).
    pub color_index: i32,
}

/// Builds the material (and the snapshot a design file stores for it) the form describes.
///
/// # Errors
///
/// A sentence for a blank name, a name of a built-in material, or a number that is missing,
/// not a number or outside its range (the refractive index must lie between 1.30 and 3.20).
pub fn custom_material_from_form(
    form: &CustomMaterialInput,
) -> Result<BuiltCustomMaterial, String> {
    build_custom_material(&CustomMaterialForm {
        name: &form.name,
        ri: &form.ri,
        dispersion: &form.dispersion,
        birefringence: &form.birefringence,
        specific_gravity: &form.specific_gravity,
        color_index: form.color_index,
    })
}

/// Builds the custom material from `form`, adds it to the session's catalogue and makes
/// it the design's material (one undoable edit, no RI override so the new index shows).
///
/// # Errors
///
/// A sentence when the form is refused (see [`custom_material_from_form`]) or the editor
/// refuses the edit; the catalogue is left as it was then.
pub fn add_custom_material(
    state: &mut AppState,
    form: &CustomMaterialInput,
) -> Result<bool, String> {
    let built = custom_material_from_form(form)?;
    let name = built.material.name.clone();
    let previous: Option<(GemMaterial, Option<_>)> = state
        .props
        .find_custom(&name)
        .cloned()
        .map(|material| (material, state.props.snapshot_for(&name).cloned()));
    state
        .props
        .register_custom(built.material, Some(built.snapshot))?;

    let mut selection = state.session.design.material.clone();
    selection.name = Some(name.clone());
    selection.refractive_index_override = None;
    match apply_selection(state, selection) {
        Ok(changed) => {
            state.update_settings(|settings| settings.material_name = name);
            Ok(changed)
        }
        Err(message) => {
            // Put the catalogue back the way it was.
            state
                .props
                .custom_materials
                .retain(|gem| !gem.name.eq_ignore_ascii_case(&name));
            state
                .props
                .custom_snapshots
                .retain(|(known, _)| !known.eq_ignore_ascii_case(&name));
            if let Some((material, snapshot)) = previous {
                // The name was a custom material's before; registering it again cannot be
                // refused (it passed once).
                let _ = state.props.register_custom(material, snapshot);
            }
            Err(message)
        }
    }
}
