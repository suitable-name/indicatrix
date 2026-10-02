//! The File menu's saves and downloads, each built with the same shared-crate
//! string API the desktop's save uses, then handed to [`download_bytes`]:
//!
//! | Action | Built with | File name |
//! |---|---|---|
//! | Save `.asc` | `Design::to_asc_schedule_from_solved_with` + `indicatrix_formats::asc::to_asc_string` (CRLF, `GemCAD`'s convention) | the design's `.asc` name |
//! | Save design | `indicatrix_cut_core::native::design_to_file` + `indicatrix_formats::native::design::to_string`; writes the opened file's `[meta]` table (stamped) and attachments back | `<name>.indicatrix` |
//! | Save `.asc` + older sidecar | `indicatrix_cut_core::native::save_paired_extended(_from_solved)`; the older sidecar format has no metadata or attachments | `<name>.asc` + `<name>.indicatrix.toml` |
//! | Cutting sheet | `indicatrix_editor::cut_sheet::cutting_sheet_document` | `<name>_cutting_sheet.html` |
//! | Diagram | `indicatrix_editor::cut_sheet::diagram_export_png` | `<name>_diagram.png` |
//!
//! Names follow the desktop (`indicatrix_editor::files`). A design that solves but
//! does not close is written with the desktop's "NOT A CLOSED SOLID" header (what
//! its "Export Anyway" confirmation writes) and a warning; one that does not solve
//! is saved as a draft design file (the desktop's "Save Anyway") and refused for plain
//! `.asc`, cutting sheet and diagram, exactly as the desktop refuses them.

use super::download::{MIME_ASC, MIME_DESIGN, MIME_HTML, MIME_PNG, MIME_TOML, download_bytes};
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
    native::{
        DesignExtras, SaveExtras, design_to_file, save_paired_extended,
        save_paired_extended_from_solved,
    },
};
use indicatrix_editor::{
    cut_sheet::{cutting_sheet_document, diagram_export_png},
    files::{
        cutting_sheet_file_name, degenerate_marker_header, diagram_file_name,
        native_file_name_for_asc,
    },
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
fn mark_saved(ctx: &Ctx, asc_name: String, asc_text: Option<String>) {
    {
        let mut app = ctx.state.borrow_mut();
        if let Some(design) = app.design.as_mut() {
            design.session.mark_saved();
            design.asc_filename = Some(asc_name);
            if asc_text.is_some() {
                design.original_asc_text = asc_text;
            }
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

/// File > Save `.asc` + older sidecar: the `.asc` and its older `.indicatrix.toml`
/// overlay, for use with a tool that still reads that pair (the `.asc` half
/// byte-identical to the opened file when the schedule did not change). The desktop
/// no longer writes this pair.
pub fn save_native_pair(ctx: &Ctx) {
    with_solved(ctx, |ctx, solved| {
        let built = {
            let app = ctx.state.borrow();
            let Some(state) = &app.design else {
                return;
            };
            let mut design = state.session.design.clone();
            let problem = match &solved {
                Ok(solved) => closure_problem(&design, solved),
                Err(missing) => Some(missing.clone()),
            };
            if let Some(message) = &problem
                && let Some(header) = degenerate_marker_header(&design.meta.headers, message)
            {
                design.meta.headers.insert(0, header);
            }
            let asc_name = state.save_asc_name();
            let history = state.session.history.description_log().to_vec();
            let extras = SaveExtras {
                custom_material: state.custom_snapshot(),
                history_entries: &history,
                custom_catalogue: &app.custom_materials,
            };
            let original = state.original_asc_text.as_deref();
            let printed = state.printed_proportions.as_ref();
            let paired = match &solved {
                Ok(solved) if !design.tiers.is_empty() => save_paired_extended_from_solved(
                    &design,
                    solved,
                    asc_name.clone(),
                    original,
                    None,
                    printed,
                    &extras,
                ),
                _ => save_paired_extended(
                    &design,
                    asc_name.clone(),
                    original,
                    None,
                    printed,
                    &extras,
                ),
            };
            paired.map(|paired| (paired, asc_name, problem))
        };
        let (paired, asc_name, problem) = match built {
            Ok(built) => built,
            Err(e) => {
                show_message(ctx, MessageKind::Error, &format!("Cannot save: {e}"));
                return;
            }
        };
        let native_name = native_file_name_for_asc(&asc_name);
        if !deliver(ctx, &asc_name, MIME_ASC, paired.asc_text.as_bytes())
            || !deliver(ctx, &native_name, MIME_TOML, paired.native_toml.as_bytes())
        {
            return;
        }
        let mut notes = Vec::new();
        if let Some(reason) = &paired.draft_reason {
            notes.push(format!("saved as a draft ({reason})"));
        } else if let Some(message) = &problem {
            notes.push(format!("marked NOT A CLOSED SOLID: {message}"));
        }
        if paired.asc_preserved {
            notes.push("the .asc is unchanged from the opened file".to_string());
        }
        let text = format!("Downloaded {asc_name} and {native_name}");
        if notes.is_empty() {
            show_message(ctx, MessageKind::Success, &format!("{text}."));
        } else {
            let kind = if paired.draft_reason.is_some() || problem.is_some() {
                MessageKind::Warning
            } else {
                MessageKind::Success
            };
            show_message(ctx, kind, &format!("{text}; {}.", notes.join("; ")));
        }
        mark_saved(ctx, asc_name, Some(paired.asc_text));
    });
}

/// File > Save design: one self-contained `.indicatrix` file (no `.asc`), the format
/// "Open" reads on its own. A design that does not solve is saved as a draft, as the
/// older pair save does.
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
        mark_saved(ctx, asc_name, None);
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
