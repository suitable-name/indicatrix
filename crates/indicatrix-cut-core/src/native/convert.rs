//! Conversions between the on-disk mirror types in [`indicatrix_formats::native`] and this
//! crate's own editor types ([`Design`], [`ConstraintTier`], [`MaterialSelection`],
//! [`PreformSpec`]) plus `indicatrix`'s own
//! [`indicatrix::geometry::meet_solver::MeetConstraint`] -- the half of the native-file
//! story that DOES need editor types in scope, so it lives here rather than in the
//! dependency-free format crate. See [`indicatrix_formats::native`]'s own module doc comment for
//! the schema itself, and the parent module's doc comment for why this split exists at
//! all.
//!
//! Plain free functions, not `From`/`Into` impls: every mirror type here is foreign to
//! this crate (owned by `indicatrix-formats`) and every editor type it converts with is either
//! also foreign (`MeetConstraint`, owned by `indicatrix`) or would need the impl's
//! `Self` type to be foreign too -- Rust's orphan rules don't allow a trait impl
//! bridging two crates neither of which is this one, so a free function is the only
//! option (not just the simplest one).

use crate::{
    design::{ConstraintTier, Design},
    material::MaterialSelection,
    preform::{PreformShape, PreformSpec},
};
use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_formats::native::{
    MaterialTable, NativeDesignFile, NativeMeetConstraint, NativePreformShape, PreformTable,
    TierTable, sha256_hex,
};

pub(super) fn native_meet_constraint_from(constraint: &MeetConstraint) -> NativeMeetConstraint {
    match constraint {
        MeetConstraint::MeetExisting => NativeMeetConstraint::MeetExisting,
        MeetConstraint::MeetNamed(names) => NativeMeetConstraint::MeetNamed {
            names: names.clone(),
        },
        MeetConstraint::ScaleReference(mast) => {
            NativeMeetConstraint::ScaleReference { mast: *mast }
        }
    }
}

pub(super) fn meet_constraint_from_native(constraint: NativeMeetConstraint) -> MeetConstraint {
    match constraint {
        NativeMeetConstraint::MeetExisting => MeetConstraint::MeetExisting,
        NativeMeetConstraint::MeetNamed { names } => MeetConstraint::MeetNamed(names),
        NativeMeetConstraint::ScaleReference { mast } => MeetConstraint::ScaleReference(mast),
    }
}

pub(super) const fn native_preform_shape_from(shape: PreformShape) -> NativePreformShape {
    match shape {
        PreformShape::Block => NativePreformShape::Block,
        PreformShape::Cylinder { sides } => NativePreformShape::Cylinder { sides },
    }
}

pub(super) const fn preform_shape_from_native(shape: NativePreformShape) -> PreformShape {
    match shape {
        NativePreformShape::Block => PreformShape::Block,
        NativePreformShape::Cylinder { sides } => PreformShape::Cylinder { sides },
    }
}

#[must_use]
pub(super) fn preform_table_from_spec(spec: &PreformSpec) -> PreformTable {
    PreformTable::new(
        native_preform_shape_from(spec.shape),
        spec.half_width,
        spec.length_over_width,
        spec.depth,
    )
}

#[must_use]
pub(super) const fn preform_spec_from_table(table: &PreformTable) -> PreformSpec {
    PreformSpec {
        shape: preform_shape_from_native(table.shape),
        half_width: table.half_width,
        length_over_width: table.length_over_width,
        depth: table.depth,
    }
}

#[must_use]
pub(super) fn material_table_from_selection(selection: &MaterialSelection) -> MaterialTable {
    MaterialTable::new(
        selection.name.clone(),
        selection.specific_gravity_override,
        selection.refractive_index_override,
    )
}

#[must_use]
pub(super) fn material_selection_from_table(table: &MaterialTable) -> MaterialSelection {
    MaterialSelection {
        name: table.name.clone(),
        specific_gravity_override: table.specific_gravity_override,
        refractive_index_override: table.refractive_index_override,
    }
}

#[must_use]
pub(super) fn tier_table_from_tier(tier: &ConstraintTier) -> TierTable {
    TierTable::new(
        tier.name.clone(),
        native_meet_constraint_from(&tier.constraint),
        tier.detached.clone(),
    )
}

/// Builds a [`NativeDesignFile`] from `design`'s current state.
///
/// `asc_bytes` are the exact bytes written (or already written) as `asc_filename` --
/// see [`super::save::save_paired`] for the higher-level function that decides what
/// those bytes are (preserved original text vs. a fresh export) and calls this with
/// the result.
#[must_use]
pub fn to_native_file(
    design: &Design,
    asc_filename: impl Into<String>,
    asc_bytes: &[u8],
) -> NativeDesignFile {
    NativeDesignFile::new(
        asc_filename,
        sha256_hex(asc_bytes),
        preform_table_from_spec(&design.preform),
        design.girdle_diameter_mm,
        material_table_from_selection(&design.material),
        design.tiers.iter().map(tier_table_from_tier).collect(),
    )
}
