//! Tests of what a save writes: from the results on screen, through the snapshot and the
//! write, to the stored plan read back, against an in-memory library.
//!
//! The fixture layout uses designs 1 and 2. Each test gives the library those two designs
//! at sizes (3.0 and 4.0 model units wide, ratios 1.5, 0.7, 0.9) other than the ones the
//! plan is taken to have been planned with (2.0 and 2.5 wide, ratios 1.2, 0.5, 0.6), so a
//! save that measured the library again could not pass for one that kept the planned
//! shapes.

use super::{
    LoadedFingerprints,
    convert::CandidateSource,
    dto::DesignShape,
    fixtures::{plain_block, sample_layout, settings},
    format::{design_shape, parse_and_validate_plan},
    save::{LibraryId, snapshot_of, stored_shapes_and_library, write_plan},
    store,
    tests_db::{add_design, library},
};
use crate::gui::rough_plan::run::{ResultsSource, RunState};
use indicatrix_vault::{db::sqlite::Database, model::solid_extents::SolidExtents};
use std::{collections::BTreeMap, sync::Mutex};

/// The extents a design `width` model units wide had when the layouts were planned: the
/// ratios length/width 1.2, height/width 0.5 and volume/width^3 0.6, which no design made
/// by `add_design` (1.5, 0.7, 0.9) has.
fn planned_extents(width: f64) -> SolidExtents {
    SolidExtents {
        width_caliper: width,
        length_caliper: 1.2 * width,
        width_axis: width,
        length_axis: 1.2 * width,
        height: 0.5 * width,
        volume: 0.6 * width * width * width,
    }
}

/// The shapes of designs 1 and 2 as planned: 2.0 and 2.5 model units wide.
fn planned_shapes() -> BTreeMap<i64, DesignShape> {
    BTreeMap::from([
        (1, design_shape(&planned_extents(2.0))),
        (2, design_shape(&planned_extents(2.5))),
    ])
}

/// The shape design `id` has in `db` now.
fn today_shape(db: &Mutex<Database>, id: i64) -> DesignShape {
    let stored = db
        .lock()
        .expect("lock")
        .solid_extents_for(&[id])
        .expect("extents");
    design_shape(&stored[&id].extents.expect("measured"))
}

/// A library holding designs 1 and 2 at today's sizes (3.0 and 4.0 wide).
fn library_today() -> Mutex<Database> {
    let db = library();
    assert_eq!(add_design(&db, "Barion Oval", 3.0), 1);
    assert_eq!(add_design(&db, "Emerald", 4.0), 2);
    db
}

/// The results of a plan on screen: the fixture layout for the plain block, with `shapes`
/// as the shapes its designs had when it was planned, shown as from `source`.
fn results_on_screen(shapes: BTreeMap<i64, DesignShape>, source: ResultsSource) -> RunState {
    let mut run = RunState::default();
    show_fixture(&mut run, shapes, source);
    run
}

/// Puts the fixture plan into `run` (the fields a save reads).
fn show_fixture(run: &mut RunState, shapes: BTreeMap<i64, DesignShape>, source: ResultsSource) {
    run.layouts = vec![sample_layout()];
    run.plan_model = Some(plain_block());
    run.plan_settings = Some(settings());
    run.material_name = "Aquamarine".to_string();
    run.weighed_ct = Some(8.9);
    run.candidate_source = CandidateSource::Library;
    run.titles = BTreeMap::from([(1, "Barion Oval".to_string()), (2, "Emerald".to_string())]);
    run.shapes = shapes;
    run.keep = vec![false];
    run.source = source;
}

/// Saves `run` as plan "Saved" (with the loaded plan's stored shapes `loaded`, if any),
/// reads the stored plan back and returns it.
fn save_and_reload(
    db: &Mutex<Database>,
    run: &RunState,
    loaded: Option<&LoadedFingerprints>,
) -> super::format::LoadedPlan {
    let snapshot = snapshot_of(run, loaded, false).expect("there are results to save");
    write_plan(db, "Saved", &snapshot).expect("the plan is saved");
    let listed = store::list_saved(db).expect("listed");
    let stored = store::load_saved(db, listed[0].id).expect("loaded");
    parse_and_validate_plan(&stored.payload).expect("the saved plan parses")
}

/// A width in model units stored as a shape's caliper, to the nine digits a file keeps.
fn width_of(shape: &DesignShape) -> f64 {
    shape.width_caliper.expect("the shape has a width")
}

#[test]
fn a_fresh_plan_is_saved_with_the_shapes_it_was_planned_for_not_todays() {
    let db = library_today();
    let planned = planned_shapes();
    for id in [1, 2] {
        assert_ne!(
            planned[&id],
            today_shape(&db, id),
            "the library was rescaled after planning"
        );
    }

    let run = results_on_screen(planned.clone(), ResultsSource::Planned);
    let plan = save_and_reload(&db, &run, None);

    for id in [1, 2] {
        let design = plan
            .designs
            .iter()
            .find(|design| design.entry_id == id)
            .expect("the design is recorded");
        assert_eq!(
            design.shape(),
            planned[&id],
            "design {id}: the planned shape"
        );
        assert_ne!(
            design.shape(),
            today_shape(&db, id),
            "design {id}: not today's"
        );
    }
    // The widths are the planned 2.0 and 2.5, not today's 3.0 and 4.0, to 9 digits.
    let widths: Vec<f64> = plan.designs.iter().map(|d| width_of(&d.shape())).collect();
    assert!((widths[0] - 2.0).abs() < 1e-8, "{widths:?}");
    assert!((widths[1] - 2.5).abs() < 1e-8, "{widths:?}");
    // A plan made here carries this library's stamp.
    assert!(plan.library_id.is_some());
    assert_eq!(
        plan.library_id,
        db.lock().expect("lock").library_stamp().ok()
    );
}

#[test]
fn a_design_without_a_planned_shape_is_measured_from_the_library() {
    let db = library_today();
    // Only design 1 has a planned shape; design 2 has none.
    let shapes = BTreeMap::from([(1, design_shape(&planned_extents(2.0)))]);
    let run = results_on_screen(shapes.clone(), ResultsSource::Planned);
    let plan = save_and_reload(&db, &run, None);

    let shape_of = |id: i64| {
        plan.designs
            .iter()
            .find(|design| design.entry_id == id)
            .expect("the design is recorded")
            .shape()
    };
    assert_eq!(shape_of(1), shapes[&1]);
    assert_eq!(shape_of(2), today_shape(&db, 2));
}

#[test]
fn a_loaded_plan_saved_again_keeps_its_stored_shapes_and_its_library() {
    let db = library_today();
    let stored = planned_shapes();
    let source = ResultsSource::Loaded {
        plan_id: 7,
        name: "Aqua".to_string(),
        created_at: 0,
    };
    // The results on screen carry no shapes of their own here: the file's are the ones.
    let run = results_on_screen(BTreeMap::new(), source);
    let loaded = LoadedFingerprints {
        plan_id: 7,
        library_id: Some(5),
        shapes: stored.clone(),
    };
    let plan = save_and_reload(&db, &run, Some(&loaded));

    for id in [1, 2] {
        let design = plan
            .designs
            .iter()
            .find(|design| design.entry_id == id)
            .expect("the design is recorded");
        assert_eq!(design.shape(), stored[&id], "design {id}: the file's shape");
    }
    assert_eq!(
        plan.library_id,
        Some(5),
        "the ids still belong to the library the file came from"
    );
}

#[test]
fn the_stored_shapes_come_from_the_loaded_plan_only_while_that_plan_is_on_screen() {
    let sized = |width: f64| DesignShape {
        width_caliper: Some(width),
        ..DesignShape::default()
    };
    let planned = BTreeMap::from([(1, sized(1.0))]);
    let file = BTreeMap::from([(1, sized(2.0))]);
    let loaded = LoadedFingerprints {
        plan_id: 7,
        library_id: None,
        shapes: file.clone(),
    };
    let shown = |plan_id: i64| ResultsSource::Loaded {
        plan_id,
        name: String::new(),
        created_at: 0,
    };

    // The loaded plan itself: its file's shapes and its file's library (none named).
    let run = results_on_screen(planned.clone(), shown(7));
    assert_eq!(
        stored_shapes_and_library(&run, Some(&loaded)),
        (file, LibraryId::Kept(None))
    );
    // Another stored plan on screen, or a fresh one: what was planned, in this library.
    let other = results_on_screen(planned.clone(), shown(8));
    assert_eq!(
        stored_shapes_and_library(&other, Some(&loaded)),
        (planned.clone(), LibraryId::Local)
    );
    let fresh = results_on_screen(planned.clone(), ResultsSource::Planned);
    assert_eq!(
        stored_shapes_and_library(&fresh, Some(&loaded)),
        (planned.clone(), LibraryId::Local)
    );
    assert_eq!(
        stored_shapes_and_library(&fresh, None),
        (planned, LibraryId::Local)
    );
}
