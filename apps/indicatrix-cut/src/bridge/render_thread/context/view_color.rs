//! The Live Render toolbar's quick colour change, as the render pipeline sees it.
//!
//! A cutter can recolour the rendered stone from the toolbar without making a custom
//! material. When the view shows the editor's open design with the render material linked
//! to it, the pick is the design's own colour override and already travels in
//! `RenderContext::material_override` (see `gui::editor::view::sync_viewport_material_link`).
//! In every other case (a library design, or the render material unlinked) the pick is
//! only a view setting: [`RenderContext::view_body_color`]. It is never written to a design.
//!
//! [`RenderContext::tinted_material_override`] is the ONE hook that turns the view setting
//! into the material every consumer renders: the render loop's frame snapshot, the export
//! and tilt-video scene snapshot (which the remote workers are sent), and the tilt
//! analysis. The decisions are plain functions ([`color_target`], [`effective_view_color`])
//! so they are tested without a window.

use super::{PlanesOwner, RenderContext, resolve_material_with_override};
use indicatrix::optics::materials::GemMaterial;
use std::sync::OnceLock;

/// Who a colour pick in the Live Render toolbar changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorTarget {
    /// The open design's own colour override: an undoable edit, like the Edit tab's
    /// colour box.
    Design,
    /// Only the colour the view renders; the design and the catalogue are not touched.
    View,
}

/// Where a colour pick goes: into the design only while the view shows the editor's
/// open design (`owner`) with the render material linked to it (`linked`).
#[must_use]
pub const fn color_target(linked: bool, owner: PlanesOwner) -> ColorTarget {
    if linked && matches!(owner, PlanesOwner::Editor { .. }) {
        ColorTarget::Design
    } else {
        ColorTarget::View
    }
}

/// The view colour that is in force: `view` only when picks go to the view
/// ([`ColorTarget::View`]) and the traced material does not define its own colour
/// (`physics`). A design's own override is never a view colour, and a material with a
/// physics recipe is never recoloured.
#[must_use]
pub const fn effective_view_color(
    view: Option<[f32; 3]>,
    target: ColorTarget,
    physics: bool,
) -> Option<[f32; 3]> {
    match (target, physics) {
        (ColorTarget::View, false) => view,
        _ => None,
    }
}

/// The built-in material table, built once: the hook below runs every frame while a
/// view colour is in force.
fn builtin_materials() -> &'static [GemMaterial] {
    static BUILTINS: OnceLock<Vec<GemMaterial>> = OnceLock::new();
    BUILTINS.get_or_init(GemMaterial::all_materials)
}

impl RenderContext {
    /// Who a colour pick changes right now: see [`color_target`].
    #[must_use]
    pub const fn color_target(&self) -> ColorTarget {
        color_target(self.material_linked, self.planes_owner)
    }

    /// The view-only body colour that applies to the stone being rendered, if any: see
    /// [`effective_view_color`]. Part of the scene identity, so a change restarts the
    /// accumulation (local and remote).
    #[must_use]
    pub fn effective_view_body_color(&self) -> Option<[f32; 3]> {
        // Asked every frame: nothing else is computed while the setting is off.
        let view = self.view_body_color?;
        effective_view_color(Some(view), self.color_target(), self.physics_color())
    }

    /// [`Self::material_override`] with the view colour applied: the material every
    /// render of this context uses in place of the raw override.
    ///
    /// With no view colour in force this is exactly `material_override.clone()`, so a
    /// scene without the setting renders bit-identically to one made before it existed.
    /// With one, the material that would have been rendered (the override, or the
    /// by-name lookup) is recoloured the way a design's own colour override recolours
    /// it (`GemMaterial::with_body_color`): same species, optics and name, isotropic
    /// absorption of the chosen colour. A name that resolves to nothing stays
    /// unresolved (`None` with no override), exactly as before.
    #[must_use]
    pub fn tinted_material_override(&self) -> Option<GemMaterial> {
        let Some(rgb) = self.effective_view_body_color() else {
            return self.material_override.clone();
        };
        resolve_material_with_override(
            builtin_materials(),
            &self.custom_materials,
            self.material_override.as_ref(),
            &self.material_name,
        )
        .map(|material| material.with_body_color(rgb))
        .or_else(|| self.material_override.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix::optics::materials::body_color::BODY_COLOR_PRESETS;
    use std::sync::Arc;

    const YELLOW: [f32; 3] = BODY_COLOR_PRESETS[5].absorption_rgb;
    const EDITOR: PlanesOwner = PlanesOwner::Editor { generation: 3 };
    const LIBRARY: PlanesOwner = PlanesOwner::Catalogue { entry_id: 7 };

    fn view_ctx(name: &str, colour: Option<[f32; 3]>) -> RenderContext {
        RenderContext {
            material_name: name.to_string(),
            view_body_color: colour,
            material_linked: false,
            ..Default::default()
        }
    }

    #[test]
    fn a_pick_goes_to_the_design_only_while_the_view_shows_the_linked_open_design() {
        assert_eq!(color_target(true, EDITOR), ColorTarget::Design);
        assert_eq!(color_target(false, EDITOR), ColorTarget::View);
        assert_eq!(color_target(true, LIBRARY), ColorTarget::View);
        assert_eq!(color_target(false, LIBRARY), ColorTarget::View);
        assert_eq!(color_target(true, PlanesOwner::Builtin), ColorTarget::View);
        assert_eq!(color_target(false, PlanesOwner::Builtin), ColorTarget::View);
    }

    #[test]
    fn the_view_colour_applies_only_to_the_view_and_never_to_a_physics_material() {
        assert_eq!(
            effective_view_color(Some(YELLOW), ColorTarget::View, false),
            Some(YELLOW)
        );
        assert_eq!(
            effective_view_color(Some(YELLOW), ColorTarget::Design, false),
            None,
            "a design colour is the design's own override, never a view colour on top"
        );
        assert_eq!(
            effective_view_color(Some(YELLOW), ColorTarget::View, true),
            None,
            "a material that defines its own colour is not recoloured"
        );
        assert_eq!(effective_view_color(None, ColorTarget::View, false), None);
    }

    #[test]
    fn without_a_view_colour_the_override_is_passed_through_untouched() {
        let mut ctx = view_ctx("Sapphire", None);
        assert_eq!(ctx.tinted_material_override(), None);
        ctx.material_override = Some(GemMaterial::diamond());
        assert_eq!(ctx.tinted_material_override(), Some(GemMaterial::diamond()));
    }

    #[test]
    fn a_view_colour_recolours_the_named_material_and_keeps_its_optics() {
        let tinted = view_ctx("Sapphire", Some(YELLOW))
            .tinted_material_override()
            .expect("Sapphire resolves");
        let base = GemMaterial::sapphire();
        let expected = GemMaterial::sapphire().with_body_color(YELLOW);
        assert_eq!(tinted.name, "Sapphire", "the species name must not change");
        assert_eq!(tinted.dispersion, base.dispersion);
        assert_eq!(tinted.absorption, expected.absorption);
        assert_ne!(
            tinted.absorption, base.absorption,
            "test premise: the yellow variant differs from sapphire's own colour"
        );
    }

    #[test]
    fn a_view_colour_recolours_a_custom_material_found_by_name() {
        let mut custom = GemMaterial::diamond();
        custom.name = "My Garnet".to_string();
        let mut ctx = view_ctx("my garnet", Some(YELLOW));
        ctx.custom_materials = Arc::new(vec![custom.clone()]);
        let tinted = ctx.tinted_material_override().expect("custom resolves");
        assert_eq!(tinted.name, "My Garnet");
        assert_eq!(tinted.absorption, custom.with_body_color(YELLOW).absorption);
    }

    #[test]
    fn a_view_colour_goes_on_top_of_an_override_material() {
        let mut ctx = view_ctx("Diamond", Some(YELLOW));
        ctx.material_override = Some(GemMaterial::sapphire());
        let tinted = ctx.tinted_material_override().expect("override resolves");
        assert_eq!(tinted.name, "Sapphire", "the override decides the species");
        assert_eq!(
            tinted.absorption,
            GemMaterial::sapphire().with_body_color(YELLOW).absorption
        );
    }

    #[test]
    fn the_linked_open_design_keeps_its_own_colour_and_ignores_the_view_setting() {
        let design_material =
            GemMaterial::sapphire().with_body_color(BODY_COLOR_PRESETS[1].absorption_rgb);
        let mut ctx = view_ctx("Sapphire", Some(YELLOW));
        ctx.material_override = Some(design_material.clone());
        ctx.planes_owner = EDITOR;
        ctx.material_linked = true;
        assert_eq!(ctx.effective_view_body_color(), None);
        assert_eq!(ctx.tinted_material_override(), Some(design_material));

        // Unlinking turns the view colour back on, on top of the dropdown material.
        ctx.material_linked = false;
        ctx.material_override = None;
        assert_eq!(ctx.effective_view_body_color(), Some(YELLOW));
        // A library design takes the view colour even while the link toggle is on.
        ctx.material_linked = true;
        ctx.planes_owner = LIBRARY;
        assert_eq!(ctx.effective_view_body_color(), Some(YELLOW));
    }

    #[test]
    fn a_physics_material_is_not_recoloured() {
        let mut custom = GemMaterial::diamond();
        custom.name = "Physics Ruby".to_string();
        let mut ctx = view_ctx("Physics Ruby", Some(YELLOW));
        ctx.custom_materials = Arc::new(vec![custom]);
        ctx.set_custom_material_physics("Physics Ruby", true);
        assert_eq!(ctx.effective_view_body_color(), None);
        assert_eq!(ctx.tinted_material_override(), None);
    }

    #[test]
    fn an_unresolved_name_stays_unresolved_instead_of_turning_into_something_else() {
        let ctx = view_ctx("No Such Stone", Some(YELLOW));
        assert_eq!(ctx.tinted_material_override(), None);
    }

    #[test]
    fn changing_the_effective_view_colour_restarts_the_scene_but_a_dormant_one_does_not() {
        let mut ctx = view_ctx("Sapphire", None);
        let first = ctx.scene_generation();
        ctx.view_body_color = Some(YELLOW);
        let second = ctx.scene_generation();
        assert!(
            second > first,
            "a new view colour must restart accumulation"
        );
        assert_eq!(ctx.scene_generation(), second, "nothing changed since");

        // The same setting held by a design-driven view is dormant: not a scene change.
        ctx.planes_owner = EDITOR;
        ctx.material_linked = true;
        let third = ctx.scene_generation();
        assert!(
            third > second,
            "the colour stopped applying, the scene changed"
        );
        ctx.view_body_color = Some(BODY_COLOR_PRESETS[2].absorption_rgb);
        assert_eq!(
            ctx.scene_generation(),
            third,
            "a view colour that does not apply must not restart the render"
        );
    }
}
