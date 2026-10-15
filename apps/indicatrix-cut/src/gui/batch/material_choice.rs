//! Which of the materials that fit a design's refractive index its cached previews and
//! tilt curves are rendered in.
//!
//! Refractive index only narrows the field: several presets usually sit within the match
//! tolerance of a design's index and differ in body color, absorption and dispersion.
//! The one that is kept for the design's lifetime is the one that serves the stone best
//! across a balanced set of measures, not the one that minimises any single figure.
//! A stone that windows 0% while most of its light is extinguished is not a good match,
//! so windowing, extinction and (flipped) brilliance count equally, each averaged over
//! a table-up and two tilted poses.
//!
//! # Stone size
//!
//! This module only PICKS the material, and it scores the candidates unsized
//! (`material_for_stone` at width `0.0`, the 7 mm calibration), as it always has: the pick is
//! persisted write-once, so it must depend on the design's geometry and index alone, never on a
//! size that can change when the design is edited.
//!
//! The picked material is not what the batches draw and sweep as it stands.
//! `super::catalogue_material::catalogue_material` sizes it by the design's own girdle diameter
//! (`Design::girdle_diameter_mm` of the design file, carried by
//! `gui::editor::CataloguePlanes::girdle_diameter_mm`), exactly as the editor's viewport and the
//! exports size a stone, so a library design shows and is scored at the size the editor shows. A
//! design whose file names a material is not drawn in this pick at all but in that material (the
//! editor's own rule, with the file's refractive-index and body-color overrides). A design with
//! neither -- an angle-table design, an `.asc` alone, a file without a girdle diameter -- keeps
//! the unsized pick, and with it the same thumbnails, curves and cache fingerprint it had before.
//!
//! What remains approximate: for a design whose file sets a girdle diameter but names no
//! material, the pick itself was scored at 7 mm, so a size far from 7 mm can leave a different
//! candidate of the shortlist the better one at the real size. The pick is not redone per size,
//! to keep it stable.

use crate::bridge::preview_render::{PREVIEW_LIGHT_PITCH, PREVIEW_LIGHT_YAW};
use indicatrix::{
    color::metrics::evaluate_gem_optical_metrics,
    geometry::plane::GpuFacetPlane,
    optics::{materials::GemMaterial, raytracer::LightingPreset},
};
use indicatrix_vault::{db::sqlite::Database, model::material_match::RiPresetCandidate};

/// Camera pitches the candidates are measured at, in degrees: table-up, then two tilts.
const SCORE_PITCHES_DEG: [f32; 3] = [90.0, 60.0, 30.0];

/// The preset the candidates are lit by -- the one the tilt batch scores under (the ISO
/// hemisphere, formerly the ring lights).
const SCORE_PRESET: LightingPreset = LightingPreset::IsoHemisphere;

/// `material`'s balanced loss on `planes` (0-100, LOWER is better): the mean over
/// [`SCORE_PITCHES_DEG`] of the equal-weight mean of windowing, extinction and
/// `100 - brilliance`.
#[must_use]
pub fn balanced_loss(planes: &[GpuFacetPlane], material: &GemMaterial) -> f32 {
    let environment = SCORE_PRESET.studio(1.0, PREVIEW_LIGHT_YAW, PREVIEW_LIGHT_PITCH);
    let total: f32 = SCORE_PITCHES_DEG
        .iter()
        .map(|pitch| {
            let m = evaluate_gem_optical_metrics(
                planes,
                material,
                0.0,
                pitch.to_radians(),
                environment,
            );
            (m.windowing_pct + m.extinction_pct + (100.0 - m.brilliance_pct)) / 3.0
        })
        .sum();
    total / SCORE_PITCHES_DEG.len() as f32
}

/// Index of the shortlist entry with the lowest [`balanced_loss`]; the first on a tie
/// and for a name that is not a built-in material, so the choice is deterministic.
#[must_use]
pub fn best_balanced(planes: &[GpuFacetPlane], shortlist: &[&RiPresetCandidate]) -> usize {
    let mut best = (0, f32::INFINITY);
    for (index, candidate) in shortlist.iter().enumerate() {
        let Some(material) = GemMaterial::by_name(&candidate.name) else {
            continue;
        };
        let material = indicatrix::render_setup::material_for_stone(material, 0.0, planes);
        let loss = balanced_loss(planes, &material);
        if loss.is_finite() && loss < best.1 {
            best = (index, loss);
        }
    }
    best.0
}

/// `Database::ensure_preview_material` with the balanced choice among equally fitting
/// materials. Measures nothing when the design already has a material or only one fits.
///
/// # Errors
///
/// The database error of the underlying call.
pub fn ensure_balanced_material(
    db: &Database,
    entry_id: i64,
    target_ri: f64,
    candidates: &[RiPresetCandidate],
    tolerance: f64,
    planes: &[GpuFacetPlane],
) -> anyhow::Result<Option<String>> {
    db.ensure_preview_material_by(
        entry_id,
        target_ri,
        candidates,
        tolerance,
        &mut |shortlist| best_balanced(planes, shortlist),
    )
}
