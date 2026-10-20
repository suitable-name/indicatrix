//! Films only real controls and pointer inputs; never injects tutorial completion events.
use super::pure_tables::output_path;
use crate::{director::Film, pacing, targets};
use indicatrix_cut::{ExportModel, OpticalModel};
use slint::{ComponentHandle, LogicalPosition, platform::WindowEvent};
use std::time::Duration;
fn wait_map(film: &Film<'_>) -> Result<(), String> {
    if pacing::wait_real(Duration::from_secs(30), || {
        film.ui.global::<OpticalModel>().get_ready()
    }) {
        Ok(())
    } else {
        Err("Matching optical map did not arrive".into())
    }
}

/// Waits for the layer picture to differ from `before` once a ready map is showing. A layer
/// toggle no longer blanks the overlay (the old map stays ready and the controller republishes
/// within one tick), so readiness alone would return at once with the old picture.
fn wait_new_layer(film: &Film<'_>, before: &slint::Image) -> Result<(), String> {
    if pacing::wait_real(Duration::from_secs(30), || {
        let optical = film.ui.global::<OpticalModel>();
        optical.get_ready() && optical.get_image() != *before
    }) {
        Ok(())
    } else {
        Err("The optical layer did not change after the layer toggle".into())
    }
}
pub(super) fn drive(film: &mut Film<'_>, step: usize) -> Result<(), String> {
    match step {
        0 => film.film_command("view.live_render")?,
        1 => {
            film.press_label("Optical Diagnostics")?;
            film.press_label("Enable Optical Diagnostics")?;
            wait_map(film)?;
        }
        2 => {
            let before = film.ui.global::<OpticalModel>().get_image();
            film.press_label("Windowing (/)")?;
            film.press_label("Extinction (X)")?;
            wait_new_layer(film, &before)?;
            film.hold(pacing::RESULT_HOLD)?;
            let before = film.ui.global::<OpticalModel>().get_image();
            film.press_label("Extinction (X)")?;
            film.press_label("Brilliance (Dots)")?;
            wait_new_layer(film, &before)?;
            film.hold(pacing::RESULT_HOLD)?;
            film.press_label("Brilliance (Dots)")?;
            film.press_label("Fire (Dispersion Potential)")?;
            // The visible refresh clears readiness synchronously, so an older
            // base-only map cannot satisfy this Fire demonstration's wait.
            film.press_label("Refresh Diagnostic Map")?;
            wait_map(film)?;
            film.hold(pacing::RESULT_HOLD)?;
        }
        3 => {
            film.press_label("Optical Diagnostics")?;
            let element = film
                .view()
                .root
                .query_descendants()
                .match_inherits("GemViewportView")
                .find_first()
                .ok_or("Live viewport is missing")?;
            let spot = targets::Spot::of(&element);
            let start = (spot.w.mul_add(0.55, spot.x), spot.h.mul_add(0.48, spot.y));
            film.gesture(start, (start.0 + 50.0, start.1 + 15.0), None, false, false)?;
            wait_map(film)?;
            film.press_label("Optical Diagnostics")?;
            let point = (spot.w.mul_add(0.7, spot.x), spot.h.mul_add(0.55, spot.y));
            film.glide_to(point)?;
            film.ui.window().dispatch_event(WindowEvent::PointerMoved {
                position: LogicalPosition::new(point.0, point.1),
            });
            film.hold(pacing::RESULT_HOLD)?;
        }
        4 => {
            film.press_label("Optical Diagnostics")?;
            film.film_command("tools.export_render")?;
            film.press_label("Include Optical Diagnostics (4 Samples/Pixel)")?;
            film.press_label("Custom")?;
            for (label, value) in [("Width in Pixels", "128"), ("Height in Pixels", "128")] {
                let field = targets::text_input(&film.view(), label)
                    .ok_or_else(|| format!("{label} is missing"))?;
                film.click_and_type(&field, value)?;
            }
            let folder = output_path(&film.id, "optical");
            std::fs::create_dir_all(&folder).map_err(|e| e.to_string())?;
            if std::fs::read_dir(&folder)
                .map_err(|e| e.to_string())?
                .next()
                .is_some()
            {
                return Err("Tutorial output folder must be empty".into());
            }
            indicatrix_cut::harness_support::queue_picker_path(folder.clone());
            film.press_label("Start Export")?;
            let artifacts = || {
                std::fs::read_dir(&folder).is_ok_and(|files| {
                    let files: Vec<_> = files.filter_map(Result::ok).map(|e| e.path()).collect();
                    files
                        .iter()
                        .any(|p| p.extension().is_some_and(|s| s == "png"))
                        && files
                            .iter()
                            .any(|p| p.to_string_lossy().ends_with(".optical.txt"))
                })
            };
            if !pacing::wait_real(Duration::from_secs(120), || {
                artifacts() && !film.ui.global::<ExportModel>().get_is_exporting()
            }) {
                return Err("Diagnostic PNG and companion legend did not complete".into());
            }
            if indicatrix_cut::harness_support::pending_picker_answers() != 0 {
                return Err("Diagnostic export did not consume its folder picker answer".into());
            }
            if film.ui.global::<ExportModel>().get_has_error() {
                return Err("Diagnostic export reported an error".into());
            }
        }
        _ => return Err("Unknown optical tutorial step".into()),
    }
    Ok(())
}
