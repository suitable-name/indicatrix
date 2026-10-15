//! Conversions between the on-disk mirror types in [`indicatrix_formats::native`] and this
//! crate's own editor types ([`Design`], [`ConstraintTier`], [`MaterialSelection`],
//! [`PreformSpec`]) plus `indicatrix`'s own
//! [`indicatrix::geometry::meet_solver::MeetConstraint`] -- the half of the design-file
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
    design::{ConcaveTier, ConcaveTool, ConstraintTier, Design, TierId, TierTarget, ToolMotion},
    material::{
        ColorMode, MaterialSelection, biaxial_character_from_deltas, crystal_system_from_name,
        is_biaxial, optical_character_from_name,
    },
    preform::{PlaneSet, PreformShape, PreformSpec},
};
use glam::DVec3;
use indicatrix::{
    geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_formats::native::{
    ColorRecipeDto, ConcaveTierTable, CustomMaterialSnapshot, MaterialTable, NativeMeetConstraint,
    NativePreformShape, NativeTierTarget, PreformTable, SourceTable, TierTable,
};
use std::sync::Arc;

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

/// Mirrors a [`PreformShape`] into its on-disk [`NativePreformShape`]. A plane set is
/// written as its `[nx, ny, nz, m]` rows, in order, with its source note.
pub(super) fn native_preform_shape_from(shape: &PreformShape) -> NativePreformShape {
    match shape {
        PreformShape::Block => NativePreformShape::Block,
        PreformShape::Cylinder { sides } => NativePreformShape::Cylinder { sides: *sides },
        PreformShape::Planes(set) => NativePreformShape::Planes {
            planes: set
                .planes
                .iter()
                .map(|&(n, m)| [n.x, n.y, n.z, m])
                .collect(),
            source: set.source.clone(),
        },
    }
}

/// The inverse of [`native_preform_shape_from`]; the rows come back bit for bit.
pub(super) fn preform_shape_from_native(shape: &NativePreformShape) -> PreformShape {
    match shape {
        NativePreformShape::Block => PreformShape::Block,
        NativePreformShape::Cylinder { sides } => PreformShape::Cylinder { sides: *sides },
        NativePreformShape::Planes { planes, source } => PreformShape::Planes(Arc::new(PlaneSet {
            planes: planes
                .iter()
                .map(|&[nx, ny, nz, m]| (DVec3::new(nx, ny, nz), m))
                .collect(),
            source: source.clone(),
        })),
    }
}

/// `y_offset` mirrors [`Design::preform_y_offset`] one-to-one -- a design-level
/// anchor `PreformSpec` itself does not carry, so it rides alongside rather than
/// inside the mirrored spec fields; see [`PreformTable::y_offset`]'s own doc
/// comment.
#[must_use]
pub(super) fn preform_table_from_spec(spec: &PreformSpec, y_offset: f64) -> PreformTable {
    PreformTable::new(
        native_preform_shape_from(&spec.shape),
        spec.half_width,
        spec.length_over_width,
        spec.depth,
    )
    .with_y_offset(y_offset)
}

/// The spec's size fields are read back exactly as the table holds them (a plane set's
/// are its recorded bounding box; they are not recomputed from the planes).
#[must_use]
pub(super) fn preform_spec_from_table(table: &PreformTable) -> PreformSpec {
    PreformSpec {
        shape: preform_shape_from_native(&table.shape),
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
    .with_body_color_override(selection.body_color_override.map(body_color_to_table))
    .with_body_color_bands_override(
        selection
            .body_color_bands_override
            .as_ref()
            .filter(|b| !b.is_empty())
            .map(|rows| rows.iter().copied().map(body_color_to_table).collect()),
        selection
            .absorption_path_scale_override
            .map(f32_to_f64_short),
    )
}

/// `f32` -> `f64` through the shortest round-trip decimal (see [`body_color_to_table`]).
fn f32_to_f64_short(v: f32) -> f64 {
    v.to_string()
        .parse::<f64>()
        .unwrap_or_else(|_| f64::from(v))
}

#[must_use]
pub(super) fn material_selection_from_table(table: &MaterialTable) -> MaterialSelection {
    MaterialSelection {
        name: table.name.clone(),
        specific_gravity_override: table.specific_gravity_override,
        refractive_index_override: table.refractive_index_override,
        body_color_override: table.body_color_override.map(body_color_from_table),
        body_color_bands_override: table
            .body_color_bands_override
            .as_ref()
            .filter(|b| !b.is_empty())
            .map(|rows| rows.iter().copied().map(body_color_from_table).collect()),
        absorption_path_scale_override: table
            .body_color_bands_override
            .as_ref()
            .filter(|b| !b.is_empty())
            .and(table.absorption_path_scale_override)
            .map(narrow),
    }
}

/// A stored `f64` material value at the `f32` precision the renderer and `GemMaterial` use.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the renderer's material precision; stored values are in the normal float range"
)]
pub(super) const fn narrow(value: f64) -> f32 {
    value as f32
}

/// `f32` triple -> the `f64` triple `MaterialTable` stores, by way of each
/// component's SHORTEST round-trip decimal (`f32`'s `Display`): a plain
/// `f64::from(0.2f32)` would serialise as `0.20000000298023224`, which is what the
/// on-disk file would then show a cutter for a color they picked as "0.2". The
/// decimal text uniquely identifies the `f32`, so [`body_color_from_table`]'s cast
/// back returns the identical bits (asserted by the native round-trip tests).
fn body_color_to_table(rgb: [f32; 3]) -> [f64; 3] {
    rgb.map(|v| {
        v.to_string()
            .parse::<f64>()
            .unwrap_or_else(|_| f64::from(v))
    })
}

/// The inverse of [`body_color_to_table`]: nearest `f32` per component, which for a
/// value written by that function is the original `f32` exactly.
fn body_color_from_table(rgb: [f64; 3]) -> [f32; 3] {
    rgb.map(narrow)
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
/// so a save is self-describing: a reload can restore the cutter's "Adopt Meet"
/// action and re-export the file's real note text instead of a synthesized "Set stone
/// size.".
///
/// `note`/`cheater_offset_deg`/`tier_id`/`target` are this tier's own entries (if
/// any) from [`Design::tier_notes`]/[`Design::cheater_offsets_deg`]/
/// [`Design::tier_ids`]/[`Design::tier_targets`] -- separate parameters rather
/// than `ConstraintTier` fields, since all four live on `Design` keyed by
/// position or by [`TierId`] (see those fields' own doc comments for why); the
/// caller looks each up by the tier's array position before calling this.
///
/// Also carries `tier.indices` verbatim (the design's own AUTHORED, never
/// shifted, index-wheel positions): the shift
/// `Design::to_asc_schedule_from_solved_with_cheater_offsets` bakes into an exported
/// `.asc` (see [`Design::cheater_offsets_deg`]'s own doc comment) must not be
/// re-interpreted as the tier's own base indices on the next load.
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
    .with_indices(Some(tier.indices.clone()))
    .with_imported_meet(tier.imported_meet.as_ref().map(native_meet_constraint_from))
    .with_original_notes(tier.original_notes.clone())
    .with_note(note)
    .with_cheater_offset_deg(cheater_offset_deg)
    .with_tier_id(tier_id.map(TierId::value))
    .with_target(target.map(native_tier_target_from))
}

/// Extra, optional data a caller may attach to a save beyond a bare design.
///
/// A custom material snapshot, a history trail, plus
/// [`Self::custom_catalogue`] (the wrong-`I`-line fix). Bundled into its own type,
/// rather than widening [`super::catalogue_asc`] and [`super::DesignExtras`] with more
/// positional parameters; [`super::catalogue_asc_extended`] takes it, and
/// `DesignExtras::from(&extras)` carries the snapshot and history into a design file.
/// `#[derive(Default)]` gives the "no extras at all" case used internally by
/// [`super::catalogue_asc`] itself, with [`Self::custom_catalogue`] defaulting to `&[]`
/// (an empty slice's own `Default`) -- exactly
/// [`Design::effective_refractive_index`]'s built-ins-only behaviour.
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
    /// slice writes no `[history]` table at all -- see
    /// [`indicatrix_formats::native::design::DesignFile::with_history`].
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

/// How a native snapshot's color reads (spec 6): which payload the file really carries.
#[derive(Debug, Clone, PartialEq)]
pub enum SnapshotColor {
    /// No recipe: the top-level `absorption_rgb` is the color (fantasy, or a file written
    /// before physics color existed).
    Fantasy,
    /// A recipe whose fallback color still matches the top-level `absorption_rgb`: it was not
    /// touched since this build (or one like it) wrote it, so the recipe is the color.
    Physics(ColorMode),
    /// A recipe, but the top-level `absorption_rgb` differs from the fallback the DTO recorded:
    /// an older build edited the color. Treated as fantasy-edited; the caller asks the user
    /// whether to keep the physics recipe (the [`ColorMode`] is that recipe).
    EditedElsewhere(ColorMode),
}

/// Reads `snapshot`'s color payload: see [`SnapshotColor`]. An unreadable recipe JSON is
/// treated as [`SnapshotColor::Fantasy`] (the fallback color is still there).
#[must_use]
pub fn snapshot_color(snapshot: &CustomMaterialSnapshot) -> SnapshotColor {
    let Some(dto) = &snapshot.color_recipe else {
        return SnapshotColor::Fantasy;
    };
    let Some(mode) = ColorMode::from_json(&dto.recipe_json) else {
        return SnapshotColor::Fantasy;
    };
    // A fantasy-active material renders from the top-level color; its parked recipe only
    // matters to the vault row, not to a session-local restore.
    if !mode.is_physics() {
        return SnapshotColor::Fantasy;
    }
    // Compared as `f32`, the precision `with_body_color` stores them at.
    let recorded = dto.fallback_rgb.map(narrow);
    let top_level = snapshot.body_color().unwrap_or([0.0; 3]);
    #[expect(
        clippy::float_cmp,
        reason = "both sides are f32 values stored at the same precision; equality means the snapshot was not edited"
    )]
    let unchanged = top_level == recorded;
    if unchanged {
        SnapshotColor::Physics(mode)
    } else {
        SnapshotColor::EditedElsewhere(mode)
    }
}

/// The DTO recording `mode` for the native file; `fallback_rgb` is the exact value the caller
/// writes as the top-level `absorption_rgb` ([`ColorMode::fallback_rgb`]).
#[must_use]
pub fn color_recipe_dto(mode: &ColorMode) -> ColorRecipeDto {
    ColorRecipeDto {
        recipe_json: mode.to_json(),
        fallback_rgb: mode.fallback_rgb().map(|v| {
            v.to_string()
                .parse::<f64>()
                .unwrap_or_else(|_| f64::from(v))
        }),
    }
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
/// The body color is the snapshot's [`CustomMaterialSnapshot::absorption_rgb`]
/// triple; a snapshot written before that field existed (or for a colorless
/// material) carries none and restores colorless (`[0.0; 3]`). `crystal_system`/
/// `optical_character` ARE read from the snapshot's own string fields when the name is
/// recognised (the vault row's `LibraryMaterial::gem_material` does the same), and the
/// biaxial delta is restored for the two biaxial characters. An empty or unrecognised name
/// (a file from a build that wrote no names, or the web mirror) keeps what
/// [`GemMaterial::new_custom`] infers from the sign of `birefringence_delta`, with one
/// exception: a snapshot that carries a usable [`CustomMaterialSnapshot::biaxial_delta_beta_alpha`]
/// gets its biaxial character from [`biaxial_character_from_deltas`] (the optic-sign rule on the
/// delta and `|birefringence_delta|`), because the delta is only ever written for a biaxial
/// material. A name that parses still wins, a delta that rule rejects (not finite, not positive,
/// larger than the birefringence) changes nothing, and the crystal system stays whatever the
/// inference or the name gave.
///
/// One limitation: a file written before 2026-10-12 for a biaxial material carries its names
/// but no delta, so it restores with the biaxial character and
/// `biaxial_delta_beta_alpha = None`, which the renderer treats as uniaxial until the material
/// is saved again from a library that has the delta.
///
/// A physics recipe renders from its stored `resolved_bands`, never a re-resolve. A file
/// whose top-level color was edited by an older build ([`SnapshotColor::EditedElsewhere`])
/// restores the *edited* color here; use [`gem_material_from_custom_snapshot_keeping_recipe`]
/// when the user chose to keep the recipe.
///
/// A snapshot that carries a usable [`CustomMaterialSnapshot::dispersion_model`] rebuilds the
/// material from that curve (`GemMaterial::new_custom_with_dispersion`); without one, or with
/// one that fails [`indicatrix::optics::dispersion::DispersionModel::validate`], it is the
/// Cauchy fit `new_custom` builds from `mean_ri` and `dispersion_delta`, as ever.
#[must_use]
pub fn gem_material_from_custom_snapshot(
    name: &str,
    snapshot: &CustomMaterialSnapshot,
) -> GemMaterial {
    let birefringence = narrow(snapshot.birefringence_delta);
    let color = snapshot.body_color().unwrap_or([0.0, 0.0, 0.0]);
    let mut mat = snapshot
        .dispersion_model
        .as_ref()
        .and_then(super::dispersion_dto::dispersion_model_from_dto)
        .map_or_else(
            || {
                GemMaterial::new_custom(
                    name,
                    narrow(snapshot.mean_ri),
                    narrow(snapshot.dispersion_delta),
                    birefringence,
                    color,
                )
            },
            |model| GemMaterial::new_custom_with_dispersion(name, model, birefringence, color),
        );
    // The names the file carries win over `new_custom`'s two-variant inference, exactly as a
    // library row's do (`LibraryMaterial::gem_material`); an empty or unrecognised name keeps
    // the inference, except that a usable biaxial delta then picks the biaxial character by the
    // optic-sign rule (the delta is only ever written for a biaxial material). The biaxial
    // delta only means anything for the two biaxial characters.
    if let Some(system) = crystal_system_from_name(&snapshot.crystal_system) {
        mat.crystal_system = system;
    }
    let named = optical_character_from_name(&snapshot.optical_character);
    let from_delta = || {
        snapshot
            .biaxial_delta_beta_alpha
            .and_then(|delta| biaxial_character_from_deltas(delta, snapshot.birefringence_delta))
    };
    if let Some(character) = named.or_else(from_delta) {
        mat.optical_character = character;
    }
    mat.biaxial_delta_beta_alpha = if is_biaxial(mat.optical_character) {
        snapshot.biaxial_delta_beta_alpha.map(narrow)
    } else {
        None
    };
    // The N-band colour (path-aware L*C*h editor) wins over the three-band triple written
    // next to it; a live physics recipe still wins over both below.
    // The same constructor the vault row uses, so a banded custom renders identically from a
    // library row and from a design file's snapshot: per-millimetre bands, whose real scale the
    // render setup chooses from the stone's size.
    if let Some(rows) = snapshot.absorption_bands() {
        mat = crate::material::with_library_bands(mat, &rows);
    }
    if let SnapshotColor::Physics(mode) = snapshot_color(snapshot) {
        mat = mat.with_chromophore_absorption(mode.resolve_tensor());
    }
    mat
}

/// Like [`gem_material_from_custom_snapshot`], but a recipe wins even when an older build
/// edited the top-level color (the user's "keep physics recipe" choice).
#[must_use]
pub fn gem_material_from_custom_snapshot_keeping_recipe(
    name: &str,
    snapshot: &CustomMaterialSnapshot,
) -> GemMaterial {
    let mut mat = gem_material_from_custom_snapshot(name, snapshot);
    if let SnapshotColor::EditedElsewhere(mode) = snapshot_color(snapshot) {
        mat = mat.with_chromophore_absorption(mode.resolve_tensor());
    }
    mat
}

/// A concave tier as its file record: the tool and motion become the standard's
/// own strings, everything else is copied, so the conversion is lossless in both
/// directions (see [`concave_tier_from_table`]).
#[must_use]
pub(super) fn concave_tier_table_from_tier(tier: &ConcaveTier) -> ConcaveTierTable {
    ConcaveTierTable {
        name: tier.name.clone(),
        angle_deg: tier.angle_deg,
        indices: tier.indices.clone(),
        instructions: tier.instructions.clone(),
        tool: tier.tool.code().to_owned(),
        tool_azimuth_deg: tier.tool_azimuth_deg,
        displacement: tier.displacement,
        diameter_ratio: tier.diameter_ratio,
        tool_angle_deg: tier.tool_angle_deg,
        motion: tier.motion.word().to_owned(),
        // The id is the design's, not the tier's: `concave_tier_tables` attaches it.
        concave_tier_id: None,
        unknown: toml::Table::new(),
    }
}

/// The inverse of [`concave_tier_table_from_tier`]. The error is the reason a
/// human can act on: a tool code or motion this build does not know is refused
/// rather than replaced by a default, since a guessed tool cuts a different stone.
/// Range checks (angles, indices, diameter) are the caller's
/// [`Design::validate_concave_tiers`], which needs the design's gear.
pub(super) fn concave_tier_from_table(table: ConcaveTierTable) -> Result<ConcaveTier, String> {
    let tool = table
        .tool
        .parse::<ConcaveTool>()
        .map_err(|e| e.to_string())?;
    let motion = ToolMotion::from_word(&table.motion).ok_or_else(|| {
        format!(
            "unknown tool motion {:?} (expected reciprocating or plunge)",
            table.motion
        )
    })?;
    Ok(ConcaveTier {
        name: table.name,
        angle_deg: table.angle_deg,
        indices: table.indices,
        instructions: table.instructions,
        tool,
        tool_azimuth_deg: table.tool_azimuth_deg,
        displacement: table.displacement,
        diameter_ratio: table.diameter_ratio,
        tool_angle_deg: table.tool_angle_deg,
        motion,
    })
}

/// Every concave tier of `design` as file records, in stored order, each carrying its
/// stable id (the cutting-mode marks of a concave step hang on it, so it must survive a
/// reopen). A tier the design has no id for yet is written without one and gets a fresh
/// id on load.
#[must_use]
pub(super) fn concave_tier_tables(design: &Design) -> Vec<ConcaveTierTable> {
    design
        .concave_tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| ConcaveTierTable {
            concave_tier_id: design
                .concave_tier_ids
                .get(index)
                .copied()
                .map(TierId::value),
            ..concave_tier_table_from_tier(tier)
        })
        .collect()
}
