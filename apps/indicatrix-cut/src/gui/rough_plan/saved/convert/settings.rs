//! The plan file's settings section and the candidate source: reading them into the core's
//! [`PlanSettings`] (every number checked, every wrong one named) and writing them back.
//!
//! The design cap lives under the key `count` (a name kept from the time the planner had one
//! cap only, so older files still open). The stone cap is the optional key `max_stones`:
//! absent in every file written before it existed and in every plan without a cap, so uncapped
//! plans are written exactly as they always were.

use super::{non_negative_up_to, positive_up_to};
use crate::gui::rough_plan::saved::dto::{MAX_LOSS_MM, MAX_MIN_WIDTH_MM, SettingsDto};
use indicatrix_cut_core::rough_plan::PlanSettings;

/// The stone cap of `dto`: 0 (no limit) when the file has none.
fn max_stones_from_dto(dto: &SettingsDto) -> Result<u8, String> {
    let Some(value) = dto.max_stones else {
        return Ok(0);
    };
    u8::try_from(value)
        .ok()
        .filter(|stones| (0..=99).contains(stones))
        .ok_or_else(|| format!("settings.max_stones must be 0 to 99, found {value}"))
}

/// The planning settings of `dto` for a material of specific gravity `specific_gravity`.
///
/// # Errors
///
/// Returns a message naming the field when the design cap is outside `1..=99` or the stone
/// cap outside `0..=99` (both reported, never clamped; a file without a stone cap means 0,
/// no limit), a length is negative (zero for the minimum width), or a length is beyond what
/// the planner form accepts (`MAX_LOSS_MM` for kerf, allowance and skin, `MAX_MIN_WIDTH_MM`
/// for the minimum width).
pub fn settings_from_dto(dto: &SettingsDto, specific_gravity: f64) -> Result<PlanSettings, String> {
    let max_designs = u8::try_from(dto.count)
        .ok()
        .filter(|count| (1..=99).contains(count))
        .ok_or_else(|| format!("settings.count must be 1 to 99, found {}", dto.count))?;
    let max_stones = max_stones_from_dto(dto)?;
    // `min_count` of older files is ignored: the planner shows every layout from one stone up.
    Ok(PlanSettings {
        max_designs,
        max_stones,
        kerf_mm: non_negative_up_to(dto.kerf_mm, MAX_LOSS_MM, "settings.kerf_mm")?,
        allowance_mm: non_negative_up_to(dto.allowance_mm, MAX_LOSS_MM, "settings.allowance_mm")?,
        skin_mm: non_negative_up_to(dto.skin_mm, MAX_LOSS_MM, "settings.skin_mm")?,
        min_width_mm: positive_up_to(dto.min_width_mm, MAX_MIN_WIDTH_MM, "settings.min_width_mm")?,
        specific_gravity,
    })
}

/// Where a plan's candidate designs came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CandidateSource {
    /// The designs the library filter selected. Also what a file without the key means.
    #[default]
    Filter,
    /// Every design in the library.
    Library,
}

impl CandidateSource {
    /// The source of a run that used the library filter (`true`) or the whole library.
    #[must_use]
    pub const fn from_use_filter(use_filter: bool) -> Self {
        if use_filter {
            Self::Filter
        } else {
            Self::Library
        }
    }

    /// Whether the candidates were the library filter's designs.
    #[must_use]
    pub const fn uses_filter(self) -> bool {
        matches!(self, Self::Filter)
    }

    /// The value written to the file.
    const fn key(self) -> &'static str {
        match self {
            Self::Filter => "filter",
            Self::Library => "library",
        }
    }
}

/// The candidate source of `dto`: "filter" when the file has none.
///
/// # Errors
///
/// Returns a message naming the field when the key holds anything but "filter" or
/// "library".
pub fn candidate_source_from_dto(dto: &SettingsDto) -> Result<CandidateSource, String> {
    match dto.candidate_source.as_deref() {
        None => Ok(CandidateSource::default()),
        Some("filter") => Ok(CandidateSource::Filter),
        Some("library") => Ok(CandidateSource::Library),
        Some(other) => Err(format!(
            "settings.candidate_source must be \"filter\" or \"library\", found \"{other}\""
        )),
    }
}

/// The file section of `settings`, planned from `source`. The stone cap is written only
/// when the plan has one, so a plan without a cap reads back as it always did.
#[must_use]
pub fn settings_to_dto(settings: &PlanSettings, source: CandidateSource) -> SettingsDto {
    SettingsDto {
        count: i64::from(settings.max_designs),
        max_stones: Some(i64::from(settings.max_stones)).filter(|stones| *stones > 0),
        min_count: None,
        kerf_mm: settings.kerf_mm,
        allowance_mm: settings.allowance_mm,
        skin_mm: settings.skin_mm,
        min_width_mm: settings.min_width_mm,
        candidate_source: Some(source.key().to_string()),
    }
}
