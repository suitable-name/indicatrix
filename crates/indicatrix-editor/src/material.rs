//! Material/RI and gear-tooth view-model helpers.
//!
//! The built-in and design material combo lists, the RI-source explanation text, and the
//! gear preset/remap-preview helpers the design settings panel's gear control uses.

use crate::loading::number_expr::eval_number;
use indicatrix::optics::materials::{
    GemMaterial,
    body_color::{BODY_COLOR_PRESETS, preset_index_for_rgb},
};
use indicatrix_cut_core::{Design, Edit, MaterialSelection, RemapRounding};

/// The gear combo's fixed pill choices, before the combo's own trailing "Custom"
/// entry -- shared by the design settings panel's gear control and the new-design
/// dialog so both present the same list.
pub const GEAR_PRESETS: [i32; 6] = [96, 80, 77, 72, 64, 120];

/// One row of the gear-remap confirmation panel -- see [`gear_remap_preview`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GearRemapPreviewRow {
    /// The tier's name.
    pub name: String,
    /// The tier's current index list, `", "`-joined.
    pub old_indices: String,
    /// The index list confirming the remap would write, `", "`-joined.
    pub new_indices: String,
    /// Whether some position's ideal new value is not a whole tooth.
    pub non_integral: bool,
}

impl MaterialComboCache {
    /// [`design_material_options`], rebuilt only when `custom`'s own name list has
    /// changed since the last call -- see this type's own doc comment. A design
    /// settings refresh calls this every time; only a save/delete/rename in the
    /// material editor (which changes `custom`'s names) actually pays for a rebuild.
    pub fn options(&mut self, custom: &[GemMaterial]) -> Vec<String> {
        let signature: Vec<String> = custom.iter().map(|m| m.name.clone()).collect();
        if self.signature.as_ref() != Some(&signature) {
            self.options = design_material_options(custom);
            self.signature = Some(signature);
        }
        self.options.clone()
    }
}

/// The material-combo cache: [`design_material_options`]'s last result, plus the
/// custom-material name list (in `custom_materials` order) it was built from.
///
/// `view::refresh_design_settings` only calls
/// `design_material_options(&ctx.custom_materials)` again when the combo's actual
/// contents change -- a custom material saved, deleted, or renamed in the
/// material editor dialog -- rather than on every editor refresh: every other
/// refresh (an angle nudge, a solve, a tier edit) would otherwise rebuild the same
/// now-33-plus-customs-entry `Vec<String>` for nothing. `signature` is only the
/// NAME list, not the full `GemMaterial`: `design_material_options` lists names
/// only (an RI/absorption edit to an existing custom material changes no name in
/// the list, so it cannot change this combo's contents either).
///
/// `signature` is `None` until the first build, not a plain `Vec` default: with no
/// custom materials in the vault the incoming name list is empty, which would
/// equal an empty default signature, so the cache would never rebuild and the
/// combo would receive an EMPTY model -- Slint's `ComboBox` then clears
/// `current-value` to `""`, which shows as a blank Material box for every loaded
/// design.
#[derive(Default)]
pub struct MaterialComboCache {
    signature: Option<Vec<String>>,
    options: Vec<String>,
}

/// The material `ComboBox`'s preset names, in EXACT index order.
///
/// Index 0 is `"(none)"`; every following index is a
/// [`indicatrix_cut_core::MaterialCatalogue`] built-in name, in that catalogue's own
/// order (which is `GemMaterial::all_materials`'s order, unchanged by this function).
///
/// This lists every built-in species the renderer supports, not just a
/// hand-picked THIRTEEN-name subset (`"Diamond"`..`"Cubic Zirconia"`) of the
/// renderer's THIRTY-TWO built-ins -- every garnet, Aquamarine, Morganite,
/// Chrysoberyl (Yellow), Amethyst, Citrine, Peridot, YAG, GGG, Benitoite,
/// Andalusite, Opal, both glasses and Rutile can already be traced in Live
/// Render (`gui::startup_settings::built_in_material_option_names` already reads
/// `GemMaterial::all_materials()` directly), and can also be NAMED on a design
/// or offered by the New Design dialog. `MaterialCatalogue::build(&[])` -- no
/// custom materials, since this is the CAD side's built-in-only picker -- is
/// the one place that decides "every built-in species exists", so this list
/// cannot drift from the renderer's own.
///
/// `new_design_dialog.slint`'s New Design material `ComboBox`
/// binds its `model` to `EditorModel.new_material_options`,
/// pushed from this same function every refresh (`view.rs`) -- see that property's
/// own doc comment (`ui/models/editor.slint`).
#[must_use]
pub fn builtin_preset_names() -> Vec<String> {
    std::iter::once("(none)".to_string())
        .chain(indicatrix_cut_core::MaterialCatalogue::build(&[]).names())
        .collect()
}

/// [`builtin_preset_names`]'s index for `name` (`None` -> `0`, an unrecognized name
/// -> `0` as a safe fallback rather than an out-of-range `ComboBox` index).
///
/// Test-only: the design settings panel names materials by
/// [`design_material_index_from_name`] instead (built-ins and custom catalogue
/// entries alike; this function has no production call site),
/// but the round-trip test against every [`builtin_preset_names`] entry still
/// wants this narrower, built-ins-only accessor directly.
#[cfg(test)]
#[must_use]
pub fn material_index_from_name(name: Option<&str>) -> i32 {
    let names = builtin_preset_names();
    name.and_then(|n| names.iter().position(|p| p == n))
        .map_or(0, |i| i as i32)
}

/// The inverse of [`material_index_from_name`]: the name at `index` in
/// [`builtin_preset_names`], or `None` for index `0` ("(none)") or an out-of-range index
/// (defensive only.
///
/// `EditorView`'s `ComboBox` can never produce one).
#[must_use]
pub fn material_name_from_index(index: i32) -> Option<String> {
    let names = builtin_preset_names();
    usize::try_from(index)
        .ok()
        .and_then(|i| names.get(i).cloned())
        .filter(|name| name != "(none)")
}

/// Parses the Yield form's three fields into
/// [`indicatrix_cut_core::Edit::SetGirdleDiameterMm`]/[`indicatrix_cut_core::Edit::SetMaterial`]'s
/// payloads.
///
/// Pure and unit tested directly. An empty `girdle_diameter_mm`/
/// `specific_gravity_override` field parses to `None` (the "unset, use the preset's
/// own figure" state), never an error: blank is a valid, deliberate choice here,
/// unlike the preform's three fields, which always name a real dimension.
///
/// The returned [`MaterialSelection`] is `current.with_specific_gravity_override(..)`
/// -- `name`/`refractive_index_override` always carry through from `current`
/// unchanged. Deriving `name` from `material_index` against
/// [`builtin_preset_names`] (built-ins only) instead would silently rename a
/// custom catalogue material to "(none)" (since `material_index_from_name` has
/// no custom entry to map it to) and would drop the RI override on every
/// "Apply Yield Inputs" click, even one that only touched the girdle diameter --
/// the Yield tab's own Material combo can only ever name a built-in preset or
/// "(none)", so it has no way to name a custom material correctly either.
/// `_material_index` is consequently unread: it stays a
/// parameter (prefixed `_`) only so this signature keeps matching the
/// existing `on_apply_yield_inputs` call site; the combo itself is
/// inert for naming purposes here (changing it and clicking Apply does not rename the
/// design's material) -- redesigning or removing that
/// control is a separate UI decision.
///
/// # Errors
///
/// A message when a non-blank girdle diameter or specific-gravity override is not a
/// positive, finite number.
pub fn parse_yield_form(
    girdle_diameter_mm: &str,
    _material_index: i32,
    specific_gravity_override: &str,
    current: &MaterialSelection,
) -> Result<(Option<f64>, MaterialSelection), String> {
    let girdle_diameter_mm = if girdle_diameter_mm.trim().is_empty() {
        None
    } else {
        let value = eval_number(girdle_diameter_mm, None)
            .map_err(|error| error.message("Girdle diameter", girdle_diameter_mm))?;
        if !value.is_finite() || value <= 0.0 {
            return Err("Girdle diameter must be a positive, finite number.".to_string());
        }
        Some(value)
    };

    let specific_gravity_override = if specific_gravity_override.trim().is_empty() {
        None
    } else {
        let value = eval_number(specific_gravity_override, None)
            .map_err(|error| error.message("Specific gravity", specific_gravity_override))?;
        if !value.is_finite() || value <= 0.0 {
            return Err("Specific gravity override must be a positive, finite number.".to_string());
        }
        Some(value)
    };

    Ok((
        girdle_diameter_mm,
        current.with_specific_gravity_override(specific_gravity_override),
    ))
}

/// Explains what `Design::effective_refractive_index_with`'s current result (shown
/// as "Eff.
///
/// RI" in the design settings panel) actually IS and where it came from,
/// so a cutter picking a custom catalogue material
/// can tell whether the critical angle/MARGIN/render/export are using
/// that material's real index or a stale fallback, because nothing on screen named
/// the source. Mirrors `Design::effective_refractive_index_with`'s own precedence
/// exactly (typed override, then a custom catalogue material by name, then a
/// built-in preset, then the design's legacy imported `I` line) so this text can
/// never disagree with the number it explains.
#[must_use]
pub fn ri_source_text(material: &MaterialSelection, custom: &[GemMaterial]) -> String {
    if let Some(value) = material.refractive_index_override {
        return format!("typed override ({value:.4})");
    }
    if let Some(name) = material.name.as_deref() {
        if custom.iter().any(|gem| gem.name.eq_ignore_ascii_case(name)) {
            return format!("custom catalogue material '{name}'");
        }
        if indicatrix_cut_core::built_in_refractive_index(name).is_some() {
            return format!("built-in material '{name}'");
        }
        return format!(
            "'{name}' is not a recognized material -- falling back to this design's legacy imported value"
        );
    }
    "no material selected -- using this design's legacy imported value".to_string()
}

/// Appended to a custom material's own name when it collides
/// case-insensitively with a built-in already in the combo (`design_material_options`'s
/// list order guarantees the built-in's own plain entry always comes first, from
/// [`builtin_preset_names`]), so the combo shows a second, clearly-labeled entry
/// instead of silently dropping the custom material from the list entirely -- a
/// cutter selecting "Diamond" can tell that a custom "Diamond"
/// exists too, and that `EditorMaterialLookup`'s custom-over-built-in
/// precedence (`material_lookup.rs`) means it is the one actually rendered.
/// [`design_material_name_from_index`] strips this suffix back off before it ever
/// becomes a real [`MaterialSelection::name`] -- see that function's own doc comment.
const CUSTOM_BUILTIN_COLLISION_SUFFIX: &str = " (custom)";

/// The design settings panel's material combo, in the EXACT order it must list:
/// [`builtin_preset_names`] verbatim, then `custom`'s own names.
///
/// Labeled with [`CUSTOM_BUILTIN_COLLISION_SUFFIX`] for any that collides
/// case-insensitively with a built-in already listed, rather than skipped outright --
/// then a final `"Custom RI…"` sentinel -- see [`design_material_name_from_index`] for
/// what selecting it means.
///
/// Pure and cheap enough to call directly for a
/// one-off need; `view::refresh_design_settings`'s own per-refresh call instead goes
/// through [`MaterialComboCache::options`], which caches this result and
/// only re-derives it when `custom`'s own name list has actually changed.
#[must_use]
pub fn design_material_options(custom: &[GemMaterial]) -> Vec<String> {
    let mut options: Vec<String> = builtin_preset_names();
    for material in custom {
        if options
            .iter()
            .any(|name| name.eq_ignore_ascii_case(&material.name))
        {
            options.push(format!(
                "{}{CUSTOM_BUILTIN_COLLISION_SUFFIX}",
                material.name
            ));
        } else {
            options.push(material.name.clone());
        }
    }
    options.push("Custom RI\u{2026}".to_string());
    options
}

/// The inverse of [`design_material_name_from_index`]: `options`'s index for `name`
/// (case-insensitive, matched against the REAL name.
///
/// A labeled entry's own suffix is stripped before comparing, so a stored
/// `MaterialSelection::name` of "Diamond" still finds a match even if the only remaining
/// occurrence in `options` were the labeled one), or `0` ("(none)") when `name` is absent
/// or not found.
#[must_use]
pub fn design_material_index_from_name(name: Option<&str>, options: &[String]) -> i32 {
    name.and_then(|n| {
        options.iter().position(|o| {
            o.strip_suffix(CUSTOM_BUILTIN_COLLISION_SUFFIX)
                .unwrap_or(o)
                .eq_ignore_ascii_case(n)
        })
    })
    .map_or(0, |i| i as i32)
}

/// The name at `index` in `options`, or `None` for index `0` ("(none)"), the trailing
/// `"Custom RI…"` sentinel, or an out-of-range index.
///
/// Selecting `"Custom RI…"`
/// clears `MaterialSelection::name` the same way "(none)" does -- it exists so a
/// species with no built-in or catalogue preset (e.g. garnet) can still be given a
/// real, typed refractive index without pretending to pick a preset that isn't used.
///
/// A [`CUSTOM_BUILTIN_COLLISION_SUFFIX`]-labeled entry
/// (`"Diamond (custom)"`) is stripped back to the real material name (`"Diamond"`)
/// here -- the label is a DISPLAY convenience only; the stored
/// [`MaterialSelection::name`] must stay the real name so it keeps resolving through
/// `crate::material_lookup::EditorMaterialLookup`'s custom-over-built-in precedence
/// exactly like the plain built-in entry does (both name the same real material,
/// since the label exists only to show the collision, not to distinguish two
/// different selections).
#[must_use]
pub fn design_material_name_from_index(index: i32, options: &[String]) -> Option<String> {
    usize::try_from(index)
        .ok()
        .and_then(|i| options.get(i))
        .filter(|&name| name != "(none)" && name != "Custom RI\u{2026}")
        .map(|name| {
            name.strip_suffix(CUSTOM_BUILTIN_COLLISION_SUFFIX)
                .map_or_else(|| name.clone(), str::to_string)
        })
}

/// Parses the design settings panel's material combo index plus its RI override text
/// field into a new [`MaterialSelection`].
///
/// Reads `current` first and carries its `specific_gravity_override` through unchanged
/// (that stays the Yield panel's own field), matching [`Edit::SetMaterial`]'s "wholesale,
/// not per-field" contract.
///
/// `current`'s `body_color_override` carries through only while the parsed name
/// stays the same material (case-insensitively; two nameless selections count as
/// the same): a color picked for one species is a what-if about THAT stone, so
/// switching species (e.g. the Retarget dialog, which has no color control of its
/// own) falls back to the new material's own color. The design settings panel sets
/// the color explicitly afterwards from its own color combo, via
/// [`MaterialSelection::with_body_color`].
///
/// An
/// empty override field means `None` (use the resolved material's own `n_D`); a
/// non-blank field must parse as a finite refractive index strictly greater than 1.0
/// (no real gem material has RI <= 1.0).
///
/// # Errors
///
/// A message when a non-blank RI override is not a finite number greater than 1.0.
pub fn parse_design_material_form(
    combo_index: i32,
    ri_override_text: &str,
    options: &[String],
    current: &MaterialSelection,
) -> Result<MaterialSelection, String> {
    let refractive_index_override = if ri_override_text.trim().is_empty() {
        None
    } else {
        let value = eval_number(ri_override_text, None)
            .map_err(|error| error.message("Refractive index", ri_override_text))?;
        if !value.is_finite() || value <= 1.0 {
            return Err(
                "Refractive index override must be a finite number greater than 1.0.".to_string(),
            );
        }
        Some(value)
    };
    let name = design_material_name_from_index(combo_index, options);
    let same_material = match (name.as_deref(), current.name.as_deref()) {
        (Some(new), Some(old)) => new.eq_ignore_ascii_case(old),
        (None, None) => true,
        _ => false,
    };
    Ok(MaterialSelection {
        name,
        specific_gravity_override: current.specific_gravity_override,
        refractive_index_override,
        body_color_override: current.body_color_override.filter(|_| same_material),
        body_color_bands_override: current
            .body_color_bands_override
            .clone()
            .filter(|_| same_material),
        absorption_path_scale_override: current
            .absorption_path_scale_override
            .filter(|_| same_material),
    })
}

/// The design settings panel's color combo options.
///
/// `"Material default"` (index 0, no override) followed by every
/// [`indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS`] label, in
/// that table's own order -- so combo index `i >= 1` is preset `i - 1` -- and a last
/// entry, [`BODY_COLOR_CUSTOM_LABEL`], that opens the hue/saturation/brightness picker
/// ([`body_color_custom_index`]).
#[must_use]
pub fn body_color_options() -> Vec<String> {
    std::iter::once("Material default".to_string())
        .chain(BODY_COLOR_PRESETS.iter().map(|p| p.label.to_string()))
        .chain(std::iter::once(BODY_COLOR_CUSTOM_LABEL.to_string()))
        .collect()
}

/// The label of the last [`body_color_options`] entry.
pub const BODY_COLOR_CUSTOM_LABEL: &str = "Custom\u{2026}";

/// The index of the trailing "Custom..." entry of [`body_color_options`]: one past the last
/// preset.
#[must_use]
pub fn body_color_custom_index() -> i32 {
    i32::try_from(BODY_COLOR_PRESETS.len() + 1).unwrap_or(i32::MAX)
}

/// [`body_color_options`]' index for `rgb`.
///
/// `0` for no override, `preset + 1` for a triple that matches a preset exactly, and
/// [`body_color_custom_index`] for a triple that matches none (a colour picked with the
/// picker, or typed into a design file by hand).
#[must_use]
pub fn body_color_index_for(rgb: Option<[f32; 3]>) -> i32 {
    rgb.map_or(0, |rgb| {
        preset_index_for_rgb(rgb).map_or_else(body_color_custom_index, |index| index as i32 + 1)
    })
}

/// The combo index of a design's whole colour: [`body_color_custom_index`] while the
/// N-band form (the L*C*h editor's colour) is set, whatever its stored triple is, else
/// [`body_color_index_for`] the triple.
#[must_use]
pub fn body_color_index_for_material(material: &MaterialSelection) -> i32 {
    if material
        .body_color_bands_override
        .as_ref()
        .is_some_and(|bands| !bands.is_empty())
    {
        body_color_custom_index()
    } else {
        body_color_index_for(material.body_color_override)
    }
}

/// The inverse of [`body_color_index_for`] for the fixed entries.
///
/// `None` ("Material default") for index `0`, an out-of-range index or the custom index (a custom triple is
/// not in any table; the caller keeps the design's current triple or the picked one), else
/// preset `index - 1`'s absorption triple.
#[must_use]
pub fn body_color_from_index(index: i32) -> Option<[f32; 3]> {
    usize::try_from(index)
        .ok()
        .and_then(|i| i.checked_sub(1))
        .and_then(|i| BODY_COLOR_PRESETS.get(i))
        .map(|preset| preset.absorption_rgb)
}

/// The colour an Apply stores for the combo's selected `index`: the custom entry keeps the
/// design's `current` triple, every other index resolves through [`body_color_from_index`].
#[must_use]
pub fn body_color_for_apply(index: i32, current: Option<[f32; 3]>) -> Option<[f32; 3]> {
    if index == body_color_custom_index() {
        current
    } else {
        body_color_from_index(index)
    }
}

/// `material` with the colour the combo's selected `index` stands for, given the design's
/// `current` selection.
///
/// The custom entry keeps the design's whole current colour (the triple AND the N-band form
/// with its path scale, so a colour made in the L*C*h editor survives an Apply of the
/// material row); every other index sets that preset's triple (or none) and DROPS the bands,
/// which would otherwise win over it.
#[must_use]
pub fn with_body_color_choice(
    material: MaterialSelection,
    index: i32,
    current: &MaterialSelection,
) -> MaterialSelection {
    if index == body_color_custom_index() {
        material.with_body_color_bands(
            current.body_color_override,
            current.body_color_bands_override.clone(),
            current.absorption_path_scale_override,
        )
    } else {
        material.with_body_color(body_color_from_index(index))
    }
}

/// The gear combo's index for `gear_teeth` -- a position in [`GEAR_PRESETS`] when it
/// matches exactly, else `GEAR_PRESETS.len()` (the trailing "Custom" entry), matching
/// [`gear_choice_to_teeth`]'s inverse mapping.
#[must_use]
pub fn gear_index_from_teeth(gear_teeth: i32) -> i32 {
    GEAR_PRESETS
        .iter()
        .position(|&g| g == gear_teeth)
        .map_or(GEAR_PRESETS.len() as i32, |i| i as i32)
}

/// The inverse of [`gear_index_from_teeth`]: resolves the gear combo's selected index
/// (plus, only for the trailing "Custom" entry, the paired text field) to a real gear
/// tooth count.
///
/// `preset_index` outside `0..=GEAR_PRESETS.len()` is defensive-only and
/// falls back to reading `custom_text`, same as an explicit "Custom" pick.
///
/// # Errors
///
/// A message when the custom tooth count is not a positive whole number.
pub fn gear_choice_to_teeth(preset_index: i32, custom_text: &str) -> Result<i32, String> {
    if let Ok(i) = usize::try_from(preset_index)
        && let Some(&teeth) = GEAR_PRESETS.get(i)
    {
        return Ok(teeth);
    }
    let teeth: i32 = custom_text.trim().parse().map_err(|_| {
        format!(
            "Gear tooth count '{}' is not a whole number.",
            custom_text.trim()
        )
    })?;
    if teeth <= 0 {
        return Err("Gear tooth count must be a positive whole number.".to_string());
    }
    Ok(teeth)
}

/// A real dry run of [`Edit::RemapIndices`] against a scratch clone of `design`
/// (never `design` itself), turned into the [`GearRemapPreviewRow`]s the gear-remap
/// confirmation panel shows.
///
/// `new_indices` comes from a real `apply_edit` call
/// (staying byte-identical to what confirming would actually write), while
/// `non_integral` is computed from the UNROUNDED ratio so it flags a tier whose ideal
/// new position isn't a whole tooth regardless of which `rounding` this preview uses.
#[must_use]
pub fn gear_remap_preview(
    design: &Design,
    from_gear: i32,
    to_gear: i32,
    rounding: RemapRounding,
) -> Vec<GearRemapPreviewRow> {
    let format_indices = |v: &[f64]| v.iter().map(f64::to_string).collect::<Vec<_>>().join(", ");
    let original: Vec<String> = design
        .tiers
        .iter()
        .map(|t| format_indices(&t.indices))
        .collect();
    // Same magnitude-only ratio `Edit::RemapIndices` itself applies -- computing it
    // here from the signed tooth counts is what made this preview disagree with the
    // edit for a design whose `.asc` header carries a negative `g`.
    let ratio = indicatrix_cut_core::remap_ratio(from_gear, to_gear);
    let non_integral: Vec<bool> = design
        .tiers
        .iter()
        .map(|t| t.indices.iter().any(|&i| (i * ratio).fract() != 0.0))
        .collect();

    let mut remapped = design.clone();
    if remapped
        .apply_edit(Edit::RemapIndices {
            from_gear,
            to_gear,
            rounding,
        })
        .is_err()
    {
        // `RemapIndices` names no tier index, so `apply_edit` cannot fail for it.
        // Defensive only: an empty preview reads as "nothing to remap" rather than
        // panicking on a future change to that contract.
        return Vec::new();
    }

    design
        .tiers
        .iter()
        .zip(&remapped.tiers)
        .enumerate()
        .map(|(i, (before, after))| GearRemapPreviewRow {
            name: before.name.clone(),
            old_indices: original[i].clone(),
            new_indices: format_indices(&after.indices),
            non_integral: non_integral[i],
        })
        .collect()
}
