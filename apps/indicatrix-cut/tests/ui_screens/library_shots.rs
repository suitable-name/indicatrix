//! The shots of the library side: the main screen, Live Render in Solid shading, the cutting
//! instructions, the three Parameters tabs, the Tilt Performance and Export dialogs, and the
//! "Build This Design" lesson.

use crate::{
    kit::{Rect, Sc, card_rect, find_inherits, hover_over, rect_of},
    nav::{FVS, STUDIO},
    support::app::pump,
};
use indicatrix_cut::{
    BuildDesignModel, ExportModel, GuideModel, LibraryModel, ParametersModel, SolidPreviewModel,
    TiltModel, ViewportModel,
};
use slint::{ComponentHandle, Model};
use slint_harness::{AccessibleRole, screenshot::Shot};

/// What the Export dialog's Output Location shows instead of the real folder.
const NEUTRAL_FOLDER: &str = "D:\\Renders";
/// Library list row labels of the card menu, top to bottom.
const CARD_MENU_ITEMS: [&str; 6] = [
    "Generate Previews",
    "Compute Tilt Curves",
    "Ignore",
    "Exclude from Planner",
    "Build This Design",
    "Add Tag",
];

/// `main.png`: the library with the design selected, 3D Spectral Preview, Path-Traced, settled.
///
/// # Errors
///
/// The design is missing or the capture fails.
pub fn main_window(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.studio_look();
    sc.show_live_view();
    sc.wait_live();
    sc.snap_main()
}

/// `solid_rendering.png`: the Live Render panel in Solid shading.
///
/// # Errors
///
/// The design is missing or the capture fails.
pub fn solid_rendering(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.studio_look();
    sc.show_live_view();
    let ui = sc.ui();
    ui.global::<ViewportModel>().set_live_view_mode(0);
    pump(500);
    let solid = ui.global::<SolidPreviewModel>();
    let _ = Sc::wait(60, || solid.get_has_solid());
    // Framed again for the Solid picture, which has its own shape, then wait for the
    // raster drawn at the viewport's full pixel size (an older, smaller one is shown until
    // it arrives).
    sc.fit_camera();
    let scale = ui.window().scale_factor();
    let drawn = Sc::wait(30, || {
        let wanted = (solid.get_live_viewport_width() * scale).round();
        let size = solid.get_image().size();
        size.width > 0 && (f64::from(size.width) - f64::from(wanted)).abs() <= 2.0
    });
    if !drawn {
        sc.note("solid rendering: the full-size solid picture did not arrive in 30 s");
    }
    pump(1500);
    let shot = sc.snap_main()?;
    let panel = rect_of(&sc.view(), "GemViewportView")?;
    Ok(panel.crop(&shot))
}

/// `cutting_instructions.png`: the design header, the tabs and the table, All Steps.
///
/// # Errors
///
/// The design is missing or the capture fails.
pub fn cutting_instructions(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.show_tab(1);
    let ui = sc.ui();
    let library = ui.global::<LibraryModel>();
    let loaded = Sc::wait(30, || library.get_current_angles().row_count() > 0);
    if !loaded {
        return Err("the cutting instructions did not load".to_owned());
    }
    pump(800);
    let shot = sc.snap_main()?;
    let view = sc.view();
    let area = rect_of(&view, "DetailHeaderView")?.union(rect_of(&view, "CuttingTableView")?);
    Ok(area.crop(&shot))
}

/// Opens the Tilt Performance dialog and waits for all four axes.
fn open_tilt(sc: &Sc<'_, '_>) -> Result<(), String> {
    let ui = sc.ui();
    let tilt = ui.global::<TiltModel>();
    if !tilt.get_dialog_open() {
        tilt.invoke_toggle_dialog();
    }
    pump(500);
    if !tilt.get_dialog_open() {
        return Err("the Tilt Performance dialog did not open".to_owned());
    }
    let ready = Sc::wait(300, || {
        tilt.get_graph_brilliance_extra_axes().row_count() >= 4 && !tilt.get_extra_axes_loading()
    });
    if !ready {
        return Err("the tilt curves were not computed within 300 s".to_owned());
    }
    pump(1500);
    Ok(())
}

/// `tilt_performance.png`: the dialog for a library design in Spinel, curve computed.
///
/// # Errors
///
/// The design or the material is missing, or the curve is not computed.
pub fn tilt_performance(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.choose_material("Spinel")?;
    sc.show_live_view();
    open_tilt(sc)?;
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "PerformanceGraphDialog")?.crop(&shot))
}

/// `tilt_performance_export.png`: the same dialog with Export Tilt Video unfolded.
///
/// # Errors
///
/// As [`tilt_performance`], or the section is not found.
pub fn tilt_performance_export(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.choose_material("Spinel")?;
    sc.show_live_view();
    open_tilt(sc)?;
    let section = sc
        .view()
        .try_find("Export Tilt Video")
        .ok_or_else(|| "the Export Tilt Video section is not in the dialog".to_owned())?;
    section.invoke_accessible_default_action();
    pump(800);
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "PerformanceGraphDialog")?.crop(&shot))
}

/// `tilt_performance_cad.png`: the dialog opened from the Edit view's Diagram view. The dialog
/// is an overlay of the main window, so the window is captured whole.
///
/// # Errors
///
/// The design does not open in the editor, or the curve is not computed.
pub fn tilt_performance_cad(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    sc.set_view_mode(3);
    sc.set_inspector_tab(3);
    open_tilt(sc)?;
    sc.snap_main()
}

/// `emerald_export_settings_4k.png`: the Export dialog, 4K, Rec.2020, Emerald, Local Only.
///
/// # Errors
///
/// The design or the material is missing, or the dialog does not open.
pub fn emerald_export_settings_4k(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(FVS)?;
    sc.choose_material("Emerald")?;
    sc.show_live_view();
    sc.wait_live();
    let ui = sc.ui();
    let export = ui.global::<ExportModel>();
    export.set_is_open(true);
    pump(1000);
    if !export.get_is_open() {
        return Err("the Export dialog did not open".to_owned());
    }
    // The dialog seeds its choices from the settings file when it opens; these are set after.
    export.set_selected_size_preset(1);
    export.set_selected_color_space_index(2);
    export.set_compute_target_index(0);
    export.set_directory(NEUTRAL_FOLDER.into());
    pump(1000);
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), "ExportDialog")?.crop(&shot))
}

/// One Parameters dock tab, cropped to the dock.
fn parameters(sc: &mut Sc<'_, '_>, tab: i32) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    sc.show_live_view();
    let ui = sc.ui();
    ui.global::<ParametersModel>().invoke_show_tab(tab);
    pump(1500);
    let shot = sc.snap_main()?;
    let view = sc.view();
    let dock = rect_of(&view, "ParametersDock").or_else(|_| rect_of(&view, "ParametersPanel"))?;
    Ok(dock.crop(&shot))
}

/// `parameters_stone.png`: the Stone tab.
///
/// # Errors
///
/// The design is missing, the dock is not on screen, or the capture fails.
pub fn parameters_stone(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    parameters(sc, 0)
}

/// `parameters_lighting.png`: the Lighting & Environment tab.
///
/// # Errors
///
/// As [`parameters_stone`].
pub fn parameters_lighting(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    parameters(sc, 1)
}

/// `parameters_render.png`: the Render & Output tab.
///
/// # Errors
///
/// As [`parameters_stone`].
pub fn parameters_render(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    parameters(sc, 2)
}

/// `tutorial_from_design.png`: the card menu of the design with Build This Design hovered.
///
/// # Errors
///
/// The design or its card is missing, or the menu does not open.
pub fn tutorial_from_design(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    let design = sc.select_design(FVS)?;
    sc.show_live_view();
    // Live Render at this window width is in the band where the library folds onto its rail for
    // the Parameters dock; the card menu needs the list open, as a user opens it from the rail.
    let library = sc.ctx.app.ui().global::<LibraryModel>();
    library.set_panel_collapsed(false);
    pump(300);
    let find_card = |sc: &Sc<'_, '_>| {
        sc.view().elements().into_iter().find(|element| {
            element.accessible_role() == Some(AccessibleRole::ListItem)
                && element
                    .accessible_label()
                    .is_some_and(|label| label.starts_with(design.title.as_str()))
        })
    };
    let _ = Sc::wait(10, || find_card(&*sc).is_some());
    // The card sits further down the first page than the list shows: scroll the list towards it
    // with the wheel, a step at a time, as a user does.
    for _ in 0..80 {
        if find_card(&*sc).is_some() {
            break;
        }
        let view = sc.view();
        let Some(list) = find_inherits(&view, "DiagramListView") else {
            break;
        };
        let area = ["ScrollView", "ListView", "Flickable"]
            .iter()
            .flat_map(|&name| list.query_descendants().match_inherits(name).find_all())
            .max_by(|a, b| {
                let (sa, sb) = (a.size(), b.size());
                (sa.width * sa.height).total_cmp(&(sb.width * sb.height))
            });
        area.as_ref().unwrap_or(&list).scroll(0.0, -400.0);
        pump(150);
    }
    let card = find_card(&*sc).ok_or_else(|| {
        format!(
            "the card of {:?} is not in the visible list (library folded: {})",
            design.title,
            library.get_panel_collapsed()
        )
    })?;
    // Up to the top of the list, so the menu that opens below the card is not clipped by the
    // list's bottom edge.
    let view = sc.view();
    if let Some(list) = find_inherits(&view, "DiagramListView") {
        // The scroll area's own top: the list component also holds the catalog header above it.
        let area = list
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first()
            .unwrap_or(list);
        let below = card.absolute_position().y - area.absolute_position().y - 8.0;
        if below > 0.0 {
            area.scroll(0.0, -below);
            pump(300);
        }
    }
    let card = find_card(&*sc).ok_or_else(|| {
        format!(
            "the card of {:?} left the visible list while scrolling it up",
            design.title
        )
    })?;
    // The three-dots button exists only while the pointer is over the card.
    let window = sc.ctx.app.ui().window();
    hover_over(window, &card);
    let kebab = sc
        .view()
        .try_find(&format!("More Actions for {}", design.title))
        .ok_or_else(|| "the card shows no more-actions button".to_owned())?;
    kebab.invoke_accessible_default_action();
    pump(500);
    let card_area = Rect::of(&card);
    let view = sc.view();
    let mut area = card_area;
    let mut build = None;
    for label in CARD_MENU_ITEMS {
        for item in view.by_label(label) {
            let rect = Rect::of(&item);
            let beside = rect.x >= card_area.x - 40.0
                && rect.right() <= card_area.right() + 40.0
                && rect.y >= card_area.y - 4.0
                && rect.y <= card_area.bottom() + 300.0;
            if beside {
                area = area.union(rect);
                if label == "Build This Design" {
                    build = Some(item);
                }
            }
        }
    }
    let build = build.ok_or_else(|| "the card menu does not list Build This Design".to_owned())?;
    hover_over(sc.ctx.app.ui().window(), &build);
    let shot = sc.snap_main()?;
    Ok(area.grow(8.0).crop(&shot))
}

/// Starts the lesson for the design and waits for the guide to open.
fn start_lesson(sc: &mut Sc<'_, '_>) -> Result<(), String> {
    let design = sc.select_design(FVS)?;
    sc.show_live_view();
    let ui = sc.ui();
    ui.global::<BuildDesignModel>().invoke_start(design.id);
    let guide = ui.global::<GuideModel>();
    let opened = Sc::wait(180, || {
        guide.get_open() && guide.get_guide_id().starts_with("build:")
    });
    if !opened {
        return Err(format!("the lesson for {:?} did not open", design.title));
    }
    pump(1500);
    Ok(())
}

/// `tutorial_from_design_2.png`: the guide panel at step 1.
///
/// # Errors
///
/// The lesson does not open.
pub fn tutorial_from_design_2(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    start_lesson(sc)?;
    let shot = sc.snap_main()?;
    Ok(rect_of(&sc.view(), "GuidePanel")?.crop(&shot))
}

/// `tutorial_from_design_3.png`: the guide panel at step 2 (Add G1) with the tier table beside it.
///
/// # Errors
///
/// The lesson does not open or does not reach its second step.
pub fn tutorial_from_design_3(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    start_lesson(sc)?;
    let ui = sc.ui();
    let guide = ui.global::<GuideModel>();
    if guide.get_current_is_manual() {
        guide.invoke_next();
    } else {
        guide.invoke_go_on();
    }
    let _ = Sc::wait(90, || guide.get_step_index() >= 1 || guide.get_step_done());
    if guide.get_step_index() < 1 && guide.get_step_done() {
        guide.invoke_next();
    }
    let second = Sc::wait(60, || guide.get_step_index() >= 1);
    if !second {
        return Err("the lesson did not reach step 2".to_owned());
    }
    pump(1500);
    let shot = sc.snap_main()?;
    let view = sc.view();
    let panel = rect_of(&view, "GuidePanel")?;
    let area =
        find_inherits(&view, "EditorView").map_or(panel, |editor| panel.union(Rect::of(&editor)));
    Ok(area.crop(&shot))
}
