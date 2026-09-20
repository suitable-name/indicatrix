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
use indicatrix::geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions};
use indicatrix_formats::native::{
    MaterialTable, NativeDesignFile, NativeMeetConstraint, NativePreformShape, PreformTable,
    SourceTable, TierTable, sha256_hex,
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

/// Mirrors an [`ExternalProportions`] into its on-disk [`SourceTable`] row -- see that
/// type's own doc comment (Item 178).
#[must_use]
pub(super) const fn source_table_from_proportions(props: &ExternalProportions) -> SourceTable {
    SourceTable {
        vol_w3: props.vol_w3,
        lw: props.lw,
        cw: props.cw,
        pw: props.pw,
        hw: props.hw,
    }
}

/// The inverse of [`source_table_from_proportions`].
#[must_use]
pub(super) const fn external_proportions_from_source(table: &SourceTable) -> ExternalProportions {
    ExternalProportions {
        vol_w3: table.vol_w3,
        lw: table.lw,
        cw: table.cw,
        pw: table.pw,
        hw: table.hw,
    }
}

/// Mirrors one editor tier into its on-disk row.
///
/// The imported meet instruction and the `.asc` file's original `G` note ride along
/// so an ordinary save is self-describing: a reload can restore the cutter's "Adopt
/// Meet" action and re-export the file's real note text instead of a synthesized
/// "Set stone size.". [`super::load::load_paired`] recomputes both from the paired
/// `.asc` when the fingerprint matches, so these two fields matter for a draft, for
/// hand inspection and for later tooling.
#[must_use]
pub(super) fn tier_table_from_tier(tier: &ConstraintTier) -> TierTable {
    TierTable::new(
        tier.name.clone(),
        native_meet_constraint_from(&tier.constraint),
        tier.detached.clone(),
    )
    .with_imported_meet(tier.imported_meet.as_ref().map(native_meet_constraint_from))
    .with_original_notes(tier.original_notes.clone())
}

/// Builds a [`NativeDesignFile`] from `design`'s current state.
///
/// `asc_bytes` are the exact bytes written (or already written) as `asc_filename` --
/// see [`super::save::save_paired`] for the higher-level function that decides what
/// those bytes are (preserved original text vs. a fresh export) and calls this with
/// the result.
///
/// `printed_proportions`, when `Some`, is mirrored into the sidecar's own `[source]`
/// table (Item 178) so a later Open Native can restore it -- see [`NativeDesignFile::
/// with_source`] for why an all-`None` [`ExternalProportions`] still writes no table
/// at all.
#[must_use]
pub fn to_native_file(
    design: &Design,
    asc_filename: impl Into<String>,
    asc_bytes: &[u8],
    printed_proportions: Option<&ExternalProportions>,
) -> NativeDesignFile {
    let native = NativeDesignFile::new(
        asc_filename,
        sha256_hex(asc_bytes),
        preform_table_from_spec(&design.preform),
        design.girdle_diameter_mm,
        material_table_from_selection(&design.material),
        design.tiers.iter().map(tier_table_from_tier).collect(),
    );
    match printed_proportions {
        Some(props) => native.with_source(source_table_from_proportions(props)),
        None => native,
    }
}
