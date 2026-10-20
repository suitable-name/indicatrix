//! The thumbnail cache kind selected by a frozen batch.

use super::{BatchContext, CacheKind, PREVIEW_MAX_BOUNCES};

pub(super) const fn cache_kind(ctx: &BatchContext<'_>) -> CacheKind {
    if ctx.solid {
        CacheKind::SolidDraft {
            size: ctx.preview_size,
        }
    } else if ctx.engine.is_ray() {
        let ray = ctx.engine.ray.sanitized();
        if ctx.ray_draft {
            CacheKind::RayDraft {
                size: ctx.preview_size,
                taps: ray.aa_taps,
                fire: ray.fire,
            }
        } else {
            CacheKind::RayPreview {
                size: ctx.preview_size,
                taps: ray.aa_taps,
                fire: ray.fire,
            }
        }
    } else {
        CacheKind::Preview {
            size: ctx.preview_size,
            spp: ctx.preview_spp,
            max_bounces: PREVIEW_MAX_BOUNCES,
        }
    }
}

/// Catalogue materials store emitters beside `GemMaterial`; check before either lane dispatches.
pub(super) fn refuses_fluorescence(
    ctx: &BatchContext<'_>,
    resolved: &crate::gui::editor::CataloguePlanes,
    chosen: &indicatrix::optics::materials::GemMaterial,
) -> bool {
    use indicatrix_cut_core::native::{SnapshotColor, snapshot_color};
    if resolved
        .material
        .as_ref()
        .is_some_and(|m| m.has_body_color_override() || m.body_color_bands_override.is_some())
    {
        return false;
    }
    if ctx
        .custom_materials
        .iter()
        .any(|m| m.name.eq_ignore_ascii_case(&chosen.name))
    {
        let rows = ctx
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get_custom_materials();
        let Ok(rows) = rows else {
            return true;
        };
        return crate::gui::optics::crystal_optics::custom_material_fluorescence_from_rows(&rows)
            .iter()
            .any(|(name, glow)| name.eq_ignore_ascii_case(&chosen.name) && !glow.is_empty());
    }
    if indicatrix::optics::materials::GemMaterial::by_name(&chosen.name).is_some() {
        return false;
    }
    resolved
        .custom_material
        .as_ref()
        .is_some_and(|snapshot| match snapshot_color(snapshot) {
            SnapshotColor::Physics(mode) => !mode
                .fluorescence(indicatrix::optics::chromophore::ChromophoreCatalogue::global())
                .is_empty(),
            _ => false,
        })
}
