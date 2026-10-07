//! Opening a saved plan: the model and the settings come back into the window and the
//! saved layouts are shown as they were saved, marked with what changed in the library
//! since.
//!
//! Reading the plan and checking its designs happen on a worker thread ([`prepare_open`]);
//! [`apply_opened`] then runs on the UI thread and fills the window in a fixed order:
//! form fields, the model (one undo step), the results, the banner.
//!
//! Results on screen that were never saved are not replaced without asking: opening goes
//! through `guard_unsaved_results`.

use super::{
    LoadedFingerprints, PendingBanner,
    dto::{DesignShape, SavedDesignDto},
    format::{LoadedPlan, parse_and_validate_plan},
    naming::format_unix_date,
    show_error, show_status, show_warning, spawn_task,
    staleness::{Staleness, rekey_statuses, remap_of, resolve_designs},
    store,
};
use crate::{
    RoughPlanModel,
    gui::rough_plan::{
        editing,
        format::to_i32,
        host::Host,
        mesh_task,
        run::{ResultsSource, ShownLayouts, guard_unsaved_results, show_layouts},
    },
};
use indicatrix_cut_core::rough_plan::RoughLayout;
use indicatrix_vault::db::sqlite::Database;
use slint::ComponentHandle;
use std::{collections::BTreeMap, rc::Rc, sync::Mutex};

/// A stored plan that was read, validated and checked against the library.
pub(super) struct OpenedPlan {
    /// The plan's id in the table.
    pub(super) plan_id: i64,
    /// The plan's name in the table.
    pub(super) name: String,
    /// When the plan was stored (Unix seconds).
    pub(super) created_at: i64,
    /// The validated document.
    pub(super) plan: LoadedPlan,
    /// The state of its designs in the library.
    pub(super) staleness: Staleness,
}

/// Checks the designs of `plan` against the library (worker thread).
pub(super) fn check_staleness(
    db: &Mutex<Database>,
    plan: &LoadedPlan,
) -> Result<Staleness, String> {
    resolve_designs(db, &plan.designs, plan.library_id)
}

/// The warning for a stored plan whose recorded schema version is not the one its text
/// declares, if the two differ.
fn version_note(recorded: u32, declared: u32) -> Option<String> {
    (recorded != declared).then(|| {
        format!(
            "The plan's record says schema version {recorded} but its text declares version {declared}; it was read as version {declared}."
        )
    })
}

/// Reads stored plan `plan_id` and checks it (worker thread).
pub(super) fn prepare_open(db: &Mutex<Database>, plan_id: i64) -> Result<OpenedPlan, String> {
    let stored = store::load_saved(db, plan_id)?;
    let plan = parse_and_validate_plan(&stored.payload)
        .map_err(|e| format!("The saved plan \"{}\" cannot be opened: {e}", stored.name))?;
    let mut staleness = check_staleness(db, &plan)?;
    staleness
        .warnings
        .extend(version_note(stored.payload_version, plan.version));
    Ok(OpenedPlan {
        plan_id,
        name: stored.name,
        created_at: stored.created_at,
        plan,
        staleness,
    })
}

/// Shown when an open or import meets a running plan.
pub(super) const PLAN_RUNNING_MESSAGE: &str =
    "Wait for the running plan to finish, or cancel it, first.";

/// Starts an open or import: refuses while a plan runs (the results would be replaced
/// under it) and returns the sequence number the answer must still match. A mesh that is
/// still being read or scaled in the background is dropped: the plan brings its own rough.
pub(super) fn begin_open(host: &Host) -> Option<u64> {
    if host.window.global::<RoughPlanModel>().get_running() {
        show_error(host, PLAN_RUNNING_MESSAGE);
        return None;
    }
    mesh_task::cancel(host);
    let mut session = host.session.borrow_mut();
    session.saved.open_seq += 1;
    Some(session.saved.open_seq)
}

/// `RoughPlanModel.open_saved`: opens the plan, after asking whether to replace results
/// that were not saved.
pub(super) fn open_saved(host: &Rc<Host>, plan_id: i64) {
    guard_unsaved_results(host, move |host| start_open(host, plan_id));
}

/// Reads plan `plan_id` on a worker thread and shows it when it is ready.
fn start_open(host: &Rc<Host>, plan_id: i64) {
    let Some(seq) = begin_open(host) else {
        return;
    };
    show_error(host, "");
    spawn_task(
        host,
        move |db| prepare_open(db, plan_id),
        move |host, result| finish_open(host, seq, result),
    );
}

/// The UI-thread end of an open or import.
pub(super) fn finish_open(host: &Rc<Host>, seq: u64, result: Result<OpenedPlan, String>) {
    if host.session.borrow().saved.open_seq != seq {
        return;
    }
    // A plan may have been started while the file was being read; its results must not be
    // replaced under it.
    if host.window.global::<RoughPlanModel>().get_running() {
        show_error(host, PLAN_RUNNING_MESSAGE);
        return;
    }
    match result {
        Ok(opened) => apply_opened(host, opened),
        Err(message) => show_error(host, &message),
    }
}

/// `value` in the shortest text that reads back as the same number ("0.3", "12"), so that
/// planning again from the form reproduces the numbers the plan was made with.
fn fmt_exact(value: f64) -> String {
    if value == 0.0 {
        "0".to_string()
    } else {
        format!("{value}")
    }
}

/// The note for a plan made with another specific gravity than the list now gives its
/// material: a re-plan weighs the stones with the list's.
fn gravity_note(material: &str, saved: f64, listed: f64) -> Option<String> {
    let differs = (saved - listed).abs() > 1e-6 * saved.abs().max(listed.abs());
    differs.then(|| {
        format!(
            "The plan was made with a specific gravity of {saved} for {material}; the material list now gives {listed}, so planning again weighs the stones differently."
        )
    })
}

/// Writes the plan's settings, weighed carat and candidate source into the form and picks
/// its material. Returns the notes: the material is not in the list, or its specific
/// gravity is not the plan's.
fn push_settings(host: &Host, plan: &LoadedPlan) -> Vec<String> {
    let model = host.window.global::<RoughPlanModel>();
    let settings = &plan.settings;
    model.set_count(i32::from(settings.count));
    model.set_kerf_mm(fmt_exact(settings.kerf_mm).into());
    model.set_allowance_mm(fmt_exact(settings.allowance_mm).into());
    model.set_skin_mm(fmt_exact(settings.skin_mm).into());
    model.set_min_width_mm(fmt_exact(settings.min_width_mm).into());
    model.set_weighed_ct(plan.weighed_ct.map_or_else(String::new, fmt_exact).into());
    model.set_use_filter(plan.candidate_source.uses_filter());
    let found = host
        .session
        .borrow()
        .choices
        .iter()
        .enumerate()
        .find(|(_, choice)| choice.name.eq_ignore_ascii_case(&plan.material_name))
        .map(|(index, choice)| (index, choice.specific_gravity));
    match found {
        None => vec![format!(
            "The material \"{}\" is not in the material list; the form keeps the current material.",
            plan.material_name
        )],
        Some((index, listed)) => {
            model.set_material_index(to_i32(index));
            gravity_note(&plan.material_name, settings.specific_gravity, listed)
                .into_iter()
                .collect()
        }
    }
}

/// Moves the stones of designs that were found again under their title to the design
/// they resolved to.
fn remap_layouts(mut layouts: Vec<RoughLayout>, remap: &BTreeMap<i64, i64>) -> Vec<RoughLayout> {
    for layout in &mut layouts {
        for stone in &mut layout.stones {
            if let Some(&resolved) = remap.get(&stone.entry_id) {
                stone.entry_id = resolved;
            }
        }
    }
    layouts
}

/// The stored titles and shapes of the designs, keyed by the entry id the layouts use
/// after the remap (the first design wins if two end on one id).
fn design_records(
    designs: &[SavedDesignDto],
    remap: &BTreeMap<i64, i64>,
) -> (BTreeMap<i64, String>, BTreeMap<i64, DesignShape>) {
    let mut titles = BTreeMap::new();
    let mut shapes = BTreeMap::new();
    for design in designs {
        let id = remap
            .get(&design.entry_id)
            .copied()
            .unwrap_or(design.entry_id);
        titles.entry(id).or_insert_with(|| design.title.clone());
        shapes.entry(id).or_insert_with(|| design.shape());
    }
    (titles, shapes)
}

/// The line above a loaded plan's results.
pub(super) fn banner_text(name: &str, created_at: i64) -> String {
    format!(
        "Saved plan \"{name}\" \u{b7} {} \u{b7} Re-plan uses the current model and settings",
        format_unix_date(created_at)
    )
}

/// Fills the window from an opened plan (UI thread).
fn apply_opened(host: &Rc<Host>, opened: OpenedPlan) {
    let OpenedPlan {
        plan_id,
        name,
        created_at,
        plan,
        staleness,
    } = opened;
    let mut notes = staleness.warnings;
    notes.extend(push_settings(host, &plan));

    let model = host.window.global::<RoughPlanModel>();
    host.session.borrow_mut().replace_model(plan.model.clone());
    model.set_selected_cut(-1);
    model.set_model_error("".into());
    editing::resync(host, true);

    let remap = remap_of(&staleness.statuses);
    let (titles, shapes) = design_records(&plan.designs, &remap);
    let statuses = rekey_statuses(staleness.statuses, &remap);
    let layouts = remap_layouts(plan.layouts, &remap);
    {
        let mut session = host.session.borrow_mut();
        session.saved.loaded = Some(LoadedFingerprints {
            plan_id,
            library_id: plan.library_id,
            shapes: shapes.clone(),
        });
        // The banner and the closing of the saved list wait for the rows (the results
        // are built on a worker), so the old results never sit under the new banner.
        session.saved.pending_banner = Some(PendingBanner {
            plan_id,
            text: banner_text(&name, created_at),
        });
    }
    show_layouts(
        host,
        ShownLayouts {
            layouts,
            plan_model: Some(plan.model),
            plan_settings: Some(plan.settings),
            material_name: plan.material_name,
            weighed_ct: plan.weighed_ct,
            candidate_source: plan.candidate_source,
            titles,
            statuses,
            shapes,
            source: ResultsSource::Loaded {
                plan_id,
                name,
                created_at,
            },
        },
        Some(show_loaded_banner),
    );
    show_error(host, "");
    if notes.is_empty() {
        show_status(host, "");
    } else {
        show_warning(host, &notes.join(" "));
    }
}

/// Runs once the rows of an opened plan are on screen: closes the saved list and shows
/// the banner. Does nothing when other results took the place of the plan's meanwhile.
fn show_loaded_banner(host: &Rc<Host>) {
    let text = {
        let mut session = host.session.borrow_mut();
        let shown = match (&session.saved.pending_banner, &session.run.source) {
            (Some(pending), ResultsSource::Loaded { plan_id, .. }) => pending.plan_id == *plan_id,
            _ => false,
        };
        if !shown {
            return;
        }
        let Some(pending) = session.saved.pending_banner.take() else {
            return;
        };
        pending.text
    };
    let model = host.window.global::<RoughPlanModel>();
    model.set_saved_open(false);
    model.set_loaded_banner(text.into());
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::rough_plan::{Axis, CutOrder, CutPlan, PlacedStone, fit::StonePose};

    fn stone(entry_id: i64) -> PlacedStone {
        PlacedStone {
            entry_id,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [1.0; 3],
            stone_size_mm: [1.0; 3],
            table_axis: Axis::Y,
            carat: 0.1,
            volume_mm3: 1.0,
            pose: StonePose {
                center_mm: [0.5; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }
    }

    #[test]
    fn stones_of_title_matched_designs_move_to_the_resolved_design() {
        let layout = RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: vec![stone(1), stone(2), stone(1)],
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: 0.3,
            total_volume_mm3: 3.0,
            yield_fraction: 0.5,
            exact_fit: false,
        };
        let remap = BTreeMap::from([(1, 50)]);
        let out = remap_layouts(vec![layout], &remap);
        let ids: Vec<i64> = out[0].stones.iter().map(|s| s.entry_id).collect();
        assert_eq!(ids, vec![50, 2, 50]);
    }

    #[test]
    fn the_banner_names_the_plan_and_its_date() {
        assert_eq!(
            banner_text("Aqua pebble", 1_790_726_400),
            "Saved plan \"Aqua pebble\" \u{b7} 2026-09-30 \u{b7} Re-plan uses the current model and settings"
        );
    }

    #[test]
    fn form_numbers_are_written_so_that_they_read_back_as_the_same_number() {
        for value in [
            0.3,
            0.25,
            12.0,
            1e-7,
            0.1 + 0.2,
            49.999_999_999_9,
            0.000_123_456_789,
        ] {
            let text = fmt_exact(value);
            let back: f64 = text.parse().expect("the text is a number");
            assert_eq!(back.to_bits(), value.to_bits(), "{value} -> {text}");
        }
        assert_eq!(fmt_exact(0.3), "0.3");
        assert_eq!(fmt_exact(12.0), "12");
        assert_eq!(fmt_exact(0.0), "0");
        assert_eq!(fmt_exact(-0.0), "0", "no negative zero in the form");
    }

    #[test]
    fn a_different_specific_gravity_is_noted_and_an_equal_one_is_not() {
        assert_eq!(gravity_note("Quartz", 2.65, 2.65), None);
        assert_eq!(gravity_note("Quartz", 2.65, 2.650_000_1), None);
        let note = gravity_note("Quartz", 2.65, 2.7).expect("2.65 and 2.7 differ");
        assert!(note.contains("2.65") && note.contains("2.7") && note.contains("Quartz"));
    }

    #[test]
    fn a_stored_version_that_differs_from_the_text_is_noted() {
        assert_eq!(version_note(1, 1), None);
        let note = version_note(2, 1).expect("the versions differ");
        assert!(
            note.contains("version 2") && note.contains("version 1"),
            "{note}"
        );
    }

    #[test]
    fn the_stored_titles_and_shapes_follow_the_remap_and_the_first_design_wins() {
        let design = |entry_id: i64, title: &str, ratio: f64| SavedDesignDto {
            entry_id,
            title: title.to_string(),
            fingerprint: [ratio, 0.5, 0.5],
            width_caliper: Some(ratio),
            extents_version: 1,
        };
        let designs = [
            design(1, "Old", 1.5),
            design(50, "New", 2.5),
            design(2, "Two", 3.5),
        ];
        let remap = BTreeMap::from([(1, 50)]);
        let (titles, shapes) = design_records(&designs, &remap);
        assert_eq!(titles.len(), 2);
        assert_eq!(titles.get(&50).map(String::as_str), Some("Old"));
        assert_eq!(shapes.get(&50).map(|shape| shape.fingerprint[0]), Some(1.5));
        assert_eq!(
            shapes.get(&2).and_then(|shape| shape.width_caliper),
            Some(3.5)
        );
    }
}
