//! [`Design::effective_refractive_index`] and its catalogue-aware sibling
//! [`Design::effective_refractive_index_with`] -- what the `.asc` export's `I`
//! line and the optimizer/tilt curves/viewport derive their material from.

use crate::design::Design;
use indicatrix::optics::materials::GemMaterial;

impl Design {
    /// This design's effective refractive index -- what [`Self::to_asc_schedule`]
    /// writes to the exported `.asc`'s `I` line, and what the optimizer/tilt
    /// curves/viewport derive their material from.
    ///
    /// Precedence: `self.material.refractive_index_override` when set, else the
    /// selected material's own resolved `n_D` (built-ins only -- a caller that wants
    /// custom catalogue materials folded in too should call
    /// [`crate::material::MaterialSelection::resolve`] directly instead), else the
    /// legacy [`crate::design::ScheduleMeta::refractive_index`] this crate carried before a
    /// material model existed, read only for a design with neither an override nor a
    /// recognized material name (e.g. an untouched `.asc` import).
    #[must_use]
    pub fn effective_refractive_index(&self) -> f64 {
        self.material
            .refractive_index_override
            .or_else(|| {
                self.material
                    .name
                    .as_deref()
                    .and_then(crate::material::built_in_refractive_index)
            })
            .unwrap_or(self.meta.refractive_index)
    }

    /// Like [`Self::effective_refractive_index`], but also consults `custom` -- a
    /// caller's own resolved catalogue materials -- so a design whose
    /// [`crate::material::MaterialSelection::name`] names a CUSTOM catalogue entry
    /// (not one of the thirteen built-ins [`crate::material::built_in_refractive_index`]
    /// knows about) still scores against that material's real `n_D` instead of
    /// silently falling through to the legacy schedule field.
    ///
    /// Precedence: `self.material.refractive_index_override` when set, else a
    /// `custom` entry whose `name` matches `self.material.name`
    /// (ASCII-case-insensitively, the same match [`crate::material::MaterialLookup`]
    /// implementations in this codebase use), else the built-in table, else the
    /// legacy [`crate::design::ScheduleMeta::refractive_index`] fallback -- exactly
    /// [`Self::effective_refractive_index`]'s own order with one more rung inserted
    /// between "override" and "built-in".
    ///
    /// A caller that already resolves a design's material against a full catalogue
    /// (built-ins plus custom) elsewhere -- the optimizer/tilt-curve/solid-preview
    /// paths that build an `EditorMaterialLookup`-style lookup over
    /// `RenderContext::custom_materials` -- should call this instead of
    /// [`Self::effective_refractive_index`], passing that same custom list;
    /// [`Self::to_asc_schedule`]/[`Self::to_asc_schedule_from_solved`] and the
    /// optimizer's own objective keep calling the built-ins-only version, since
    /// neither has a custom catalogue on hand.
    #[must_use]
    pub fn effective_refractive_index_with(&self, custom: &[GemMaterial]) -> f64 {
        self.material
            .refractive_index_override
            .or_else(|| {
                self.material.name.as_deref().and_then(|name| {
                    custom
                        .iter()
                        .find(|gem| gem.name.eq_ignore_ascii_case(name))
                        .map(|gem| f64::from(gem.dispersion.evaluate(589.3)))
                })
            })
            .or_else(|| {
                self.material
                    .name
                    .as_deref()
                    .and_then(crate::material::built_in_refractive_index)
            })
            .unwrap_or(self.meta.refractive_index)
    }
}
