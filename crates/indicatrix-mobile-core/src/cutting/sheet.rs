//! The printable cutting sheet and the diagram picture, as bytes.
//!
//! Building either takes a moment (the sheet renders four views, the picture is drawn at
//! print resolution), so [`sheet_job`] first copies what is needed out of the state; the
//! returned [`SheetJob`] is `Send` and does the work on any thread.

use indicatrix::{geometry::meet_solver::SolvedTier, optics::materials::GemMaterial};
use indicatrix_cut_core::Design;
use indicatrix_editor::cut_sheet::{
    SheetDetails, cutting_sheet_document_with, diagram_export_png, month_year_text,
};

use super::{CutError, NotSolved, plan};
use crate::{app_state::AppState, document};

/// Everything the cutting sheet and the diagram picture are drawn from, copied out of the
/// app state.
#[derive(Debug, Clone)]
pub struct SheetJob {
    design: Design,
    solved: Vec<SolvedTier>,
    custom: Vec<GemMaterial>,
    details: SheetDetails,
}

impl SheetJob {
    /// The sheet's title (the design's title).
    #[must_use]
    pub fn title(&self) -> &str {
        &self.details.title
    }

    /// The self-contained cutting sheet HTML.
    #[must_use]
    pub fn html(&self) -> String {
        cutting_sheet_document_with(&self.design, &self.solved, &self.custom, &self.details)
    }

    /// The diagram picture at print resolution, PNG-encoded.
    ///
    /// # Errors
    /// The reason when the design does not close to a solid.
    pub fn diagram_png(&self) -> Result<Vec<u8>, String> {
        diagram_export_png(&self.design, &self.solved)
    }
}

/// Copies the open design and its current solve for the sheet and the picture.
/// `date_text` is the export date as the sheet prints it (see [`date_text_now`]).
///
/// # Errors
/// [`NotSolved`] when the design has no solve of its current generation (or no tiers).
pub fn sheet_job(state: &AppState, date_text: &str) -> Result<SheetJob, NotSolved> {
    // The plan check also proves the solve fits the design, which the renderers need.
    let _plan = plan(state)?;
    let generation = state.session.current_generation();
    let solved = state
        .solve
        .current_tiers(generation)
        .ok_or(NotSolved)?
        .to_vec();
    let design = state.session.design.clone();
    let mut details = SheetDetails::from_metadata(&design, &state.document.metadata, date_text);
    if details.title.is_empty() {
        details.title = document::display_title(state);
    }
    Ok(SheetJob {
        design,
        solved,
        custom: document::custom_catalogue(state),
        details,
    })
}

/// The cutting sheet HTML of the open design.
///
/// # Errors
/// [`NotSolved`] when the design has no solve of its current generation (or no tiers).
pub fn cutting_sheet_html(state: &AppState, date_text: &str) -> Result<String, NotSolved> {
    sheet_job(state, date_text).map(|job| job.html())
}

/// The diagram of the open design as PNG bytes.
///
/// # Errors
/// [`CutError::NotSolved`] without a current solve, [`CutError::Failed`] when the design
/// does not close to a solid.
pub fn diagram_png(state: &AppState) -> Result<Vec<u8>, CutError> {
    let job = sheet_job(state, "")?;
    job.diagram_png().map_err(CutError::Failed)
}

/// "October 2026" for the clock's current month (UTC).
#[must_use]
pub fn date_text_now() -> String {
    date_text_for(document::unix_now())
}

/// "October 2026" for the month `unix_seconds` falls in (UTC).
#[must_use]
pub fn date_text_for(unix_seconds: i64) -> String {
    let (year, month) = year_and_month(unix_seconds.div_euclid(86_400));
    month_year_text(year, month)
}

/// The proleptic Gregorian year and month (1 to 12) of the day `days` after 1970-01-01.
fn year_and_month(days: i64) -> (i64, u32) {
    // Howard Hinnant's `civil_from_days`, shifted to start the year in March.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, u32::try_from(month).unwrap_or(1))
}

/// A file name `<title>_<suffix>` with the characters that file systems refuse replaced
/// by underscores, e.g. `Round Brilliant_cutting_sheet.html`.
#[must_use]
pub fn file_name(title: &str, suffix: &str) -> String {
    let cleaned: String = title
        .trim()
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stem = cleaned.trim_matches(|c: char| c == '.' || c == ' ');
    if stem.is_empty() {
        format!("Design{suffix}")
    } else {
        format!("{stem}{suffix}")
    }
}
