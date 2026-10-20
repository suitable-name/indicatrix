//! The Stone, Light and Checks tabs of the Design page, UI-free: materials, stone effects,
//! lighting, and the design's checks (verdict, warnings, proportions, preform and yield).
//!
//! # Where each value lives
//!
//! The split mirrors the desktop and the web app, so a design behaves the same everywhere:
//!
//! | Value | Lives in | Undoable | Saved with |
//! |---|---|---|---|
//! | Material, RI override, specific-gravity override, body color | the design (`Edit::SetMaterial`) | yes | the design file |
//! | Stone width (girdle diameter, mm), preform size | the design (`Edit::SetGirdleDiameterMm`, `Edit::SetPreform`) | yes | the design file |
//! | Inclusion haze, frosted girdle, edge rounding, crystal axis | the design (`Edit::SetRenderEffects`) | yes | the design file |
//! | Lighting rig, exposure, light azimuth/elevation, backdrop, head shadow, surface glare | `MobileSettings::lighting` | no | `settings.toml` |
//! | Render material name | `MobileSettings::material_name` | no | `settings.toml` |
//! | Custom materials, the link flag | [`PropsState`] | no | not saved (custom materials travel in a design file's snapshot) |
//! | The imported HDR map and "HDR Map" as the chosen lighting | [`crate::hdr`] | no | `<files>/hdr/` (picking a rig turns the choice off) |
//!
//! The render material is *linked* to the design by default, as on the web: while
//! [`PropsState::link_material`] is on, the render traces the design's own material and
//! stone width, and `settings.material_name` is only what the next unlinked render uses.
//!
//! # Modules
//!
//! - `materials`: the catalogue rows, picking a material, the RI override, body colors and
//!   custom materials.
//! - `stone`: the Stone tab's values and the stone-effect edits.
//! - `lighting`: the lighting presets, the light values, and the scene settings handed to
//!   the renderer.
//! - `checks`: the verdict and everything beside it, the preform fields and the
//!   one-step fixes.
//! - `number_entry`: the number keypad's state (a field's value and what is typed).
//!
//! Every function that edits the design returns `Result<bool, String>`: `Ok(true)` when
//! the design changed (the caller then runs its after-edit hook: auto-solve and refresh),
//! `Ok(false)` when the value was already what was asked, and `Err` with a sentence for
//! the user when the input was refused (nothing changed).

mod checks;
mod lighting;
mod materials;
mod number_entry;
mod stone;

#[cfg(test)]
mod tests;

use std::sync::Mutex;

use indicatrix::optics::materials::GemMaterial;
use indicatrix_editor::loading::eval_number;
use indicatrix_formats::native::CustomMaterialSnapshot;
use indicatrix_web_core::custom_material::{builtin_name_conflict, builtin_name_message};

pub use checks::{
    ChecksView, FixApplied, FixRequest, PlannedFix, PreformField, PreformForm, ProportionRow,
    WarningRow, apply_planned_fix, checks, fix_request, set_preform_field,
};
pub use lighting::{
    ELEVATION_RANGE_DEG, LightValues, LightingRow, LightingView, backdrop_from_index,
    backdrop_index, lighting_rows, lighting_view, render_settings, set_backdrop, set_light_values,
    set_lighting,
};
pub use materials::{
    CustomMaterialInput, MaterialRow, MaterialSummary, add_custom_material, body_color_available,
    body_color_names, custom_material_from_form, material_rows, material_summary, select_material,
    set_body_color, set_refractive_index_override,
};
pub use number_entry::{NumberEntry, NumberView};
pub use stone::{
    AXIS_AZIMUTH_RANGE_DEG, AXIS_TILT_RANGE_DEG, EDGE_ROUNDING_RANGE, StoneView, set_crystal_axis,
    set_edge_rounding, set_frosted_girdle, set_inclusion_haze, set_stone_width, stone_view,
};

/// The surface-glare scale a fresh settings file starts with: unscaled (100 %). The same
/// value as `LightingSettings::default().surface_glare_pct` (a settings test checks it).
pub const DEFAULT_SURFACE_GLARE_PCT: f32 = 100.0;

/// Material and lighting state that is not part of the design or the settings.
#[derive(Debug)]
pub struct PropsState {
    /// The custom materials of this session: created in the material form or restored from
    /// an opened design file. They are offered after the built-ins and found by name when
    /// the design names one. Register through [`Self::register_custom`].
    pub custom_materials: Vec<GemMaterial>,
    /// What a design file stores for each custom material, by the material's name: the
    /// snapshot the next save must carry while the design's material is that one, and the
    /// specific gravity the carat estimate reads. Kept in step with [`Self::custom_materials`].
    pub custom_snapshots: Vec<(String, CustomMaterialSnapshot)>,
    /// Recipe changes that belong to history steps (opening a variant with another
    /// definition of a custom material), so Undo and Redo bring the recipe along.
    pub recipe_steps: Vec<crate::recipe_steps::RecipeStep>,
    /// Whether the render material follows the design's own (see the module docs). On by
    /// default, as on the web.
    pub link_material: bool,
    /// The last [`checks`] result, kept for the generation it was made for so a refresh
    /// that changed nothing does not rebuild the solid mesh.
    checks_cache: Mutex<Option<checks::CachedChecks>>,
}

impl Default for PropsState {
    fn default() -> Self {
        Self {
            custom_materials: Vec::new(),
            custom_snapshots: Vec::new(),
            recipe_steps: Vec::new(),
            link_material: true,
            checks_cache: Mutex::new(None),
        }
    }
}

impl PropsState {
    /// Adds `material` to the session's catalogue (or replaces the one of the same name,
    /// ignoring case), keeping `snapshot` -- what a design file stores for it -- beside it.
    ///
    /// Call this for a custom material read from an opened design file as well as for one
    /// made in the material form.
    ///
    /// # Errors
    ///
    /// The refusal sentence when `material`'s name is a built-in material's: a custom
    /// material under it would shadow the built-in wherever its name is selected.
    pub fn register_custom(
        &mut self,
        material: GemMaterial,
        snapshot: Option<CustomMaterialSnapshot>,
    ) -> Result<(), String> {
        if let Some(builtin) = builtin_name_conflict(&material.name) {
            return Err(builtin_name_message(&material.name, &builtin));
        }
        let name = material.name.clone();
        self.custom_snapshots
            .retain(|(known, _)| !known.eq_ignore_ascii_case(&name));
        if let Some(snapshot) = snapshot {
            self.custom_snapshots.push((name.clone(), snapshot));
        }
        if let Some(existing) = self
            .custom_materials
            .iter_mut()
            .find(|known| known.name.eq_ignore_ascii_case(&name))
        {
            *existing = material;
        } else {
            self.custom_materials.push(material);
        }
        Ok(())
    }

    /// The custom material called `name` (ignoring case), if there is one.
    #[must_use]
    pub fn find_custom(&self, name: &str) -> Option<&GemMaterial> {
        self.custom_materials
            .iter()
            .find(|known| known.name.eq_ignore_ascii_case(name))
    }

    /// The snapshot a design file must carry for the custom material called `name`
    /// (ignoring case), if one was recorded.
    #[must_use]
    pub fn snapshot_for(&self, name: &str) -> Option<&CustomMaterialSnapshot> {
        self.custom_snapshots
            .iter()
            .find(|(known, _)| known.eq_ignore_ascii_case(name))
            .map(|(_, snapshot)| snapshot)
    }

    /// The `(name, specific gravity)` pairs of the custom materials that have one, in the
    /// shape the yield and carat helpers take.
    #[must_use]
    pub fn custom_specific_gravity(&self) -> Vec<(String, f64)> {
        self.custom_snapshots
            .iter()
            .filter_map(|(name, snapshot)| {
                snapshot
                    .specific_gravity
                    .map(|specific_gravity| (name.clone(), specific_gravity))
            })
            .collect()
    }

    /// Forgets the remembered checks so the next [`checks`] call rebuilds them (use after
    /// something the cache cannot see changed).
    pub fn forget_checks(&self) {
        if let Ok(mut cache) = self.checks_cache.lock() {
            *cache = None;
        }
    }
}

/// `value` with `decimals` places and the trailing zeros (and a bare point) removed:
/// `7.5000` reads `7.5`, `7.0000` reads `7`. What an editable number field shows.
#[must_use]
pub(crate) fn trimmed(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// The number in `text` (arithmetic allowed), `None` for a blank field.
///
/// # Errors
///
/// A sentence naming `label` when the text is not a number or is not positive and finite.
pub(crate) fn parse_optional_positive(label: &str, text: &str) -> Result<Option<f64>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let value = eval_number(text, None).map_err(|error| error.message(label, text))?;
    if value.is_finite() && value > 0.0 {
        Ok(Some(value))
    } else {
        Err(format!("{label} must be a positive, finite number."))
    }
}

/// `value` as an `f32`. The values passed are slider positions and degrees, far inside
/// `f32`'s range, so the narrowing loses nothing a screen could show.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the slider/UI precision; the values passed are slider positions and degrees, far inside f32's range"
)]
pub(crate) const fn narrow(value: f64) -> f32 {
    value as f32
}
