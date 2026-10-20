//! Tests of the document module: creating, opening, saving and the autosave, over a
//! [`MockPlatform`] with temporary directories.

use std::{fs, sync::Arc};

use indicatrix_cut_core::{
    Edit, PreformShape,
    native::{DesignExtras, design_to_string},
};
use indicatrix_editor::material::builtin_preset_names;

use super::*;
use crate::{
    platform::{PickedFile, Platform, mime},
    platform_mock::MockPlatform,
};

/// The smallest `.asc` the parser accepts.
const TINY_ASC: &str = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";

fn state() -> (AppState, Arc<MockPlatform>) {
    let mock = Arc::new(MockPlatform::default());
    (state_on(&mock), mock)
}

fn state_on(mock: &Arc<MockPlatform>) -> AppState {
    let platform: Arc<dyn Platform> = mock.clone();
    AppState::new(platform)
}

fn picked(name: &str, bytes: &[u8]) -> PickedFile {
    PickedFile {
        display_name: name.to_owned(),
        bytes: bytes.to_vec(),
    }
}

/// A state holding the standard round brilliant.
fn brilliant() -> (AppState, Arc<MockPlatform>) {
    let (mut state, mock) = self::state();
    new_from_template(&mut state, 1).expect("template 1 exists");
    (state, mock)
}

/// The design as the plain file text, without metadata (comparable across a save).
fn plain_text(state: &AppState) -> String {
    design_to_string(&state.session.design, None, &DesignExtras::default())
        .expect("a design serializes")
}

fn tier_count(state: &AppState) -> usize {
    state.session.design.tiers.len()
}

fn edit_girdle(state: &mut AppState, mm: f64) {
    state
        .session
        .apply(Edit::SetGirdleDiameterMm {
            girdle_diameter_mm: Some(mm),
        })
        .expect("the edit applies");
}

#[test]
fn cards_list_empty_first_and_every_template() {
    let cards = template_cards();
    assert_eq!(
        cards.len(),
        indicatrix_cut_core::templates::TEMPLATES.len() + 1
    );
    assert_eq!(cards[0].index, 0);
    assert_eq!(cards[0].name, "Empty Design");
    assert_eq!(cards[0].facets, 0);
    for (position, card) in cards.iter().enumerate() {
        assert_eq!(card.index, position);
        assert!(!card.name.is_empty() && !card.description.is_empty());
    }
    assert_eq!(cards[1].name, "Standard Round Brilliant");
    assert!(cards[1].facets > 30, "{} facets", cards[1].facets);
}

#[test]
fn a_template_replaces_the_design() {
    let (mut state, _mock) = self::state();
    let before = state.session.current_generation();
    state.selected_tier = Some(2);
    new_from_template(&mut state, 1).expect("template 1 exists");
    assert!(tier_count(&state) > 0);
    assert_eq!(state.document.origin, DocumentOrigin::Template { index: 1 });
    assert!(
        state.session.is_dirty(),
        "a design the user created is unsaved until the first save"
    );
    assert!(state.session.current_generation() > before);
    assert_eq!(state.selected_tier, None);
    assert_eq!(display_title(&state), "Standard Round Brilliant");
    assert!(state.document.has_design(&state.session.design));
}

#[test]
fn the_empty_card_and_the_start_up_design_differ_in_has_design() {
    let (mut state, _mock) = self::state();
    assert!(!state.document.has_design(&state.session.design));
    new_from_template(&mut state, 0).expect("the empty card");
    assert_eq!(tier_count(&state), 0);
    assert!(state.document.has_design(&state.session.design));
}

#[test]
fn an_unknown_template_is_an_error_and_keeps_the_design() {
    let (mut state, _mock) = brilliant();
    let tiers = tier_count(&state);
    let message = new_from_template(&mut state, 99).expect_err("no template 99");
    assert!(message.contains("99"), "{message}");
    assert_eq!(tier_count(&state), tiers);
}

#[test]
fn the_form_builds_each_shape() {
    let mut form = NewDesignForm {
        shape: PreformShapeChoice::Oval,
        length_over_width: 1.35,
        ..NewDesignForm::default()
    };
    let oval = form.to_spec().expect("valid");
    assert_eq!(oval.preform.shape, PreformShape::Cylinder { sides: 96 });
    assert_eq!(oval.preform.length_over_width, 1.35);

    form.shape = PreformShapeChoice::Round;
    let round = form.to_spec().expect("valid");
    assert_eq!(
        round.preform.length_over_width, 1.0,
        "round ignores the ratio"
    );

    form.shape = PreformShapeChoice::Rectangle;
    let rectangle = form.to_spec().expect("valid");
    assert_eq!(rectangle.preform.shape, PreformShape::Block);
    assert_eq!(rectangle.preform.length_over_width, 1.35);

    form.shape = PreformShapeChoice::Square;
    assert_eq!(
        form.to_spec().expect("valid").preform.length_over_width,
        1.0
    );
    for (position, shape) in PreformShapeChoice::ALL.into_iter().enumerate() {
        assert_eq!(
            usize::try_from(shape.index()).expect("shape indices are non-negative"),
            position
        );
        assert_eq!(PreformShapeChoice::from_index(shape.index()), Some(shape));
    }
}

#[test]
fn the_form_names_the_field_that_is_wrong() {
    let base = NewDesignForm::default();
    let bad_symmetry = NewDesignForm {
        gear_teeth: 80,
        symmetry_order: 3,
        ..base.clone()
    };
    assert!(
        bad_symmetry
            .to_spec()
            .expect_err("3 does not divide 80")
            .contains("80")
    );
    let zero_symmetry = NewDesignForm {
        symmetry_order: 0,
        ..base.clone()
    };
    assert!(zero_symmetry.to_spec().is_err());
    let bad_gear = NewDesignForm {
        gear_teeth: 0,
        ..base.clone()
    };
    assert!(bad_gear.to_spec().is_err());
    let bad_size = NewDesignForm {
        depth: f64::NAN,
        ..base.clone()
    };
    assert!(bad_size.to_spec().is_err());
    let bad_ratio = NewDesignForm {
        shape: PreformShapeChoice::Oval,
        length_over_width: -1.0,
        ..base.clone()
    };
    assert!(bad_ratio.to_spec().is_err());
    let bad_material = NewDesignForm {
        material_name: Some("Unobtainium".to_owned()),
        ..base
    };
    assert!(
        bad_material
            .to_spec()
            .expect_err("not built in")
            .contains("Unobtainium")
    );
    let good_material = NewDesignForm {
        material_name: Some("Diamond".to_owned()),
        ..base
    };
    let spec = good_material.to_spec().expect("a built-in material");
    assert_eq!(spec.material.name.as_deref(), Some("Diamond"));
}

fn raw<'a>(ratio: &'a str, width_mm: &'a str) -> RawNewDesignForm<'a> {
    RawNewDesignForm {
        shape_index: 1,
        ratio,
        gear_teeth: 80,
        symmetry: 4,
        mirror: false,
        material_index: 0,
        width_mm,
    }
}

#[test]
fn raw_form_values_are_parsed_with_a_decimal_comma() {
    let form = NewDesignForm::from_raw(&raw("1,35", " 7,5 ")).expect("valid");
    assert_eq!(form.shape, PreformShapeChoice::Oval);
    assert_eq!(form.length_over_width, 1.35);
    assert_eq!(form.width_mm, Some(7.5));
    assert_eq!(form.gear_teeth, 80);
    assert_eq!(form.symmetry_order, 4);
    assert!(!form.mirror);
    assert_eq!(form.material_name, None, "index 0 is (none)");
    assert!(form.to_spec().is_ok());

    let blank = NewDesignForm::from_raw(&raw("1.35", "")).expect("a blank width is fine");
    assert_eq!(blank.width_mm, None);

    let round = RawNewDesignForm {
        shape_index: 0,
        ..raw("not a number", "")
    };
    let form = NewDesignForm::from_raw(&round).expect("a round shape ignores the ratio");
    assert_eq!(form.length_over_width, 1.0);

    let material = RawNewDesignForm {
        material_index: 1,
        ..raw("1.35", "")
    };
    let form = NewDesignForm::from_raw(&material).expect("valid");
    assert_eq!(
        form.material_name,
        Some(builtin_preset_names()[1].clone()),
        "index 1 is the first built-in"
    );
}

#[test]
fn raw_form_values_that_are_wrong_name_the_field() {
    let ratio = NewDesignForm::from_raw(&raw("long", "")).expect_err("not a number");
    assert!(
        ratio.contains("Length over width") && ratio.contains("long"),
        "{ratio}"
    );
    let width = NewDesignForm::from_raw(&raw("1.35", "-2")).expect_err("negative");
    assert!(width.contains("Stone width"), "{width}");
    let shape = RawNewDesignForm {
        shape_index: 9,
        ..raw("1.35", "")
    };
    assert!(NewDesignForm::from_raw(&shape).is_err());
    let symmetry = RawNewDesignForm {
        symmetry: -1,
        ..raw("1.35", "")
    };
    assert!(NewDesignForm::from_raw(&symmetry).is_err());
}

#[test]
fn creating_from_the_form_sets_the_stone_width() {
    let (mut state, _mock) = brilliant();
    let form = NewDesignForm {
        width_mm: Some(6.5),
        gear_teeth: 80,
        symmetry_order: 8,
        ..NewDesignForm::default()
    };
    new_from_form(&mut state, &form).expect("valid form");
    assert_eq!(tier_count(&state), 0);
    assert_eq!(state.session.design.girdle_diameter_mm, Some(6.5));
    assert_eq!(state.session.design.meta.gear_teeth, 80);
    assert_eq!(state.document.origin, DocumentOrigin::New);

    let bad = NewDesignForm {
        symmetry_order: 7,
        ..form
    };
    assert!(new_from_form(&mut state, &bad).is_err());
    assert_eq!(
        state.session.design.meta.gear_teeth, 80,
        "unchanged on error"
    );
}

#[test]
fn a_design_file_survives_open_save_open() {
    let (mut state, _mock) = brilliant();
    let saved = save_bytes(&mut state).expect("saves");
    assert_eq!(saved.file.mime, mime::INDICATRIX_DESIGN);
    assert_eq!(saved.file.suggested_name, "Standard Round Brilliant.isdd");
    assert_eq!(saved.warning, None);
    let first = plain_text(&state);

    let (mut other, _other_mock) = self::state();
    let report = open_picked(
        &mut other,
        vec![picked(&saved.file.suggested_name, &saved.file.bytes)],
    )
    .expect("opens");
    assert_eq!(report.label, "Standard Round Brilliant.isdd");
    assert!(!report.is_warning(), "{}", report.message());
    assert_eq!(plain_text(&other), first);
    assert!(!other.session.is_dirty());
    assert_eq!(
        other.document.asc_filename.as_deref(),
        Some("Standard Round Brilliant.asc")
    );
    assert!(
        !other.document.metadata.id.is_empty(),
        "the save stamped an id"
    );

    // The id is kept by the second save.
    let id = other.document.metadata.id.clone();
    save_bytes(&mut other).expect("saves again");
    assert_eq!(other.document.metadata.id, id);
}

#[test]
fn saving_stamps_the_document_metadata() {
    let (mut state, _mock) = brilliant();
    assert_eq!(state.document.metadata.id.len(), 0);
    save_bytes(&mut state).expect("saves");
    assert_ne!(state.document.metadata.id.len(), 0);
    assert!(state.document.metadata.modified_at.ends_with('Z'));
    assert_ne!(state.document.metadata.created_at.len(), 0);
}

#[test]
fn mark_saved_makes_the_document_that_file() {
    let (mut state, _mock) = brilliant();
    edit_girdle(&mut state, 7.0);
    assert!(state.session.is_dirty());
    mark_saved(&mut state, "Mine.isdd");
    assert!(!state.session.is_dirty());
    assert_eq!(state.document.display_name.as_deref(), Some("Mine.isdd"));
    assert_eq!(state.document.asc_filename.as_deref(), Some("Mine.asc"));
    assert_eq!(
        state.document.origin,
        DocumentOrigin::Document {
            display_name: "Mine.isdd".to_owned()
        }
    );
    let saved = save_bytes(&mut state).expect("saves");
    assert_eq!(saved.file.suggested_name, "Mine.isdd");
}

#[test]
fn the_asc_export_opens_again() {
    let (state, _mock) = brilliant();
    let exported = asc_bytes(&state).expect("a template solves and exports");
    assert_eq!(exported.file.mime, mime::ASC);
    assert_eq!(exported.file.suggested_name, "Standard Round Brilliant.asc");
    assert_eq!(exported.warning, None);

    let (mut other, _other_mock) = self::state();
    let report = open_picked(
        &mut other,
        vec![picked(&exported.file.suggested_name, &exported.file.bytes)],
    )
    .expect("opens");
    assert_eq!(report.label, "Standard Round Brilliant.asc");
    assert_eq!(tier_count(&other), tier_count(&state));
    assert!(other.document.original_asc_text.is_some());
    assert_eq!(
        other.document.asc_filename.as_deref(),
        Some("Standard Round Brilliant.asc")
    );
}

#[test]
fn a_legacy_indicatrix_file_opens_like_an_isdd_file() {
    let (mut origin, _origin_mock) = brilliant();
    let saved = save_bytes(&mut origin).expect("saves");
    assert!(
        std::path::Path::new(&saved.file.suggested_name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("isdd"))
    );

    let (mut other, _other_mock) = self::state();
    let report = open_picked(
        &mut other,
        vec![picked("Old Design.indicatrix", &saved.file.bytes)],
    )
    .expect("a legacy file opens");
    assert_eq!(report.label, "Old Design.indicatrix");
    assert_eq!(tier_count(&other), tier_count(&origin));
    assert_eq!(
        other.document.asc_filename.as_deref(),
        Some("Old Design.asc")
    );
}

#[test]
fn an_older_sidecar_is_refused_with_a_sentence() {
    let (mut state, _mock) = self::state();
    let message = open_picked(
        &mut state,
        vec![
            picked("pair.indicatrix.toml", b"format_version = 1"),
            picked("pair.asc", TINY_ASC.as_bytes()),
        ],
    )
    .expect("the .asc still opens");
    assert_eq!(message.label, "pair.asc");
    assert_eq!(message.skipped.len(), 1, "{message:?}");

    let alone = open_picked(
        &mut state,
        vec![picked("pair.indicatrix.toml", b"format_version = 1")],
    )
    .expect_err("a sidecar alone is no design");
    assert!(alone.message.contains("pair.indicatrix.toml"), "{alone}");
}

#[test]
fn the_design_file_wins_and_the_rest_are_named() {
    let (state, _mock) = brilliant();
    let asc = asc_bytes(&state).expect("exports");
    let (mut origin, _origin_mock) = brilliant();
    let design = save_bytes(&mut origin).expect("saves");

    let (mut other, _other_mock) = self::state();
    let report = open_picked(
        &mut other,
        vec![
            picked("notes.txt", b"hello"),
            picked("a.asc", &asc.file.bytes),
            picked(&design.file.suggested_name, &design.file.bytes),
        ],
    )
    .expect("opens the design file");
    assert_eq!(report.label, design.file.suggested_name);
    assert_eq!(report.ignored, vec!["a.asc".to_owned()]);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].contains("notes.txt"));
    assert!(report.is_warning());
    let message = report.message();
    assert!(message.contains("not opened: a.asc"), "{message}");
}

#[test]
fn bad_files_are_readable_errors_and_leave_the_design_alone() {
    let (mut state, _mock) = brilliant();
    let before = plain_text(&state);
    let generation = state.session.current_generation();

    let gem = open_picked(&mut state, vec![picked("x.gem", &[1, 2, 3])]).expect_err("not a .gem");
    assert!(
        gem.message
            .starts_with("Cannot open \"x.gem\": not a readable .gem file"),
        "{gem}"
    );
    let junk = open_picked(&mut state, vec![picked("y.indicatrix", b"hello")])
        .expect_err("not a design file");
    assert!(junk.message.contains("y.indicatrix"), "{junk}");
    let text = open_picked(&mut state, vec![picked("z.txt", b"hello")]).expect_err("no design");
    assert!(text.message.contains("z.txt"), "{text}");
    let empty = open_picked(&mut state, Vec::new()).expect_err("nothing picked");
    assert_ne!(empty.message.len(), 0);
    let map = open_picked(&mut state, vec![picked("sky.hdr", b"#?RADIANCE")])
        .expect_err("a map is no design");
    assert!(map.message.contains("environment map"), "{map}");

    assert_eq!(plain_text(&state), before);
    assert_eq!(state.session.current_generation(), generation);
}

#[test]
fn a_file_over_the_size_cap_is_refused() {
    let (mut state, _mock) = self::state();
    let big = vec![b'a'; 16 * 1024 * 1024 + 1];
    let error = open_picked(&mut state, vec![picked("big.asc", &big)]).expect_err("too large");
    assert!(error.message.contains("limited to 16 MiB"), "{error}");
}

#[test]
fn opening_replaces_the_design_and_forgets_the_selection() {
    let (mut state, _mock) = brilliant();
    state.selected_tier = Some(1);
    let before = state.session.current_generation();
    let report = open_picked(&mut state, vec![picked("x.asc", TINY_ASC.as_bytes())])
        .expect("the tiny schedule opens");
    assert_eq!(report.label, "x.asc");
    assert_eq!(tier_count(&state), 1);
    assert_eq!(state.selected_tier, None);
    assert!(state.session.current_generation() > before);
    assert!(!state.session.is_dirty());
    assert_eq!(
        state.document.origin,
        DocumentOrigin::Document {
            display_name: "x.asc".to_owned()
        }
    );
    // The file has no title line, so the title is the file's name without extension.
    assert_eq!(display_title(&state), "x");
}

#[test]
fn opening_records_a_recent_item() {
    let (mut state, _mock) = self::state();
    open_picked(&mut state, vec![picked("x.asc", TINY_ASC.as_bytes())]).expect("opens");
    let recent = &state.settings.recent;
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].title, "x");
    assert_eq!(
        recent[0].kind,
        crate::settings::RecentKind::Document {
            display_name: "x.asc".to_owned()
        }
    );
}

#[test]
fn a_clean_asc_report_reads_as_good_news() {
    let (mut state, _mock) = self::state();
    let report =
        open_picked(&mut state, vec![picked("x.asc", TINY_ASC.as_bytes())]).expect("opens");
    assert!(!report.is_warning());
    assert_eq!(report.message(), "Opened x.asc.");
}

#[test]
fn the_autosave_skips_an_untouched_start_up_design() {
    let (mut state, mock) = self::state();
    autosave(&mut state).expect("nothing to write is fine");
    assert!(!mock.app_dirs().files.join("autosave").exists());
}

#[test]
fn the_autosave_writes_only_when_something_changed() {
    let (mut state, mock) = brilliant();
    let design_path = mock.app_dirs().files.join("autosave").join("current.isdd");
    autosave(&mut state).expect("writes");
    assert!(design_path.is_file());
    assert!(design_path.with_file_name("current.json").is_file());

    fs::remove_file(&design_path).expect("removable");
    autosave(&mut state).expect("nothing changed");
    assert!(
        !design_path.exists(),
        "an unchanged design is not rewritten"
    );

    edit_girdle(&mut state, 7.0);
    autosave(&mut state).expect("writes again");
    assert!(design_path.is_file());
}

#[test]
fn the_autosave_restores_an_unsaved_design() {
    let (mut state, mock) = brilliant();
    edit_girdle(&mut state, 7.0);
    mark_saved(&mut state, "Mine.isdd");
    edit_girdle(&mut state, 8.0);
    assert!(state.session.is_dirty());
    autosave(&mut state).expect("writes");
    let expected = plain_text(&state);

    let mut restarted = state_on(&mock);
    assert!(!restarted.document.has_design(&restarted.session.design));
    let report = restore(&mut restarted).expect("restores");
    assert!(report.dirty);
    assert_eq!(report.title, "Standard Round Brilliant");
    assert_eq!(plain_text(&restarted), expected);
    assert!(restarted.session.is_dirty());
    assert_eq!(
        restarted.document.origin,
        DocumentOrigin::Document {
            display_name: "Mine.isdd".to_owned()
        }
    );
    assert_eq!(
        restarted.document.display_name.as_deref(),
        Some("Mine.isdd")
    );
    // Nothing changed since the restore, so the next autosave writes nothing.
    let design_path = mock.app_dirs().files.join("autosave").join("current.isdd");
    fs::remove_file(&design_path).expect("removable");
    autosave(&mut restarted).expect("cheap");
    assert!(!design_path.exists());
}

#[test]
fn a_clean_design_restores_clean() {
    let (mut state, mock) = self::state();
    open_picked(&mut state, vec![picked("x.asc", TINY_ASC.as_bytes())]).expect("opens");
    autosave(&mut state).expect("writes");
    let mut restarted = state_on(&mock);
    let report = restore(&mut restarted).expect("restores");
    assert!(!report.dirty);
    assert!(!restarted.session.is_dirty());
    assert_eq!(tier_count(&restarted), 1);
}

#[test]
fn restoring_without_files_does_nothing() {
    let (mut state, _mock) = self::state();
    assert!(restore(&mut state).is_none());
    assert!(!state.document.has_design(&state.session.design));
}

#[test]
fn a_damaged_autosave_is_moved_aside() {
    let (mut state, mock) = self::state();
    let dir = mock.app_dirs().files.join("autosave");
    fs::create_dir_all(&dir).expect("creates");
    fs::write(dir.join("current.isdd"), b"this is not a design").expect("writes");
    fs::write(dir.join("current.json"), b"{").expect("writes");
    assert!(restore(&mut state).is_none());
    assert!(!dir.join("current.isdd").exists());
    let moved: Vec<String> = fs::read_dir(&dir)
        .expect("reads")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert!(
        moved
            .iter()
            .any(|name| name.starts_with("current.isdd.corrupt-")),
        "{moved:?}"
    );
    assert!(!state.document.has_design(&state.session.design));
}

#[test]
fn a_design_file_without_its_note_restores_as_a_new_design() {
    let (mut state, mock) = brilliant();
    autosave(&mut state).expect("writes");
    let dir = mock.app_dirs().files.join("autosave");
    fs::remove_file(dir.join("current.json")).expect("removable");
    let mut restarted = state_on(&mock);
    let report = restore(&mut restarted).expect("restores");
    assert!(!report.dirty);
    assert_eq!(restarted.document.origin, DocumentOrigin::New);
    assert_eq!(tier_count(&restarted), tier_count(&state));
}

#[test]
fn custom_materials_survive_a_new_design_and_never_shadow_a_built_in() {
    let (mut state, _mock) = self::state();
    let mut material = indicatrix::optics::materials::GemMaterial::all_materials()
        .into_iter()
        .next()
        .expect("there are built-ins");
    assert!(
        state
            .document
            .register_custom_material(material.clone())
            .is_err()
    );
    material.name = "My Glass".to_owned();
    state
        .document
        .register_custom_material(material.clone())
        .expect("a new name registers");
    state
        .document
        .register_custom_material(material)
        .expect("the same name replaces");
    assert_eq!(state.document.custom_materials.len(), 1);
    new_from_template(&mut state, 1).expect("template");
    assert_eq!(state.document.custom_materials.len(), 1);
}

#[test]
fn closing_a_design_returns_to_the_empty_start_state() {
    let (mut state, mock) = brilliant();
    edit_girdle(&mut state, 6.0);
    assert!(state.document.has_design(&state.session.design));
    assert!(state.session.is_dirty());
    autosave(&mut state).expect("the autosave writes");
    let note = mock.app_dirs().files.join("autosave").join("current.json");
    assert!(note.is_file());
    let generation = state.session.current_generation();
    state.selected_tier = Some(0);

    close_design(&mut state);

    assert!(!state.document.has_design(&state.session.design));
    assert_eq!(state.document.origin, DocumentOrigin::Startup);
    assert_eq!(tier_count(&state), 0);
    assert!(!state.session.is_dirty());
    assert!(state.session.history.description_log().is_empty());
    assert_eq!(state.selected_tier, None);
    // Jobs in flight see a new generation, and the closed design does not come back.
    assert!(state.session.current_generation() > generation);
    assert!(!note.exists());
    autosave(&mut state).expect("nothing to write");
    assert!(!note.exists());
}
