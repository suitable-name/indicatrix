//! From the rough's zones to a stone's material.
//!
//! The preview material of a planned stone, the custom material "<plan> - <design> color" an
//! adopted design gets, and the "relative to stone" scaling of library zoned materials.
//!
//! # What adopting stores
//!
//! The adopted design gets an ordinary custom material row whose seven-band colour is the BASE
//! zone (so a default build, which ignores zones, renders the stone in its base colour), and a
//! `material_zoning` row holding the zones in the STONE frame (mm). The stone is then rendered at
//! its real size: the material is always `PerMm`, and the zone geometry is in mm, so nothing
//! scales with a size slider. The width the application should render the stone at is
//! [`AdoptedColour::stone_width_mm`].

use super::frame::{DesignPlacement, zones_in_stone_frame};
use crate::rough_plan::fit::StonePose;
use indicatrix::optics::{
    absorption::BandShape,
    materials::GemMaterial,
    zoning::{ZoneAbsorption, ZonedAbsorption},
};

/// The suffix of an adopted material's name (the name is shown to the user, so it spells
/// "color").
pub const ADOPTED_SUFFIX: &str = " color";
/// The longest rough (plan) name used in a material name (in characters).
const MAX_ROUGH_NAME_CHARS: usize = 40;
/// The longest design title used in a material name (in characters).
const MAX_DESIGN_TITLE_CHARS: usize = 40;
/// The stem used when the rough has no usable name.
const DEFAULT_STEM: &str = "Rough";
/// The stem used when the design has no usable title.
const DEFAULT_DESIGN_STEM: &str = "design";
/// What stands between the plan stem and the design stem of an adopted material's name.
const NAME_SEPARATOR: &str = " - ";

/// Where one planned stone sits: its pose in the rough and how its design relates to the
/// planner's caliper frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StonePlacement {
    /// The stone's pose (a pose variant when the cutter chose one, see `pose_variant`).
    pub pose: StonePose,
    /// The design's caliper turn and centring.
    pub design: DesignPlacement,
}

/// The material of a planned stone's preview: the rough's host material with the rough's zones
/// moved into the stone's frame.
///
/// `None` when the placement is not usable (see `rough_to_stone_frame`) or the zones are
/// invalid.
#[must_use]
pub fn stone_preview_material(
    host: &GemMaterial,
    rough_zoned: &ZonedAbsorption,
    placement: &StonePlacement,
) -> Option<GemMaterial> {
    let zoning = zones_in_stone_frame(rough_zoned, &placement.pose, &placement.design)?;
    zoning.validate().ok()?;
    Some(host.clone().with_zoning(zoning))
}

/// `text` stripped of control characters, cut to `max_chars` characters and trimmed; `fallback`
/// when nothing is left.
fn name_stem(text: &str, max_chars: usize, fallback: &str) -> String {
    let cleaned: String = text
        .chars()
        .filter(|c| !c.is_control())
        .take(max_chars)
        .collect();
    let stem = cleaned.trim();
    String::from(if stem.is_empty() { fallback } else { stem })
}

/// The name of the custom material an adopted design gets: `"<plan> - <design> color"`.
///
/// Two designs adopted from one plan do not replace each other's material. Both stems (the plan's
/// name and the design's title) are stripped of control characters, cut to 40 characters and
/// trimmed; a blank plan name gives `"Rough"` and a blank design title `"design"`, so the name
/// is never empty (`"Rough - design color"` at worst).
#[must_use]
pub fn adopted_material_name(rough_name: &str, design_title: &str) -> String {
    let plan = name_stem(rough_name, MAX_ROUGH_NAME_CHARS, DEFAULT_STEM);
    let design = name_stem(design_title, MAX_DESIGN_TITLE_CHARS, DEFAULT_DESIGN_STEM);
    format!("{plan}{NAME_SEPARATOR}{design}{ADOPTED_SUFFIX}")
}

/// What adopting one planned stone produces.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptedColour {
    /// The custom material's name, see [`adopted_material_name`] (`"<plan> - <design> color"`).
    pub material_name: String,
    /// The zones in the stone frame (mm, the design's own model origin and axes); stored in
    /// `material_zoning` with `relative_to_stone = false`.
    pub zoning: ZonedAbsorption,
    /// The base zone as the seven-band rows (`[centre_nm, width_nm, amplitude_per_mm]`) the
    /// ordinary material row stores. Empty when the base zone is clear.
    pub base_bands: Vec<[f32; 3]>,
    /// The width the stone is to be rendered at (its real width), in mm.
    pub stone_width_mm: f64,
}

/// Adopts one planned stone: its zones in the stone frame, the base-zone band rows, the
/// material name and the real stone width.
///
/// `rough_name` is the plan's name and `design_title` the stone's design's title; they make the
/// material's name ([`adopted_material_name`]). `stone_width_mm` is the stone's girdle width at
/// its planned size (the design's axis-aligned width in model units times
/// `StonePose::mm_per_unit`, which is what the editor's girdle diameter measures). `None` when
/// the placement is not usable, the zones are invalid, or the width is not positive.
#[must_use]
pub fn adopt_colour(
    rough_name: &str,
    design_title: &str,
    rough_zoned: &ZonedAbsorption,
    placement: &StonePlacement,
    stone_width_mm: f64,
) -> Option<AdoptedColour> {
    if !(stone_width_mm.is_finite() && stone_width_mm > 0.0) {
        return None;
    }
    let zoning = zones_in_stone_frame(rough_zoned, &placement.pose, &placement.design)?;
    zoning.validate().ok()?;
    Some(AdoptedColour {
        material_name: adopted_material_name(rough_name, design_title),
        base_bands: base_zone_bands(&zoning.base),
        zoning,
        stone_width_mm,
    })
}

/// The seven-band rows a library material row stores for a zone: the ordinary-ray bands that
/// are Gaussian in wavelength and usable (finite, with a positive width and peak).
///
/// A pleochroic
/// zone's extraordinary and third-axis bands are not representable in a row and are dropped; the
/// zones themselves keep them in `material_zoning`.
#[must_use]
pub fn base_zone_bands(base: &ZoneAbsorption) -> Vec<[f32; 3]> {
    base.tensor
        .o_ray
        .iter()
        .filter(|band| {
            band.shape == BandShape::GaussianWavelength
                && band.center_nm.is_finite()
                && band.width_nm.is_finite()
                && band.peak.is_finite()
                && band.width_nm > 0.0
                && band.peak > 0.0
        })
        .map(|band| [band.center_nm, band.width_nm, band.peak])
        .collect()
}

/// The adopted stone's material for rendering: `host` renamed, with the stone-frame zones
/// installed and the absorption scale set to the stone's real size (`mm_per_unit`
/// millimetres per model unit).
#[must_use]
pub fn adopted_material(
    host: &GemMaterial,
    adopted: &AdoptedColour,
    mm_per_unit: f32,
) -> GemMaterial {
    let mut material = host.clone().with_zoning(adopted.zoning.clone());
    material.name.clone_from(&adopted.material_name);
    if mm_per_unit.is_finite() && mm_per_unit > 0.0 {
        material = material.with_absorption_path_scale(mm_per_unit);
    }
    material
}

/// The zones of a "relative to stone" library material at a stone width: its stored geometry is
/// for a stone one unit wide, so every length is multiplied by `stone_width_mm`.
///
/// `None` for a width that is not positive and finite, or when the scaled zones are invalid.
#[must_use]
pub fn resolve_relative(stored: &ZonedAbsorption, stone_width_mm: f64) -> Option<ZonedAbsorption> {
    if !(stone_width_mm.is_finite() && stone_width_mm > 0.0) {
        return None;
    }
    let scaled = stored.scaled(stone_width_mm);
    scaled.validate().ok()?;
    Some(scaled)
}

/// The inverse of [`resolve_relative`]: zones in mm for a stone `stone_width_mm` wide, as the
/// geometry of a stone one unit wide (what a "relative to stone" row stores).
#[must_use]
pub fn make_relative(zoning_mm: &ZonedAbsorption, stone_width_mm: f64) -> Option<ZonedAbsorption> {
    if !(stone_width_mm.is_finite() && stone_width_mm > 0.0) {
        return None;
    }
    let unit = zoning_mm.scaled(1.0 / stone_width_mm);
    unit.validate().ok()?;
    Some(unit)
}
