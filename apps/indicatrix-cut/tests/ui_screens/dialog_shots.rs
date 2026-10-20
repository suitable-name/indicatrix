//! The shots of the dialogs and overlays the user manual describes: New Design, the Gem Material
//! Editor (plain and with the zones of Ametrine), Preferences, the command palette and the
//! Remote Coordinator dialog.
//!
//! Each dialog is opened the way a user reaches it (the model flag the menu item sets) over a
//! design that is open, and the shot is cropped to the dialog's card.

use crate::{
    kit::{Rect, Sc, card_rect, find_inherits, rect_of},
    nav::{FVS, STUDIO},
    support::{ElementHandle, app::pump},
};
use indicatrix_cut::{
    CommandPaletteModel, EditorModel, PreferencesModel, RemoteAvailabilityItem, RemoteWorkerModel,
    TemplateGalleryModel, ViewportModel, WorkerItem,
};
use slint::{ComponentHandle, Model};
use uiprobe::screenshot::Shot;

/// The label of the zone list's check box, the first thing of the zone list the dialog shows.
const ZONE_CHECKBOX: &str = "Zone Geometry Relative to Stone Size";
/// The heading of the material dialog's zone list.
const ZONES_HEADING: &str = "Zones";
/// The heading right below the zone list.
const BODY_COLOR_HEADING: &str = "Gem Body Color & Transmission:";
/// Room above the zone heading in the cropped picture, in logical pixels.
const ZONES_MARGIN: f32 = 12.0;
/// The zone heading and its note above the check box, in logical pixels (used when the
/// heading text has no accessible label).
const ZONES_ABOVE_CHECKBOX: f32 = 52.0;
/// The first body color swatch, right below the zone list's heading of the next block.
const FIRST_SWATCH: &str = "Clear";
/// The body color heading above the swatches, in logical pixels.
const SWATCH_HEADING_ROOM: f32 = 30.0;
/// How far one wheel step scrolls a dialog, in logical pixels.
const WHEEL_STEP: f32 = 300.0;
/// What the Remote Coordinator dialog shows instead of a real machine.
const REMOTE_NAME: &str = "Render Box";
/// The address shown in the Remote Coordinator dialog.
const REMOTE_ADDRESS: &str = "render-box.local:7878";

/// Waits until the component `name` is on screen.
fn wait_component(sc: &Sc<'_, '_>, name: &str) -> Result<(), String> {
    if Sc::wait(15, || find_inherits(&sc.view(), name).is_some()) {
        pump(500);
        Ok(())
    } else {
        Err(format!("the dialog {name} did not open"))
    }
}

/// Captures the window and keeps the card of the dialog `name`.
fn card_shot(sc: &Sc<'_, '_>, name: &str) -> Result<Shot, String> {
    let shot = sc.snap_main()?;
    Ok(card_rect(&sc.view(), name)?.crop(&shot))
}

/// Scrolls the scroll area of the dialog `component` with the wheel until a control labelled
/// `label` is on screen (a control scrolled out of a `ScrollView` is not), then returns it.
fn scroll_to_label(sc: &Sc<'_, '_>, component: &str, label: &str) -> Result<ElementHandle, String> {
    for _ in 0..40 {
        if let Some(found) = sc.view().try_find(label) {
            return Ok(found);
        }
        let view = sc.view();
        let root = find_inherits(&view, component)
            .ok_or_else(|| format!("the dialog {component} is not on screen"))?;
        let area = root
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first();
        area.as_ref().unwrap_or(&root).scroll(0.0, -WHEEL_STEP);
        pump(150);
    }
    Err(format!("{label:?} did not come into view in {component}"))
}

/// Opens the Gem Material Editor over the library's Live Render view with `material` chosen.
fn open_material_editor(sc: &Sc<'_, '_>, material: &str) -> Result<(), String> {
    sc.select_design(STUDIO)?;
    let name = sc.choose_material(material)?;
    sc.show_live_view();
    let ui = sc.ui();
    let viewport = ui.global::<ViewportModel>();
    // The dialog fills its fields (and the zone list) when the selected material changes: make
    // sure the selection is the chosen one.
    let options: Vec<String> = viewport
        .get_material_options()
        .iter()
        .map(String::from)
        .collect();
    let index = options
        .iter()
        .position(|option| *option == name)
        .and_then(|index| i32::try_from(index).ok());
    if let Some(index) = index.filter(|index| viewport.get_selected_material_index() != *index) {
        viewport.set_selected_material_index(index);
    }
    pump(600);
    viewport.set_material_editor_open(true);
    wait_component(sc, "MaterialEditorDialog")?;
    // The library row's planes load in the background and, when they land, put the design's
    // own material back on the viewport, which can be after the pick above (the start-up Live
    // Render view keeps the event loop busy). Keep re-picking until the choice holds for a
    // while; the selection change refills the dialog's zone list.
    let mut held = 0;
    let _ = Sc::wait(10, || {
        pump(100);
        if viewport.get_traced_material_name().as_str() == name {
            held += 1;
        } else {
            held = 0;
            viewport.invoke_library_material_chosen(name.as_str().into());
            if let Some(index) = index {
                viewport.set_selected_material_index(index);
            }
        }
        held >= 15
    });
    pump(300);
    if viewport.get_traced_material_name().as_str() == name {
        Ok(())
    } else {
        Err(format!("the viewport did not keep the material {name:?}"))
    }
}

/// `new_design_dialog.png`: the New Design dialog over the Edit view.
///
/// # Errors
///
/// The design does not open, or the dialog does not appear.
pub fn new_design_dialog(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    let ui = sc.ui();
    ui.global::<EditorModel>().set_new_dialog_open(true);
    wait_component(sc, "NewDesignDialog")?;
    // The worker draws the cards' solid thumbnails after the dialog opens.
    let gallery = ui.global::<TemplateGalleryModel>();
    let drawn = Sc::wait(60, || {
        [gallery.get_shapes(), gallery.get_variants()]
            .iter()
            .all(|cards| cards.iter().all(|card| card.has_thumbnail))
    });
    if !drawn {
        sc.note("new design dialog: not every template thumbnail was drawn in 60 s".to_owned());
    }
    pump(300);
    card_shot(sc, "NewDesignDialog")
}

/// `material_editor.png`: the Gem Material Editor on a built-in material.
///
/// # Errors
///
/// The design or the material is missing, or the dialog does not appear.
pub fn material_editor(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    open_material_editor(sc, "Emerald")?;
    card_shot(sc, "MaterialEditorDialog")
}

/// `material_zones.png`: the material dialog with the Zones list of the built-in Ametrine.
///
/// # Errors
///
/// As [`material_editor`], or the zone list does not appear.
pub fn material_zones(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    open_material_editor(sc, "Ametrine")?;
    scroll_to_label(sc, "MaterialEditorDialog", ZONE_CHECKBOX)?;
    // Scroll on in small steps while the zone heading above the check box stays inside the
    // card, so the rows below it come into view too. A step that hides the check box is
    // taken back.
    let step = WHEEL_STEP / 6.0;
    for _ in 0..8 {
        let view = sc.view();
        let card = card_rect(&view, "MaterialEditorDialog")?;
        let Some(check) = view
            .try_find(ZONE_CHECKBOX)
            .map(|element| Rect::of(&element))
        else {
            break;
        };
        if check.y - ZONES_ABOVE_CHECKBOX - step < card.y + ZONES_MARGIN {
            break;
        }
        let Some(root) = find_inherits(&view, "MaterialEditorDialog") else {
            break;
        };
        let area = root
            .query_descendants()
            .match_inherits("ScrollView")
            .find_first();
        let target = area.as_ref().unwrap_or(&root);
        target.scroll(0.0, -step);
        pump(200);
        if sc.view().try_find(ZONE_CHECKBOX).is_none() {
            target.scroll(0.0, step);
            pump(200);
            break;
        }
    }
    // Only the Zones block: the dialog's name and optics fields above it are the editor's
    // new-material template, not Ametrine's values, and would mislabel the picture.
    let shot = sc.snap_main()?;
    let view = sc.view();
    let card = card_rect(&view, "MaterialEditorDialog")?;
    let root = find_inherits(&view, "MaterialEditorDialog")
        .ok_or("the dialog MaterialEditorDialog is not on screen")?;
    let labelled = |label: &'static str| {
        root.query_descendants()
            .match_predicate(move |element| {
                element
                    .accessible_label()
                    .is_some_and(|text| text.as_str() == label)
            })
            .find_first()
            .map(|element| Rect::of(&element))
    };
    let top = labelled(ZONES_HEADING)
        .map_or_else(
            || {
                view.try_find(ZONE_CHECKBOX)
                    .map(|element| Rect::of(&element))
                    .ok_or_else(|| format!("the zone list has no {ZONE_CHECKBOX:?} check box"))
                    .map(|check| check.y - ZONES_ABOVE_CHECKBOX)
            },
            |heading| Ok::<f32, String>(heading.y - ZONES_MARGIN),
        )?
        .max(card.y);
    let bottom = labelled(BODY_COLOR_HEADING)
        .map(|heading| heading.y)
        .or_else(|| labelled(FIRST_SWATCH).map(|swatch| swatch.y - SWATCH_HEADING_ROOM))
        .unwrap_or(card.y + card.h)
        .min(card.y + card.h);
    if bottom <= top {
        return Err("the zone list is not on screen".to_owned());
    }
    Ok(shot.crop_logical(card.x, top, card.w, bottom - top))
}

/// `preferences.png`: the Preferences dialog.
///
/// # Errors
///
/// The design is missing, or the dialog does not appear.
pub fn preferences(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    let ui = sc.ui();
    ui.global::<PreferencesModel>().set_dialog_open(true);
    wait_component(sc, "PreferencesDialog")?;
    card_shot(sc, "PreferencesDialog")
}

/// `command_palette.png`: the command palette with the query `export`.
///
/// # Errors
///
/// The design does not open, or the palette does not list the export commands.
pub fn command_palette(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.open_in_editor(FVS)?;
    let ui = sc.ui();
    let palette = ui.global::<CommandPaletteModel>();
    palette.invoke_open_palette();
    wait_component(sc, "CommandPalette")?;
    palette.set_query("export".into());
    // The query's change handler asks Rust to rank the commands again.
    pump(300);
    // The best match comes first; before the new ranking arrives the list holds the recent
    // commands.
    let listed = Sc::wait(15, || {
        palette
            .get_rows()
            .row_data(0)
            .is_some_and(|row| row.title.to_lowercase().contains("export"))
    });
    if !listed {
        return Err("the palette did not list the export commands".to_owned());
    }
    pump(500);
    card_shot(sc, "CommandPalette")
}

/// `remote_coordinator.png`: the Remote Coordinator dialog with an endpoint filled in. Only the
/// model is filled; nothing is saved and no connection is made.
///
/// # Errors
///
/// The dialog does not appear.
pub fn remote_coordinator(sc: &mut Sc<'_, '_>) -> Result<Shot, String> {
    sc.select_design(STUDIO)?;
    // A still page behind the popup: a live render would change pixels between the two
    // captures and widen the popup's box.
    sc.show_tab(1);
    let before = sc.snap_main()?;
    let ui = sc.ui();
    let remote = ui.global::<RemoteWorkerModel>();
    remote.invoke_open_dialog();
    remote.set_endpoint(WorkerItem {
        name: REMOTE_NAME.into(),
        address: REMOTE_ADDRESS.into(),
        cert_dir: "".into(),
        transfer_mode_index: 0,
        cadence_ms: 500,
        preview_scale_index: 3,
        preview_scale_percent: 50,
        export_transfer_index: 0,
    });
    remote.set_configured(true);
    remote.set_availability(RemoteAvailabilityItem {
        configured: true,
        reachable_render_capable: true,
    });
    pump(800);
    let after = sc.snap_main()?;
    let view = sc.view();
    let header = rect_of(&view, "HeaderToolbar")?;
    let window = Rect::of(&view.root);
    // Everything below the header: the header's own dimmed backdrop changes too.
    let top = header.y + header.h;
    let height = window.h - top;
    popup_in(
        &before.crop_logical(0.0, top, window.w, height),
        &after.crop_logical(0.0, top, window.w, height),
    )
}

/// The popup that opened between `before` and `after` (two crops of the same region). A
/// `PopupWindow`'s content is a separate item tree the element queries do not reach, so the
/// popup is found by the pixels it changed. Other parts of the window can change a few pixels
/// in the meantime (a scroll bar fading, a hover), so only the columns where at least half as
/// many pixels changed as in the busiest column count, and the popup is the rightmost run of
/// them (the header aligns it to the window's right edge); its rows are those that changed
/// inside that run.
fn popup_in(before: &Shot, after: &Shot) -> Result<Shot, String> {
    if before.width() != after.width() || before.height() != after.height() {
        return Err("the window changed size while the popup opened".to_owned());
    }
    let changed = |x: u32, y: u32| before.pixel(x, y) != after.pixel(x, y);
    let counts: Vec<u32> = (0..after.width())
        .map(|x| {
            let rows = (0..after.height()).filter(|&y| changed(x, y)).count();
            u32::try_from(rows).unwrap_or(u32::MAX)
        })
        .collect();
    let busiest = counts.iter().copied().max().unwrap_or(0);
    if busiest == 0 {
        return Err("the dialog RemoteWorkerDialog did not open".to_owned());
    }
    let busy = |x: u32| counts.get(x as usize).is_some_and(|&n| n * 2 >= busiest);
    let right = (0..after.width())
        .rev()
        .find(|&x| busy(x))
        .ok_or("the dialog RemoteWorkerDialog did not open")?;
    let left = (0..=right)
        .rev()
        .take_while(|&x| busy(x))
        .last()
        .unwrap_or(right);
    let rows: Vec<u32> = (0..after.height())
        .filter(|&y| (left..=right).any(|x| changed(x, y)))
        .collect();
    let (Some(&top), Some(&bottom)) = (rows.first(), rows.last()) else {
        return Err("the dialog RemoteWorkerDialog did not open".to_owned());
    };
    Ok(after.crop(left, top, right - left + 1, bottom - top + 1))
}
