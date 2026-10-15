//! Saving: the bytes of the self-contained `.isdd` file and of the `.asc`
//! cutting schedule, built with the same shared-crate APIs the desktop's save and the web
//! app's downloads use.
//!
//! | Action | Built with | File name |
//! |---|---|---|
//! | Save design | `design_to_file` + `native::design::to_string`; writes the opened file's `[meta]` table (stamped) and attachments back | `<name>.isdd` |
//! | Export `.asc` | `Design::try_to_asc_schedule_from_solved_with` + `to_asc_string` (CRLF, `GemCAD`'s convention) | the design's `.asc` name |
//!
//! A design that solves but does not close is written with the desktop's "NOT A CLOSED
//! SOLID" header (`.asc`) and a warning; one that does not solve is saved as a draft
//! design file (the desktop's "Save Anyway") and refused for `.asc`, exactly as the
//! desktop refuses it. The solve comes from the current [`crate::design_state`] result
//! when there is one, else it runs here (a save is rare and the answer is needed now).

use std::{fmt, sync::atomic::AtomicBool};

use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, design_to_file},
};
use indicatrix_editor::{
    files::{converted_asc_file_name, degenerate_marker_header},
    solve_policy::solve_cancellably,
    view_model::solid_status::status_text_and_is_problem_from_solved,
};
use indicatrix_formats::{asc::to_asc_string, native::design as design_file};
use indicatrix_web_core::open_route::design_file_name_for_asc;

use super::{DocumentOrigin, stamp::stamped};
use crate::{
    app_state::AppState,
    platform::{OutgoingFile, mime},
};

/// Why nothing was written. The message is a sentence the UI shows as it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveError {
    /// What went wrong, for the user.
    pub message: String,
}

impl SaveError {
    /// An error with this message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SaveError {}

/// A file ready for the platform, with what the user should hear about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedFile {
    /// The bytes, name and MIME type.
    pub file: OutgoingFile,
    /// A sentence when the design is saved as a draft or does not close; `None` for an
    /// ordinary file.
    pub warning: Option<String>,
}

/// The current design's solved tiers: the current solve's, or a solve run now. A design
/// without tiers needs none.
fn solved_tiers(state: &AppState) -> Result<Vec<SolvedTier>, String> {
    let design = &state.session.design;
    if design.tiers.is_empty() {
        return Ok(Vec::new());
    }
    if let Some(tiers) = state
        .solve
        .current_tiers(state.session.current_generation())
    {
        return Ok(tiers.to_vec());
    }
    solve_cancellably(design, &AtomicBool::new(false)).map_err(|error| error.to_string())
}

/// `design`'s "does not close" message for a solve that succeeded, `None` when it closes
/// (or has no tiers).
fn closure_problem(design: &Design, solved: &[SolvedTier]) -> Option<String> {
    let (text, problem) = status_text_and_is_problem_from_solved(design, solved);
    problem.then_some(text)
}

/// The self-contained `.isdd` file of the open design.
///
/// The `[meta]` table the design was opened with is written back stamped with the current
/// time (and an id when it had none), and the stamped table becomes the document's own so
/// the next save keeps the id. A design that does not solve is written as a draft and the
/// result carries the reason.
///
/// # Errors
/// [`SaveError`] when the file cannot be built (an attachment over the size cap or with
/// a bad name, in practice).
pub fn save_bytes(state: &mut AppState) -> Result<SavedFile, SaveError> {
    let solved = solved_tiers(state);
    let design = &state.session.design;
    let problem = match &solved {
        Ok(solved) => closure_problem(design, solved),
        Err(message) => Some(message.clone()),
    };
    let draft = solved.is_err() && !design.tiers.is_empty();
    let history = state.session.history.description_log().to_vec();
    let metadata = stamped(&state.document.metadata);
    let snapshot = super::save_snapshot(state);
    let file = design_to_file(
        design,
        state.document.printed_proportions.as_ref(),
        &DesignExtras {
            custom_material: snapshot.as_ref(),
            history_entries: &history,
            metadata: Some(&metadata),
            attachments: &state.document.attachments,
        },
    )
    .with_draft(draft);
    let text = design_file::to_string(&file)
        .map_err(|error| SaveError::new(format!("Cannot save: {error}")))?;
    let name = design_file_name_for_asc(&state.document.save_asc_name(design));
    state.document.metadata = metadata;
    let warning = problem.map(|message| {
        if draft {
            format!("Saved as a draft because the design does not solve: {message}")
        } else {
            format!("The solid does not close: {message}")
        }
    });
    Ok(SavedFile {
        file: OutgoingFile {
            suggested_name: name,
            mime: mime::INDICATRIX_DESIGN.to_owned(),
            bytes: text.into_bytes(),
        },
        warning,
    })
}

/// The `.asc` cutting schedule of the open design.
///
/// # Errors
/// [`SaveError`] when the design does not solve (the sentence says why) or its schedule
/// cannot be written as `.asc`.
pub fn asc_bytes(state: &AppState) -> Result<SavedFile, SaveError> {
    let solved = solved_tiers(state)
        .map_err(|message| SaveError::new(format!("Cannot export: {message}.")))?;
    let design = &state.session.design;
    let mut schedule = design
        .try_to_asc_schedule_from_solved_with(&solved, &super::custom_catalogue(state))
        .map_err(|mismatch| {
            SaveError::new(format!(
                "Cannot export: the solve has {} tiers but the design has {}. Solve again.",
                mismatch.got_tiers, mismatch.expected_tiers
            ))
        })?;
    let problem = closure_problem(design, &solved);
    if let Some(message) = &problem
        && let Some(header) = degenerate_marker_header(&schedule.headers, message)
    {
        schedule.headers.insert(0, header);
    }
    let text = to_asc_string(&schedule)
        .map_err(|error| SaveError::new(format!("Cannot write this schedule as .asc: {error}")))?;
    Ok(SavedFile {
        file: OutgoingFile {
            suggested_name: state.document.save_asc_name(design),
            mime: mime::ASC.to_owned(),
            bytes: text.into_bytes(),
        },
        warning: problem.map(|message| format!("Marked NOT A CLOSED SOLID: {message}")),
    })
}

/// Records that the design was saved as `display_name`: the session reads clean, the
/// document is now that file, and later saves propose its name.
pub fn mark_saved(state: &mut AppState, display_name: &str) {
    state.session.mark_saved();
    state.document.display_name = Some(display_name.to_owned());
    state.document.asc_filename = Some(converted_asc_file_name(display_name));
    state.document.origin = DocumentOrigin::Document {
        display_name: display_name.to_owned(),
    };
}
