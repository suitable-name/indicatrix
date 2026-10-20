//! Conversions between a [`Design`] and the self-contained `.isdd` design file
//! ([`indicatrix_formats::native::design::DesignFile`]; files saved as `.indicatrix`
//! before are read the same way).
//!
//! The per-field conversions live in [`super::convert`], so a design survives
//! `Design -> file -> Design` unchanged, including one that came from a catalogue
//! `.asc`. Every tier is written in full with its stable tier id; notes, cheater
//! offsets and targets travel inside the tier's own record, never matched to a tier by
//! array position.

use super::{
    convert::{
        SaveExtras, concave_tier_tables, external_proportions_from_source,
        material_selection_from_table, material_table_from_selection, preform_spec_from_table,
        preform_table_from_spec, source_table_from_proportions, tier_table_from_tier,
    },
    load::{
        MaterialResolution, apply_tier_frosting, apply_tier_ids_and_targets, apply_tier_relations,
        cheater_offsets_from_native, material_resolution_of, raw_tier_frosting_from_native,
        raw_tier_ids_from_native, raw_tier_relations_from_native, raw_tier_targets_from_native,
        restore_concave_tiers, tier_from_table_with_full_geometry, tier_notes_from_native,
    },
    unknown_keys::UnknownFileKeys,
};
use crate::design::{Design, ScheduleMeta, StoneRenderEffects, TierFrosting, TierId, TierRelation};
use indicatrix::geometry::stone_metrics::ExternalProportions;
use indicatrix_formats::native::{
    CustomMaterialSnapshot, HistoryTable, SourceTable,
    design::{
        self, AttachmentBlob, DesignFile, DesignFileError, DesignMetadata, RenderEffectsTable,
        ScheduleTable, attachment_blobs,
    },
};
use std::{fmt, sync::Arc};

/// The most history entries a design file keeps; the newest are retained.
pub const DESIGN_HISTORY_LIMIT: usize = 200;

/// Why a design file could not be turned into a [`Design`].
#[derive(Debug)]
pub enum DesignLoadError {
    /// The text is not a valid design file (see [`DesignFileError`]).
    File(DesignFileError),
    /// A tier has no `angle_deg` or `indices` (only reachable from a hand-built
    /// [`DesignFile`]; [`design::parse`] already refuses such a file).
    TierMissingGeometry {
        /// Zero-based position of the tier in the file.
        index: usize,
    },
    /// A concave tier names a tool or motion this build does not know, or holds a
    /// value that fails validation (an out-of-range angle or index, a name that
    /// clashes with a flat tier). Refused rather than repaired: the file is the
    /// only copy of what the cutter authored.
    ConcaveTier {
        /// Zero-based position among the concave tiers in the file.
        index: usize,
        /// What is wrong with it.
        reason: String,
    },
    /// A tier's `angle_relation` cannot be read or satisfied: the text is not
    /// canonical, it names a tier the file does not have, tiers read each other in a
    /// loop, or the result is not a facet angle. Refused rather than repaired, like a
    /// concave tier.
    Relation {
        /// Zero-based position of the tier in the file.
        index: usize,
        /// What is wrong with it.
        reason: String,
    },
}

impl fmt::Display for DesignLoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::File(e) => write!(f, "{e}"),
            Self::TierMissingGeometry { index } => write!(
                f,
                "tier {} has no recorded angle/indices to rebuild it from",
                index + 1
            ),
            Self::ConcaveTier { index, reason } => {
                write!(f, "concave tier {} is not usable: {reason}", index + 1)
            }
            Self::Relation { index, reason } => {
                write!(
                    f,
                    "the relation for tier {} is not usable: {reason}",
                    index + 1
                )
            }
        }
    }
}

impl std::error::Error for DesignLoadError {}

/// What a design file carries besides the [`Design`] itself and its printed
/// proportions: the custom-material snapshot, the history trail, the `[meta]`
/// descriptive table and the byte-exact attachments.
///
/// `Default` is "none of it". Convert from [`SaveExtras`] with
/// `DesignExtras::from(&save_extras)` (which carries the snapshot and history only).
#[derive(Debug, Clone, Default)]
pub struct DesignExtras<'a> {
    /// Snapshot of a custom material the design names; see
    /// [`SaveExtras::custom_material`].
    pub custom_material: Option<&'a CustomMaterialSnapshot>,
    /// Edit trail, oldest first; the newest [`DESIGN_HISTORY_LIMIT`] are written.
    pub history_entries: &'a [String],
    /// The `[meta]` table, with caller-supplied id and ISO-8601 UTC timestamps; `None`
    /// writes no table. See [`DesignMetadata`] for what belongs in it.
    pub metadata: Option<&'a DesignMetadata>,
    /// Files kept byte for byte, written in this order; limits (names, 64 MiB total)
    /// are checked by [`design::to_string`].
    pub attachments: &'a [AttachmentBlob],
}

impl<'a> From<&SaveExtras<'a>> for DesignExtras<'a> {
    fn from(extras: &SaveExtras<'a>) -> Self {
        Self {
            custom_material: extras.custom_material,
            history_entries: extras.history_entries,
            metadata: None,
            attachments: &[],
        }
    }
}

/// Everything [`design_from_file`] found.
#[derive(Debug)]
pub struct LoadedDesign {
    /// The loaded design.
    pub design: Design,
    /// Whether the material name is a built-in preset (see [`MaterialResolution`]).
    pub material_resolution: MaterialResolution,
    /// The printed proportions of the catalogue row the design came from, if any.
    pub printed_proportions: Option<ExternalProportions>,
    /// The custom-material snapshot, when the material is not a built-in preset and
    /// the file carried one -- see [`super::gem_material_from_custom_snapshot`].
    pub restorable_custom_material: Option<CustomMaterialSnapshot>,
    /// The history trail, oldest first.
    pub history_entries: Vec<String>,
    /// The file's `draft` flag: the design did not solve when it was saved.
    pub draft: bool,
    /// The `[meta]` table (all fields unset when the file has none).
    pub metadata: DesignMetadata,
    /// The attachments with their bytes decoded and verified, in file order.
    pub attachments: Vec<AttachmentBlob>,
}

fn schedule_table_from_meta(meta: &ScheduleMeta) -> ScheduleTable {
    ScheduleTable {
        gemcad_version: meta.gemcad_version.clone(),
        gear_teeth: meta.gear_teeth,
        gear_reference_angle: meta.gear_reference_angle,
        symmetry_order: meta.symmetry_order,
        mirror: meta.mirror,
        refractive_index: meta.refractive_index,
        headers: meta.headers.clone(),
        footnotes: meta.footnotes.clone(),
        unknown: toml::Table::new(),
    }
}

fn schedule_meta_from_table(table: &ScheduleTable) -> ScheduleMeta {
    ScheduleMeta {
        gemcad_version: table.gemcad_version.clone(),
        gear_teeth: table.gear_teeth,
        gear_reference_angle: table.gear_reference_angle,
        symmetry_order: table.symmetry_order,
        mirror: table.mirror,
        refractive_index: table.refractive_index,
        headers: table.headers.clone(),
        footnotes: table.footnotes.clone(),
    }
}

fn render_effects_table(effects: &StoneRenderEffects) -> RenderEffectsTable {
    RenderEffectsTable {
        inclusion_sigma_s: effects.inclusion_sigma_s,
        c_axis_override_enabled: effects.c_axis_override_enabled,
        c_axis_tilt_deg: effects.c_axis_tilt_deg,
        c_axis_azimuth_deg: effects.c_axis_azimuth_deg,
        girdle_frosted: effects.girdle_frosted,
        edge_rounding_radius: effects.edge_rounding_radius,
        unknown: toml::Table::new(),
    }
}

fn render_effects_from_table(table: &RenderEffectsTable) -> StoneRenderEffects {
    StoneRenderEffects {
        inclusion_sigma_s: table.inclusion_sigma_s,
        c_axis_override_enabled: table.c_axis_override_enabled,
        c_axis_tilt_deg: table.c_axis_tilt_deg,
        c_axis_azimuth_deg: table.c_axis_azimuth_deg,
        girdle_frosted: table.girdle_frosted,
        edge_rounding_radius: table.edge_rounding_radius,
    }
    .sanitised()
}

/// The newest [`DESIGN_HISTORY_LIMIT`] entries of `entries`.
fn bounded_history(entries: &[String]) -> Vec<String> {
    let start = entries.len().saturating_sub(DESIGN_HISTORY_LIMIT);
    entries.get(start..).unwrap_or_default().to_vec()
}

/// A tier's frosting as the two columns of its file record: the whole-tier flag and the
/// single frosted facets' positions (a polished tier is `(false, [])`, which is not written).
fn frosting_columns(frosting: Option<&TierFrosting>) -> (bool, Vec<f64>) {
    match frosting {
        Some(TierFrosting::Tier) => (true, Vec::new()),
        Some(TierFrosting::Facets(positions)) => (false, positions.clone()),
        None => (false, Vec::new()),
    }
}

fn build_file(
    design: &Design,
    source: Option<SourceTable>,
    custom: Option<&CustomMaterialSnapshot>,
    history: &[String],
) -> DesignFile {
    // A design whose id list is not in step with its tiers (built by pushing onto
    // `tiers` directly) gets one synthetic id per position, consistently.
    let ids_in_step = design.tier_ids.len() == design.tiers.len();
    let tiers = design
        .tiers
        .iter()
        .enumerate()
        .map(|(index, tier)| {
            let id = ids_in_step
                .then(|| design.tier_id_at(index))
                .flatten()
                .unwrap_or(TierId(index as u64));
            let (frosted, frosted_facets) =
                frosting_columns(ids_in_step.then(|| design.tier_frosting(index)).flatten());
            tier_table_from_tier(
                tier,
                design.tier_notes.get(&index).cloned(),
                design.cheater_offsets_deg.get(&index).copied(),
                Some(id),
                design.tier_target(index),
            )
            .with_angle_deg(Some(tier.angle_deg))
            .with_angle_relation(
                ids_in_step
                    .then(|| design.tier_relation(index))
                    .flatten()
                    .map(TierRelation::to_canonical),
            )
            .with_frosting(frosted, frosted_facets)
        })
        .collect();
    // `DesignFile::new` picks version 4 for a plane-set preform and version 3 when a
    // tier has a relation; the concave list below only ever raises the version to 2
    // for a file with neither.
    let file = DesignFile::new(
        preform_table_from_spec(&design.preform, design.preform_y_offset),
        material_table_from_selection(&design.material).with_custom(custom.cloned()),
        schedule_table_from_meta(&design.meta),
        design.girdle_diameter_mm,
        tiers,
    )
    .with_history(HistoryTable::new(bounded_history(history)))
    // Written only when an effect is on, so a design without effects keeps its bytes.
    .with_render_effects(render_effects_table(&design.render_effects))
    // Also sets `version = 2` and the frame, and only when the list is non-empty.
    .with_concave_tiers(concave_tier_tables(design));
    let file = with_unknown_keys(file, design);
    match source {
        Some(source) => file.with_source(source),
        None => file,
    }
}

/// Writes back the keys the file this design was opened from carried and this build
/// does not claim (see [`UnknownFileKeys`]). A design that was not opened from a
/// design file has none, and `file` is returned unchanged.
fn with_unknown_keys(mut file: DesignFile, design: &Design) -> DesignFile {
    let Some(extras) = design.file_extras.as_deref() else {
        return file;
    };
    file.unknown.clone_from(&extras.top);
    file.preform.unknown.clone_from(&extras.preform);
    file.material.unknown.clone_from(&extras.material);
    file.schedule.unknown.clone_from(&extras.schedule);
    if !extras.history.is_empty() {
        file.history
            .get_or_insert_with(|| HistoryTable::new(Vec::new()))
            .unknown
            .clone_from(&extras.history);
    }
    for table in &mut file.tiers {
        if let Some(keys) = table.tier_id.and_then(|id| extras.tiers.get(&TierId(id))) {
            table.unknown.clone_from(keys);
        }
    }
    for (table, id) in file.concave_tiers.iter_mut().zip(&design.concave_tier_ids) {
        if let Some(keys) = extras.concave_tiers.get(id) {
            table.unknown.clone_from(keys);
        }
    }
    file
}

/// Builds the self-contained design file for `design`.
///
/// `printed_proportions` becomes the `[source]` table (nothing is written when it is
/// all-`None`). `extras` supplies the custom-material snapshot, the history trail
/// (bounded to [`DESIGN_HISTORY_LIMIT`], newest kept), the `[meta]` table and the
/// attachments. The `draft` flag is `false`; a caller that knows the design did not
/// solve sets [`DesignFile::draft`] afterwards (or uses [`DesignFile::with_draft`]).
///
/// Infallible: `[meta]` and attachment limits are checked by [`design::to_string`]
/// (and so by [`design_to_string`]), which is where an oversize or duplicate-named
/// attachment surfaces as a typed error.
#[must_use]
pub fn design_to_file(
    design: &Design,
    printed_proportions: Option<&ExternalProportions>,
    extras: &DesignExtras<'_>,
) -> DesignFile {
    build_file(
        design,
        printed_proportions.map(source_table_from_proportions),
        extras.custom_material,
        extras.history_entries,
    )
    .with_meta(extras.metadata.cloned().unwrap_or_default())
    .with_attachments(extras.attachments)
}

/// [`design_to_file`] plus [`design::to_string`]: the text to write to a `.isdd`
/// file.
///
/// # Errors
///
/// [`DesignFileError::InvalidField`] for a `[meta]` value the reader would refuse,
/// [`DesignFileError::Attachments`] for a bad, duplicate or oversize attachment, and
/// [`DesignFileError::Serialize`] (in practice unreachable).
pub fn design_to_string(
    design: &Design,
    printed_proportions: Option<&ExternalProportions>,
    extras: &DesignExtras<'_>,
) -> Result<String, DesignFileError> {
    design::to_string(&design_to_file(design, printed_proportions, extras))
}

/// Rebuilds a [`Design`] from a design file.
///
/// Every tier's id, note, cheater offset and target are read from the tier's own
/// record. A tier with no id gets a fresh one; ids the file names are kept. The
/// attachments are decoded and their size and SHA-256 re-checked. Tier relations are
/// restored and applied (a stored angle that drifted from its relation is corrected),
/// and keys this build does not claim are kept on [`Design::file_extras`] so the next
/// save writes them back.
///
/// # Errors
///
/// [`DesignLoadError::TierMissingGeometry`] for a tier without `angle_deg`/`indices`;
/// [`DesignLoadError::File`] wrapping [`DesignFileError::Attachments`] for attachments
/// that do not verify; [`DesignLoadError::Relation`] for a relation that cannot be
/// read or satisfied.
pub fn design_from_file(file: DesignFile) -> Result<LoadedDesign, DesignLoadError> {
    let DesignFile {
        girdle_diameter_mm,
        draft,
        preform,
        material,
        schedule,
        source,
        history,
        render_effects,
        tiers,
        concave_tiers,
        meta,
        attachments,
        unknown,
        ..
    } = file;
    let attachments = attachment_blobs(&attachments)
        .map_err(|e| DesignLoadError::File(DesignFileError::Attachments(e)))?;
    let notes = tier_notes_from_native(&tiers);
    let offsets = cheater_offsets_from_native(&tiers);
    let raw_ids = raw_tier_ids_from_native(&tiers);
    let raw_targets = raw_tier_targets_from_native(&tiers);
    let raw_relations = raw_tier_relations_from_native(&tiers);
    let raw_frosting = raw_tier_frosting_from_native(&tiers);
    // The keys this build does not claim, set aside before the tables are consumed;
    // the per-tier ones are matched to their tier's id once the ids are assigned.
    let tier_unknown: Vec<toml::Table> = tiers.iter().map(|t| t.unknown.clone()).collect();
    let concave_unknown: Vec<toml::Table> =
        concave_tiers.iter().map(|t| t.unknown.clone()).collect();
    let history_unknown = history
        .as_ref()
        .map(|h| h.unknown.clone())
        .unwrap_or_default();
    let constraint_tiers = tiers
        .into_iter()
        .enumerate()
        .map(|(index, saved)| {
            tier_from_table_with_full_geometry(saved)
                .ok_or(DesignLoadError::TierMissingGeometry { index })
        })
        .collect::<Result<Vec<_>, _>>()?;

    let mut design = Design::new(
        preform_spec_from_table(&preform),
        schedule_meta_from_table(&schedule),
        constraint_tiers,
    );
    design.girdle_diameter_mm = girdle_diameter_mm;
    design.render_effects = render_effects
        .as_ref()
        .map_or_else(StoneRenderEffects::default, render_effects_from_table);
    design.preform_y_offset = preform.y_offset;
    design.material = material_selection_from_table(&material);
    design.tier_notes = notes;
    design.cheater_offsets_deg = offsets;
    apply_tier_ids_and_targets(&mut design, raw_ids, raw_targets);
    apply_tier_frosting(&mut design, &raw_frosting);
    apply_tier_relations(&mut design, &raw_relations)
        .map_err(|(index, reason)| DesignLoadError::Relation { index, reason })?;
    restore_concave_tiers(&mut design, concave_tiers)
        .map_err(|(index, reason)| DesignLoadError::ConcaveTier { index, reason })?;
    let extras = UnknownFileKeys {
        top: unknown,
        preform: preform.unknown,
        material: material.unknown.clone(),
        schedule: schedule.unknown,
        history: history_unknown,
        tiers: design
            .tier_ids
            .iter()
            .copied()
            .zip(tier_unknown)
            .filter(|(_, keys)| !keys.is_empty())
            .collect(),
        concave_tiers: design
            .concave_tier_ids
            .iter()
            .copied()
            .zip(concave_unknown)
            .filter(|(_, keys)| !keys.is_empty())
            .collect(),
    };
    if !extras.is_empty() {
        design.file_extras = Some(Arc::new(extras));
    }

    let material_resolution = material_resolution_of(design.material.name.as_deref());
    let restorable_custom_material = matches!(material_resolution, MaterialResolution::Unresolved)
        .then(|| material.custom.clone())
        .flatten();
    Ok(LoadedDesign {
        design,
        material_resolution,
        printed_proportions: source.as_ref().map(external_proportions_from_source),
        restorable_custom_material,
        history_entries: history.map_or_else(Vec::new, |h| h.entries),
        draft,
        metadata: meta,
        attachments,
    })
}

/// [`design::parse`] plus [`design_from_file`]: the one call that opens a
/// `.isdd` (or legacy `.indicatrix`) file's text.
///
/// # Errors
///
/// [`DesignLoadError::File`] for text that is not a valid design file (including a
/// newer major version), or the error [`design_from_file`] returns.
pub fn design_from_str(text: &str) -> Result<LoadedDesign, DesignLoadError> {
    design_from_file(design::parse(text).map_err(DesignLoadError::File)?)
}
