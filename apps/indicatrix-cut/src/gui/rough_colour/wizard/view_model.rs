//! Pushing the wizard's state into the Slint window. Models are replaced, but the text fields the
//! user types in are never touched here (replacing one while typing costs the keyboard focus).

use super::{
    checklist::{CHECKLIST, CHECKLIST_TITLE, report_lines},
    compare::{TextRow, chip_rows, lovo_rows, model_rows, warning_rows, zoned_prompt},
    host::Host,
    raster::{FLAG_COLOURS, Rgba},
    state::State,
    steps::{Step, blocker, reachable_all},
    zone_rows::{self, ITERATIONS_HINT, ShapeKind},
};
use crate::{
    CanvasMarker, ChipItem, ImportSlot, LegendRow, ParamEditRow, ReportRow, ViewRow, ZoneListRow,
};
use indicatrix::color::led::LedKind;
use slint::{Color, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, VecModel};
use std::fmt::Write as _;

/// A Slint image from a raster.
#[must_use]
pub fn to_image(raster: &Rgba) -> Image {
    if raster.is_empty() {
        return Image::default();
    }
    let mut buffer = SharedPixelBuffer::<Rgba8Pixel>::new(
        u32::try_from(raster.width).unwrap_or(1),
        u32::try_from(raster.height).unwrap_or(1),
    );
    buffer
        .make_mut_slice()
        .copy_from_slice(bytemuck::cast_slice(&raster.data));
    Image::from_rgba8(buffer)
}

fn strings(items: Vec<String>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

fn rows(items: Vec<TextRow>) -> ModelRc<ReportRow> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(|r| ReportRow {
                text: r.text.into(),
                warn: r.warn,
            })
            .collect::<Vec<_>>(),
    ))
}

fn plain(items: Vec<String>) -> Vec<TextRow> {
    items
        .into_iter()
        .map(|text| TextRow { text, warn: false })
        .collect()
}

fn to_i32(n: usize) -> i32 {
    i32::try_from(n).unwrap_or(i32::MAX)
}

/// The words for the camera and backlight lists.
pub const CAMERA_NAMES: [&str; 3] = [
    "sRGB fallback (least certain)",
    "Measured curves (CSV)",
    "Reference filter set",
];
/// The backlight choices.
pub const BACKLIGHT_NAMES: [&str; 3] = [
    "Find from the white frame",
    "A CIE LED kind",
    "Measured spectrum (CSV)",
];

/// Pushes everything.
pub fn push_all(host: &Host) {
    push_steps(host);
    push_rig(host);
    push_slots(host);
    push_calibration(host);
    push_views(host);
    push_canvas(host);
    push_masks(host);
    push_surfaces(host);
    push_zones(host);
    push_compare(host);
    push_accept(host);
}

pub fn push_steps(host: &Host) {
    let (facts, step) = {
        let state = host.state.borrow();
        (state.facts(), state.step)
    };
    let ok = reachable_all(&facts);
    let titles: Vec<String> = Step::ALL.iter().map(|s| s.title().to_owned()).collect();
    let next_hint = Step::ALL
        .get(step.index() + 1)
        .map_or_else(String::new, |next| {
            blocker(*next, &facts).map_or_else(|| format!("Next: {}.", next.title()), str::to_owned)
        });
    let w = &host.window;
    w.set_step(to_i32(step.index()));
    w.set_step_titles(strings(titles));
    w.set_step_ok(ModelRc::new(VecModel::from(ok.to_vec())));
    w.set_next_hint(next_hint.into());
}

pub fn push_rig(host: &Host) {
    let (names, index, align) = {
        let state = host.state.borrow();
        let align = if state.rig().is_none() {
            "No camera rig yet. Press Locate... to create one.".to_owned()
        } else if state.aligned() {
            "The mesh is aligned to this rig. Continue with the photos.".to_owned()
        } else if state.alignment.is_some() {
            "The locate window's alignment belongs to another rig or rough. Align the mesh to this rig there (Locate inclusion from photos, step 2).".to_owned()
        } else if state.context.is_none() {
            "The Rough Planner has no mesh rough.".to_owned()
        } else {
            "The mesh is not aligned to the rig yet. Open Locate..., load the stone photos, outline the stone in two views and press Align mesh to rig.".to_owned()
        };
        (
            crate::locate_io::rig_store::names(&state.rigs),
            state.rig_index.map_or(-1, to_i32),
            align,
        )
    };
    let w = &host.window;
    w.set_rig_names(strings(names));
    w.set_rig_index(index);
    w.set_align_text(align.into());
    w.set_checklist_title(CHECKLIST_TITLE.into());
    w.set_checklist(strings(CHECKLIST.iter().map(ToString::to_string).collect()));
}

pub fn push_slots(host: &Host) {
    let (slots, lines) = {
        let state = host.state.borrow();
        let file = |p: &std::path::Path| {
            p.file_name().map_or_else(
                || p.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            )
        };
        let slots: Vec<ImportSlot> = (0..state.view_count())
            .map(|v| {
                let files = &state.files[v];
                let status = match state.prepared.get(v).and_then(Option::as_ref) {
                    Some(p) => format!("prepared, {:.0} % usable", p.usable_fraction() * 100.0),
                    None if files.is_complete() => "ready to prepare".to_owned(),
                    None => String::new(),
                };
                ImportSlot {
                    name: state.view_name(v).into(),
                    stone: files.stone.as_deref().map(file).unwrap_or_default().into(),
                    white: match files.white.len() {
                        0 => String::new(),
                        1 => file(&files.white[0]),
                        n => format!("{n} frames"),
                    }
                    .into(),
                    dark: match files.dark.len() {
                        0 => String::new(),
                        1 => file(&files.dark[0]),
                        n => format!("{n} frames"),
                    }
                    .into(),
                    second: files.second.as_deref().map(file).unwrap_or_default().into(),
                    status: status.into(),
                    active: v == state.active_view,
                }
            })
            .collect();
        let mut lines = Vec::new();
        for p in state.prepared.iter().flatten() {
            for line in report_lines(&p.name, &p.consistency) {
                lines.push(TextRow {
                    text: line,
                    warn: true,
                });
            }
        }
        (slots, lines)
    };
    host.window.set_slots(ModelRc::new(VecModel::from(slots)));
    host.window.set_consistency_rows(rows(lines));
}

pub fn push_calibration(host: &Host) {
    let (camera_index, camera_file, backlight_index, backlight_file, led_index, lines) = {
        use super::work::{BacklightChoice, CameraChoice};
        let state = host.state.borrow();
        let (ci, cf) = match &state.camera {
            CameraChoice::Fallback => (0, String::new()),
            CameraChoice::Curves(p) => (1, p.display().to_string()),
            CameraChoice::Filters(p) => (2, p.display().to_string()),
        };
        let (bi, bf) = match &state.backlight {
            BacklightChoice::Auto => (0, String::new()),
            BacklightChoice::Led(_) => (1, String::new()),
            BacklightChoice::Csv(p) => (2, p.display().to_string()),
        };
        let led = LedKind::ALL
            .iter()
            .position(|k| *k == state.led)
            .unwrap_or(0);
        let notes = state
            .calibration
            .as_ref()
            .map(|c| plain(c.notes.clone()))
            .unwrap_or_default();
        (ci, cf, bi, bf, led, notes)
    };
    let w = &host.window;
    w.set_camera_names(strings(
        CAMERA_NAMES.iter().map(ToString::to_string).collect(),
    ));
    w.set_camera_index(camera_index);
    w.set_camera_file_text(camera_file.into());
    w.set_backlight_names(strings(
        BACKLIGHT_NAMES.iter().map(ToString::to_string).collect(),
    ));
    w.set_backlight_index(backlight_index);
    w.set_backlight_file_text(backlight_file.into());
    w.set_led_names(strings(
        LedKind::ALL.iter().map(|k| k.name().to_owned()).collect(),
    ));
    w.set_led_index(to_i32(led_index));
    w.set_calibration_rows(rows(lines));
}

pub fn push_views(host: &Host) {
    let views: Vec<ViewRow> = {
        let state = host.state.borrow();
        (0..state.view_count())
            .map(|v| ViewRow {
                name: state.view_name(v).into(),
                status: state
                    .prepared
                    .get(v)
                    .and_then(Option::as_ref)
                    .map_or_else(
                        || "not prepared".to_owned(),
                        |p| format!("{:.0} % usable", p.usable_fraction() * 100.0),
                    )
                    .into(),
                active: v == state.active_view,
            })
            .collect()
    };
    host.window
        .set_view_rows(ModelRc::new(VecModel::from(views)));
}

pub fn push_canvas(host: &Host) {
    let (image, size, markers) = {
        let mut state = host.state.borrow_mut();
        let image = state.canvas_image();
        let markers: Vec<CanvasMarker> = match (state.step, state.active_prepared()) {
            (Step::Fit, Some(p)) => {
                let (w, h, grid) = (p.grid.width as f64, p.grid.height as f64, p.grid);
                state
                    .marks
                    .views
                    .get(state.active_view)
                    .map(|points| {
                        points
                            .iter()
                            .enumerate()
                            .map(|(i, q)| CanvasMarker {
                                fx: (((q[0] - grid.origin[0]) / grid.scale) / w) as f32,
                                fy: (((q[1] - grid.origin[1]) / grid.scale) / h) as f32,
                                kind: 0,
                                label: (i + 1).to_string().into(),
                            })
                            .collect()
                    })
                    .unwrap_or_default()
            }
            _ => Vec::new(),
        };
        (
            image,
            state
                .active_prepared()
                .map(|p| (p.grid.width, p.grid.height)),
            markers,
        )
    };
    let w = &host.window;
    if let (Some(image), Some((width, height))) = (image, size) {
        w.set_photo(to_image(&image));
        w.set_image_w(to_i32(width));
        w.set_image_h(to_i32(height));
        w.set_has_photo(true);
    } else {
        w.set_photo(Image::default());
        w.set_image_w(1);
        w.set_image_h(1);
        w.set_has_photo(false);
    }
    w.set_markers(ModelRc::new(VecModel::from(markers)));
}

pub fn push_masks(host: &Host) {
    let text = {
        let state = host.state.borrow();
        state
            .active_prepared()
            .map_or_else(String::new, super::work::PreparedView::mask_note)
    };
    let legend: Vec<LegendRow> = FLAG_COLOURS
        .iter()
        .map(|(_, c, name)| LegendRow {
            name: (*name).into(),
            swatch: Color::from_rgb_u8(c[0], c[1], c[2]),
        })
        .collect();
    host.window.set_mask_text(text.into());
    host.window.set_legend(ModelRc::new(VecModel::from(legend)));
}

pub fn push_surfaces(host: &Host) {
    let text = {
        let state = host.state.borrow();
        format!(
            "{} faces are painted as polished windows.",
            state.windows.len()
        )
    };
    host.window.set_windows_text(text.into());
}

pub fn push_zones(host: &Host) {
    let (zone_list, params, suggestions, mark_text, fit_rows) = {
        let state = host.state.borrow();
        let geometry = state.geometry_or_empty();
        let list: Vec<ZoneListRow> = zone_rows::zone_rows(&geometry)
            .into_iter()
            .enumerate()
            .map(|(i, r)| ZoneListRow {
                title: r.title.into(),
                summary: r.summary.into(),
                selected: i == state.selected_zone && i > 0,
            })
            .collect();
        let params: Vec<ParamEditRow> =
            zone_rows::param_rows(&geometry, &state.locks, state.selected_zone)
                .into_iter()
                .map(|p| ParamEditRow {
                    name: p.name.into(),
                    value: p.value.into(),
                    locked: p.locked,
                })
                .collect();
        let suggestions: Vec<String> = state
            .suggestions
            .iter()
            .map(|s| {
                format!(
                    "{} (score {:.2}, {} views)",
                    zone_rows::shape_name(&s.shape),
                    s.score,
                    s.views_supporting
                )
            })
            .collect();
        let mark_text = match &state.pending_first {
            Some(_) => format!("First boundary kept. {}", state.marks.summary()),
            None => state.marks.summary(),
        };
        let fit_rows = state
            .fit
            .as_ref()
            .map(|f| {
                let mut r = vec![TextRow {
                    text: format!(
                        "Fitted {} zone(s); residual rms {:.2} sigma; {} model.",
                        f.fit.n_zones,
                        f.fit.residual_rms,
                        super::compare::model_name(f.fit.chosen_fit().kind)
                    ),
                    warn: false,
                }];
                r.extend(warning_rows(&f.fit));
                r
            })
            .unwrap_or_default();
        (list, params, suggestions, mark_text, fit_rows)
    };
    let w = &host.window;
    w.set_shape_names(strings(
        ShapeKind::ALL.iter().map(|k| k.name().to_owned()).collect(),
    ));
    w.set_zone_rows(ModelRc::new(VecModel::from(zone_list)));
    w.set_param_rows(ModelRc::new(VecModel::from(params)));
    w.set_suggestions(strings(suggestions));
    w.set_mark_text(mark_text.into());
    w.set_fit_rows(rows(fit_rows));
    w.set_iterations_hint(ITERATIONS_HINT.into());
    // The handles of the selected zone in the Rough Planner's 3D view follow the zones.
    crate::gui::rough_plan::redraw_rough_view();
}

pub fn push_compare(host: &Host) {
    let zoom = u32::try_from(host.window.get_compare_zoom()).unwrap_or(1);
    let (images, fit_texts) = {
        let state = host.state.borrow();
        let names: Vec<String> = (0..state.view_count())
            .map(|v| state.view_name(v))
            .collect();
        let texts = state.fit.as_ref().map(|f| {
            (
                f.fit
                    .lovo
                    .as_ref()
                    .map_or_else(Vec::new, |l| lovo_rows(l, &names)),
                model_rows(&f.fit.comparison),
                chip_rows(&f.fit),
                warning_rows(&f.fit),
                zoned_prompt(&f.fit).unwrap_or_default(),
            )
        });
        (state.compare_images(zoom), texts)
    };
    let w = &host.window;
    if let Some((photo, render, heat, stats)) = images {
        w.set_compare_photo(to_image(&photo));
        w.set_compare_render(to_image(&render));
        w.set_compare_heat(to_image(&heat));
        w.set_compare_stats(stats.into());
    } else {
        w.set_compare_photo(Image::default());
        w.set_compare_render(Image::default());
        w.set_compare_heat(Image::default());
        w.set_compare_stats("".into());
    }
    let (lovo, models, chips, warns, prompt) = fit_texts.unwrap_or_default();
    w.set_lovo_rows(rows(lovo));
    w.set_model_rows(rows(models));
    w.set_chips(ModelRc::new(VecModel::from(
        chips
            .into_iter()
            .map(|c| ChipItem {
                label: c.label.into(),
                swatch: Color::from_rgb_u8(c.srgb[0], c.srgb[1], c.srgb[2]),
                text: c.text.into(),
            })
            .collect::<Vec<_>>(),
    )));
    w.set_warning_rows(rows(warns));
    w.set_zoned_prompt(prompt.into());
}

pub fn push_accept(host: &Host) {
    let (text, can) = accept_state(&host.state.borrow());
    host.window.set_accept_text(text.into());
    host.window.set_can_accept(can);
}

/// The Accept step's sentence and whether Accept is possible.
#[must_use]
pub fn accept_state(state: &State) -> (String, bool) {
    let Some(fit) = &state.fit else {
        return ("Run the fit first.".to_owned(), false);
    };
    let plan = state.context.as_ref().and_then(|c| c.plan_id);
    let can = plan.is_some();
    let mut text = format!(
        "{} zone(s), residual rms {:.2} sigma",
        fit.fit.n_zones, fit.fit.residual_rms
    );
    if let Some(l) = &fit.fit.lovo {
        let _ = write!(
            text,
            ", held-out views off by {:.1} dE (median), {:.1} dE (worst)",
            l.median_delta_e, l.max_delta_e
        );
    }
    text.push('.');
    if can {
        text.push_str(" Accept stores the colour and the cached photos with the saved plan.");
    } else {
        text.push_str(" The plan is not saved yet: save it in the Rough Planner, then accept. You can still export the report.");
    }
    (text, can)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lists_have_the_choices_the_window_switches_on() {
        assert_eq!(CAMERA_NAMES.len(), 3);
        assert_eq!(BACKLIGHT_NAMES.len(), 3);
    }

    #[test]
    fn accept_needs_a_fit_and_a_saved_plan() {
        let state = State::new();
        assert_eq!(
            accept_state(&state),
            ("Run the fit first.".to_owned(), false)
        );
    }

    #[test]
    fn rasters_become_images_of_the_same_size() {
        let image = to_image(&Rgba::filled(3, 2, [1, 2, 3, 255]));
        assert_eq!(image.size().width, 3);
        assert_eq!(image.size().height, 2);
        assert_eq!(to_image(&Rgba::filled(0, 0, [0; 4])).size().width, 0);
    }
}
