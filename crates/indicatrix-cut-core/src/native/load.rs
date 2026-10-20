//! What loading a design file shares: [`MaterialResolution`] and the `restore` helpers
//! that put per-tier state from a file's tier tables onto a freshly built
//! [`crate::design::Design`] (used by [`super::design_from_file`]).

use std::fmt;

mod restore;

pub(super) use restore::{
    apply_tier_frosting, apply_tier_ids_and_targets, apply_tier_relations,
    cheater_offsets_from_native, material_resolution_of, raw_tier_frosting_from_native,
    raw_tier_ids_from_native, raw_tier_relations_from_native, raw_tier_targets_from_native,
    restore_concave_tiers, tier_from_table_with_full_geometry, tier_notes_from_native,
};

/// Whether the loaded design's [`crate::material::MaterialSelection::name`] resolves
/// to a built-in [`indicatrix::optics::materials::GemMaterial`] this process
/// recognizes outright.
///
/// Checked purely against the built-in preset table via
/// [`indicatrix::optics::materials::GemMaterial::by_name`] -- this crate has no
/// dependency on a custom-material catalogue (that lives several layers up, in
/// `indicatrix-vault`/the editor's own session-local registry), so
/// [`Self::Unresolved`] is a HEURISTIC, not a guarantee: a name reported unresolved
/// here may still resolve fine against the caller's own catalogue (a custom material
/// saved under that name -- see [`crate::material::MaterialSelection::resolve`]'s own
/// `catalogue` parameter). It exists so a caller with no such catalogue wired up yet
/// (or one that hasn't re-checked) can still warn "this design names a material this
/// build doesn't recognize on its own" rather than silently accepting
/// [`crate::material::MaterialSelection::resolve`]'s "always returns something
/// usable" fallback to [`indicatrix::optics::materials::GemMaterial::diamond`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaterialResolution {
    /// `design.material.name` is `None` -- nothing to resolve.
    NoneSelected,
    /// `design.material.name` matches a built-in preset by exact name.
    Known,
    /// `design.material.name` matches no built-in preset. Likely (not certainly) a
    /// custom material this loading process's own catalogue would need to supply --
    /// see this type's own doc comment for why that can't be checked here.
    Unresolved,
}

impl fmt::Display for MaterialResolution {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoneSelected => write!(f, "no material selected"),
            Self::Known => write!(f, "material recognized"),
            Self::Unresolved => write!(
                f,
                "material not recognized as a built-in preset -- it may render as \
                 Diamond until a matching custom material is restored"
            ),
        }
    }
}
