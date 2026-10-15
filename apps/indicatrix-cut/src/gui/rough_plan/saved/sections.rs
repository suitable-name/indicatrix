//! The two sections of a plan file that live next to a plan in the library: its rough colour and
//! the cutter's stone orientation choices (schema version 4).
//!
//! The library keeps both in side tables keyed by the plan's id (`rough_colour` and
//! `stone_pose_choice`, see `gui::rough_colour::store`), never in the stored payload. A file
//! carries them as the `[colour]` table and the `[[pose_choices]]` tables, so that an export
//! keeps them and an import brings them back:
//!
//! - **load** ([`check_sections`], called by the loader): both sections are checked against the
//!   plan they are in;
//! - **export** (`stored_sections`, `payload_with_sections`): the side tables are read and the
//!   stored payload is written again with the sections added, as a version 4 document;
//! - **import** ([`stored_payload`], `restore_sections`): the sections are taken out of the
//!   file's document before it is stored, and written to the new plan's side tables.
//!
//! Every document is written by the TOML writer (never by appending text) and read back by the
//! loader before it is handed on, so no export is a file this build would refuse and no import
//! stores a payload that would not open.

use super::{
    dto::{COLOUR_SCHEMA_VERSION, PoseChoiceDto, RoughColourDto, SavedPlanDto},
    format::{LoadedPlan, parse_and_validate_plan},
};
use indicatrix_cut_core::rough_plan::{POSE_COUNT, RoughLayout};
use std::{borrow::Cow, collections::BTreeSet};

use crate::gui::rough_colour::store::{
    load_pose_choices, load_rough_colour, save_layout_pose_choices, save_rough_colour,
};
use indicatrix_cut_core::rough_plan::zoned_plan::{
    RoughColour, check_version, decode_zoned, encode_zoned,
};
use indicatrix_vault::db::sqlite::Database;
use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

/// Fails when the sections of a plan document do not belong to it: a version older than 4 that
/// has a section, a colour this build cannot use (a format version it does not read, or zones
/// that do not decode and validate), or a stone orientation
/// that names a layout or stone the plan does not have, a pose that does not exist, or a stone
/// twice. `layouts` are the plan's layouts as the loader returns them.
pub(super) fn check_sections(
    version: u32,
    colour: Option<&RoughColourDto>,
    pose_choices: &[PoseChoiceDto],
    layouts: &[RoughLayout],
) -> Result<(), String> {
    if colour.is_none() && pose_choices.is_empty() {
        return Ok(());
    }
    if version < COLOUR_SCHEMA_VERSION {
        let section = if colour.is_some() {
            "colour"
        } else {
            "pose_choices"
        };
        return Err(format!(
            "{section} needs plan version {COLOUR_SCHEMA_VERSION}; this plan declares version {version}."
        ));
    }
    if let Some(colour) = colour {
        check_colour(colour)?;
    }
    check_poses(pose_choices, layouts)
}

/// Fails when the colour was written in a format this build does not read or its zones do not
/// decode and validate (`decode_zoned`).
fn check_colour(colour: &RoughColourDto) -> Result<(), String> {
    check_version(colour.version).map_err(|e| format!("colour: {e}"))?;
    decode_zoned(&colour.zoned_json).map_err(|e| format!("colour.zoned_json: {e}"))?;
    Ok(())
}

/// Fails when a choice names a layout or stone `layouts` do not have, a pose of `POSE_COUNT`
/// or more, or a stone a second time.
fn check_poses(pose_choices: &[PoseChoiceDto], layouts: &[RoughLayout]) -> Result<(), String> {
    let mut seen = BTreeSet::new();
    for (i, choice) in pose_choices.iter().enumerate() {
        let path = format!("pose_choices[{i}]");
        let Some(layout) = usize::try_from(choice.layout)
            .ok()
            .and_then(|index| layouts.get(index))
        else {
            return Err(format!(
                "{path}.layout {} names no layout; the plan has {}",
                choice.layout,
                layouts.len()
            ));
        };
        let has_stone =
            usize::try_from(choice.stone).is_ok_and(|index| index < layout.stones.len());
        if !has_stone {
            return Err(format!(
                "{path}.stone {} names no stone of layouts[{}]; it has {}",
                choice.stone,
                choice.layout,
                layout.stones.len()
            ));
        }
        if choice.pose >= POSE_COUNT {
            return Err(format!(
                "{path}.pose {} does not exist; a stone has {POSE_COUNT} poses",
                choice.pose
            ));
        }
        if !seen.insert((choice.layout, choice.stone)) {
            return Err(format!(
                "{path} chooses layouts[{}].stones[{}] a second time",
                choice.layout, choice.stone
            ));
        }
    }
    Ok(())
}

/// `dto` as TOML text, proven to read back with the loader.
fn write_checked(dto: &SavedPlanDto) -> Result<String, String> {
    let text = toml::to_string_pretty(dto).map_err(|e| format!("Could not write the plan: {e}"))?;
    parse_and_validate_plan(&text)
        .map_err(|e| format!("The written plan would not open again: {e}"))?;
    Ok(text)
}

/// The document of `text`, the text of a plan that passed the loader.
fn document_of(text: &str) -> Result<SavedPlanDto, String> {
    toml::from_str(text).map_err(|e| format!("The plan cannot be read: {}", e.message()))
}

/// `text` without its sections, with the version its remaining content needs: a plan whose
/// colour is taken out is a version 3, 2 or 1 plan again.
fn payload_without_sections(text: &str) -> Result<String, String> {
    let mut dto = document_of(text)?;
    dto.colour = None;
    dto.pose_choices.clear();
    dto.version = dto.content_version();
    write_checked(&dto)
}

/// The text an import stores for the file `text` that loaded as `plan`: the file itself when
/// it has no sections and is no version 4 file (so unknown keys and comments stay, as they
/// always did), and otherwise the document without its sections, written again.
pub(super) fn stored_payload<'a>(text: &'a str, plan: &LoadedPlan) -> Result<Cow<'a, str>, String> {
    let has_sections = plan.colour.is_some() || !plan.pose_choices.is_empty();
    if has_sections || plan.version >= COLOUR_SCHEMA_VERSION {
        payload_without_sections(text).map(Cow::Owned)
    } else {
        Ok(Cow::Borrowed(text))
    }
}

/// A plan's colour and orientation choices as the library holds them, ready for a file.
pub(super) struct StoredSections {
    /// The rough colour, if the plan has one.
    pub(super) colour: Option<RoughColourDto>,
    /// The orientation choices, ordered by layout and stone.
    pub(super) pose_choices: Vec<PoseChoiceDto>,
}

/// The colour and orientation choices stored for plan `plan_id`, or `None` when it has neither.
///
/// # Errors
///
/// Returns a message when a side table cannot be read, the stored colour cannot be used, or its
/// zones cannot be written.
pub(super) fn stored_sections(
    db: &Mutex<Database>,
    plan_id: i64,
) -> Result<Option<StoredSections>, String> {
    let database = db.lock().unwrap_or_else(PoisonError::into_inner);
    let colour = load_rough_colour(&database, plan_id)
        .map_err(|e| format!("Could not read the color of plan #{plan_id}: {e:#}"))?;
    let choices = load_pose_choices(&database, plan_id)
        .map_err(|e| format!("Could not read the stone orientations of plan #{plan_id}: {e:#}"))?;
    drop(database);
    if colour.is_none() && choices.is_empty() {
        return Ok(None);
    }
    let colour = colour.as_ref().map(colour_dto).transpose()?;
    let pose_choices = choices
        .into_iter()
        .map(|((layout, stone), pose)| PoseChoiceDto {
            layout,
            stone,
            pose,
        })
        .collect();
    Ok(Some(StoredSections {
        colour,
        pose_choices,
    }))
}

/// The file section of a stored colour; the zones are written again by `encode_zoned`, the
/// same encoder the library stored them with.
fn colour_dto(colour: &RoughColour) -> Result<RoughColourDto, String> {
    let zoned_json = encode_zoned(&colour.zoned)
        .map_err(|e| format!("The color of the plan cannot be written: {e}"))?;
    Ok(RoughColourDto {
        version: colour.version,
        zoned_json,
        fit_json: colour.fit_json.clone(),
        created: colour.created,
    })
}

/// The stored plan `payload` under `name` with `sections` added, as the text an export writes
/// (version 4 by content).
///
/// # Errors
///
/// Returns a message when the payload cannot be read, or the document with the sections would
/// not pass the loader (a stored orientation that names a stone the plan does not have, say).
pub(super) fn payload_with_sections(
    payload: &str,
    name: &str,
    sections: StoredSections,
) -> Result<String, String> {
    let mut dto = document_of(payload)?;
    dto.name = name.to_string();
    dto.colour = sections.colour;
    dto.pose_choices = sections.pose_choices;
    dto.version = dto.content_version();
    write_checked(&dto)
}

/// The library colour of a file section.
fn rough_colour_of(dto: &RoughColourDto) -> Result<RoughColour, String> {
    let zoned = decode_zoned(&dto.zoned_json)
        .map_err(|e| format!("The color in the file cannot be used: {e}"))?;
    Ok(RoughColour {
        zoned,
        fit_json: dto.fit_json.clone(),
        version: dto.version,
        created: dto.created,
    })
}

/// The chosen poses of each layout as the stone-indexed lists `save_layout_pose_choices`
/// takes: pose 0, the planner's own, for every stone without a choice.
fn poses_by_layout(plan: &LoadedPlan) -> BTreeMap<u32, Vec<u8>> {
    let mut by_layout: BTreeMap<u32, Vec<u8>> = BTreeMap::new();
    for choice in &plan.pose_choices {
        let stones = usize::try_from(choice.layout)
            .ok()
            .and_then(|index| plan.layouts.get(index))
            .map_or(0, |layout| layout.stones.len());
        let poses = by_layout
            .entry(choice.layout)
            .or_insert_with(|| vec![0; stones]);
        if let Some(slot) = usize::try_from(choice.stone)
            .ok()
            .and_then(|index| poses.get_mut(index))
        {
            *slot = choice.pose;
        }
    }
    by_layout
}

/// Writes the colour and the orientation choices of `plan` (a file that passed the loader) to
/// the side tables of the stored plan `plan_id`.
///
/// # Errors
///
/// Returns a message when a write fails; what was written before stays, so the caller removes
/// the plan with `store::delete_saved`, which clears the side tables as well.
pub(super) fn restore_sections(
    db: &Mutex<Database>,
    plan_id: i64,
    plan: &LoadedPlan,
) -> Result<(), String> {
    let colour = plan.colour.as_ref().map(rough_colour_of).transpose()?;
    let database = db.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(colour) = &colour {
        save_rough_colour(&database, plan_id, colour)
            .map_err(|e| format!("The color could not be stored: {e:#}"))?;
    }
    for (layout, poses) in poses_by_layout(plan) {
        save_layout_pose_choices(&database, plan_id, layout, &poses)
            .map_err(|e| format!("The stone orientations could not be stored: {e:#}"))?;
    }
    Ok(())
}
