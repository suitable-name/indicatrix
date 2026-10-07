//! The New Design template gallery: its cards, and how a template becomes a design.
//!
//! The template data itself lives in `indicatrix_cut_core::templates` (nine
//! entries, each a statically verified closed solid); this module
//!
//! - lists them as cards -- [`template_cards`] in the flat order the web app and the
//!   worked-example guide index (card `i` creates template `i`, `0` is "Empty"), and
//!   [`gallery_cards`] grouped for the desktop dialog ("Shapes", then "Round variants
//!   and teaching designs", then the blank card), each card carrying the template's
//!   own index so the grouping never changes what an index means;
//! - decides what a dialog choice builds ([`create_from_template`]): the template on
//!   its default preform, remapped to the chosen gear, in the chosen material, at the
//!   chosen stone width -- with the pavilion angles adapted by the retarget engine
//!   (its default policy: the crown stays as authored) when the material's index
//!   differs from the one the template was designed for, and kept as authored when
//!   the adapted stone would not be valid.
//!
//! Everything here is pure (no window, no thread), so the decisions are unit tested.

use crate::{
    EditorSession,
    material::GEAR_PRESETS,
    retarget::{
        CrownShift, apply_with_anchors,
        check::{CheckInputs, check_retarget},
        plan::build_plan_from,
        validity::ValidityStatus,
        view::resolved_material_from_selection,
    },
};
use indicatrix::optics::{LightingPreset, materials::GemMaterial};
use indicatrix_cut_core::{
    Design, Edit, FreshDesignSpec, MaterialSelection, PreformSpec, RemapRounding, ResolvedMaterial,
    templates::{TEMPLATES, TemplateSpec},
};

/// One gallery card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateCard {
    /// The card's title and the "Start From" selection text.
    pub name: String,
    /// A short shape/fold line under the name.
    pub shape: String,
    /// One sentence describing the design.
    pub description: String,
    /// Whether the card can be selected (every card can today).
    pub ready: bool,
}

/// Every gallery card, in display order: "Empty" first, then every
/// `indicatrix_cut_core::templates::TEMPLATES` entry in order -- every one of them
/// selectable.
#[must_use]
pub fn template_cards() -> Vec<TemplateCard> {
    let mut cards = vec![TemplateCard {
        name: "Empty".to_string(),
        shape: "Blank design".to_string(),
        description: "No starting tiers -- author the schedule from scratch.".to_string(),
        ready: true,
    }];
    for spec in indicatrix_cut_core::templates::TEMPLATES {
        cards.push(TemplateCard {
            name: spec.name.to_string(),
            shape: spec.shape.to_string(),
            description: spec.description.to_string(),
            ready: true,
        });
    }
    cards
}

/// Which section of the New Design dialog a card is listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GalleryGroup {
    /// The "Empty" card on its own: no tiers, every setting typed by hand.
    Blank,
    /// "Shapes": the top five -- round brilliant, oval, cushion, emerald, princess.
    Shapes,
    /// "Round variants and teaching designs".
    Variants,
}

impl GalleryGroup {
    /// The section heading.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Blank => "Blank",
            Self::Shapes => "Shapes",
            Self::Variants => "Round variants and teaching designs",
        }
    }

    /// The number the Slint card struct carries: `0` blank, `1` shapes, `2` variants.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Blank => 0,
            Self::Shapes => 1,
            Self::Variants => 2,
        }
    }
}

/// One card of the dialog's grouped gallery.
#[derive(Debug, Clone, PartialEq)]
pub struct GalleryCard {
    /// The index [`EditorSession::from_template`] takes: `0` is "Empty", `n >= 1` is
    /// `TEMPLATES[n - 1]`. Not the card's position in [`gallery_cards`].
    pub template_index: i32,
    /// The section the card is listed in.
    pub group: GalleryGroup,
    /// The card's position inside its section (left to right, top to bottom).
    pub slot: usize,
    /// The card's title.
    pub name: String,
    /// A short shape/fold line under the name.
    pub shape: String,
    /// One sentence describing the design.
    pub description: String,
    /// The refractive index the template's angles were designed for; `None` for "Empty".
    pub design_ri: Option<f64>,
    /// The built-in material that index belongs to; `None` for "Empty".
    pub default_material: Option<String>,
    /// The gears the dialog offers for this template, the template's own gear first.
    /// For "Empty": every preset (a custom tooth count is typed in the form).
    pub allowed_gears: Vec<i32>,
}

impl GalleryCard {
    /// "Designed for Sapphire (n 1.768)" -- the line under a template's name; a blank
    /// design has none.
    #[must_use]
    pub fn designed_for(&self) -> String {
        match (&self.default_material, self.design_ri) {
            (Some(material), Some(ri)) => format!("Designed for {material} (n {ri:.3})"),
            _ => String::new(),
        }
    }
}

/// The template behind a session template index (`1..=N`), or `None` for "Empty" (`0`)
/// and for any index the table has no entry for.
#[must_use]
pub fn template_spec(template_index: i32) -> Option<&'static TemplateSpec> {
    template_index
        .checked_sub(1)
        .and_then(|position| usize::try_from(position).ok())
        .and_then(|position| TEMPLATES.get(position))
}

/// The session template index of `TEMPLATES[position]`.
fn session_index(position: usize) -> i32 {
    i32::try_from(position).map_or(i32::MAX, |p| p.saturating_add(1))
}

/// The dialog's cards in section order.
///
/// The featured shapes come first, then the round variants and teaching designs, then
/// the blank card. Inside a section the order is the order of `TEMPLATES`, so the
/// featured shapes read round brilliant, oval, cushion, emerald, princess.
#[must_use]
pub fn gallery_cards() -> Vec<GalleryCard> {
    let card_of =
        |position: usize, spec: &TemplateSpec, group: GalleryGroup, slot: usize| GalleryCard {
            template_index: session_index(position),
            group,
            slot,
            name: spec.name.to_string(),
            shape: spec.shape.to_string(),
            description: spec.description.to_string(),
            design_ri: Some(spec.design_ri),
            default_material: Some(spec.default_material.to_string()),
            allowed_gears: spec.allowed_gears_among(&GEAR_PRESETS),
        };
    let mut cards = Vec::with_capacity(TEMPLATES.len() + 1);
    for (group, featured) in [
        (GalleryGroup::Shapes, true),
        (GalleryGroup::Variants, false),
    ] {
        let mut slot = 0;
        for (position, spec) in TEMPLATES.iter().enumerate() {
            if spec.featured == featured {
                cards.push(card_of(position, spec, group, slot));
                slot += 1;
            }
        }
    }
    cards.push(GalleryCard {
        template_index: 0,
        group: GalleryGroup::Blank,
        slot: 0,
        name: "Empty".to_string(),
        shape: "Blank design".to_string(),
        description: "No starting tiers -- author the schedule from scratch.".to_string(),
        design_ri: None,
        default_material: None,
        allowed_gears: GEAR_PRESETS.to_vec(),
    });
    cards
}

/// The design a gallery thumbnail shows.
///
/// That is the template at its own gear on its own preform (a stone with a closed solid
/// from the first frame), or for "Empty" -- and for an index with no template -- the bare
/// legacy preform.
#[must_use]
pub fn gallery_design(template_index: i32) -> Design {
    let spec = template_spec(template_index).map_or_else(
        || FreshDesignSpec {
            gear_teeth: 96,
            symmetry_order: 8,
            mirror: true,
            material: MaterialSelection::none(),
            preform: PreformSpec::cylinder(96, 1.5, 1.0, 1.5),
        },
        |template| template.fresh_spec(template.default_material_selection()),
    );
    EditorSession::from_template(spec, template_index).design
}

/// How far a material's index may sit from a template's design index before its angles move.
///
/// Quartz (1.544) against the 1.54 the teaching designs assumed is close enough to leave
/// alone; sapphire (1.768) against quartz is not.
pub const RI_TOLERANCE: f64 = 0.02;

/// Whether a stone designed for index `design_ri` and cut from a material of index
/// `material_n_d` needs its pavilion angles adapted.
#[must_use]
pub const fn adaptation_needed(design_ri: f64, material_n_d: f64) -> bool {
    (design_ri - material_n_d).abs() > RI_TOLERANCE
}

/// What creating a design did about its facet angles.
#[derive(Debug, Clone, PartialEq)]
pub enum AngleAdaptation {
    /// The material's index matches the one the template was designed for: the
    /// template's angles are untouched.
    NotNeeded,
    /// The pavilion angles were shifted to the material's index (the crown stays as authored)
    /// and the validity gate passed.
    Adapted {
        /// The index the template was designed for.
        n_from: f64,
        /// The material's index.
        n_to: f64,
        /// The gate's one-line verdict, e.g. "Valid: girdle 2.1 % (was 2.3 %), ...".
        headline: String,
    },
    /// Adapting the angles would not have left a valid stone (or could not be done): the
    /// template's own angles were kept.
    KeptTemplateAngles {
        /// The index the template was designed for.
        n_from: f64,
        /// The material's index.
        n_to: f64,
        /// Why the adapted stone was refused.
        reason: String,
    },
}

impl AngleAdaptation {
    /// The sentence to tell the cutter, or `None` when nothing happened to the angles.
    #[must_use]
    pub fn message(&self) -> Option<String> {
        match self {
            Self::NotNeeded => None,
            Self::Adapted {
                n_from,
                n_to,
                headline,
            } => Some(format!(
                "Pavilion angles adapted from n {n_from:.3} to n {n_to:.3}; the crown is as \
                 authored. {headline}"
            )),
            Self::KeptTemplateAngles {
                n_from,
                n_to,
                reason,
            } => Some(format!(
                "Kept the template's angles: adapting the pavilion from n {n_from:.3} to \
                 n {n_to:.3} would not leave a valid stone ({reason}). Use Retarget to adapt \
                 the angles later."
            )),
        }
    }

    /// Whether the design starts with adapted angles.
    #[must_use]
    pub const fn adapted(&self) -> bool {
        matches!(self, Self::Adapted { .. })
    }
}

/// The adaptation decision with the geometry left out.
///
/// When the indices differ, `attempt` runs (it builds the adapted stone and asks the
/// validity gate) and its `Ok` headline or `Err` reason decides between
/// [`AngleAdaptation::Adapted`] and [`AngleAdaptation::KeptTemplateAngles`]; when they
/// match, `attempt` is never called.
pub fn settle_adaptation(
    n_from: f64,
    n_to: f64,
    attempt: impl FnOnce() -> Result<String, String>,
) -> AngleAdaptation {
    if !adaptation_needed(n_from, n_to) {
        return AngleAdaptation::NotNeeded;
    }
    match attempt() {
        Ok(headline) => AngleAdaptation::Adapted {
            n_from,
            n_to,
            headline,
        },
        Err(reason) => AngleAdaptation::KeptTemplateAngles {
            n_from,
            n_to,
            reason,
        },
    }
}

/// The line under the dialog's material combo: what the chosen material means for this
/// template. `chosen_label` is the material's name (or "this material"), `chosen_n_d`
/// its refractive index.
#[must_use]
pub fn material_note(spec: &TemplateSpec, chosen_label: &str, chosen_n_d: f64) -> String {
    let designed = format!(
        "Designed for {} (n {:.3}).",
        spec.default_material, spec.design_ri
    );
    if adaptation_needed(spec.design_ri, chosen_n_d) {
        format!(
            "{designed} {chosen_label} has n {chosen_n_d:.3}, so the pavilion angles are adapted \
             when the design is created (the crown stays as authored) -- if the adapted stone \
             stays valid; otherwise the template's angles are kept and you are told."
        )
    } else {
        format!("{designed} {chosen_label} has n {chosen_n_d:.3}: the angles are used as authored.")
    }
}

/// What the New Design dialog asks to be built from a template.
#[derive(Debug, Clone)]
pub struct NewDesignChoice {
    /// The template's session index (`1..=N`; `0`, "Empty", is built from the form).
    pub template_index: i32,
    /// The index gear, one of the template's allowed gears.
    pub gear_teeth: i32,
    /// The material the design is cut from.
    pub material: MaterialSelection,
    /// The stone width in millimetres (the design's `girdle_diameter_mm`), if given.
    pub girdle_diameter_mm: Option<f64>,
    /// The vault's custom materials, for resolving a custom material's index.
    pub custom_materials: Vec<GemMaterial>,
}

/// A design built from a dialog choice.
pub struct CreatedDesign {
    /// The new session: no history, clean, generation 0 -- like any fresh design.
    pub session: EditorSession,
    /// What happened to the facet angles.
    pub adaptation: AngleAdaptation,
}

/// Shifts `design`'s angles from index `n_from` to `target`'s and applies them (with the
/// re-anchored masts) when the validity gate passes. `Ok` is the gate's headline; `Err`
/// is why nothing was applied, and `design` is then unchanged.
fn adapt_angles(
    design: &mut Design,
    n_from: f64,
    target: &ResolvedMaterial,
) -> Result<String, String> {
    let plan = build_plan_from(design, n_from, target, CrownShift::default());
    if plan.is_empty() {
        return Err("none of the template's angles move".to_string());
    }
    let check = check_retarget(&CheckInputs {
        girdle: None,
        design,
        plan: &plan,
        current_gem: None,
        lighting: LightingPreset::RingLights,
    });
    if check.validity.status != ValidityStatus::Valid {
        return Err(check.validity.headline());
    }
    let edit = apply_with_anchors(design, &plan.proposal(), &check.anchors);
    design
        .apply_edit(edit)
        .map(|_inverse| ())
        .map_err(|error| format!("the angles could not be applied: {error}"))?;
    Ok(check.validity.headline())
}

/// The refractive index and resolved material a design cut from `material` has: the
/// named material's `n_D`, an override's, or -- for "(none)" -- the design's own legacy
/// fallback index, so "no material" is never mistaken for diamond.
fn target_material(design: &Design, custom: &[GemMaterial]) -> ResolvedMaterial {
    let mut selection = design.material.clone();
    if selection.name.is_none() && selection.refractive_index_override.is_none() {
        selection.refractive_index_override = Some(design.effective_refractive_index_with(custom));
    }
    resolved_material_from_selection(&selection, custom)
}

/// Builds the design a New Design dialog choice asks for.
///
/// 1. The template on its default preform, cut from the chosen material, seeded as a
///    starting state (no history, clean) exactly like [`EditorSession::from_template`].
/// 2. Remapped to the chosen gear with the gear-remap panel's own edit
///    (`Edit::RemapIndices`, nearest rounding, then `Edit::SetSchedule`).
/// 3. The stone width written as the design's `girdle_diameter_mm`.
/// 4. If the material's index differs from the template's
///    ([`TemplateSpec::design_ri`]) by more than [`RI_TOLERANCE`], the facet angles are
///    shifted with the retarget engine and the result is kept only when its validity
///    gate says the stone is valid; otherwise the template's angles stay
///    ([`AngleAdaptation::KeptTemplateAngles`]).
///
/// # Errors
///
/// A message when `template_index` names no template (`0`, "Empty", included), or the
/// template cannot be remapped onto `gear_teeth` ([`TemplateSpec::allows_gear`]).
pub fn create_from_template(choice: &NewDesignChoice) -> Result<CreatedDesign, String> {
    let spec = template_spec(choice.template_index).ok_or_else(|| {
        format!(
            "There is no template number {} to start from.",
            choice.template_index
        )
    })?;
    if !spec.allows_gear(choice.gear_teeth) {
        return Err(format!(
            "{} cannot be cut on a {}-tooth gear: its facets would not land on whole teeth \
             symmetrically. Pick one of the offered gears.",
            spec.name, choice.gear_teeth
        ));
    }
    let mut session = EditorSession::from_template(
        spec.fresh_spec(choice.material.clone()),
        choice.template_index,
    );
    if choice.gear_teeth != spec.gear_teeth {
        session
            .design
            .apply_edit(Edit::Batch(vec![
                Edit::RemapIndices {
                    from_gear: spec.gear_teeth,
                    to_gear: choice.gear_teeth,
                    rounding: RemapRounding::Nearest,
                },
                Edit::SetSchedule {
                    gear_teeth: choice.gear_teeth,
                    symmetry_order: spec.symmetry_order,
                    mirror: spec.mirror,
                },
            ]))
            .map(|_inverse| ())
            .map_err(|error| {
                format!(
                    "Could not remap {} to {} teeth: {error}",
                    spec.name, choice.gear_teeth
                )
            })?;
    }
    session.design.girdle_diameter_mm = choice
        .girdle_diameter_mm
        .filter(|mm| mm.is_finite() && *mm > 0.0);
    let target = target_material(&session.design, &choice.custom_materials);
    let adaptation = settle_adaptation(spec.design_ri, target.n_d, || {
        adapt_angles(&mut session.design, spec.design_ri, &target)
    });
    Ok(CreatedDesign {
        session,
        adaptation,
    })
}

#[cfg(test)]
mod tests;
