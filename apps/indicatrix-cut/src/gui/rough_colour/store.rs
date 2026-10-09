//! The storage API of rough colour (`zoning` feature only).
//!
//! Every function the Rough colour UI calls to save, load and delete a plan's colour, photos, pose choices, a material's zones and a
//! render job's zones.
//!
//! All functions take the vault [`Database`] (callers hold the `Mutex` guard) and return an
//! `anyhow::Result`; "nothing stored" is `Ok(None)` / an empty collection, never an error, and a
//! stored text that no longer parses is an `Err` with a sentence the UI can show. Reads work on a
//! read-only database (and on one no zoning build has opened yet, which simply has nothing).
//!
//! The vault tables are opaque to the vault; the formats are those of
//! `indicatrix_cut_core::rough_plan::zoned_plan` (JSON for zones and fit, packed bytes for
//! photos). See the parent module for the flows these functions serve.
//!
//! # Function index
//!
//! | area | functions |
//! |---|---|
//! | rough colour | [`save_rough_colour`], [`load_rough_colour`], [`delete_rough_colour`], [`plans_with_rough_colour`] |
//! | photos | [`save_view_photos`], [`load_view_photos`], [`cached_views`], [`delete_view_photos`] |
//! | pose choice | [`choose_and_save_poses`], [`save_layout_pose_choices`], [`load_pose_choices`], [`posed_stone_pose`], [`clear_pose_choices`] |
//! | planner preview | [`planner_stone_material`] |
//! | material zones | [`save_material_zoning`], [`load_material_zoning`], [`delete_material_zoning`], [`attach_zoning`], [`with_stored_zoning`] |
//! | adopt | [`adopt_stone`] |
//! | render jobs | [`save_job_zoning`], [`attach_job_zoning`], [`delete_job_zoning`] |
//! | housekeeping | [`prune_orphans`] |

use crate::gui::optics::crystal_optics::gem_material_from_row;
use anyhow::{Context, Result, bail};
use indicatrix::{
    optics::{materials::GemMaterial, zoning::ZonedAbsorption},
    render_setup::PHYSICS_DEFAULT_STONE_WIDTH_MM,
};
use indicatrix_cut_core::{
    material::{
        absorption_bands_to_json, built_in_specific_gravity, crystal_system_name,
        optical_character_name,
    },
    native::dispersion_model_to_json,
    rough_plan::{
        PlacedStone, RoughLayout,
        fit::StonePose,
        photometry::ResampledImage,
        zoned_plan::{
            AdoptedColour, POSE_COUNT, PhotoBlob, PhotoEncoding, PoseGoal, RoughColour,
            StonePlacement, ZONING_FORMAT_VERSION, adopt_colour, adopted_material,
            blobs_from_resampled, check_version, choose_poses, decode_zoned, encode_zoned,
            make_relative, pose_variant, resampled_from_blobs, resolve_relative,
            stone_preview_material,
        },
    },
};
use indicatrix_net::SceneState;
use indicatrix_vault::{
    db::sqlite::{CustomMaterialParams, Database},
    model::{
        material::CustomMaterialRow,
        zoning::{MaterialZoningRow, RoughColourPhotoRow, RoughColourRow},
    },
};
use std::collections::{BTreeMap, BTreeSet};
use tracing::warn;

// ---- rough colour ----------------------------------------------------------------------------

/// Stores (replaces) the rough colour of saved plan `plan_id`: the zones in the rough frame and
/// the fit report.
///
/// # Errors
///
/// The zones fail validation, or the vault write fails.
pub fn save_rough_colour(db: &Database, plan_id: i64, colour: &RoughColour) -> Result<()> {
    // Re-encoded here (not taken from a caller's text) so what is stored is always validated.
    let zoned_json = encode_zoned(&colour.zoned)?;
    db.save_rough_colour(&RoughColourRow {
        plan_id,
        zoned_json,
        fit_json: colour.fit_json.clone(),
        version: ZONING_FORMAT_VERSION,
        created: colour.created,
    })
}

/// The rough colour of plan `plan_id`, or `None` when it has none.
///
/// # Errors
///
/// The vault read fails, the row was written by a newer build, or its zones do not parse or do
/// not validate.
pub fn load_rough_colour(db: &Database, plan_id: i64) -> Result<Option<RoughColour>> {
    let Some(row) = db.load_rough_colour(plan_id)? else {
        return Ok(None);
    };
    check_version(row.version)?;
    let zoned = decode_zoned(&row.zoned_json)
        .with_context(|| format!("The colour of saved plan {plan_id} cannot be used"))?;
    Ok(Some(RoughColour {
        zoned,
        fit_json: row.fit_json,
        version: row.version,
        created: row.created,
    }))
}

/// Removes everything stored for plan `plan_id`: colour, photos and pose choices.
///
/// # Errors
///
/// The vault write fails.
pub fn delete_rough_colour(db: &Database, plan_id: i64) -> Result<()> {
    db.delete_rough_colour(plan_id)
}

/// The ids of the plans that have a rough colour.
///
/// # Errors
///
/// The vault read fails.
pub fn plans_with_rough_colour(db: &Database) -> Result<BTreeSet<i64>> {
    db.rough_colour_plan_ids()
}

// ---- cached photos ---------------------------------------------------------------------------

/// Caches the working-resolution image of rig view `view` of plan `plan_id` (replacing an earlier
/// one), so the plan reopens without the original photos.
///
/// # Errors
///
/// The image is too large or inconsistent, or a vault write fails.
pub fn save_view_photos(
    db: &Database,
    plan_id: i64,
    view: u32,
    image: &ResampledImage,
) -> Result<()> {
    for blob in blobs_from_resampled(image)? {
        db.save_rough_colour_photo(&RoughColourPhotoRow {
            plan_id,
            view,
            kind: blob.kind,
            width: blob.width,
            height: blob.height,
            encoding: blob.encoding.as_str().to_string(),
            data: blob.data,
        })?;
    }
    Ok(())
}

/// The cached working-resolution image of view `view`, or `None` when it is not cached.
///
/// # Errors
///
/// The vault read fails or the cached blobs are damaged (the message says which).
pub fn load_view_photos(db: &Database, plan_id: i64, view: u32) -> Result<Option<ResampledImage>> {
    let metas: Vec<_> = db
        .list_rough_colour_photos(plan_id)?
        .into_iter()
        .filter(|meta| meta.view == view)
        .collect();
    if metas.is_empty() {
        return Ok(None);
    }
    let mut blobs = Vec::with_capacity(metas.len());
    for meta in metas {
        let Some(row) = db.load_rough_colour_photo(plan_id, view, &meta.kind)? else {
            continue;
        };
        let Some(encoding) = PhotoEncoding::parse(&row.encoding) else {
            bail!(
                "The cached {} image of view {view} has an unknown encoding '{}'",
                row.kind,
                row.encoding
            );
        };
        blobs.push(PhotoBlob {
            kind: row.kind,
            width: row.width,
            height: row.height,
            encoding,
            data: row.data,
        });
    }
    let image = resampled_from_blobs(&blobs)
        .with_context(|| format!("The cached photos of view {view} cannot be used"))?;
    Ok(Some(image))
}

/// The rig views of plan `plan_id` that have cached photos, ascending.
///
/// # Errors
///
/// The vault read fails.
pub fn cached_views(db: &Database, plan_id: i64) -> Result<Vec<u32>> {
    let views: BTreeSet<u32> = db
        .list_rough_colour_photos(plan_id)?
        .into_iter()
        .map(|meta| meta.view)
        .collect();
    Ok(views.into_iter().collect())
}

/// Removes every cached photo of plan `plan_id` (the colour itself stays). Returns how many
/// blobs were removed.
///
/// # Errors
///
/// The vault write fails.
pub fn delete_view_photos(db: &Database, plan_id: i64) -> Result<usize> {
    db.delete_rough_colour_photos(plan_id)
}

// ---- pose choice -----------------------------------------------------------------------------

/// The stored pose choices of one plan: `(layout_index, stone_index)` to pose index. Only
/// non-canonical choices are present; a missing key means pose `0`.
pub type PoseChoices = BTreeMap<(u32, u32), u8>;

/// The stored pose choices of plan `plan_id`.
///
/// # Errors
///
/// The vault read fails.
pub fn load_pose_choices(db: &Database, plan_id: i64) -> Result<PoseChoices> {
    Ok(db
        .stone_pose_choices(plan_id)?
        .into_iter()
        .map(|row| ((row.layout_index, row.stone_index), row.pose))
        .collect())
}

/// Stores the pose of every stone of layout `layout_index`: `poses[i]` is stone `i`'s pose index
/// (`0` removes the stored choice).
///
/// # Errors
///
/// A pose index of `POSE_COUNT` or more, or a vault write fails.
pub fn save_layout_pose_choices(
    db: &Database,
    plan_id: i64,
    layout_index: u32,
    poses: &[u8],
) -> Result<()> {
    if let Some(bad) = poses.iter().find(|&&p| p >= POSE_COUNT) {
        bail!("Pose {bad} does not exist; a stone has {POSE_COUNT} poses");
    }
    for (stone_index, &pose) in poses.iter().enumerate() {
        let stone_index = u32::try_from(stone_index).context("Too many stones in a layout")?;
        db.set_stone_pose_choice(plan_id, layout_index, stone_index, pose)?;
    }
    Ok(())
}

/// Scores the box-symmetric poses of every stone of `layout` against the plan's zoned rough with
/// the cheap face-up predictor, picks by `goal` and stores the result.
///
/// Returns the chosen pose
/// index per stone. [`PoseGoal::KeepCanonical`] (the default) stores nothing and clears earlier
/// choices of this layout.
///
/// # Errors
///
/// A vault write fails.
pub fn choose_and_save_poses(
    db: &Database,
    plan_id: i64,
    layout_index: u32,
    layout: &RoughLayout,
    rough_zoned: &ZonedAbsorption,
    goal: PoseGoal,
) -> Result<Vec<u8>> {
    let poses = choose_poses(rough_zoned, layout, goal);
    save_layout_pose_choices(db, plan_id, layout_index, &poses)?;
    Ok(poses)
}

/// The pose to use for stone `stone_index` of layout `layout_index` given the stored `choices`:
/// the stone's own pose turned to the stored index, or unchanged when none is stored.
#[must_use]
pub fn posed_stone_pose(
    choices: &PoseChoices,
    layout_index: u32,
    stone_index: u32,
    stone: &PlacedStone,
) -> StonePose {
    choices
        .get(&(layout_index, stone_index))
        .and_then(|&index| pose_variant(&stone.pose, index))
        .unwrap_or(stone.pose)
}

/// Forgets every pose choice of plan `plan_id` (all stones back to the planner's pose).
///
/// # Errors
///
/// The vault write fails.
pub fn clear_pose_choices(db: &Database, plan_id: i64) -> Result<usize> {
    db.clear_stone_pose_choices(plan_id)
}

// ---- planner preview -------------------------------------------------------------------------

/// The material to preview a planned stone of plan `plan_id` with.
///
/// This is `host` with the plan's rough zones moved into the stone's frame. `None` when the plan
/// has no rough colour, or the placement is unusable.
///
/// # Errors
///
/// The stored colour cannot be read (see [`load_rough_colour`]).
pub fn planner_stone_material(
    db: &Database,
    plan_id: i64,
    host: &GemMaterial,
    placement: &StonePlacement,
) -> Result<Option<GemMaterial>> {
    let Some(colour) = load_rough_colour(db, plan_id)? else {
        return Ok(None);
    };
    Ok(stone_preview_material(host, &colour.zoned, placement))
}

// ---- material zones --------------------------------------------------------------------------

/// The zones stored for a custom material.
#[derive(Debug, Clone, PartialEq)]
pub struct StoredZoning {
    /// The zones. In mm for an adopted material; for a "relative to stone" library material the
    /// geometry of a stone one unit wide (scale it with
    /// `indicatrix_cut_core::rough_plan::zoned_plan::resolve_relative`).
    pub zoning: ZonedAbsorption,
    /// Whether the geometry scales with the stone's width.
    pub relative_to_stone: bool,
}

/// Stores the zones of custom material `material_name`.
///
/// `zoning` is the geometry to store: in mm when `relative_to_stone` is `false`; for `true`, the
/// geometry of a stone one unit wide (use [`save_relative_material_zoning`] to convert from mm).
///
/// # Errors
///
/// The zones fail validation, or the vault write fails.
pub fn save_material_zoning(
    db: &Database,
    material_name: &str,
    zoning: &ZonedAbsorption,
    relative_to_stone: bool,
) -> Result<()> {
    db.save_material_zoning(&MaterialZoningRow {
        material_name: material_name.to_string(),
        zoned_json: encode_zoned(zoning)?,
        relative_to_stone,
        version: ZONING_FORMAT_VERSION,
    })
}

/// Stores a library zoned material whose zones scale with the stone's width (owner decision 4):
/// `zoning_mm` is the geometry for a stone `stone_width_mm` wide, stored as that of a stone one
/// unit wide.
///
/// # Errors
///
/// The width is not positive, the zones fail validation, or the vault write fails.
pub fn save_relative_material_zoning(
    db: &Database,
    material_name: &str,
    zoning_mm: &ZonedAbsorption,
    stone_width_mm: f64,
) -> Result<()> {
    let Some(unit) = make_relative(zoning_mm, stone_width_mm) else {
        bail!("The zones cannot be stored relative to a stone width of {stone_width_mm} mm");
    };
    save_material_zoning(db, material_name, &unit, true)
}

/// The zones stored for custom material `material_name` (compared ASCII case-insensitively), or
/// `None`.
///
/// # Errors
///
/// The vault read fails, the row was written by a newer build, or its zones are unusable.
pub fn load_material_zoning(db: &Database, material_name: &str) -> Result<Option<StoredZoning>> {
    let Some(row) = db.load_material_zoning(material_name)? else {
        return Ok(None);
    };
    check_version(row.version)?;
    let zoning = decode_zoned(&row.zoned_json)
        .with_context(|| format!("The zones of '{material_name}' cannot be used"))?;
    Ok(Some(StoredZoning {
        zoning,
        relative_to_stone: row.relative_to_stone,
    }))
}

/// Removes the zones of `material_name`; the material itself stays. Returns how many rows were
/// removed (the material editor's delete calls this next to the material delete).
///
/// # Errors
///
/// The vault write fails.
pub fn delete_material_zoning(db: &Database, material_name: &str) -> Result<usize> {
    db.delete_material_zoning(material_name)
}

/// `material` with its stored zones installed (`GemMaterial::with_zoning`: the material becomes
/// `PerMm` and its plain absorption the base zone's), or `material` unchanged when none are
/// stored.
///
/// A "relative to stone" row is scaled to `stone_width_mm` first
/// (`0.0` or less means the default stone width).
///
/// A row that cannot be read is logged and the material is returned unzoned, so one damaged row
/// never stops the material list from loading.
#[must_use]
pub fn with_stored_zoning(
    db: &Database,
    material: GemMaterial,
    stone_width_mm: f64,
) -> GemMaterial {
    match load_material_zoning(db, &material.name) {
        Ok(Some(stored)) => {
            let width = if stone_width_mm > 0.0 {
                stone_width_mm
            } else {
                f64::from(PHYSICS_DEFAULT_STONE_WIDTH_MM)
            };
            let zoning = if stored.relative_to_stone {
                resolve_relative(&stored.zoning, width)
            } else {
                Some(stored.zoning)
            };
            if let Some(zoning) = zoning {
                material.with_zoning(zoning)
            } else {
                warn!(
                    "Zones of '{}' could not be scaled; using its base colour",
                    material.name
                );
                material
            }
        }
        Ok(None) => material,
        Err(error) => {
            warn!("Zones of '{}' not used: {error:#}", material.name);
            material
        }
    }
}

/// [`with_stored_zoning`] for a whole list of custom materials, in place. Returns how many got
/// zones.
///
/// Called wherever the application builds its custom-material list (startup, the
/// material editor's save), so every later `resolve_material` returns a zoned material.
///
/// One query per material is avoided for the common case of no zoned materials: when the vault
/// holds no `material_zoning` rows at all the list is left alone.
pub fn attach_zoning(db: &Database, materials: &mut [GemMaterial], stone_width_mm: f64) -> usize {
    let any = db.all_material_zonings().map(|rows| !rows.is_empty());
    match any {
        Ok(false) => return 0,
        Ok(true) => {}
        Err(error) => {
            warn!("Material zones could not be listed: {error:#}");
            return 0;
        }
    }
    let mut attached = 0;
    for material in &mut *materials {
        let before = material.zoning.is_some();
        let owned = std::mem::replace(material, GemMaterial::diamond());
        *material = with_stored_zoning(db, owned, stone_width_mm);
        if !before && material.zoning.is_some() {
            attached += 1;
        }
    }
    attached
}

// ---- adopt -----------------------------------------------------------------------------------

/// The material the rough was planned with, as the adopted material copies its optics from.
#[derive(Debug, Clone, Copy)]
pub enum HostMaterial<'a> {
    /// A catalogue (custom) material: its row is copied.
    Custom(&'a CustomMaterialRow),
    /// A built-in preset: its optics are read off the material.
    BuiltIn(&'a GemMaterial),
}

/// Everything [`adopt_stone`] needs.
#[derive(Debug, Clone, Copy)]
pub struct AdoptRequest<'a> {
    /// The rough's name (the plan's name), the stem of the material name.
    pub rough_name: &'a str,
    /// The rough colour's zones in the rough frame.
    pub rough_zoned: &'a ZonedAbsorption,
    /// Where the stone sits (use [`posed_stone_pose`] for the cutter's pose choice).
    pub placement: StonePlacement,
    /// The stone's girdle width at its planned size, in mm
    /// (`DesignHull::width * pose.mm_per_unit`).
    pub stone_width_mm: f64,
    /// The rough's host material.
    pub host: HostMaterial<'a>,
}

/// What [`adopt_stone`] produced.
#[derive(Debug, Clone, PartialEq)]
pub struct AdoptOutcome {
    /// The custom material's name, `"<rough name> colour"`.
    pub material_name: String,
    /// The width to render the stone at: set the editor's stone width to this so the colour
    /// scales physically (no slider is involved otherwise).
    pub stone_width_mm: f64,
    /// The render-ready material: the host's optics, the zones in the stone frame, the real
    /// absorption scale.
    pub material: GemMaterial,
    /// Whether a custom material of that name already existed and was replaced.
    pub replaced: bool,
}

/// The custom-material row of a host material with the base zone's bands.
fn adopted_row(name: &str, host: HostMaterial<'_>, adopted: &AdoptedColour) -> CustomMaterialRow {
    let bands_json = absorption_bands_to_json(&adopted.base_bands);
    match host {
        HostMaterial::Custom(row) => CustomMaterialRow {
            name: name.to_string(),
            absorption_rgb: [0.0; 3],
            // The host's colour recipe would take precedence over the bands: dropped.
            color_recipe_json: None,
            absorption_bands_json: bands_json,
            ..row.clone()
        },
        HostMaterial::BuiltIn(material) => {
            let n_d = material.dispersion.n_d();
            let n_f = material.dispersion.evaluate(486.13);
            let n_c = material.dispersion.evaluate(656.27);
            CustomMaterialRow {
                name: name.to_string(),
                refractive_index: n_d,
                dispersion: n_f - n_c,
                birefringence: material.birefringence_delta,
                absorption_rgb: [0.0; 3],
                crystal_system: Some(crystal_system_name(material.crystal_system).to_string()),
                optical_character: Some(
                    optical_character_name(material.optical_character).to_string(),
                ),
                biaxial_delta_beta_alpha: material.biaxial_delta_beta_alpha,
                per_axis_dispersion_json: None,
                specific_gravity: built_in_specific_gravity(&material.name)
                    .map(|sg| sg.representative as f32),
                color_recipe_json: None,
                dispersion_model_json: Some(dispersion_model_to_json(&material.dispersion)),
                absorption_bands_json: bands_json,
            }
        }
    }
}

/// Adopts one planned stone as a design's material.
///
/// Creates the custom material `"<rough name> colour"` (its row holds the BASE zone, so a default
/// build renders it) and its `material_zoning` row (the zones in the stone frame, mm, not
/// relative to stone), replacing both when the name exists. Returns the render-ready material and
/// the real stone width.
///
/// # Errors
///
/// The placement or width is unusable, the name belongs to a built-in material, or a vault write
/// fails. The material row is written first; if the zones then fail to store, the material stays
/// with its base colour and the error is returned.
pub fn adopt_stone(db: &Database, request: &AdoptRequest<'_>) -> Result<AdoptOutcome> {
    let Some(adopted) = adopt_colour(
        request.rough_name,
        request.rough_zoned,
        &request.placement,
        request.stone_width_mm,
    ) else {
        bail!("This stone cannot be adopted: its pose, size or colour zones are not usable");
    };
    let name = adopted.material_name.clone();
    if GemMaterial::all_materials()
        .iter()
        .any(|built_in| built_in.name.eq_ignore_ascii_case(&name))
    {
        bail!("'{name}' is the name of a built-in material; rename the rough first");
    }
    let replaced = db
        .get_custom_materials()?
        .iter()
        .any(|row| row.name.eq_ignore_ascii_case(&name));

    let row = adopted_row(&name, request.host, &adopted);
    db.save_custom_material(&CustomMaterialParams {
        name: &row.name,
        refractive_index: row.refractive_index,
        dispersion: row.dispersion,
        birefringence: row.birefringence,
        absorption_rgb: row.absorption_rgb,
        crystal_system: row.crystal_system.as_deref(),
        optical_character: row.optical_character.as_deref(),
        biaxial_delta_beta_alpha: row.biaxial_delta_beta_alpha,
        per_axis_dispersion_json: row.per_axis_dispersion_json.as_deref(),
        specific_gravity: row.specific_gravity,
        color_recipe_json: row.color_recipe_json.as_deref(),
        dispersion_model_json: row.dispersion_model_json.as_deref(),
        absorption_bands_json: row.absorption_bands_json.as_deref(),
    })
    .with_context(|| format!("The material '{name}' could not be saved"))?;
    save_material_zoning(db, &name, &adopted.zoning, false)
        .with_context(|| format!("The colour zones of '{name}' could not be saved"))?;

    let base = gem_material_from_row(&row);
    let mm_per_unit = request.placement.pose.mm_per_unit as f32;
    Ok(AdoptOutcome {
        material: adopted_material(&base, &adopted, mm_per_unit),
        material_name: name,
        stone_width_mm: adopted.stone_width_mm,
        replaced,
    })
}

// ---- render jobs -----------------------------------------------------------------------------

/// Persists the zones of `material` beside queued render job `job_id`, because the job's frozen
/// scene (the wire's `SceneState`) carries none.
///
/// Returns whether anything was stored (`false` for
/// a material without zones). Call right after the job is added.
///
/// # Errors
///
/// The zones fail validation or the vault write fails.
pub fn save_job_zoning(db: &Database, job_id: i64, material: &GemMaterial) -> Result<bool> {
    let Some(zoning) = material.zoning.as_ref() else {
        return Ok(false);
    };
    db.save_render_job_zoning(job_id, &encode_zoned(zoning)?)?;
    Ok(true)
}

/// Puts the zones persisted for job `job_id` back on `scene`'s material when the job runs.
/// Returns whether zones were attached (`false`: none stored, the scene renders as its base
/// zone).
///
/// # Errors
///
/// The vault read fails or the stored zones are unusable; the caller should fail the job with
/// the message rather than render it without its zones.
pub fn attach_job_zoning(db: &Database, job_id: i64, scene: &mut SceneState) -> Result<bool> {
    let Some(text) = db.load_render_job_zoning(job_id)? else {
        return Ok(false);
    };
    let zoning = decode_zoned(&text)
        .with_context(|| format!("The colour zones of render job {job_id} cannot be used"))?;
    scene.material.zoning = Some(zoning);
    Ok(true)
}

/// Forgets the zones of job `job_id`.
///
/// # Errors
///
/// The vault write fails.
pub fn delete_job_zoning(db: &Database, job_id: i64) -> Result<usize> {
    db.delete_render_job_zoning(job_id)
}

// ---- housekeeping ----------------------------------------------------------------------------

/// Removes the side-table rows whose owner (saved plan, custom material, render job) no longer
/// exists.
///
/// The side tables carry no foreign keys, so a default build deleting an owner cannot
/// cascade into them; call this from a zoning build at startup. Returns how many rows went.
///
/// # Errors
///
/// The vault write fails.
pub fn prune_orphans(db: &Database) -> Result<usize> {
    db.prune_zoning_orphans()
}
