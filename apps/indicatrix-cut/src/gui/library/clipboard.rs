use crate::{LibraryModel, MainWindow, TiltModel, ViewportModel, gui::show_toast};
use slint::{ComponentHandle, Model, SharedString};
use std::{
    fmt::Write as _,
    io::{self, Write as _},
    process::{Command, Stdio},
};
use tracing::{info, warn};

/// The platform's clipboard tools in the order they are tried: each entry is the
/// program and its fixed arguments. The text is piped into the tool's stdin.
#[cfg(target_os = "linux")]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] =
    &[("xclip", &["-selection", "clipboard"]), ("wl-copy", &[])];
/// See the Linux definition above.
#[cfg(target_os = "windows")]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[("clip", &[])];
/// See the Linux definition above.
#[cfg(target_os = "macos")]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[("pbcopy", &[])];
/// Platforms without a known clipboard tool: copying reports failure.
#[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
const CLIPBOARD_TOOLS: &[(&str, &[&str])] = &[];

/// `CREATE_NO_WINDOW`: keeps the console program `clip` from flashing a console window
/// in front of this GUI-subsystem process.
#[cfg(target_os = "windows")]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Builds the command for one [`CLIPBOARD_TOOLS`] entry.
fn clipboard_command(program: &str, args: &[&str]) -> Command {
    let mut command = Command::new(program);
    command.args(args);
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Runs `command` with `text` on its stdin and waits for it to exit.
///
/// The clipboard tools (`clip`, `xclip`, `wl-copy`, `pbcopy`) read stdin until EOF and
/// only then exit. `Child::wait` closes the child's stdin itself only while the pipe is
/// still stored in `child.stdin`; taking it out to write moves ownership here, so the
/// handle is dropped explicitly before waiting. Leaving it open makes `wait` block
/// forever.
///
/// The child's stdout and stderr are discarded so a chatty tool can never stall on a
/// full pipe.
///
/// # Errors
///
/// The spawn or write failure, or an error naming the exit status when the child exits
/// unsuccessfully.
fn pipe_text_to(mut command: Command, text: &str) -> io::Result<()> {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let written = child.stdin.take().map_or_else(
        || Err(io::Error::other("the child process has no stdin pipe")),
        |mut stdin| {
            let result = stdin.write_all(text.as_bytes());
            // Closing the pipe is what tells the tool the text is complete.
            drop(stdin);
            result
        },
    );
    let status = child.wait()?;
    written?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("exited with {status}")))
    }
}

/// Copies `text` to the system clipboard through the platform's clipboard tool and
/// returns whether one of them accepted it.
///
/// Blocks until the tool has exited, which takes milliseconds; callers on the UI
/// thread that want the outcome reported use [`copy_to_clipboard_with_toast`] instead,
/// which does the copy on a worker thread.
pub fn copy_to_clipboard(text: &str) -> bool {
    info!("Copying text to clipboard ({} bytes)", text.len());
    for (program, args) in CLIPBOARD_TOOLS {
        match pipe_text_to(clipboard_command(program, args), text) {
            Ok(()) => return true,
            Err(e) => warn!("Clipboard tool `{program}` failed: {e}"),
        }
    }
    false
}

/// Copies `text` to the clipboard on a worker thread and then toasts the outcome on the
/// UI thread: `success_message` when a clipboard tool accepted the text, an error
/// otherwise. Returns immediately, so a slow or wedged clipboard tool can never freeze
/// the window.
pub(in crate::gui) fn copy_to_clipboard_with_toast(
    ui: &MainWindow,
    text: String,
    success_message: String,
) {
    let ui_weak = ui.as_weak();
    std::thread::spawn(move || {
        let copied = copy_to_clipboard(&text);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            if copied {
                show_toast(&ui, &success_message, "success");
            } else {
                show_toast(
                    &ui,
                    "Could not copy to the clipboard: no clipboard tool is available.",
                    "error",
                );
            }
        });
    });
}

/// Axis labels for the four full-axis (±90°) rows [`build_curve_data_csv`] appends --
/// same order and text as `gui::tilt_profile`'s `PROFILE_AZIMUTHS_DEG` and
/// `performance_graph_dialog.slint`'s `axis_labels`. Axis 0 (the canonical azimuth) is
/// included here too: `graph_*_extra_axes` holds all four axes' full-range sweeps, not
/// just the three non-canonical ones -- see that property's doc comment in
/// `performance_graph_dialog.slint`.
const FULL_AXIS_LABELS: [&str; 4] = ["0 (length)", "45", "90 (width)", "135"];

/// Number of samples per full-axis row: -90..=+90 in exact 1° steps -- matches
/// `indicatrix::color::metrics::TILT_ANGLES_DEG`.
const FULL_AXIS_SAMPLE_COUNT: usize = 181;

/// Builds the "Copy 181-Point Data Table (4 Axes, ±90°)" CSV: one row per 1°-step
/// sample, for all four `PROFILE_AZIMUTHS_DEG` axes across their full `-90..=+90°`
/// range, once `gui::tilt_profile`'s background sweep has landed (it's lazy -- see
/// that module's doc comment; ~1.36s for all four axes together, so the table is
/// genuinely not ready immediately after opening the dialog). Split out of
/// [`setup_copy_callbacks`] purely to keep that function under clippy's
/// function-length lint. This is the measured-numbers escape hatch the dialog's
/// honesty caption points to: unlike the chart and hover tooltip, every row here is
/// one of the 181 actually-raytraced 1°-step samples, not an interpolation.
///
/// Falls back to the always-available canonical (azimuth-0) 19-point/5°-step/
/// `0..=90°` sweep -- the same one the render thread computes every frame,
/// `graph_brilliance`/etc -- if the full-axis sweep hasn't landed yet, rather than
/// exporting nothing: the button should never produce an empty table just because the
/// dialog was opened a moment ago.
fn build_curve_data_csv(ui: &MainWindow) -> String {
    let mut csv = String::from("Tilt Angle (°),Axis,Brilliance (%),Windowing (%),Extinction (%)\n");

    let extra_brilliance = ui.global::<TiltModel>().get_graph_brilliance_extra_axes();
    let extra_extinction = ui.global::<TiltModel>().get_graph_extinction_extra_axes();
    let extra_windowing = ui.global::<TiltModel>().get_graph_windowing_extra_axes();
    if extra_brilliance.row_count() >= FULL_AXIS_LABELS.len()
        && extra_extinction.row_count() >= FULL_AXIS_LABELS.len()
        && extra_windowing.row_count() >= FULL_AXIS_LABELS.len()
    {
        for (axis_idx, label) in FULL_AXIS_LABELS.iter().enumerate() {
            let Some(axis_b) = extra_brilliance.row_data(axis_idx) else {
                continue;
            };
            let Some(axis_w) = extra_windowing.row_data(axis_idx) else {
                continue;
            };
            let Some(axis_e) = extra_extinction.row_data(axis_idx) else {
                continue;
            };
            for i in 0..FULL_AXIS_SAMPLE_COUNT {
                let angle = i as i32 - 90; // TILT_ANGLES_DEG[i]
                let b = axis_b.row_data(i).unwrap_or(0.0);
                let w = axis_w.row_data(i).unwrap_or(0.0);
                let e = axis_e.row_data(i).unwrap_or(0.0);
                let _ = writeln!(csv, "{angle},{label},{b:.1},{w:.1},{e:.1}");
            }
        }
        return csv;
    }

    // Fallback: the full-axis sweep hasn't landed yet -- export the always-available
    // canonical positive-half-only sweep instead of an empty table.
    let gb = ui.global::<TiltModel>().get_graph_brilliance();
    let ge = ui.global::<TiltModel>().get_graph_extinction();
    let gw = ui.global::<TiltModel>().get_graph_windowing();
    for i in 0..19 {
        let angle = i * 5;
        let b = gb.row_data(i).unwrap_or(0.0);
        let w = gw.row_data(i).unwrap_or(0.0);
        let e = ge.row_data(i).unwrap_or(0.0);
        let _ = writeln!(csv, "{angle},0 (length),{b:.1},{w:.1},{e:.1}");
    }
    csv
}

/// Wires up the clipboard-copy callbacks (metrics, curve data, generic text, cutting
/// table, single cutting row). Split out of `run_gui` purely to keep that function
/// under clippy's function-length lint. Every copy runs on a worker thread and toasts
/// its own outcome -- see [`copy_to_clipboard_with_toast`].
pub(in crate::gui) fn setup_copy_callbacks(ui: &MainWindow) {
    let ui_weak_metrics = ui.as_weak();
    ui.global::<ViewportModel>().on_copy_metrics(move || {
        if let Some(ui) = ui_weak_metrics.upgrade() {
            let b = ui.global::<ViewportModel>().get_brilliance_pct();
            let f = ui.global::<ViewportModel>().get_fire_index();
            let w = ui.global::<ViewportModel>().get_windowing_pct();
            let text = format!(
                "Optical Metrics:\n• Brilliance: {b:.1}%\n• Fire Index: {f:.2}\n• Windowing: {w:.1}%"
            );
            copy_to_clipboard_with_toast(
                &ui,
                text,
                "Optical metrics copied to clipboard!".to_owned(),
            );
        }
    });

    // Copy Performance Curve Data callback (19-Point Tilt Table, all 4 tilt axes) --
    // see `build_curve_data_csv`.
    let ui_weak_curve = ui.as_weak();
    ui.global::<TiltModel>().on_copy_curve_data(move || {
        if let Some(ui) = ui_weak_curve.upgrade() {
            let csv = build_curve_data_csv(&ui);
            copy_to_clipboard_with_toast(
                &ui,
                csv,
                "181-Point performance curve data (4 axes, ±90°) copied to clipboard!".to_owned(),
            );
        }
    });

    let ui_weak_copy = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_copy_text(move |text: SharedString| {
            if let Some(ui) = ui_weak_copy.upgrade() {
                copy_to_clipboard_with_toast(
                    &ui,
                    text.to_string(),
                    "Copied to clipboard!".to_owned(),
                );
            }
        });

    let ui_weak_sched = ui.as_weak();
    ui.global::<LibraryModel>().on_copy_cutting_table(move || {
        if let Some(ui) = ui_weak_sched.upgrade() {
            let angles_model = ui.global::<LibraryModel>().get_current_angles();
            let mut text = String::from("#\tFacet\tAngle\tIndex\tNotes\n");
            for i in 0..angles_model.row_count() {
                if let Some(row) = angles_model.row_data(i) {
                    let _ = writeln!(
                        text,
                        "{}\t{}\t{}\t{}\t{}",
                        row.order_idx + 1,
                        row.facet,
                        row.angle,
                        row.index_val,
                        row.notes
                    );
                    // A concave tier's tool line follows its facet line, as on the
                    // cutting sheet, so a pasted table does not silently lose it.
                    if !row.second_line.is_empty() {
                        let _ = writeln!(text, "\t{}", row.second_line);
                    }
                }
            }
            copy_to_clipboard_with_toast(
                &ui,
                text,
                "Cutting instructions copied as TSV!".to_owned(),
            );
        }
    });

    let ui_weak_row = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_copy_cutting_row(move |idx: i32| {
            if let Some(ui) = ui_weak_row.upgrade() {
                let angles_model = ui.global::<LibraryModel>().get_current_angles();
                if let Some(row) = angles_model.row_data(idx as usize) {
                    let mut text = format!(
                        "Facet: {}, Angle: {}, Index: {}, Notes: {}",
                        row.facet, row.angle, row.index_val, row.notes
                    );
                    if !row.second_line.is_empty() {
                        let _ = write!(text, "\nTool: {}", row.second_line);
                    }
                    copy_to_clipboard_with_toast(&ui, text, format!("Copied step #{}", idx + 1));
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{Command, pipe_text_to};
    use std::{sync::mpsc, time::Duration};

    /// A program that reads stdin until EOF and then exits: the behaviour the
    /// clipboard tools share, and the one that hung the copy buttons.
    #[cfg(target_os = "windows")]
    fn read_until_eof_command() -> Command {
        let mut command = Command::new("cmd");
        command.args(["/C", "more"]);
        command
    }

    /// See the Windows definition above.
    #[cfg(not(target_os = "windows"))]
    fn read_until_eof_command() -> Command {
        Command::new("cat")
    }

    /// Runs `pipe_text_to` on a helper thread so a regression shows up as a failed
    /// assertion after the timeout instead of hanging the test run.
    fn pipe_with_timeout(text: String) -> std::io::Result<()> {
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(pipe_text_to(read_until_eof_command(), &text));
        });
        receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("pipe_text_to did not return: the child's stdin was never closed")
    }

    #[test]
    fn piping_to_a_read_until_eof_child_completes() {
        pipe_with_timeout("clipboard text\n".to_owned()).expect("the child must exit cleanly");
    }

    /// More than a pipe buffer's worth of text, so the write itself only finishes
    /// because the child drains its stdin while the parent is still writing.
    #[test]
    fn piping_more_than_a_pipe_buffer_completes() {
        pipe_with_timeout("clipboard text\n".repeat(8_000)).expect("the child must exit cleanly");
    }

    #[test]
    fn a_missing_program_is_an_error_not_a_hang() {
        let result = pipe_text_to(Command::new("indicatrix-no-such-clipboard-tool"), "text");
        assert!(result.is_err());
    }
}
