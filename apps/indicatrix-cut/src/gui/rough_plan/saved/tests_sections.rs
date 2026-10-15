//! Tests of the `colour` and `pose_choices` sections of a plan file (schema version 4): what the
//! loader checks, that an export carries the library's color and stone orientations and an
//! import brings them back, and that a plan without them is written and stored exactly as it
//! always was.

use super::{
    dto::{PoseChoiceDto, RoughColourDto, SavedPlanDto},
    fixtures::{expect_error, plain_block, sample_layout, write},
    format::{parse_and_validate_plan, payload_version_of, payload_with_name},
    store,
    tests_db::library,
    transfer::{export_text, import_text},
};

/// The document of `text`.
fn document(text: &str) -> SavedPlanDto {
    toml::from_str(text).expect("the document parses")
}

/// One orientation choice.
const fn choice(layout: u32, stone: u32, pose: u8) -> PoseChoiceDto {
    PoseChoiceDto {
        layout,
        stone,
        pose,
    }
}

/// The fixture plan (one layout of three stones) as a document with `colour` and `choices`
/// (`(layout, stone, pose)`), written by the TOML writer with the version its content needs.
fn sectioned(colour: Option<RoughColourDto>, choices: &[(u32, u32, u8)]) -> String {
    let mut dto = document(&write(&plain_block(), &[sample_layout()]));
    dto.colour = colour;
    dto.pose_choices = choices
        .iter()
        .map(|&(layout, stone, pose)| choice(layout, stone, pose))
        .collect();
    dto.version = dto.content_version();
    toml::to_string_pretty(&dto).expect("the document is written")
}

#[test]
fn a_plan_with_orientation_choices_is_version_4_and_the_choices_are_checked_against_its_layouts() {
    let good = sectioned(None, &[(0, 0, 1), (0, 2, 3)]);
    assert_eq!(payload_version_of(&good), Some(4), "{good}");
    assert!(
        good.contains("[[pose_choices]]\nlayout = 0\nstone = 0\npose = 1\n"),
        "{good}"
    );
    let loaded = parse_and_validate_plan(&good).expect("the choices fit the plan");
    assert_eq!(loaded.version, 4);
    assert_eq!(loaded.pose_choices, vec![choice(0, 0, 1), choice(0, 2, 3)]);
    assert_eq!(loaded.colour, None);
}

#[test]
fn a_choice_that_fits_no_stone_is_refused_by_the_loader() {
    // The fixture has one layout of three stones (0 to 2) and four poses (0 to 3).
    for (choices, wanted) in [
        (vec![(1, 0, 1)], "pose_choices[0].layout"),
        (vec![(0, 3, 1)], "pose_choices[0].stone"),
        (vec![(0, 0, 4)], "pose_choices[0].pose"),
        (vec![(0, 1, 1), (0, 1, 2)], "pose_choices[1]"),
    ] {
        let error = expect_error(&sectioned(None, &choices));
        assert!(error.contains(wanted), "{error}");
    }
}

#[test]
fn a_file_older_than_version_4_may_not_carry_a_section() {
    let downgraded = sectioned(None, &[(0, 0, 1)]).replacen("version = 4", "version = 3", 1);
    let error = expect_error(&downgraded);
    assert!(
        error.contains("pose_choices needs plan version 4"),
        "{error}"
    );
}

#[test]
fn an_import_of_a_bad_choice_names_the_file_and_stores_nothing() {
    for (choices, wanted) in [
        (vec![(1, 0, 1)], "pose_choices[0].layout"),
        (vec![(0, 9, 1)], "pose_choices[0].stone"),
        (vec![(0, 0, 4)], "pose_choices[0].pose"),
        (vec![(0, 2, 1), (0, 2, 3)], "pose_choices[1]"),
    ] {
        let db = library();
        let text = sectioned(None, &choices);
        let error = import_text(&db, "orient.indicatrix-rough.toml", &text)
            .err()
            .expect("the choice does not fit the plan");
        assert!(error.contains("orient.indicatrix-rough.toml"), "{error}");
        assert!(error.contains(wanted), "{error}");
        assert_eq!(store::list_saved(&db).expect("listed"), []);
    }
}

#[test]
fn a_plan_without_sections_is_exported_and_imported_exactly_as_before() {
    let first = library();
    let text = format!(
        "future_key = \"kept\"\n{}",
        write(&plain_block(), &[sample_layout()])
    );
    let id = store::save_plan(&first, "Test plan", &text).expect("stored");
    store::rename_saved(&first, id, "Renamed").expect("renamed");

    // The export is the stored text with its name line replaced, byte for byte.
    let stored = store::load_saved(&first, id).expect("loaded");
    let exported = export_text(&first, id).expect("exported");
    assert_eq!(exported, payload_with_name(&stored.payload, &stored.name));
    assert_eq!(payload_version_of(&exported), Some(1));
    assert!(exported.contains("future_key = \"kept\""), "{exported}");

    // The import stores the file's text as it is, unknown keys included.
    let second = library();
    let opened = import_text(&second, "plan.indicatrix-rough.toml", &exported).expect("imported");
    let again = store::load_saved(&second, opened.plan_id).expect("loaded");
    assert_eq!(again.payload, exported);
    assert_eq!(again.payload_version, 1);
    assert!(
        opened
            .staleness
            .warnings
            .iter()
            .all(|w| !w.contains("rough color")),
        "{:?}",
        opened.staleness.warnings
    );
}

#[test]
fn versions_1_to_3_still_import_and_a_newer_file_is_refused() {
    let current = write(&plain_block(), &[sample_layout()]);
    for version in 1..=3_u32 {
        let text = current.replacen("version = 1", &format!("version = {version}"), 1);
        let db = library();
        let opened =
            import_text(&db, "old.indicatrix-rough.toml", &text).expect("an older plan imports");
        assert_eq!(opened.plan.version, version);
        let stored = store::load_saved(&db, opened.plan_id).expect("loaded");
        assert_eq!(stored.payload, text, "the file is stored as it is");
        assert_eq!(stored.payload_version, version);
    }

    let db = library();
    let newer = current.replacen("version = 1", "version = 5", 1);
    let error = import_text(&db, "new.indicatrix-rough.toml", &newer)
        .err()
        .expect("a version 5 file is refused");
    assert!(error.contains("newer Indicatrix"), "{error}");
    assert!(error.contains("new.indicatrix-rough.toml"), "{error}");
    assert_eq!(store::list_saved(&db).expect("listed"), []);
}

/// The tests that read and write the library's side tables.
mod with_colour {
    use super::{super::sections::restore_sections, *};
    use crate::gui::rough_colour::store::{
        load_pose_choices, load_rough_colour, save_layout_pose_choices, save_rough_colour,
    };
    use glam::DVec3;
    use indicatrix::optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    };
    use indicatrix_cut_core::rough_plan::zoned_plan::{
        RoughColour, ZONING_FORMAT_VERSION, encode_zoned,
    };
    use std::collections::BTreeMap;

    fn absorber(centre_nm: f32, peak: f32) -> ZoneAbsorption {
        ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
            centre_nm, 40.0, peak,
        )]))
    }

    /// A pale rind with a red rod along x through `(y, z) = (-3, 0)`, 3 mm in radius.
    fn watermelon() -> ZonedAbsorption {
        let mut zoned = ZonedAbsorption::new(absorber(480.0, 0.05));
        zoned.zones.push(Zone {
            shape: ZoneShape::CoaxialCylinder {
                axis_point: DVec3::new(0.0, -3.0, 0.0),
                axis_dir: DVec3::X,
                r_in: 0.0,
                r_out: 3.0,
            },
            absorption: absorber(600.0, 0.9),
        });
        zoned
    }

    /// The color the tests give their plans, as the library holds it.
    fn colour() -> RoughColour {
        RoughColour {
            zoned: watermelon(),
            fit_json: "{\"fit\":true}".to_string(),
            version: ZONING_FORMAT_VERSION,
            created: 1_700_000_123,
        }
    }

    /// The same color as a file section.
    fn colour_dto() -> RoughColourDto {
        RoughColourDto {
            version: ZONING_FORMAT_VERSION,
            zoned_json: encode_zoned(&watermelon()).expect("the zones are valid"),
            fit_json: "{\"fit\":true}".to_string(),
            created: 1_700_000_123,
        }
    }

    #[test]
    fn a_color_and_two_orientations_travel_through_the_text_of_an_export_and_an_import() {
        let first = library();
        let layouts = [sample_layout()];
        let payload = write(&plain_block(), &layouts);
        let id = store::save_plan(&first, "Test plan", &payload).expect("stored");
        {
            let database = first.lock().expect("lock");
            save_rough_colour(&database, id, &colour()).expect("color stored");
            // Stones 0 and 2 turned, stone 1 as the planner placed it.
            save_layout_pose_choices(&database, id, 0, &[1, 0, 3]).expect("poses stored");
        }

        // The export is a version 4 document whose two sections parse back equal.
        let text = export_text(&first, id).expect("exported");
        assert_eq!(payload_version_of(&text), Some(4), "{text}");
        assert!(
            text.contains("[colour]\nversion = 1\nzoned_json = "),
            "{text}"
        );
        let dto = document(&text);
        assert_eq!(dto.colour, Some(colour_dto()));
        assert_eq!(dto.pose_choices, vec![choice(0, 0, 1), choice(0, 2, 3)]);
        let loaded = parse_and_validate_plan(&text).expect("the export is a plan");
        assert_eq!(loaded.version, 4);
        assert_eq!(loaded.layouts, layouts);
        assert_eq!(loaded.colour, dto.colour);
        assert_eq!(loaded.pose_choices, dto.pose_choices);

        // The import into another library stores a plan without the sections and puts them in
        // the new plan's side tables.
        let second = library();
        let opened = import_text(&second, "plan.indicatrix-rough.toml", &text).expect("imported");
        let stored = store::load_saved(&second, opened.plan_id).expect("loaded");
        assert!(!stored.payload.contains("[colour]"), "{}", stored.payload);
        assert!(
            !stored.payload.contains("pose_choices"),
            "{}",
            stored.payload
        );
        assert_eq!(stored.payload_version, 1);
        assert_eq!(payload_version_of(&stored.payload), Some(1));
        let reopened = parse_and_validate_plan(&stored.payload).expect("the payload is a plan");
        assert_eq!(reopened.layouts, layouts);
        assert_eq!(reopened.colour, None);
        assert_eq!(reopened.pose_choices, []);
        {
            let database = second.lock().expect("lock");
            assert_eq!(
                load_rough_colour(&database, opened.plan_id).expect("read"),
                Some(colour())
            );
            assert_eq!(
                load_pose_choices(&database, opened.plan_id).expect("read"),
                BTreeMap::from([((0, 0), 1), ((0, 2), 3)])
            );
        }

        // Exporting the imported plan gives the same file again.
        assert_eq!(
            export_text(&second, opened.plan_id).expect("exported"),
            text
        );
    }

    #[test]
    fn a_plan_with_only_a_color_or_only_orientations_exports_just_that_section() {
        let db = library();
        let payload = write(&plain_block(), &[sample_layout()]);
        let colour_only = store::save_plan(&db, "Color only", &payload).expect("stored");
        let with_poses = store::save_plan(&db, "Poses only", &payload).expect("stored");
        let with_neither = store::save_plan(&db, "Neither", &payload).expect("stored");
        {
            let database = db.lock().expect("lock");
            save_rough_colour(&database, colour_only, &colour()).expect("color stored");
            save_layout_pose_choices(&database, with_poses, 0, &[0, 2, 0]).expect("poses stored");
        }

        let text = export_text(&db, colour_only).expect("exported");
        assert!(text.contains("[colour]\n"), "{text}");
        assert!(!text.contains("pose_choices"), "{text}");
        assert_eq!(payload_version_of(&text), Some(4));

        let text = export_text(&db, with_poses).expect("exported");
        assert!(text.contains("[[pose_choices]]\n"), "{text}");
        assert!(!text.contains("[colour]"), "{text}");
        let loaded = parse_and_validate_plan(&text).expect("the export is a plan");
        assert_eq!(loaded.pose_choices, vec![choice(0, 1, 2)]);

        // A plan with neither is exported as it always was.
        let stored = store::load_saved(&db, with_neither).expect("loaded");
        assert_eq!(
            export_text(&db, with_neither).expect("exported"),
            payload_with_name(&stored.payload, &stored.name)
        );
    }

    #[test]
    fn a_file_with_a_color_this_build_cannot_use_is_refused_and_nothing_is_stored() {
        let good = sectioned(Some(colour_dto()), &[(0, 0, 1)]);
        parse_and_validate_plan(&good).expect("the good file loads");

        let mut undecodable = colour_dto();
        undecodable.zoned_json = "{ not json".to_string();
        let mut newer = colour_dto();
        newer.version = ZONING_FORMAT_VERSION + 1;
        for (section, wanted) in [(undecodable, "colour.zoned_json"), (newer, "newer")] {
            let db = library();
            let text = sectioned(Some(section), &[]);
            let error = import_text(&db, "color.indicatrix-rough.toml", &text)
                .err()
                .expect("the color cannot be used");
            assert!(error.contains("color.indicatrix-rough.toml"), "{error}");
            assert!(error.contains(wanted), "{error}");
            assert_eq!(store::list_saved(&db).expect("listed"), []);
        }
    }

    #[test]
    fn an_export_never_writes_a_file_this_build_would_refuse() {
        let db = library();
        let payload = write(&plain_block(), &[sample_layout()]);
        let id = store::save_plan(&db, "Stale", &payload).expect("stored");
        {
            // A choice for a layout the plan does not have.
            let database = db.lock().expect("lock");
            save_layout_pose_choices(&database, id, 4, &[1]).expect("stored");
        }
        let error = export_text(&db, id).expect_err("the choice names no layout of the plan");
        assert!(error.contains("would not open again"), "{error}");
        assert!(error.contains("pose_choices[0].layout"), "{error}");
    }

    #[test]
    fn a_failed_restore_says_which_part_could_not_be_stored() {
        let db = library();
        let id = store::save_plan(&db, "Plan", &write(&plain_block(), &[sample_layout()]))
            .expect("stored");
        let mut plan =
            parse_and_validate_plan(&sectioned(None, &[(0, 0, 1)])).expect("the file loads");
        // A pose the loader would have refused.
        plan.pose_choices[0].pose = 7;
        let error = restore_sections(&db, id, &plan).expect_err("pose 7 does not exist");
        assert!(
            error.contains("stone orientations could not be stored"),
            "{error}"
        );
    }
}
