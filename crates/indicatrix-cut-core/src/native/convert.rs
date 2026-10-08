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
    design::{
        ConcaveTier, ConcaveTool, ConstraintTier, Design, ScheduleMeta, TierId, TierRelation,
        TierTarget, ToolMotion,
    },
    material::{ColorMode, MaterialSelection},
    preform::{PreformShape, PreformSpec},
};
use indicatrix::{
    geometry::{meet_solver::MeetConstraint, stone_metrics::ExternalProportions},
    optics::materials::GemMaterial,
};
use indicatrix_formats::native::{
    ColorRecipeDto, ConcaveTierTable, CustomMaterialSnapshot, HistoryTable, MaterialTable,
    NativeDesignFile, NativeMeetConstraint, NativePreformShape, NativeTierTarget, PreformTable,
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
            .map(|v| v as f32),
    }
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
    rgb.map(|v| v as f32)
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
///
/// Also carries `tier.indices` verbatim (the design's own AUTHORED, never
/// shifted, index-wheel positions) -- needed so [`super::load::load_paired`] can
/// restore them over whatever a cheater-offset-shifted `.asc` re-import would
/// otherwise produce (see [`Design::cheater_offsets_deg`]'s own doc comment):
/// without this, the shift `Design::to_asc_schedule_from_solved_with_cheater_offsets`
/// bakes into the exported `.asc` would be re-interpreted as the tier's own base
/// indices on the very next load, and doubled on the save after that.
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
    let recorded = dto.fallback_rgb.map(|v| v as f32);
    let top_level = snapshot.body_color().unwrap_or([0.0; 3]);
    if top_level == recorded {
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
/// `optical_character` are NOT read from the snapshot's own string fields: exactly
/// like [`GemMaterial::new_custom`] itself, both are re-derived from the sign of
/// `birefringence_delta`, which is what every other caller of `new_custom` in this
/// codebase already relies on.
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
    let birefringence = snapshot.birefringence_delta as f32;
    let color = snapshot.body_color().unwrap_or([0.0, 0.0, 0.0]);
    let mut mat = snapshot
        .dispersion_model
        .as_ref()
        .and_then(super::dispersion_dto::dispersion_model_from_dto)
        .map_or_else(
            || {
                GemMaterial::new_custom(
                    name,
                    snapshot.mean_ri as f32,
                    snapshot.dispersion_delta as f32,
                    birefringence,
                    color,
                )
            },
            |model| GemMaterial::new_custom_with_dispersion(name, model, birefringence, color),
        );
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

/// Builds a [`NativeDesignFile`] from `design`'s current state.
///
/// `asc_bytes` are the exact bytes written (or already written) as `asc_filename` --
/// see [`super::save::save_paired`] for the higher-level function that decides what
/// those bytes are (preserved original text vs. a fresh export) and calls this with
/// the result.
///
/// `printed_proportions`, when `Some`, is mirrored into the sidecar's own `[source]`
/// table so a later Open can restore it -- see [`NativeDesignFile::
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
                .with_angle_relation(design.tier_relation(index).map(TierRelation::to_canonical))
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
    let mut native = match printed_proportions {
        Some(props) => native.with_source(source_table_from_proportions(props)),
        None => native,
    };
    stash_concave_tiers(&mut native.unknown, design);
    native
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

/// The `unknown`-table key [`stash_concave_tiers`]/[`unstash_concave_tiers`] use,
/// namespaced like [`SELF_CONTAINED_META_KEY`].
pub(super) const CONCAVE_TIERS_STASH_KEY: &str = "indicatrix_cut_core_concave_tiers";

/// Packs `design`'s concave tiers into `unknown` so the paired sidecar and the
/// autosave (both [`NativeDesignFile`]s, which have no concave field of their own)
/// do not silently drop them: `.asc` cannot carry them, and an autosave restore
/// that lost them would be the one data loss a cutter cannot see. Writes nothing
/// for a planar design, so those sidecars stay byte-identical.
///
/// A record that cannot be serialised is never dropped (the list would silently shrink,
/// the one data loss a cutter cannot see): its slot keeps a text marker instead, which
/// [`unstash_concave_tiers`] refuses with the record's position.
pub(super) fn stash_concave_tiers(unknown: &mut toml::Table, design: &Design) {
    if design.concave_tiers.is_empty() {
        return;
    }
    unknown.insert(
        CONCAVE_FLAT_FINGERPRINT_KEY.to_string(),
        toml::Value::String(flat_schedule_fingerprint(design)),
    );
    let tables = concave_tier_tables(design)
        .into_iter()
        .enumerate()
        .map(|(index, table)| stash_entry(index, toml::Value::try_from(table)))
        .collect();
    unknown.insert(
        CONCAVE_TIERS_STASH_KEY.to_string(),
        toml::Value::Array(tables),
    );
}

/// The stash slot of concave record `index`: the serialised record, or (when
/// serialising failed with `error`) a text marker naming the record, so the position
/// stays occupied and [`unstash_concave_tiers`] reports it instead of skipping it.
pub(super) fn stash_entry<E: std::fmt::Display>(
    index: usize,
    serialised: Result<toml::Value, E>,
) -> toml::Value {
    serialised.unwrap_or_else(|error| {
        toml::Value::String(format!(
            "concave tier {} could not be written: {error}",
            index + 1
        ))
    })
}

/// The `unknown`-table key holding [`flat_schedule_fingerprint`], written next to
/// [`CONCAVE_TIERS_STASH_KEY`].
pub(super) const CONCAVE_FLAT_FINGERPRINT_KEY: &str =
    "indicatrix_cut_core_concave_flat_fingerprint";

/// A SHA-256 over the flat schedule the concave tiers were authored against: the
/// gear and every flat tier's angle and indices (3 decimals, far finer than any
/// cutting setting and immune to the `.asc` writer's own rounding). Names, meet
/// constraints and masts are left out: they are not the stone's facet layout.
pub(super) fn flat_schedule_fingerprint(design: &Design) -> String {
    use core::fmt::Write as _;
    let mut text = format!(
        "gear={};tiers={}",
        design.meta.gear_teeth,
        design.tiers.len()
    );
    for tier in &design.tiers {
        let _ = write!(text, ";{:.3}:", tier.angle_deg);
        for index in &tier.indices {
            let _ = write!(text, "{index:.3},");
        }
    }
    sha256_hex(text.as_bytes())
}

/// Checks the stashed concave tiers were authored against the flat schedule that
/// `design` now holds. A sidecar with no stash, or one written before the
/// fingerprint existed, passes; a recorded fingerprint that differs is an error
/// (an older build edited the flat tiers and re-saved the stash verbatim).
///
/// # Errors
///
/// A message saying the flat tiers changed since the concave tiers were saved.
pub(super) fn check_concave_flat_fingerprint(
    unknown: &toml::Table,
    design: &Design,
) -> Result<(), String> {
    if !unknown.contains_key(CONCAVE_TIERS_STASH_KEY) {
        return Ok(());
    }
    let Some(recorded) = unknown
        .get(CONCAVE_FLAT_FINGERPRINT_KEY)
        .and_then(toml::Value::as_str)
    else {
        return Ok(());
    };
    if recorded.eq_ignore_ascii_case(&flat_schedule_fingerprint(design)) {
        Ok(())
    } else {
        Err(
            "the flat tiers were changed after the concave tiers were saved (the file was \
             probably edited by an older version), so the concave tiers no longer fit this stone"
                .to_owned(),
        )
    }
}

/// The inverse of [`stash_concave_tiers`]: the stashed records, empty when `unknown`
/// carries none. A malformed entry is an error naming its position, never skipped.
pub(super) fn unstash_concave_tiers(
    unknown: &toml::Table,
) -> Result<Vec<ConcaveTierTable>, (usize, String)> {
    let Some(value) = unknown.get(CONCAVE_TIERS_STASH_KEY) else {
        return Ok(Vec::new());
    };
    let Some(entries) = value.as_array() else {
        return Err((0, "the concave tier list is not an array".to_owned()));
    };
    entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            entry
                .clone()
                .try_into::<ConcaveTierTable>()
                .map_err(|e| (index, e.to_string()))
        })
        .collect()
}

/// The `unknown`-table key [`stash_schedule_meta`]/[`unstash_schedule_meta`] use --
/// namespaced (not a bare `"meta"`) so it can never collide with a real top-level
/// field a future `indicatrix-formats` schema version adds.
pub(super) const SELF_CONTAINED_META_KEY: &str = "indicatrix_cut_core_self_contained_meta";

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
