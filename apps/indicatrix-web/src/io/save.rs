//! The File menu's saves and downloads, each built with the same shared-crate
//! string API the desktop's save uses, then handed to [`download_bytes`]:
//!
//! | Action | Built with | File name |
//! |---|---|---|
//! | Save `.asc` | `Design::to_asc_schedule_from_solved_with` + `indicatrix_formats::asc::to_asc_string` (CRLF, `GemCAD`'s convention) | the design's `.asc` name |
//! | Export `.gcs` (experimental) | the `.asc` schedule through `indicatrix_formats::gcs::to_gcs_string`; concave tiers ride along as footnotes | the design's name with `.gcs` |
//! | Save design | `indicatrix_cut_core::native::design_to_file` + `indicatrix_formats::native::design::to_string`; writes the opened file's `[meta]` table (stamped) and attachments back | `<name>.isdd` |
//! | Cutting sheet | `indicatrix_editor::cut_sheet::cutting_sheet_document` | `<name>_cutting_sheet.html` |
//! | Diagram | `indicatrix_editor::cut_sheet::diagram_export_png` | `<name>_diagram.png` |
//!
//! Names follow the desktop (`indicatrix_editor::files`). A design that solves but
//! does not close is written with the desktop's "NOT A CLOSED SOLID" header (what
//! its "Export Anyway" confirmation writes) and a warning; one that does not solve
//! is saved as a draft design file (the desktop's "Save Anyway") and refused for plain
//! `.asc`, cutting sheet and diagram, exactly as the desktop refuses them.

use super::download::{MIME_ASC, MIME_DESIGN, MIME_HTML, MIME_PNG, download_bytes};
use crate::app::{
    Ctx,
    persist::schedule_save,
    push::{MessageKind, push_design, show_message},
    solve::with_solved,
    stamp::stamped_metadata,
};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{
    Design,
    native::{DesignExtras, design_to_file},
};
use indicatrix_editor::{
    cut_sheet::{cutting_sheet_document, diagram_export_png},
    files::{cutting_sheet_file_name, degenerate_marker_header, diagram_file_name},
    view_model::solid_status::status_text_and_is_problem_from_solved,
};
use indicatrix_formats::native::design as design_file;
use indicatrix_web_core::open_route::design_file_name_for_asc;

/// Downloads, reporting success or failure through [`show_message`].
fn deliver(ctx: &Ctx, file_name: &str, mime: &str, bytes: &[u8]) -> bool {
    match download_bytes(file_name, mime, bytes) {
        Ok(()) => true,
        Err(message) => {
            show_message(ctx, MessageKind::Error, &message);
            false
        }
    }
}

/// `design`'s "does not close" message for a solve that succeeded, `None` when it
/// closes (or has no tiers).
fn closure_problem(design: &Design, solved: &[SolvedTier]) -> Option<String> {
    let (text, problem) = status_text_and_is_problem_from_solved(design, solved);
    problem.then_some(text)
}

/// Marks the design saved under `asc_name` after a save.
fn mark_saved(ctx: &Ctx, asc_name: String) {
    {
        let mut app = ctx.state.borrow_mut();
        if let Some(design) = app.design.as_mut() {
            design.session.mark_saved();
            design.asc_filename = Some(asc_name);
        }
    }
    if let Some(ui) = ctx.ui.upgrade() {
        push_design(&ui, &ctx.state.borrow());
    }
    schedule_save(ctx);
    // An action waiting behind the unsaved-changes dialog's Save can go on now.
    crate::editor::unsaved::design_saved(ctx);
}

/// File > Save `.asc`: the edited schedule as `.asc` text.
pub fn save_asc(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let solved = match solved {
            Ok(solved) => solved,
            Err(missing) => {
                show_message(
                    ctx,
                    MessageKind::Error,
                    &format!("Cannot export: {missing}."),
                );
                return;
            }
        };
        let built = {
            let app = ctx.state.borrow();
            let Some(state) = &app.design else {
                return;
            };
            let design = &state.session.design;
            let mut schedule =
                design.to_asc_schedule_from_solved_with(&solved, &app.custom_materials);
            let problem = closure_problem(design, &solved);
            if let Some(message) = &problem
                && let Some(header) = degenerate_marker_header(&schedule.headers, message)
            {
                schedule.headers.insert(0, header);
            }
            indicatrix_formats::asc::to_asc_string(&schedule)
                .map(|text| (text, state.save_asc_name(), problem))
                .map_err(|e| format!("Cannot write this schedule as .asc: {e}"))
        };
        match built {
            Ok((text, name, problem)) => {
                if deliver(ctx, &name, MIME_ASC, text.as_bytes()) {
                    match problem {
                        Some(message) => show_message(
                            ctx,
                            MessageKind::Warning,
                            &format!("Downloaded {name}, marked NOT A CLOSED SOLID: {message}"),
                        ),
                        None => {
                            show_message(ctx, MessageKind::Success, &format!("Downloaded {name}."));
                        }
                    }
                }
            }
            Err(message) => show_message(ctx, MessageKind::Error, &message),
        }
    });
}

/// `asc_name` with its extension replaced by `.gcs` (`round.asc` becomes `round.gcs`), or
/// `.gcs` appended when it has none -- the desktop's `file_name_for_format`.
fn gcs_file_name(asc_name: &str) -> String {
    std::path::Path::new(asc_name)
        .with_extension("gcs")
        .to_string_lossy()
        .into_owned()
}

/// File > Export `.gcs` (Experimental): the edited schedule as a Gem Cut Studio `.gcs` file.
///
/// The desktop's "Export as Gem Cut Studio (.gcs)": the SAME `AscSchedule` the `.asc` save
/// builds (from the solve already in hand), through the experimental
/// `indicatrix_formats::gcs::to_gcs_string`. `.gcs`, like `.asc`, has no concave record, so
/// the concave tiers ride along as footnotes (`Design::append_concave_footnotes`) and the
/// success message says so. A design that does not close cannot be written (the writer needs
/// a bounded stone) and the refusal is the writer's own sentence. Exporting is not saving: the
/// design is not marked saved.
pub fn save_gcs(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let solved = match solved {
            Ok(solved) => solved,
            Err(missing) => {
                show_message(
                    ctx,
                    MessageKind::Error,
                    &format!("Cannot export: {missing}."),
                );
                return;
            }
        };
        let built = {
            let app = ctx.state.borrow();
            let Some(state) = &app.design else {
                return;
            };
            let design = &state.session.design;
            let mut schedule =
                design.to_asc_schedule_from_solved_with(&solved, &app.custom_materials);
            design.append_concave_footnotes(&mut schedule);
            indicatrix_formats::gcs::to_gcs_string_with_frosting(
                &schedule,
                &design.gcs_frosting_spec(),
            )
            .map(|text| {
                (
                    text,
                    gcs_file_name(&state.save_asc_name()),
                    design.concave_tiers.len(),
                )
            })
            .map_err(|e| format!("Cannot write this schedule as .gcs: {e}"))
        };
        match built {
            Ok((text, name, concave)) => {
                if !deliver(ctx, &name, MIME_ASC, text.as_bytes()) {
                    return;
                }
                if concave == 0 {
                    show_message(ctx, MessageKind::Success, &format!("Downloaded {name}."));
                } else {
                    show_message(
                        ctx,
                        MessageKind::Warning,
                        &format!(
                            "Downloaded {name}. It has no concave tools: the {concave} concave \
                             tier(s) are written as footnotes only."
                        ),
                    );
                }
            }
            Err(message) => show_message(ctx, MessageKind::Error, &message),
        }
    });
}

/// File > Save design: one self-contained `.isdd` file (no `.asc`), the format
/// "Open" reads on its own. A design that does not solve is saved as a draft.
pub fn save_design(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let built = {
            let app = ctx.state.borrow();
            let Some(state) = &app.design else {
                return;
            };
            let design = &state.session.design;
            let problem = match &solved {
                Ok(solved) => closure_problem(design, solved),
                Err(missing) => Some(missing.clone()),
            };
            let draft = solved.is_err() && !design.tiers.is_empty();
            let history = state.session.history.description_log().to_vec();
            // The descriptive table and the attachments the file was opened with go
            // back as they were; only the stamp (modified time, a missing id) changes.
            let metadata = stamped_metadata(state);
            let file = design_to_file(
                design,
                state.printed_proportions.as_ref(),
                &DesignExtras {
                    custom_material: state.custom_snapshot(),
                    history_entries: &history,
                    metadata: Some(&metadata),
                    attachments: &state.attachments,
                },
            )
            .with_draft(draft);
            design_file::to_string(&file)
                .map(|text| (text, state.save_asc_name(), problem, draft, metadata))
        };
        let (text, asc_name, problem, draft, metadata) = match built {
            Ok(built) => built,
            Err(e) => {
                // Includes an attachment that is over the size cap or has a bad name.
                show_message(ctx, MessageKind::Error, &format!("Cannot save: {e}"));
                return;
            }
        };
        let file_name = design_file_name_for_asc(&asc_name);
        if !deliver(ctx, &file_name, MIME_DESIGN, text.as_bytes()) {
            return;
        }
        {
            // The stamped table is now the design's: the next save keeps its id.
            let mut app = ctx.state.borrow_mut();
            if let Some(state) = app.design.as_mut() {
                state.metadata = metadata;
            }
        }
        match problem {
            Some(message) if draft => show_message(
                ctx,
                MessageKind::Warning,
                &format!("Downloaded {file_name}, saved as a draft: {message}"),
            ),
            Some(message) => show_message(
                ctx,
                MessageKind::Warning,
                &format!("Downloaded {file_name}. The solid does not close: {message}"),
            ),
            None => show_message(
                ctx,
                MessageKind::Success,
                &format!("Downloaded {file_name}."),
            ),
        }
        mark_saved(ctx, asc_name);
    });
}

/// File > Download cutting sheet: the printable HTML sheet with its embedded
/// diagram.
pub fn download_cutting_sheet(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let solved = match solved {
            Ok(solved) => solved,
            Err(missing) => {
                show_message(
                    ctx,
                    MessageKind::Error,
                    &format!("Cannot build a cutting sheet: {missing}."),
                );
                return;
            }
        };
        let (html, name) = {
            let app = ctx.state.borrow();
            let Some(state) = &app.design else {
                return;
            };
            (
                cutting_sheet_document(&state.session.design, &solved, &app.custom_materials),
                cutting_sheet_file_name(&state.save_asc_name()),
            )
        };
        if deliver(ctx, &name, MIME_HTML, html.as_bytes()) {
            show_message(ctx, MessageKind::Success, &format!("Downloaded {name}."));
        }
    });
}

/// File > Download diagram PNG: the 2D diagram at print resolution.
pub fn download_diagram(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let solved = match solved {
            Ok(solved) => solved,
            Err(missing) => {
                show_message(
                    ctx,
                    MessageKind::Error,
                    &format!("Cannot draw this design: {missing}."),
                );
                return;
            }
        };
        let built = {
            let app = ctx.state.borrow();
            let Some(state) = &app.design else {
                return;
            };
            diagram_export_png(&state.session.design, &solved)
                .map(|png| (png, diagram_file_name(&state.save_asc_name())))
        };
        match built {
            Ok((png, name)) => {
                if deliver(ctx, &name, MIME_PNG, &png) {
                    show_message(ctx, MessageKind::Success, &format!("Downloaded {name}."));
                }
            }
            Err(message) => show_message(ctx, MessageKind::Error, &message),
        }
    });
}
