//! "Export .asc" / "Export Cutting Sheet" / "Export Diagram": writes the edited
//! schedule (or a rendering of it) to a user-chosen path, file only -- the
//! catalogue stays read-only on every path in this module. See this group's own
//! `mod.rs` doc comment for why this is a distinct path from "Save Native".

use super::{
    confirm::{StatusDecision, ask_write_confirm, confirm_keys, decide_write_status},
    picker::{PickKind, pick_file, suggested_file_name},
    save_finish::record_last_saved_path,
    save_helpers::{
        degenerate_marker_header, snapshot_custom_materials, stamp_source_entry_footnote,
    },
    solve::resolve_solved_then,
};
use crate::{
    EditorModel, MainWindow,
    bridge::render_thread::RenderContext,
    gui::{editor::state::EditorState, show_toast},
};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, Mutex},
};

/// "Export Cutting Sheet": the printable HTML sheet.
///
/// Goes through [`resolve_solved_then`] (a matching cached solve, else a
/// background [`SolveService`] solve -- a design that does not solve has no
/// masts to print) and [`pick_file`] (the save-as picker on a background
/// thread), never solving inline on the UI thread or blocking on a native
/// save-file dialog; the HTML build and file write also run on a background
/// thread.
pub(in crate::gui::editor) fn setup_export_cutting_sheet_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_cutting_sheet(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("export_cutting_sheet", || {
            let (design, base_name) = {
                let st = state.borrow();
                (st.design.clone(), suggested_file_name(&st))
            };
            let render_ctx = Arc::clone(&render_ctx);
            resolve_solved_then(&ui, Arc::new(design), move |ui, design, solved_result| {
                crate::gui::editor::stall_guard::stall_guard("export_cutting_sheet_solved", || {
                    let solved = match solved_result {
                        Ok(solved) => solved,
                        Err(missing) => {
                            show_toast(
                                ui,
                                &format!("Cannot build a cutting sheet: {missing}."),
                                "error",
                            );
                            return;
                        }
                    };
                    // Custom-catalogue-aware: the printed "Refractive index" row
                    // must match a CUSTOM material's own `n_D`.
                    let custom_materials = snapshot_custom_materials(&render_ctx);
                    let base = base_name.trim_end_matches(".asc").to_string();
                    pick_file(
                        ui,
                        PickKind::SaveCuttingSheet {
                            default_name: format!("{base}_cutting_sheet.html"),
                        },
                        move |ui, dest_path| {
                            // A dismissed dialog is the cutter's own cancel -- no
                            // toast.
                            let Some(dest_path) = dest_path else {
                                return;
                            };
                            let ui_weak = ui.as_weak();
                            std::thread::spawn(move || {
                                let result =
                                    crate::gui::editor::cut_sheet::write_cutting_sheet_html(
                                        &design,
                                        &solved,
                                        &dest_path,
                                        &custom_materials,
                                    );
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                                    Ok(()) => show_toast(
                                        &ui,
                                        &format!("Wrote cutting sheet to {}", dest_path.display()),
                                        "success",
                                    ),
                                    Err(message) => show_toast(&ui, &message, "error"),
                                });
                            });
                        },
                    );
                });
            });
        });
    });
}

/// "Export Diagram": the 2D crown/pavilion/profile drawing as a PNG -- the same
/// render the cutting sheet embeds, written on its own.
///
/// Follows the same [`resolve_solved_then`] + [`pick_file`] pattern as
/// [`setup_export_cutting_sheet_callback`], never solving inline on the UI
/// thread or blocking on a native file dialog.
pub(in crate::gui::editor) fn setup_export_diagram_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
) {
    let state = Rc::clone(state);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_diagram(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("export_diagram", || {
            let (design, base_name) = {
                let st = state.borrow();
                (st.design.clone(), suggested_file_name(&st))
            };
            resolve_solved_then(&ui, Arc::new(design), move |ui, design, solved_result| {
                crate::gui::editor::stall_guard::stall_guard("export_diagram_solved", || {
                    let solved = match solved_result {
                        Ok(solved) => solved,
                        Err(missing) => {
                            show_toast(
                                ui,
                                &format!("Cannot draw this design: {missing}."),
                                "error",
                            );
                            return;
                        }
                    };
                    let base = base_name.trim_end_matches(".asc").to_string();
                    pick_file(
                        ui,
                        PickKind::SaveDiagram {
                            default_name: format!("{base}_diagram.png"),
                        },
                        move |ui, dest_path| {
                            let Some(dest_path) = dest_path else {
                                return;
                            };
                            let ui_weak = ui.as_weak();
                            std::thread::spawn(move || {
                                let result = crate::gui::editor::cut_sheet::write_diagram_png(
                                    &design, &solved, &dest_path,
                                );
                                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                                    Ok(()) => show_toast(
                                        &ui,
                                        &format!("Wrote diagram to {}", dest_path.display()),
                                        "success",
                                    ),
                                    Err(message) => show_toast(&ui, &message, "error"),
                                });
                            });
                        },
                    );
                });
            });
        });
    });
}

/// "Export .asc": writes the edited schedule (never the catalogue's own, unedited
/// one -- see this group's `mod.rs` doc comment) to a user-chosen path via
/// `indicatrix_formats::to_asc_string`. File only, no database write of any kind --
/// the catalogue stays read-only on this path.
///
/// The "not a closed solid" confirmation goes through [`ask_write_confirm`]
/// (in-window, never blocking) instead of a native (`rfd` crate) message dialog,
/// the save-as picker goes through [`pick_file`] (background thread), and the
/// final `std::fs::write` runs on a background thread. Building `schedule` itself
/// (`to_asc_schedule_with`, which solves internally) stays synchronous on the UI
/// thread, unlike the other write paths in this module
/// ([`confirm_status_before_write`], export cutting sheet, export diagram, the
/// autosave tick), which resolve their solve off-thread via
/// [`resolve_solved_then`]: doing the same here would need either a new
/// `SolveService` request kind that also builds an `AscSchedule` off-thread, or
/// duplicating `Design::to_asc_schedule_with`'s own schedule-building logic in
/// this crate.
pub(in crate::gui::editor) fn setup_export_asc_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>().on_export_asc(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        crate::gui::editor::stall_guard::stall_guard("export_asc", || {
            // Custom-catalogue-aware: a design on a CUSTOM catalogue material must
            // export that material's own `n_D`, not the legacy schedule RI
            // `to_asc_schedule` alone would fall back to.
            let custom_materials = snapshot_custom_materials(&render_ctx);
            // `to_asc_schedule_with` is fallible (it solves every tier's mast --
            // see `Design::to_asc_schedule`'s doc comment): a `MissingAnchor` here
            // means the same "add a Scale Reference tier" problem the validation
            // banner already reports, so it is surfaced the same way (a toast)
            // rather than exporting a schedule with fabricated masts.
            let (mut schedule, default_file_name, design, used_placeholder) = {
                let st = state.borrow();
                let mut schedule = match st.design.to_asc_schedule_with(&custom_materials) {
                    Ok(schedule) => schedule,
                    Err(missing) => {
                        show_toast(&ui, &format!("Cannot export: {missing}."), "error");
                        return;
                    }
                };
                // Recorded so a LATER re-import of this exact file can match it
                // back to its source row instead of creating a second,
                // same-titled duplicate -- see `stamp_source_entry_footnote`'s
                // own doc comment.
                stamp_source_entry_footnote(&mut schedule.footnotes, st.source_entry_id);
                (
                    schedule,
                    suggested_file_name(&st),
                    st.design.clone(),
                    st.used_placeholder,
                )
            };
            // The same marker Save Native writes, for the plain-export path --
            // a schedule whose masts were fabricated by the angle-table
            // reconstruction must say so in the file itself, not only in the app
            // that wrote it.
            if used_placeholder {
                indicatrix_formats::asc::mark_reconstructed(
                    &mut schedule,
                    "angle-table reconstruction, no attached .asc",
                );
            }
            // `to_asc_schedule_with` succeeding only means every tier
            // solved SOME mast, never that those masts actually close a real solid
            // -- see `decide_write_status`'s own doc comment. This path exports
            // plain `.asc` text only (`schedule`, already built above), never a
            // native sidecar.
            resolve_solved_then(&ui, Arc::new(design), move |ui, design, solved_result| {
                crate::gui::editor::stall_guard::stall_guard("export_asc_status_resolved", || {
                    match decide_write_status(
                        &design,
                        solved_result.as_deref().map_err(String::as_str),
                    ) {
                        StatusDecision::Fine => finish_export_asc(ui, &schedule, default_file_name),
                        StatusDecision::NeedsConfirm(message) => {
                            let heading_message = format!(
                                "{message}\n\nExport anyway? The written file will note this in \
                                 its own header."
                            );
                            ask_write_confirm(
                                ui,
                                "This design is not a closed solid",
                                heading_message,
                                "Export Anyway",
                                Some(confirm_keys::NOT_CLOSED_SOLID),
                                move |ui| {
                                    let mut schedule = schedule;
                                    if let Some(header) =
                                        degenerate_marker_header(&schedule.headers, &message)
                                    {
                                        schedule.headers.insert(0, header);
                                    }
                                    finish_export_asc(ui, &schedule, default_file_name);
                                },
                            );
                        }
                    }
                });
            });
        });
    });
}

/// [`setup_export_asc_callback`]'s tail once the status question is settled
/// (clean, or confirmed with `schedule.headers` already stamped): picks a
/// destination ([`pick_file`], background thread) and writes it (background
/// thread).
fn finish_export_asc(
    ui: &MainWindow,
    schedule: &indicatrix_formats::asc::AscSchedule,
    default_file_name: String,
) {
    let text = indicatrix_formats::asc::to_asc_string(schedule);
    pick_file(
        ui,
        PickKind::SaveAsc {
            default_name: default_file_name,
        },
        move |ui, dest_path| {
            // A dismissed file dialog is the cutter's own deliberate cancel -- no
            // toast needed to confirm an action they just performed, and showing
            // one here could silently replace an error toast still waiting to be
            // read (see `gui::mod`'s own auto-dismiss scheduling).
            let Some(dest_path) = dest_path else {
                return;
            };
            let ui_weak = ui.as_weak();
            std::thread::spawn(move || {
                if let Some(parent) = dest_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                let result = std::fs::write(&dest_path, &text)
                    .map_err(|e| format!("Failed to write {}: {e}", dest_path.display()));
                let _ = ui_weak.upgrade_in_event_loop(move |ui| match result {
                    Ok(()) => {
                        record_last_saved_path(&ui, &dest_path);
                        show_toast(
                            &ui,
                            &format!("Exported edited schedule to {}", dest_path.display()),
                            "success",
                        );
                    }
                    Err(message) => show_toast(&ui, &message, "error"),
                });
            });
        },
    );
}
