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
    design::{ConstraintTier, Design, ScheduleMeta, TierId, TierTarget},
    material::MaterialSelection,
    preform::{PreformShape, PreformSpec},
};
use indicatrix::{
    geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_formats::native::{
    CustomMaterialSnapshot, HistoryTable, MaterialTable, NativeDesignFile, NativeMeetConstraint,
    NativePreformShape, NativeTierTarget, PreformTable, SourceTable, TierTable, sha256_hex,
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

/// Mirrors a [`TierTarget`] into its on-disk [`NativeTierTarget`] -- see that
/// type's own doc comment. The native round trip preserves every authored target.
#[must_use]
pub(super) const fn native_tier_target_from(target: TierTarget) -> NativeTierTarget {
    match target {
        TierTarget::DepthMm(mm) => NativeTierTarget::DepthMm { mm },
        TierTarget::GirdleThicknessMm(mm) => NativeTierTarget::GirdleThicknessMm { mm },
        TierTarget::TableWidthMm(mm) => NativeTierTarget::TableWidthMm { mm },
    }
}

/// The inverse of [`native_tier_target_from`].
#[must_use]
pub(super) const fn tier_target_from_native(target: NativeTierTarget) -> TierTarget {
    match target {
        NativeTierTarget::DepthMm { mm } => TierTarget::DepthMm(mm),
        NativeTierTarget::GirdleThicknessMm { mm } => TierTarget::GirdleThicknessMm(mm),
        NativeTierTarget::TableWidthMm { mm } => TierTarget::TableWidthMm(mm),
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

/// `y_offset` mirrors [`Design::preform_y_offset`] one-to-one -- a design-level
/// anchor `PreformSpec` itself does not carry, so it rides alongside rather than
/// inside the mirrored spec fields; see [`PreformTable::y_offset`]'s own doc
/// comment.
#[must_use]
pub(super) fn preform_table_from_spec(spec: &PreformSpec, y_offset: f64) -> PreformTable {
    PreformTable::new(
        native_preform_shape_from(spec.shape),
        spec.half_width,
        spec.length_over_width,
        spec.depth,
    )
    .with_y_offset(y_offset)
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

/// Mirrors an [`ExternalProportions`] into its on-disk [`SourceTable`] row -- see
/// that type's own doc comment.
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
///
/// `note`/`cheater_offset_deg`/`tier_id`/`target` are this tier's own entries (if
/// any) from [`Design::tier_notes`]/[`Design::cheater_offsets_deg`]/
/// [`Design::tier_ids`]/[`Design::tier_targets`] -- separate parameters rather
/// than `ConstraintTier` fields, since all four live on `Design` keyed by
/// position or by [`TierId`] (see those fields' own doc comments for why); the
/// caller (`to_native_file`) looks each up by the tier's array position before
/// calling this.
#[must_use]
pub(super) fn tier_table_from_tier(
    tier: &ConstraintTier,
    note: Option<String>,
    cheater_offset_deg: Option<f64>,
    tier_id: Option<TierId>,
    target: Option<TierTarget>,
) -> TierTable {
    TierTable::new(
        tier.name.clone(),
        native_meet_constraint_from(&tier.constraint),
        tier.detached.clone(),
    )
    .with_imported_meet(tier.imported_meet.as_ref().map(native_meet_constraint_from))
    .with_original_notes(tier.original_notes.clone())
    .with_note(note)
    .with_cheater_offset_deg(cheater_offset_deg)
    .with_tier_id(tier_id.map(TierId::value))
    .with_target(target.map(native_tier_target_from))
}

/// Extra, optional data a caller may attach to a native save beyond a bare design.
///
/// A custom material snapshot, a history trail, plus
/// [`Self::custom_catalogue`] (the wrong-`I`-line fix). Bundled into its own type,
/// rather than widening [`to_native_file`]/[`super::save_paired`] with more
/// positional parameters, so every existing call site of the original,
/// five-parameter [`super::save_paired`] -- four inside `apps/indicatrix-cut` --
/// keeps compiling unchanged; a caller that wants any extra
/// calls [`super::save_paired_extended`] instead. `#[derive(Default)]` gives the
/// "no extras at all" case used internally by [`super::save_paired`] itself, with
/// [`Self::custom_catalogue`] defaulting to `&[]` (an empty slice's own `Default`) --
/// exactly [`Design::effective_refractive_index`]'s built-ins-only behaviour.
#[derive(Debug, Clone, Default)]
pub struct SaveExtras<'a> {
    /// See [`CustomMaterialSnapshot`]'s own doc comment. `Some` only when
    /// `design.material.name` names a CUSTOM material (not a built-in
    /// [`GemMaterial::by_name`] preset) the caller's own catalogue resolved -- this
    /// crate has no catalogue of its own to check that against, so supplying the
    /// wrong thing here (e.g. always `Some` regardless of built-in status) is the
    /// caller's mistake, not this type's to prevent.
    pub custom_material: Option<&'a CustomMaterialSnapshot>,
    /// A bounded, human-readable trail of edits made to this design so far --
    /// [`crate::edit::History::description_log`], oldest first. An empty
    /// slice writes no `[history]` table at all -- see [`NativeDesignFile::
    /// with_history`].
    pub history_entries: &'a [String],
    /// This design's caller-resolved catalogue materials -- e.g.
    /// `RenderContext::custom_materials` in `apps/indicatrix-cut` -- consulted via
    /// [`Design::effective_refractive_index_with`] for the exported `.asc`'s `I`
    /// line whenever `design.material.name` names a CUSTOM entry (not one of the
    /// built-ins), rather than silently falling back to the legacy schedule RI.
    /// `&[]` (the `Default`) reproduces the old built-ins-only behaviour exactly --
    /// this is additive, not a breaking change to any existing caller.
    pub custom_catalogue: &'a [GemMaterial],
}

/// Reconstructs a session-local [`GemMaterial`] from a native file's custom-material
/// snapshot.
///
/// The material a design named that no longer resolves against
/// any catalogue this build has (see [`CustomMaterialSnapshot`]'s own doc comment).
/// A caller (the editor) registers the result under the design's own
/// `material.name` in its own catalogue so the design's real optics survive the
/// round trip instead of silently resolving to [`GemMaterial::diamond`].
///
/// `absorption_rgb` is always black (`[0.0; 3]`): [`CustomMaterialSnapshot`]
/// deliberately does not carry an absorption color (it would need this crate's own
/// dependency on `indicatrix` to interpret meaningfully, which the format crate
/// avoids -- see that type's own doc comment), so a restored material renders
/// colorless until a cutter picks a swatch again. `crystal_system`/
/// `optical_character` are NOT read from the snapshot's own string fields: exactly
/// like [`GemMaterial::new_custom`] itself, both are re-derived from the sign of
/// `birefringence_delta`, which is what every other caller of `new_custom` in this
/// codebase already relies on.
#[must_use]
pub fn gem_material_from_custom_snapshot(
    name: &str,
    snapshot: &CustomMaterialSnapshot,
) -> GemMaterial {
    GemMaterial::new_custom(
        name,
        snapshot.mean_ri as f32,
        snapshot.dispersion_delta as f32,
        snapshot.birefringence_delta as f32,
        [0.0, 0.0, 0.0],
    )
}

/// Builds a [`NativeDesignFile`] from `design`'s current state.
///
/// `asc_bytes` are the exact bytes written (or already written) as `asc_filename` --
/// see [`super::save::save_paired`] for the higher-level function that decides what
/// those bytes are (preserved original text vs. a fresh export) and calls this with
/// the result.
///
/// `printed_proportions`, when `Some`, is mirrored into the sidecar's own `[source]`
/// table so a later Open Native can restore it -- see [`NativeDesignFile::
/// with_source`] for why an all-`None` [`ExternalProportions`] still writes no table
/// at all. `extras` carries the optional custom-material snapshot and history trail
/// -- see [`SaveExtras`]'s own doc comment.
#[must_use]
pub fn to_native_file(
    design: &Design,
    asc_filename: impl Into<String>,
    asc_bytes: &[u8],
    printed_proportions: Option<&ExternalProportions>,
    extras: &SaveExtras<'_>,
) -> NativeDesignFile {
    let material = material_table_from_selection(&design.material)
        .with_custom(extras.custom_material.cloned());
    let native = NativeDesignFile::new(
        asc_filename,
        sha256_hex(asc_bytes),
        preform_table_from_spec(&design.preform, design.preform_y_offset),
        design.girdle_diameter_mm,
        material,
        design
            .tiers
            .iter()
            .enumerate()
            .map(|(index, tier)| {
                tier_table_from_tier(
                    tier,
                    design.tier_notes.get(&index).cloned(),
                    design.cheater_offsets_deg.get(&index).copied(),
                    design.tier_id_at(index),
                    design.tier_target(index),
                )
            })
            .collect(),
    )
    .with_history(HistoryTable::new(extras.history_entries.to_vec()))
    // The AUTHORED (legacy schedule) RI, distinct from the EFFECTIVE one `.asc`
    // export always writes -- see
    // `NativeDesignFile::authored_refractive_index`'s own doc comment.
    // Always `Some` on a fresh save: `design.meta.refractive_index` is a plain
    // `f64`, never itself optional.
    .with_authored_refractive_index(Some(design.meta.refractive_index));
    match printed_proportions {
        Some(props) => native.with_source(source_table_from_proportions(props)),
        None => native,
    }
}

/// The `unknown`-table key [`stash_schedule_meta`]/[`unstash_schedule_meta`] use --
/// namespaced (not a bare `"meta"`) so it can never collide with a real top-level
/// field a future `indicatrix-formats` schema version adds.
const SELF_CONTAINED_META_KEY: &str = "indicatrix_cut_core_self_contained_meta";

/// Packs `meta` into a nested table under [`SELF_CONTAINED_META_KEY`] in `unknown`
/// -- the extension point [`indicatrix_formats::native::NativeDesignFile::unknown`]'s
/// own doc comment describes ("keys a future build wrote that this build's four
/// named fields above don't claim"). [`ScheduleMeta`] has no counterpart anywhere
/// in `indicatrix_formats::native`'s schema at all: every `.asc`-only header field
/// (gear teeth, symmetry, mirror, the `GemCad` version banner, headers/footnotes)
/// normally stays canonical in the paired `.asc` alone (see the parent module's doc
/// comment). A self-contained save (one that must restore with no `.asc` sidecar
/// around) needs somewhere to carry it anyway, since there is no paired `.asc` to
/// fall back on; this is that somewhere, reusing the format's own
/// forward-compatibility mechanism rather than widening the schema itself
/// (`crates/indicatrix-formats`'s own schema, not this crate's to change).
pub(super) fn stash_schedule_meta(unknown: &mut toml::Table, meta: &ScheduleMeta) {
    let mut table = toml::Table::new();
    table.insert(
        "gemcad_version".to_string(),
        toml::Value::String(meta.gemcad_version.clone()),
    );
    table.insert(
        "gear_teeth".to_string(),
        toml::Value::Integer(i64::from(meta.gear_teeth)),
    );
    table.insert(
        "gear_reference_angle".to_string(),
        toml::Value::Float(meta.gear_reference_angle),
    );
    table.insert(
        "symmetry_order".to_string(),
        toml::Value::Integer(i64::from(meta.symmetry_order)),
    );
    table.insert("mirror".to_string(), toml::Value::Boolean(meta.mirror));
    table.insert(
        "refractive_index".to_string(),
        toml::Value::Float(meta.refractive_index),
    );
    table.insert(
        "headers".to_string(),
        toml::Value::Array(
            meta.headers
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    table.insert(
        "footnotes".to_string(),
        toml::Value::Array(
            meta.footnotes
                .iter()
                .cloned()
                .map(toml::Value::String)
                .collect(),
        ),
    );
    unknown.insert(
        SELF_CONTAINED_META_KEY.to_string(),
        toml::Value::Table(table),
    );
}

/// The inverse of [`stash_schedule_meta`]. `None` when `unknown` carries no such
/// entry at all (an ordinary paired-mode native file, which never stashes one) or
/// the entry is malformed (any field missing or the wrong TOML type -- defensive
/// only, since [`stash_schedule_meta`] always writes every field), in which case
/// the caller ([`super::load::load_native_only`]) reports "not a self-contained
/// save" rather than guessing at defaults that would silently misinterpret this
/// design's own index-wheel positions (`gear_teeth` wrong changes what every
/// tier's `indices` even mean).
#[must_use]
pub(super) fn unstash_schedule_meta(unknown: &toml::Table) -> Option<ScheduleMeta> {
    let table = unknown.get(SELF_CONTAINED_META_KEY)?.as_table()?;
    let strings = |key: &str| -> Option<Vec<String>> {
        table
            .get(key)?
            .as_array()?
            .iter()
            .map(|v| v.as_str().map(str::to_string))
            .collect()
    };
    Some(ScheduleMeta {
        gemcad_version: table.get("gemcad_version")?.as_str()?.to_string(),
        gear_teeth: i32::try_from(table.get("gear_teeth")?.as_integer()?).ok()?,
        gear_reference_angle: table.get("gear_reference_angle")?.as_float()?,
        symmetry_order: u32::try_from(table.get("symmetry_order")?.as_integer()?).ok()?,
        mirror: table.get("mirror")?.as_bool()?,
        refractive_index: table.get("refractive_index")?.as_float()?,
        headers: strings("headers")?,
        footnotes: strings("footnotes")?,
    })
}
