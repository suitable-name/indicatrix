//! Rough colour in the Rough Planner's results (`zoning` feature only): the coloured previews, the
//! "Stone orientation" option and "Use colour" (adopt).
//!
//! The planner's host, session and results are private to `gui::rough_plan`, so this thin file
//! reads them and calls `gui::rough_colour`, where the logic and its tests live
//! (`rough_colour::preview`, `rough_colour::adopt_link`, `rough_colour::store`). The view's own
//! reach (scene colours, the zone handles, the brush) is `view::zoning_view`.
//!
//! **A plan has a colour only when it is a saved plan.** The rough colour is stored against the
//! saved plan's id (`store::save_rough_colour`), so results that were just planned and not saved
//! show the palette colours, and results loaded from a saved plan that has a colour show it.
//! [`refresh`] reads the colour whenever the results change.

use super::{
    format::group_order,
    host::{Host, on_host},
    run::ResultsSource,
    session::Session,
    view::{self, ColourJob},
};
use crate::{
    Zoning,
    gui::{
        rough_colour::{
            adopt_link,
            preview::{
                AdoptTarget, PoseOption, PreviewInputs, adopt, apply_pose_option, load_inputs,
                resolve_host,
            },
        },
        show_toast,
    },
};
use indicatrix_cut_core::rough_plan::{RoughLayout, zoned_plan::DesignPlacement};
use slint::ComponentHandle;
use std::{cell::RefCell, rc::Rc, sync::Arc};
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

/// The stone a "Use colour" click names.
struct PickedStone {
    layout: RoughLayout,
    entry_id: i64,
    stone_index: usize,
    plan_name: String,
    material: String,
}

/// The first stone of design group `group` of result `result` of the shown results.
fn pick_stone(session: &Session, result: usize, group: usize) -> Option<PickedStone> {
    let layout = session.run.layouts.get(result)?.clone();
    let (entry_id, _) = *group_order(&layout).get(group)?;
    let stone_index = layout
        .stones
        .iter()
        .position(|stone| stone.entry_id == entry_id)?;
    let ResultsSource::Loaded { name, .. } = &session.run.source else {
        return None;
    };
    Some(PickedStone {
        layout,
        entry_id,
        stone_index,
        plan_name: name.clone(),
        material: session.run.material_name.clone(),
    })
}

/// "Use colour" on design group `group` of result `result`: the first stone of that design,
/// in the pose the cutter chose, becomes the material "<plan name> colour" (`preview::adopt`),
/// which is selected in the editor at the stone's real width.
fn adopt_colour(host: &Rc<Host>, result: i32, group: i32) {
    let main = host.main.upgrade();
    let tell = |text: &str, level: &str| {
        if let Some(main) = &main {
            show_toast(main, text, level);
        }
    };
    let (Ok(result), Ok(group)) = (usize::try_from(result), usize::try_from(group)) else {
        return;
    };
    let Some(inputs) = INPUTS.with(|cell| cell.borrow().clone()) else {
        tell("These results have no rough colour.", "info");
        return;
    };
    let Some(picked) = pick_stone(&host.session.borrow(), result, group) else {
        tell("This design has no stone to take the colour of.", "info");
        return;
    };
    // The design's placement is read from the library, so the vault is not locked here.
    let Some((design, width_units)) = super::design_placement_of(picked.entry_id) else {
        tell(
            "This design could not be read from the library, so its stone size is unknown.",
            "error",
        );
        return;
    };
    let outcome = {
        let db = host
            .db
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(owned_host) = resolve_host(&db, &picked.material) else {
            drop(db);
            tell(
                &format!("The material '{}' is not in the library.", picked.material),
                "error",
            );
            return;
        };
        adopt(
            &db,
            &owned_host,
            &inputs,
            &AdoptTarget {
                rough_name: &picked.plan_name,
                layout: &picked.layout,
                layout_index: u32::try_from(result).unwrap_or(u32::MAX),
                stone_index: picked.stone_index,
                design,
                design_width_units: width_units,
            },
        )
    };
    let outcome = match outcome {
        Ok(outcome) => outcome,
        Err(error) => {
            tell(&format!("{error:#}"), "error");
            return;
        }
    };
    let Some(main) = &main else {
        return;
    };
    match adopt_link::apply_to_editor(main, &host.db, &outcome) {
        Ok(()) => {
            // The planner may be covering the editor.
            main.window().set_minimized(false);
            let _ = main.window().show();
        }
        Err(message) => show_toast(main, &message, "error"),
    }
}

/// The placement of design `entry_id` in the planner's caliper frame and its caliper width in
/// model units, read from the library: the numbers a stone's colour needs to move into the
/// stone's frame (`DesignPlacement`). `None` for a design that is gone or has no solid.
pub(super) fn design_placement(entry_id: i64) -> Option<(DesignPlacement, f64)> {
    view::design_info(entry_id).map(|info| (info.placement, info.width_units))
}
