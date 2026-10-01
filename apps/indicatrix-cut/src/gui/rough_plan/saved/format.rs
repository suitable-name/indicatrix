//! Reading and writing whole plan documents.
//!
//! The loader is header-first: `format` and `version` are read and judged before the
//! rest of the file is parsed, so a wrong or newer file gets its own message. Then every
//! field is validated (see [`super::convert`]) and cross-checked against the rough and
//! the settings (see [`super::checks`]); a file that loads is safe to hand to the
//! renderers. No input makes it panic.

use super::{
    checks::{RoughFrame, check_layout},
    convert::{
        CandidateSource, candidate_source_from_dto, check_designs, layout_from_dto, layout_to_dto,
        material_from_dto, rough_from_dto, rough_to_dto, settings_from_dto, settings_to_dto,
    },
    dto::{
        CURRENT_SCHEMA_VERSION, DesignShape, HeaderDto, MAX_LAYOUTS, MAX_NAME_CHARS,
        MAX_PAYLOAD_BYTES, ROUGH_PLAN_FORMAT, SavedDesignDto, SavedLayoutDto, SavedPlanDto,
        SummaryDto,
    },
};
use indicatrix_cut_core::rough_plan::{PlanSettings, RoughLayout, RoughModel};
use indicatrix_vault::model::solid_extents::{SOLID_EXTENTS_VERSION, SolidExtents};
use std::{borrow::Cow, collections::BTreeSet};

/// Significant digits kept in a design fingerprint.
const FINGERPRINT_DIGITS: usize = 9;

/// A plan file that passed every check, in core types.
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedPlan {
    /// The schema version in the file's header.
    pub version: u32,
    /// The name stored in the file.
    pub name: String,
    /// The creation time stored in the file (Unix seconds).
    pub created_at: i64,
    /// The stamp of the library that wrote the file, if it recorded one.
    pub library_id: Option<u32>,
    /// The modelled rough.
    pub model: RoughModel,
    /// The material's name.
    pub material_name: String,
    /// The weighed carat, if one was entered.
    pub weighed_ct: Option<f64>,
    /// The planning settings, with the material's specific gravity.
    pub settings: PlanSettings,
    /// Where the candidate designs came from (the library filter when the file says
    /// nothing).
    pub candidate_source: CandidateSource,
    /// The designs the layouts use, with their fingerprints.
    pub designs: Vec<SavedDesignDto>,
    /// The saved layouts, best rank first.
    pub layouts: Vec<RoughLayout>,
}

/// `value` rounded to `digits` significant digits (through its decimal text, so the
/// result is the same on every platform).
#[must_use]
pub fn round_significant(value: f64, digits: usize) -> f64 {
    if !value.is_finite() {
        return value;
    }
    format!("{value:.precision$e}", precision = digits.saturating_sub(1))
        .parse()
        .unwrap_or(value)
}

/// The `[L/W, H/W, V/W^3]` fingerprint of a design's extents, to 9 significant digits.
/// All zero when the extents cannot give one.
#[must_use]
pub fn compute_fingerprint(extents: &SolidExtents) -> [f64; 3] {
    let width = extents.width_caliper.min(extents.length_caliper);
    let length = extents.width_caliper.max(extents.length_caliper);
    if !width.is_finite() || width <= 0.0 {
        return [0.0; 3];
    }
    let ratios = [
        length / width,
        extents.height / width,
        extents.volume / (width * width * width),
    ];
    if ratios.iter().all(|ratio| ratio.is_finite()) {
        ratios.map(|ratio| round_significant(ratio, FINGERPRINT_DIGITS))
    } else {
        [0.0; 3]
    }
}

/// The shape a plan stores for a design measured as `extents`: the ratio fingerprint, the
/// width in model units (which the ratios cannot tell, so a design drawn twice as large
/// is noticed) to the same 9 significant digits, and the measuring rule version. All
/// unknown when the extents cannot give a fingerprint.
#[must_use]
pub fn design_shape(extents: &SolidExtents) -> DesignShape {
    let fingerprint = compute_fingerprint(extents);
    if fingerprint
        .iter()
        .all(|ratio| ratio.abs() < f64::MIN_POSITIVE)
    {
        return DesignShape::default();
    }
    DesignShape {
        fingerprint,
        width_caliper: Some(round_significant(
            extents.width_caliper.min(extents.length_caliper),
            FINGERPRINT_DIGITS,
        )),
        extents_version: SOLID_EXTENTS_VERSION,
    }
}

/// The byte offset of the first table header line: everything before it holds the
/// top-level keys.
fn header_end(text: &str) -> usize {
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        if line.trim_start().starts_with('[') {
            return offset;
        }
        offset += line.len();
    }
    text.len()
}

/// The message for a TOML error, with the line it occurred on.
fn parse_error_text(error: &toml::de::Error, text: &str) -> String {
    let line = error
        .span()
        .and_then(|span| text.get(..span.start))
        .map(|before| before.bytes().filter(|byte| *byte == b'\n').count() + 1);
    line.map_or_else(
        || error.message().to_string(),
        |line| format!("{} (line {line})", error.message()),
    )
}

/// Reads `format` and `version`. The top-level keys precede every table, so the part
/// of the file before the first table header is tried first; that keeps the header
/// readable when the rest of the file is truncated or broken.
fn read_header(text: &str) -> Result<HeaderDto, String> {
    if let Some(prefix) = text.get(..header_end(text))
        && let Ok(header) = toml::from_str::<HeaderDto>(prefix)
    {
        return Ok(header);
    }
    toml::from_str::<HeaderDto>(text).map_err(|e| {
        format!(
            "This is not a rough plan file: {}",
            parse_error_text(&e, text)
        )
    })
}

/// Judges the header of a plan file and returns its schema version.
///
/// # Errors
///
/// Returns a message for a file that is not a rough plan, has no version, or was made
/// by a newer Indicatrix.
pub fn check_header(text: &str) -> Result<u32, String> {
    let header = read_header(text)?;
    match header.format.as_deref() {
        Some(ROUGH_PLAN_FORMAT) => {}
        Some(other) => {
            return Err(format!(
                "This is not an Indicatrix rough plan: its format is '{other}', expected '{ROUGH_PLAN_FORMAT}'."
            ));
        }
        None => {
            return Err(
                "This is not an Indicatrix rough plan: the 'format' key is missing.".to_string(),
            );
        }
    }
    match header.version {
        Some(version) if version > i64::from(CURRENT_SCHEMA_VERSION) => Err(format!(
            "This plan was made by a newer Indicatrix (plan version {version}; this one reads up to {CURRENT_SCHEMA_VERSION}). Update Indicatrix to open it."
        )),
        Some(version) if version >= 1 => u32::try_from(version)
            .map_err(|_| format!("The plan's 'version' {version} is invalid.")),
        Some(version) => Err(format!(
            "The plan's 'version' must be at least 1, found {version}."
        )),
        None => Err("The plan's 'version' key is missing.".to_string()),
    }
}

/// The schema version in the header of `text`, if it has a readable one. This is what a
/// stored plan's `payload_version` records.
#[must_use]
pub fn payload_version_of(text: &str) -> Option<u32> {
    read_header(text)
        .ok()?
        .version
        .and_then(|version| u32::try_from(version).ok())
}

/// Brings the text of a plan written with schema `version` up to the current schema.
///
/// Version 1 is the current schema and needs nothing. A later format change adds an arm
/// here that rewrites the older text, so the DTOs only ever describe the newest schema.
fn migrate(text: &str, version: u32) -> Result<Cow<'_, str>, String> {
    match version {
        CURRENT_SCHEMA_VERSION => Ok(Cow::Borrowed(text)),
        older => Err(format!(
            "This build cannot bring a version {older} plan up to version {CURRENT_SCHEMA_VERSION}."
        )),
    }
}

/// Fails when the document is bigger than a plan can be, before anything is converted or
/// looked up.
fn check_limits(dto: &SavedPlanDto) -> Result<(), String> {
    if dto.name.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "name is longer than {MAX_NAME_CHARS} characters ({})",
            dto.name.chars().count()
        ));
    }
    if dto.layouts.len() > MAX_LAYOUTS {
        return Err(format!(
            "layouts lists {} layouts; a plan holds at most {MAX_LAYOUTS}",
            dto.layouts.len()
        ));
    }
    Ok(())
}

/// Converts and cross-checks the layouts, rejects a rank used twice and orders them by
/// rank (best first), whatever order the file listed them in.
fn layouts_from_dto(
    dtos: &[SavedLayoutDto],
    frame: &RoughFrame,
) -> Result<Vec<RoughLayout>, String> {
    let mut ranked = Vec::with_capacity(dtos.len());
    let mut seen = BTreeSet::new();
    for (i, dto) in dtos.iter().enumerate() {
        let path = format!("layouts[{i}]");
        let layout = layout_from_dto(dto, &path)?;
        check_layout(&layout, frame, &path)?;
        if !seen.insert(dto.rank) {
            return Err(format!(
                "{path}.rank {} appears twice; every layout needs its own rank",
                dto.rank
            ));
        }
        ranked.push((dto.rank, layout));
    }
    ranked.sort_by_key(|(rank, _)| *rank);
    Ok(ranked.into_iter().map(|(_, layout)| layout).collect())
}

/// Fails when a stone names a design the plan does not list, or the plan lists a design
/// no stone uses.
fn check_design_use(designs: &[SavedDesignDto], layouts: &[RoughLayout]) -> Result<(), String> {
    let known: BTreeSet<i64> = designs.iter().map(|design| design.entry_id).collect();
    let mut used = BTreeSet::new();
    for (i, layout) in layouts.iter().enumerate() {
        for (j, stone) in layout.stones.iter().enumerate() {
            if !known.contains(&stone.entry_id) {
                return Err(format!(
                    "layouts[{i}].stones[{j}].entry_id {} has no entry in designs",
                    stone.entry_id
                ));
            }
            used.insert(stone.entry_id);
        }
    }
    match designs
        .iter()
        .enumerate()
        .find(|(_, design)| !used.contains(&design.entry_id))
    {
        Some((i, design)) => Err(format!(
            "designs[{i}].entry_id {} is not used by any stone",
            design.entry_id
        )),
        None => Ok(()),
    }
}

/// Checks and converts a parsed document; `version` is the one its header declared.
fn plan_from_dto(dto: SavedPlanDto, version: u32) -> Result<LoadedPlan, String> {
    check_limits(&dto)?;
    let model = rough_from_dto(&dto.rough)?;
    let measure = model.measure().map_err(|e| format!("rough: {e}"))?;
    let (material_name, specific_gravity, weighed_ct) = material_from_dto(&dto.rough)?;
    let settings = settings_from_dto(&dto.settings, specific_gravity)?;
    let candidate_source = candidate_source_from_dto(&dto.settings)?;
    check_designs(&dto.designs)?;
    let frame = RoughFrame {
        specific_gravity,
        volume_mm3: measure.volume_mm3,
        bbox_mm: model.base.bounding_box_extents(),
    };
    let layouts = layouts_from_dto(&dto.layouts, &frame)?;
    check_design_use(&dto.designs, &layouts)?;
    Ok(LoadedPlan {
        version,
        name: dto.name,
        created_at: dto.created_at,
        library_id: dto.library_id,
        model,
        material_name,
        weighed_ct,
        settings,
        candidate_source,
        designs: dto.designs,
        layouts,
    })
}

/// Parses and validates the text of a plan file.
///
/// # Errors
///
/// Returns a message naming the problem: a wrong format, a newer version, a missing
/// field, or the path of the first field that is not a valid number, name or pose, or
/// that contradicts the figures it follows from.
pub fn parse_and_validate_plan(text: &str) -> Result<LoadedPlan, String> {
    if text.len() > MAX_PAYLOAD_BYTES {
        return Err(format!(
            "The file is too large to be a rough plan ({} bytes).",
            text.len()
        ));
    }
    let version = check_header(text)?;
    let body = migrate(text, version)?;
    let dto: SavedPlanDto = toml::from_str(&body).map_err(|e| {
        format!(
            "The rough plan is incomplete or malformed: {}",
            parse_error_text(&e, &body)
        )
    })?;
    plan_from_dto(dto, version)
}

/// The one-line description of a stored plan for the saved list ("Pebble · Aquamarine
/// · 3 results"); "Saved plan" when the payload cannot be read.
#[must_use]
pub fn plan_summary(payload: &str) -> String {
    let Ok(summary) = toml::from_str::<SummaryDto>(payload) else {
        return "Saved plan".to_string();
    };
    let count = summary.layouts.len();
    let mut base = summary.rough.base.trim().chars();
    let base: String = base
        .next()
        .map(|first| first.to_uppercase().chain(base).collect())
        .unwrap_or_default();
    format!(
        "{base} · {} · {count} result{}",
        summary.rough.material.trim(),
        if count == 1 { "" } else { "s" }
    )
}

/// The byte range of the value on a `name = "..."` line (a one-line basic or literal
/// string), relative to the line; `None` for any other line or value form.
fn name_value_span(line: &str) -> Option<(usize, usize)> {
    let after_key = line.trim_start().strip_prefix("name")?;
    let value = after_key.trim_start().strip_prefix('=')?.trim_start();
    let start = line.len() - value.len();
    let mut chars = value.char_indices();
    let (_, quote) = chars.next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    if value.starts_with("\"\"\"") || value.starts_with("'''") {
        return None;
    }
    let mut escaped = false;
    for (offset, c) in chars {
        if quote == '"' && escaped {
            escaped = false;
        } else if quote == '"' && c == '\\' {
            escaped = true;
        } else if c == quote {
            return Some((start, start + offset + 1));
        }
    }
    None
}

/// `payload` with the value of its top-level `name` line replaced; everything else in the
/// text (unknown keys, comments, the trailing comment of the line itself) stays byte for
/// byte. `None` when the header region has no one-line `name` string.
fn patch_name_line(payload: &str, name: &str) -> Option<String> {
    let header = payload.get(..header_end(payload))?;
    let mut offset = 0;
    for line in header.split_inclusive('\n') {
        if let Some((start, end)) = name_value_span(line) {
            let quoted = toml::Value::String(name.to_string()).to_string();
            return Some(format!(
                "{}{quoted}{}",
                &payload[..offset + start],
                &payload[offset + end..]
            ));
        }
        offset += line.len();
    }
    None
}

/// The payload with its `name` replaced (a rename only changes the stored name, and the
/// file carries its own). Only the name line is edited, so keys this build does not know
/// and comments survive; a text whose name line has another form is re-written through
/// the document; one that cannot be read as a plan is returned unchanged.
#[must_use]
pub fn payload_with_name(payload: &str, name: &str) -> String {
    if let Some(patched) = patch_name_line(payload, name) {
        return patched;
    }
    let Ok(mut dto) = toml::from_str::<SavedPlanDto>(payload) else {
        return payload.to_string();
    };
    dto.name = name.to_string();
    toml::to_string_pretty(&dto).unwrap_or_else(|_| payload.to_string())
}

/// One layout to save and the rank (1-based) it had in the plan it came from.
pub struct RankedLayout<'a> {
    /// The rank in the original plan.
    pub rank: usize,
    /// The layout.
    pub layout: &'a RoughLayout,
}

/// Everything [`serialize_plan_to_toml`] writes into a plan file.
pub struct SerializeInput<'a> {
    /// The plan's name.
    pub name: &'a str,
    /// Unix timestamp (seconds) of the save.
    pub created_at: i64,
    /// The stamp of the library whose entry ids the plan uses, if known.
    pub library_id: Option<u32>,
    /// The modelled rough.
    pub model: &'a RoughModel,
    /// The material's name.
    pub material_name: &'a str,
    /// The weighed carat, if entered.
    pub weighed_ct: Option<f64>,
    /// The planning settings (they carry the specific gravity).
    pub settings: &'a PlanSettings,
    /// Where the candidate designs came from.
    pub candidate_source: CandidateSource,
    /// The used designs with their fingerprints.
    pub designs: &'a [SavedDesignDto],
    /// The layouts to save, best first.
    pub layouts: &'a [RankedLayout<'a>],
}

/// Writes a plan document. The values are written as they are; use
/// [`serialize_checked_plan`] to also prove the text reads back.
///
/// # Errors
///
/// Returns an error if the TOML writer fails.
pub fn serialize_plan_to_toml(input: &SerializeInput<'_>) -> Result<String, String> {
    let doc = SavedPlanDto {
        format: ROUGH_PLAN_FORMAT.to_string(),
        version: CURRENT_SCHEMA_VERSION,
        name: input.name.to_string(),
        created_at: input.created_at,
        library_id: input.library_id,
        rough: rough_to_dto(
            input.model,
            input.material_name,
            input.settings.specific_gravity,
            input.weighed_ct,
        ),
        settings: settings_to_dto(input.settings, input.candidate_source),
        designs: input.designs.to_vec(),
        layouts: input
            .layouts
            .iter()
            .map(|ranked| layout_to_dto(ranked.layout, ranked.rank))
            .collect(),
    };
    toml::to_string_pretty(&doc).map_err(|e| format!("Could not write the plan: {e}"))
}

/// [`serialize_plan_to_toml`], then reads the text back with the loader, so a plan that
/// could not be opened again is never stored.
///
/// # Errors
///
/// Returns an error if writing fails or the written text does not pass the loader.
pub fn serialize_checked_plan(input: &SerializeInput<'_>) -> Result<String, String> {
    let text = serialize_plan_to_toml(input)?;
    parse_and_validate_plan(&text)
        .map_err(|e| format!("The plan cannot be saved because it would not open again: {e}"))?;
    Ok(text)
}
