//! Rough colour in the Rough Planner's results: the coloured previews, the
//! "Stone orientation" option and "Use Color" (adopt).
//!
//! The planner's host, session and results are private to `gui::rough_plan`, so this thin file
//! reads them and calls `gui::rough_colour`, where the logic and its tests live
//! (`rough_colour::preview`, `rough_colour::adopt_link`, `rough_colour::store`). The view's own
//! reach (scene colours, the zone handles, the brush) is `view::zoning_view`.
//!
//! "Use Color" and "Open in Editor" (`editor_link`) share one path: the stone is picked and
//! placed by `editor_link`, its colour adopted here ([`adopt_for_open`]), and the main window's
//! editor loads the stone's design first and then applies the colour (and, for "Open in Editor",
//! the planned piece of rough) to it (`gui::editor::planned_stone`), so the zones always land on
//! the design they were computed for.
//!
//! **A plan has a colour only when it is a saved plan.** The rough colour is stored against the
//! saved plan's id (`store::save_rough_colour`), so results that were just planned and not saved
//! show the palette colours, and results loaded from a saved plan that has a colour show it.
//! [`refresh`] reads the colour whenever the results change.

use super::{
    counts::is_remote,
    editor_link::{self, StoneTarget},
    host::{Host, on_host},
    run::ResultsSource,
    session::Session,
    view::{self, ColourJob},
};
use crate::{
    Zoning,
    gui::{
        editor::planned_stone::PlannedStoneRequest,
        rough_colour::{
            preview::{
                AdoptTarget, PoseOption, PreviewInputs, adopt, apply_pose_option, load_inputs,
                resolve_host,
            },
            store::{AdoptOutcome, posed_stone_pose},
        },
        show_toast,
    },
};
use indicatrix_cut_core::rough_plan::{PlacedStone, RoughLayout, fit::StonePose};
use slint::ComponentHandle;
use std::{
    cell::RefCell,
    rc::Rc,
    sync::{Arc, PoisonError},
};
use tracing::warn;

thread_local! {
    /// The rough colour of the saved plan whose results are shown, if it has one.
    static INPUTS: RefCell<Option<Arc<PreviewInputs>>> = const { RefCell::new(None) };
}

/// Registers the planner's zoning callbacks: the orientation option and "Use colour".
pub(super) fn setup(host: &Rc<Host>) {
    let zoning = host.window.global::<Zoning>();
    zoning.on_pose_option_changed(|index| on_host(|host| pose_option_changed(host, index)));
    zoning.on_adopt_colour(|result, group| on_host(|host| adopt_colour(host, result, group)));
}

/// What a result's scene and thumbnail need to show the plan's colour, or `None` for a plan
/// without one.
pub(super) fn colour_job(layout_index: usize) -> Option<ColourJob> {
    let inputs = INPUTS.with(|cell| cell.borrow().clone())?;
    Some(ColourJob {
        inputs,
        layout_index: u32::try_from(layout_index).ok()?,
    })
}

/// The saved plan's id and the material it was planned with, for results loaded from a saved
/// plan.
fn plan_of(session: &Session) -> Option<(i64, String)> {
    match &session.run.source {
        ResultsSource::Loaded { plan_id, .. } => {
            Some((*plan_id, session.run.material_name.clone()))
        }
        ResultsSource::Planned => None,
    }
}

/// The results changed: reads the colour of the saved plan they belong to (none for a plan that
/// was just run) and tells the window whether the colour options are offered.
pub(super) fn refresh(host: &Rc<Host>) {
    let plan = plan_of(&host.session.borrow());
    let inputs = plan.and_then(|(plan_id, material)| {
        let db = host
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match load_inputs(&db, plan_id, &material) {
            Ok(inputs) => inputs.map(Arc::new),
            Err(error) => {
                warn!("Rough colour of plan {plan_id} not used: {error:#}");
                None
            }
        }
    });
    let zoning = host.window.global::<Zoning>();
    zoning.set_colour_available(inputs.is_some());
    if let Some(inputs) = &inputs {
        zoning.set_pose_option(PoseOption::of_choices(&inputs.choices).index());
    }
    INPUTS.with(|cell| *cell.borrow_mut() = inputs);
}

/// "Stone orientation" was picked: the choice is stored for every result of the plan and the
/// previews are drawn again with it.
fn pose_option_changed(host: &Rc<Host>, index: i32) {
    let Some(option) = PoseOption::from_index(index) else {
        return;
    };
    let Some(inputs) = INPUTS.with(|cell| cell.borrow().clone()) else {
        return;
    };
    let layouts: Vec<RoughLayout> = host.session.borrow().run.layouts.clone();
    let stored = {
        let db = host
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        apply_pose_option(&db, inputs.plan_id, &layouts, &inputs.rough_zoned, option)
    };
    match stored {
        Ok(_) => {
            // Reads the colour and the stored choices again and rebuilds the scenes and
            // thumbnails with them.
            view::results_changed(host);
            host.window
                .global::<Zoning>()
                .set_pose_option(option.index());
        }
        Err(error) => {
            if let Some(main) = host.main.upgrade() {
                show_toast(
                    &main,
                    &format!("Could not store the stone orientation: {error:#}"),
                    "error",
                );
            }
        }
    }
}

/// The pose of stone `stone_index` of result `layout_index` as the cutter chose it: the stored
/// "Stone orientation" choice (`store::posed_stone_pose`, what `preview::adopt` reads), or the
/// stone's own pose when the results carry no colour.
pub(super) fn chosen_pose(layout_index: u32, stone_index: u32, stone: &PlacedStone) -> StonePose {
    INPUTS
        .with(|cell| cell.borrow().clone())
        .map_or(stone.pose, |inputs| {
            posed_stone_pose(&inputs.choices, layout_index, stone_index, stone)
        })
}

/// The colour of the stone `target` names as the custom material "<plan> - <design> color"
/// (`preview::adopt`: the stone in the pose the cutter chose, at the design's real width), for
/// the editor to put on the design.
///
/// `None` when the results carry no colour or are not a loaded plan (no message: the design just
/// has no colour to take), and when adopting fails (the reason is toasted).
pub(super) fn adopt_for_open(host: &Rc<Host>, target: &StoneTarget) -> Option<AdoptOutcome> {
    let inputs = INPUTS.with(|cell| cell.borrow().clone())?;
    let picked = &target.picked;
    let rough_name = picked.plan_name.as_deref()?;
    let material = host.session.borrow().run.material_name.clone();
    let tell = |text: &str, level: &str| {
        if let Some(main) = host.main.upgrade() {
            show_toast(&main, text, level);
        }
    };
    let outcome = {
        let db = host.db.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(owned_host) = resolve_host(&db, &material) else {
            drop(db);
            tell(
                &format!("The material '{material}' is not in the library."),
                "error",
            );
            return None;
        };
        adopt(
            &db,
            &owned_host,
            &inputs,
            &AdoptTarget {
                rough_name,
                design_title: &picked.title,
                layout: &picked.layout,
                layout_index: picked.layout_index,
                stone_index: picked.stone_index,
                design: target.info.placement,
                design_width_axis_units: target.info.width_axis_units,
            },
        )
    };
    match outcome {
        Ok(outcome) => Some(outcome),
        Err(error) => {
            tell(&format!("{error:#}"), "error");
            None
        }
    }
}

/// "Use Color" on design group `group` of result `result`: the first stone of that design, in
/// the pose the cutter chose, becomes the material "<plan> - <design> color" (`preview::adopt`).
/// The editor loads that design (the normal Load Selected path, with its unsaved-changes guard)
/// and the material and the stone's real width are applied to it as one undoable edit, so the
/// zones land on the design they were computed for (`planned_stone`).
fn adopt_colour(host: &Rc<Host>, result: i32, group: i32) {
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let tell = |text: &str, level: &str| show_toast(&main, text, level);
    let (Ok(result), Ok(group)) = (usize::try_from(result), usize::try_from(group)) else {
        return;
    };
    if INPUTS.with(|cell| cell.borrow().is_none()) {
        tell("These results have no rough color.", "info");
        return;
    }
    if is_remote(&host.source) {
        tell(editor_link::LOCAL_LIBRARY_FIRST, "info");
        return;
    }
    // Only a loaded plan has a colour.
    let picked = editor_link::pick_stone(&host.session.borrow(), result, group)
        .filter(|picked| picked.plan_name.is_some());
    let Some(picked) = picked else {
        tell("This design has no stone to take the color of.", "info");
        return;
    };
    let Some(target) = editor_link::resolve_target(&main, picked) else {
        return;
    };
    let Some(material) = adopt_for_open(host, &target) else {
        return;
    };
    let request = PlannedStoneRequest {
        material: Some(material),
        ..editor_link::request_for(host, &target, None)
    };
    editor_link::open_in_main(&main, request);
}
