use crate::{LibraryModel, MainWindow, TiltModel, ViewportModel, gui::show_toast};
use slint::{ComponentHandle, Model, SharedString};
use std::fmt::Write as _;
use tracing::info;

pub fn copy_to_clipboard(text: &str) -> bool {
    info!("Copying text to clipboard ({} chars)", text.len());

    #[cfg(target_os = "linux")]
    {
        // Try xclip first
        if let Ok(mut child) = std::process::Command::new("xclip")
            .args(["-selection", "clipboard"])
            .stdin(std::process::Stdio::piped())
            .spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
            let _ = child.wait();
            return true;
        }

        // Try wl-copy (Wayland)
        if let Ok(mut child) = std::process::Command::new("wl-copy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
            let _ = child.wait();
            return true;
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(mut child) = std::process::Command::new("clip")
            .stdin(std::process::Stdio::piped())
            .spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
            let _ = child.wait();
            return true;
        }
    }

    #[cfg(target_os = "macos")]
    {
        if let Ok(mut child) = std::process::Command::new("pbcopy")
            .stdin(std::process::Stdio::piped())
            .spawn()
            && let Some(mut stdin) = child.stdin.take()
        {
            use std::io::Write;
            let _ = stdin.write_all(text.as_bytes());
            let _ = child.wait();
            return true;
        }
    }

    false
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
/// under clippy's function-length lint.
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
            copy_to_clipboard(&text);
            show_toast(&ui, "Optical metrics copied to clipboard!", "success");
        }
    });

    // Copy Performance Curve Data callback (19-Point Tilt Table, all 4 tilt axes) --
    // see `build_curve_data_csv`.
    let ui_weak_curve = ui.as_weak();
    ui.global::<TiltModel>().on_copy_curve_data(move || {
        if let Some(ui) = ui_weak_curve.upgrade() {
            let csv = build_curve_data_csv(&ui);
            copy_to_clipboard(&csv);
            show_toast(
                &ui,
                "181-Point performance curve data (4 axes, ±90°) copied to clipboard!",
                "success",
            );
        }
    });

    let ui_weak_copy = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_copy_text(move |text: SharedString| {
            if let Some(ui) = ui_weak_copy.upgrade() {
                copy_to_clipboard(&text);
                show_toast(&ui, "Copied to clipboard!", "success");
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
                }
            }
            copy_to_clipboard(&text);
            show_toast(&ui, "Cutting schedule copied as TSV!", "success");
        }
    });

    let ui_weak_row = ui.as_weak();
    ui.global::<LibraryModel>()
        .on_copy_cutting_row(move |idx: i32| {
            if let Some(ui) = ui_weak_row.upgrade() {
                let angles_model = ui.global::<LibraryModel>().get_current_angles();
                if let Some(row) = angles_model.row_data(idx as usize) {
                    let text = format!(
                        "Facet: {}, Angle: {}, Index: {}, Notes: {}",
                        row.facet, row.angle, row.index_val, row.notes
                    );
                    copy_to_clipboard(&text);
                    show_toast(&ui, &format!("Copied step #{}", idx + 1), "success");
                }
            }
        });
}
