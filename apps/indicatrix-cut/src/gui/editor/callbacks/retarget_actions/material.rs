//! Resolves and displays the retarget dialog's own TARGET material -- see this
//! group's own `mod.rs` doc comment ("Where the target material comes from").

use crate::gui::editor::{
    material_lookup::{EditorMaterialLookup, resolved_gem_material},
    state::{design_material_index_from_name, design_material_options, parse_design_material_form},
};
use indicatrix::optics::materials::GemMaterial;
use indicatrix_cut_core::{Design, MaterialSelection, ResolvedMaterial};

/// Just the [`MaterialSelection`] half of [`super::proposal_view::retarget_view`]'s
/// own target resolution, split out so the async `RetargetMode::Optimize` wiring
/// (which needs a real [`MaterialSelection`] to hand
/// [`super::super::super::optimize_solve::spawn_optimize_solve`], not a
/// pre-resolved [`ResolvedMaterial`]) doesn't reimplement this parsing.
///
/// # Errors
///
/// [`parse_design_material_form`]'s own message when `ri_override_text` is non-empty
/// and does not parse as a finite refractive index greater than 1.0 -- see
/// [`target_material_selection`] for the lenient wrapper most callers actually want.
pub(super) fn resolve_target_selection(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> Result<MaterialSelection, String> {
    let options = design_material_options(custom);
    parse_design_material_form(combo_index, ri_override_text, &options, &design.material)
}

/// [`resolve_target_selection`], falling back to `design.material` unchanged on a
/// parse error rather than surfacing it -- used wherever a caller needs SOME target
/// selection unconditionally (`optimize_run::start_optimize_run`'s own search
/// input). The two dialog-readout call sites (`proposal::rebuild_and_push`/
/// `optimize_run::start_optimize_run`'s own up-front check) call
/// [`resolve_target_selection`] directly instead, so a parse error actually reaches
/// the cutter -- see `proposal_view::push_target_error`.
pub(super) fn target_material_selection(
    design: &Design,
    custom: &[GemMaterial],
    combo_index: i32,
    ri_override_text: &str,
) -> MaterialSelection {
    resolve_target_selection(design, custom, combo_index, ri_override_text)
        .unwrap_or_else(|_| design.material.clone())
}

/// Resolves an already-known [`MaterialSelection`] into a [`ResolvedMaterial`]
/// against `custom`. [`resolved_gem_material`] (not plain `MaterialSelection::resolve`)
/// is used for the returned `gem`, so an RI override shows up in the target's own
/// dispersion (matters for `RetargetMode::Optimize`'s objective) -- shared by every
/// caller here that already has a `MaterialSelection` in hand, so this one three-line
/// pattern isn't repeated per call site.
pub(super) fn resolved_material_from_selection(
    selection: &MaterialSelection,
    custom: &[GemMaterial],
) -> ResolvedMaterial {
    let lookup = EditorMaterialLookup::new(custom);
    let resolved = selection.resolve(&lookup);
    let gem = resolved_gem_material(selection, &lookup);
    ResolvedMaterial { gem, ..resolved }
}

/// The readout label for a retarget target: `target.gem.name` is
/// [`resolved_gem_material`]'s pick of an actual [`GemMaterial`] to render/compute
/// dispersion against, which falls back to Diamond (`material.rs`'s own default) for
/// both "(none)" and a typed custom RI -- neither of which the cutter ever asked to
/// move toward Diamond. Derived from the [`MaterialSelection`] that was actually
/// resolved instead: the name when one was picked, `"Custom RI"` for a typed
/// override with no name, `"(none)"` for neither.
pub(super) fn target_display_name(selection: &MaterialSelection) -> String {
    selection.name.clone().unwrap_or_else(|| {
        if selection.refractive_index_override.is_some() {
            "Custom RI".to_string()
        } else {
            "(none)".to_string()
        }
    })
}

/// The target combo's initial index for `material` -- the design's own current
/// material by name, except a name-less selection carrying an RI override (the
/// "Custom RI…" case, see `design_material_name_from_index`'s own doc comment)
/// which has no name to look up at all and must instead seed the trailing sentinel
/// entry [`design_material_options`] always appends.
pub(super) fn initial_target_index(material: &MaterialSelection, options: &[String]) -> i32 {
    if material.name.is_none() && material.refractive_index_override.is_some() {
        return i32::try_from(options.len()).unwrap_or(i32::MAX) - 1;
    }
    design_material_index_from_name(material.name.as_deref(), options)
}
