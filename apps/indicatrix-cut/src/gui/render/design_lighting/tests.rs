use super::{
    opening::Decision,
    stored::{EXPOSURE_RANGE, FORMAT_VERSION, env_source, known_rig},
    *,
};
use crate::settings::{
    SettingsFile,
    model::{AppSettings, Backdrop},
};

const DESIGN: &str = "0b9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10";

fn tent() -> LightingValues {
    LightingValues {
        version: FORMAT_VERSION,
        lighting_rig: "Light tent + black cards".to_string(),
        light_yaw_deg: 120.0,
        light_pitch_deg: 40.0,
        exposure: 1.5,
        surface_glare: 0.5,
        backdrop: Backdrop::White,
        env_map_path: Some("C:/hdr/studio.hdr".to_string()),
    }
}

fn memory_db() -> Mutex<Database> {
    Mutex::new(Database::new(Some(":memory:")).expect("open an in-memory library"))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

// --- the stored JSON ---

#[test]
fn the_stored_values_round_trip_through_json() {
    let values = tent();
    let json = values.to_json().expect("serialises");
    assert_eq!(LightingValues::from_json(&json), Ok(values));
}

#[test]
fn a_row_with_only_the_required_keys_gets_defaults_for_the_rest() {
    let json = r#"{"lighting_rig":"Gem Studio Ring Lights","light_yaw_deg":10.0,
                       "light_pitch_deg":30.0,"exposure":1.0}"#;
    let values = LightingValues::from_json(json).expect("loads");
    assert_eq!(values.version, FORMAT_VERSION);
    assert!(close(values.surface_glare, 1.0), "full glare by default");
    assert_eq!(values.backdrop, Backdrop::default());
    assert_eq!(values.env_map_path, None);
}

#[test]
fn keys_a_newer_version_added_are_ignored() {
    let json = r#"{"version":7,"lighting_rig":"ISO hemisphere","light_yaw_deg":10.0,
                       "light_pitch_deg":30.0,"exposure":1.0,"a_new_setting":{"x":1}}"#;
    let values = LightingValues::from_json(json).expect("loads");
    assert_eq!(values.lighting_rig, "ISO hemisphere");
}

#[test]
fn text_that_is_not_the_stored_format_does_not_load() {
    assert!(LightingValues::from_json("not json").is_err());
    assert!(
        LightingValues::from_json("{}").is_err(),
        "the rig is required"
    );
    assert!(LightingValues::from_json(r#"{"lighting_rig":"x"}"#).is_err());
}

#[test]
fn values_are_limited_like_the_live_controls_and_never_not_a_number() {
    let wild = LightingValues {
        light_yaw_deg: 400.0,
        light_pitch_deg: 0.0,
        exposure: 99.0,
        surface_glare: 7.0,
        env_map_path: Some("   ".to_string()),
        ..tent()
    }
    .sanitised();
    assert!(
        close(wild.light_yaw_deg, 40.0),
        "yaw wraps: {}",
        wild.light_yaw_deg
    );
    assert!(
        close(wild.light_pitch_deg, PITCH_RANGE_RAD.0.to_degrees()),
        "pitch is raised to the lowest the controls allow: {}",
        wild.light_pitch_deg
    );
    assert!(close(wild.exposure, EXPOSURE_RANGE.1));
    assert!(close(wild.surface_glare, 1.0));
    assert_eq!(wild.env_map_path, None, "a blank path is no path");

    let nan = LightingValues {
        light_yaw_deg: f32::NAN,
        light_pitch_deg: f32::INFINITY,
        exposure: f32::NAN,
        ..tent()
    }
    .sanitised();
    let defaults = AppSettings::default();
    assert!(close(nan.light_yaw_deg, defaults.light_yaw_deg));
    assert!(nan.light_pitch_deg.is_finite());
    assert!(close(nan.exposure, defaults.exposure));
}

#[test]
fn the_normal_lighting_is_read_from_the_settings() {
    let mut app = AppSettings::default();
    let normal = LightingValues::from_app_settings(&app);
    assert_eq!(normal.lighting_rig, app.lighting_rig);
    assert!(close(normal.light_yaw_deg, app.light_yaw_deg));
    assert_eq!(normal.env_map_path, None, "no map is stored as no path");

    app.env_map_path = "C:/hdr/studio.hdr".to_string();
    app.backdrop = Backdrop::White;
    let with_map = LightingValues::from_app_settings(&app);
    assert_eq!(with_map.env_map_path.as_deref(), Some("C:/hdr/studio.hdr"));
    assert_eq!(with_map.backdrop, Backdrop::White);
}

#[test]
fn the_live_view_is_captured_from_the_render_context() {
    let ctx = RenderContext {
        light_yaw: 120.0_f32.to_radians(),
        light_pitch: 40.0_f32.to_radians(),
        exposure: 1.5,
        lighting_preset: LightingPreset::LightTent,
        surface_glare: 0.5,
        backdrop: Backdrop::White,
        ..RenderContext::default()
    };
    let values = LightingValues::from_live(&ctx, Some("C:/hdr/studio.hdr".to_string()));
    assert_eq!(values.lighting_rig, "Light tent + black cards");
    assert!(
        close(values.light_yaw_deg, 120.0),
        "{}",
        values.light_yaw_deg
    );
    assert!(
        close(values.light_pitch_deg, 40.0),
        "{}",
        values.light_pitch_deg
    );
    assert!(close(values.exposure, 1.5));
    assert!(close(values.surface_glare, 0.5));
    assert_eq!(values.backdrop, Backdrop::White);
    assert_eq!(values.env_map_path.as_deref(), Some("C:/hdr/studio.hdr"));
}

#[test]
fn a_path_typed_but_never_loaded_is_not_stored() {
    assert_eq!(env_path_for_capture(false, "C:/hdr/studio.hdr"), None);
    assert_eq!(env_path_for_capture(true, "   "), None);
    assert_eq!(
        env_path_for_capture(true, "  C:/hdr/studio.hdr "),
        Some("C:/hdr/studio.hdr".to_string())
    );
}

#[test]
fn the_summary_says_what_was_saved_in_plain_words() {
    let text = tent().summary();
    assert!(text.contains("Light tent + black cards"), "{text}");
    assert!(text.contains("120°"), "{text}");
    assert!(text.contains("40°"), "{text}");
    assert!(text.contains("1.50x"), "{text}");
    assert!(text.contains("HDR environment map"), "{text}");
    assert!(text.ends_with('.'), "{text}");

    let plain = LightingValues {
        env_map_path: None,
        ..tent()
    }
    .summary();
    assert!(!plain.contains("HDR"), "{plain}");
}

// --- is the rig still there ---

#[test]
fn every_rig_this_build_has_is_known_under_its_own_label() {
    for rig in LightingPreset::ALL {
        // A UV lamp is a rig only of a build with the `physical-color` feature.
        let expected = (!rig.is_uv_lamp() || cfg!(feature = "physical-color")).then_some(rig);
        assert_eq!(known_rig(rig.label()), expected, "{}", rig.label());
    }
}

#[test]
fn older_labels_the_parser_migrates_are_still_known() {
    assert_eq!(
        known_rig("D65 Daylight (5500K)"),
        Some(LightingPreset::Daylight)
    );
    assert_eq!(
        known_rig("ISO hemisphere (GemRay-style)"),
        Some(LightingPreset::IsoHemisphere)
    );
    assert_eq!(
        known_rig("Soft dome + ring lights"),
        Some(LightingPreset::LightTent)
    );
}

#[test]
fn a_rig_this_build_does_not_have_is_not_known() {
    assert_eq!(known_rig("Ring flash"), None);
    assert_eq!(known_rig(""), None);
}

// --- reading a row ---

#[test]
fn a_stored_row_becomes_a_lookup() {
    assert_eq!(lookup_from_row(None), Lookup::NoRow);
    let json = tent().to_json().expect("serialises");
    assert_eq!(lookup_from_row(Some(&json)), Lookup::Row(tent()));

    let gone = LightingValues {
        lighting_rig: "Ring flash".to_string(),
        ..tent()
    }
    .to_json()
    .expect("serialises");
    assert_eq!(
        lookup_from_row(Some(&gone)),
        Lookup::UnknownRig("Ring flash".to_string())
    );
    assert!(matches!(
        lookup_from_row(Some("garbage")),
        Lookup::Unreadable(_)
    ));
}

#[test]
fn only_a_missing_row_has_no_row() {
    assert!(!Lookup::NoRow.has_row());
    assert!(Lookup::Row(tent()).has_row());
    assert!(Lookup::UnknownRig("x".to_string()).has_row());
    assert!(Lookup::Unreadable("x".to_string()).has_row());
}

#[test]
fn the_dialog_line_explains_an_unusable_row() {
    assert_eq!(Lookup::NoRow.summary_text(), "");
    assert_eq!(Lookup::Row(tent()).summary_text(), tent().summary());
    let unknown = Lookup::UnknownRig("Ring flash".to_string()).summary_text();
    assert!(unknown.contains("Ring flash"), "{unknown}");
    assert!(unknown.contains("normal lighting"), "{unknown}");
    let unreadable = Lookup::Unreadable("bad".to_string()).summary_text();
    assert!(unreadable.contains("bad"), "{unreadable}");
}

// --- what opening a design does ---

#[test]
fn saved_lighting_is_applied_whether_or_not_another_designs_is_showing() {
    for active in [false, true] {
        let decision = decide(&Lookup::Row(tent()), active);
        assert_eq!(decision.plan, Plan::Apply(tent()));
        assert_eq!(decision.note, Some(Note::UsingSaved));
    }
}

#[test]
fn a_design_without_lighting_changes_nothing_unless_another_designs_is_showing() {
    assert_eq!(
        decide(&Lookup::NoRow, false),
        Decision {
            plan: Plan::Keep,
            note: None
        }
    );
    assert_eq!(
        decide(&Lookup::NoRow, true),
        Decision {
            plan: Plan::Restore,
            note: Some(Note::BackToNormal)
        }
    );
}

#[test]
fn unusable_lighting_falls_back_to_the_normal_lighting_and_says_so() {
    let unknown = Lookup::UnknownRig("Ring flash".to_string());
    assert_eq!(
        decide(&unknown, false),
        Decision {
            plan: Plan::Keep,
            note: Some(Note::UnknownRig("Ring flash".to_string()))
        }
    );
    assert_eq!(decide(&unknown, true).plan, Plan::Restore);

    let unreadable = Lookup::Unreadable("bad".to_string());
    assert_eq!(decide(&unreadable, false).plan, Plan::Keep);
    assert_eq!(decide(&unreadable, true).plan, Plan::Restore);
    assert_eq!(
        decide(&unreadable, true).note,
        Some(Note::Unreadable("bad".to_string()))
    );
}

#[test]
fn the_notes_are_plain_sentences_and_problems_are_warnings() {
    assert_eq!(
        note_text(&Note::UsingSaved),
        ("Using this design's saved lighting.".to_string(), "info")
    );
    assert_eq!(note_text(&Note::BackToNormal).1, "info");
    let (text, kind) = note_text(&Note::UnknownRig("Ring flash".to_string()));
    assert!(text.contains("Ring flash"), "{text}");
    assert_eq!(kind, "warning");
    assert_eq!(note_text(&Note::Unreadable("bad".to_string())).1, "warning");
}

#[test]
fn a_problem_is_reported_once_per_design() {
    let mut told = HashSet::new();
    assert!(first_time_for(&mut told, DESIGN));
    assert!(!first_time_for(&mut told, DESIGN), "not again");
    assert!(first_time_for(
        &mut told,
        "1c9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10"
    ));
}

#[test]
fn a_note_never_replaces_a_warning_or_an_error_on_screen() {
    assert!(may_show_over(false, "error"), "nothing is showing");
    assert!(may_show_over(true, "info"));
    assert!(may_show_over(true, "success"));
    assert!(!may_show_over(true, "warning"));
    assert!(!may_show_over(true, "error"));
}

// --- the library ---

#[test]
fn saved_lighting_is_read_back_and_forgotten() {
    let db = memory_db();
    assert_eq!(read_lookup(&db, DESIGN), Lookup::NoRow);

    store_values(&db, DESIGN, &tent(), 1_700_000_000).expect("stores");
    assert_eq!(read_lookup(&db, DESIGN), Lookup::Row(tent()));

    let changed = LightingValues {
        exposure: 2.0,
        ..tent()
    };
    store_values(&db, DESIGN, &changed, 1_700_000_100).expect("replaces");
    assert_eq!(read_lookup(&db, DESIGN), Lookup::Row(changed));

    forget_values(&db, DESIGN).expect("forgets");
    assert_eq!(read_lookup(&db, DESIGN), Lookup::NoRow);
    forget_values(&db, DESIGN).expect("forgetting nothing is not an error");
}

#[test]
fn the_row_is_filed_under_the_rig_name_and_the_design() {
    let db = memory_db();
    store_values(&db, DESIGN, &tent(), 5).expect("stores");
    let row = db
        .lock()
        .expect("lock")
        .design_lighting(DESIGN)
        .expect("reads")
        .expect("a row");
    assert_eq!(row.preset_name, "Light tent + black cards");
    assert_eq!(row.updated_at, 5);
    // Another design has its own (no) lighting.
    assert_eq!(
        read_lookup(&db, "1c9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10"),
        Lookup::NoRow
    );
}

#[test]
fn a_stored_rig_this_build_lacks_reads_as_unknown() {
    let db = memory_db();
    let gone = LightingValues {
        lighting_rig: "Ring flash".to_string(),
        ..tent()
    };
    store_values(&db, DESIGN, &gone, 1).expect("stores");
    assert_eq!(
        read_lookup(&db, DESIGN),
        Lookup::UnknownRig("Ring flash".to_string())
    );
}

#[test]
fn a_damaged_row_and_a_bad_design_id_are_reported_not_trusted() {
    let db = memory_db();
    db.lock()
        .expect("lock")
        .set_design_lighting(DESIGN, "Light tent + black cards", "{broken", 1)
        .expect("the library keeps the text as given");
    assert!(matches!(read_lookup(&db, DESIGN), Lookup::Unreadable(_)));

    assert!(store_values(&db, "not-a-uuid", &tent(), 1).is_err());
    assert!(forget_values(&db, "not-a-uuid").is_err());
    assert!(matches!(
        read_lookup(&db, "not-a-uuid"),
        Lookup::Unreadable(_)
    ));
}

// --- the session ---

fn remember_normal() {
    SESSION.with(|session| {
        session.borrow_mut().normal = Some(Normal {
            values: tent(),
            env_map: None,
            env_ui: EnvUi {
                loaded: false,
                status: String::new(),
                path_text: String::new(),
            },
        });
    });
}

#[test]
fn choosing_a_named_preset_lets_go_of_the_remembered_normal_lighting() {
    assert!(
        !override_active(),
        "a fresh session shows no design lighting"
    );
    remember_normal();
    assert!(override_active());
    normal_lighting_chosen();
    assert!(
        !override_active(),
        "the preset is the normal lighting now; nothing is restored over it later"
    );
}

#[test]
fn a_fresh_session_has_no_open_design_and_no_handles() {
    assert_eq!(open_design(), None);
    assert!(handles().is_none(), "the hook does nothing before setup");
}

// --- where the HDR map comes from ---

#[test]
fn a_map_that_is_loaded_already_is_not_decoded_again() {
    assert_eq!(
        env_source(Some("C:/hdr/studio.hdr"), Some("C:/hdr/studio.hdr")),
        EnvSource::AlreadyLoaded
    );
    // The dialog's field may carry blanks at the ends.
    assert_eq!(
        env_source(Some(" C:/hdr/studio.hdr"), Some("C:/hdr/studio.hdr  ")),
        EnvSource::AlreadyLoaded
    );
}

#[test]
fn another_map_or_none_loaded_is_decoded_and_no_map_is_the_studio_rig() {
    assert_eq!(
        env_source(Some("C:/hdr/other.hdr"), Some("C:/hdr/studio.hdr")),
        EnvSource::Decode("C:/hdr/other.hdr".to_string())
    );
    assert_eq!(
        env_source(Some(" C:/hdr/studio.hdr "), None),
        EnvSource::Decode("C:/hdr/studio.hdr".to_string()),
        "nothing loaded yet; the path is read as typed, without the blanks"
    );
    assert_eq!(env_source(None, None), EnvSource::Studio);
    assert_eq!(
        env_source(None, Some("C:/hdr/studio.hdr")),
        EnvSource::Studio,
        "the design uses no map, so the loaded one goes"
    );
}

#[test]
fn a_map_typed_but_never_loaded_is_not_taken_for_the_loaded_one() {
    // `loaded_env_path` hands `env_path_for_capture`'s answer to `env_source`.
    let loaded = env_path_for_capture(false, "C:/hdr/studio.hdr");
    assert_eq!(
        env_source(Some("C:/hdr/studio.hdr"), loaded.as_deref()),
        EnvSource::Decode("C:/hdr/studio.hdr".to_string())
    );
}

// --- answers that arrive late ---

#[test]
fn an_answer_for_an_earlier_opening_is_dropped() {
    let first = begin_generation();
    assert!(generation_is_current(first));
    let second = begin_generation();
    assert!(!generation_is_current(first), "another design opened");
    assert!(generation_is_current(second));
    assert!(answer_is_current(3, 3));
    assert!(!answer_is_current(3, 4));
}

#[test]
fn choosing_a_named_preset_voids_what_is_on_its_way() {
    let asked = begin_generation();
    remember_normal();
    normal_lighting_chosen();
    assert!(
        !generation_is_current(asked),
        "a map decoded for the design must not land over the preset"
    );
    assert!(!override_active());
}

// --- saving the lighting while a map is still being decoded ---

#[test]
fn the_map_being_decoded_is_stored_instead_of_the_one_still_on_screen() {
    let studio = Some("C:/hdr/studio.hdr".to_string());
    let big = Some("C:/hdr/big.hdr".to_string());
    // The dialog still shows studio.hdr while big.hdr, the one the lighting names, decodes.
    assert_eq!(
        env_path_to_store(Some(" C:/hdr/big.hdr "), true, "C:/hdr/studio.hdr"),
        big
    );
    assert_eq!(
        env_path_to_store(Some("C:/hdr/big.hdr"), false, ""),
        big,
        "no map was loaded before; the one on its way still counts"
    );
    // Nothing on its way: the map in use, exactly as before.
    assert_eq!(env_path_to_store(None, true, " C:/hdr/studio.hdr "), studio);
    assert_eq!(env_path_to_store(None, false, "C:/hdr/typed.hdr"), None);
    assert_eq!(env_path_to_store(None, true, "  "), None);
    // A blank pending path is no path.
    assert_eq!(
        env_path_to_store(Some("  "), true, "C:/hdr/studio.hdr"),
        studio
    );
}

#[test]
fn a_decode_on_its_way_is_remembered_until_something_newer_begins() {
    assert_eq!(pending_env_path(), None, "a fresh session decodes nothing");
    set_pending_env_path(Some("C:/hdr/big.hdr".to_string()));
    assert_eq!(pending_env_path().as_deref(), Some("C:/hdr/big.hdr"));

    // Another design opening voids it: its map is not the one the lighting names any more.
    begin_generation();
    assert_eq!(pending_env_path(), None);

    // So does choosing a named lighting preset.
    set_pending_env_path(Some("C:/hdr/big.hdr".to_string()));
    remember_normal();
    normal_lighting_chosen();
    assert_eq!(pending_env_path(), None);

    // And the decode's own answer clears it.
    set_pending_env_path(Some("C:/hdr/big.hdr".to_string()));
    set_pending_env_path(None);
    assert_eq!(pending_env_path(), None);
}

// --- the cutter's own map beats a decode on its way ---

#[test]
fn a_map_the_cutter_loads_while_a_decode_is_on_its_way_is_the_one_that_is_stored() {
    let opening = begin_generation();
    let decode = begin_env_decode("C:/hdr/big.hdr");
    assert_eq!(pending_env_path().as_deref(), Some("C:/hdr/big.hdr"));
    assert!(env_decode_is_current(decode));
    // Before the cutter acts, the decode's path wins over the map the dialog still shows.
    assert_eq!(
        env_path_to_store(pending_env_path().as_deref(), true, "C:/hdr/small.hdr"),
        Some("C:/hdr/big.hdr".to_string())
    );

    // The cutter loads small.hdr through the dialog: the newest choice.
    env_map_chosen();
    assert!(
        !env_decode_is_current(decode),
        "big.hdr must not land over the map the cutter just loaded"
    );
    assert_eq!(pending_env_path(), None);
    assert_eq!(
        env_path_to_store(pending_env_path().as_deref(), true, "C:/hdr/small.hdr"),
        Some("C:/hdr/small.hdr".to_string()),
        "the stored map is the one the dialog shows"
    );
    assert!(
        generation_is_current(opening),
        "the rest of the design's lighting, and a lookup still waiting, are not voided"
    );
}

#[test]
fn clearing_the_map_by_hand_voids_a_decode_on_its_way_too() {
    let decode = begin_env_decode("C:/hdr/big.hdr");
    env_map_chosen();
    assert!(!env_decode_is_current(decode));
    assert_eq!(pending_env_path(), None);
}

// --- the cutter's own map beats the saved lighting's, before its decode starts (F1-17) ---

#[test]
fn a_map_chosen_by_hand_while_the_lookup_waits_is_not_replaced_by_the_saved_map() {
    // The design opens and the library is busy: the lookup moves to a thread.
    let opening = begin_generation();
    assert!(
        !map_chosen_since_open(),
        "nothing chosen yet: the saved lighting's map may be used"
    );

    // The cutter loads small.hdr through the settings dialog meanwhile.
    env_map_chosen();
    assert!(map_chosen_since_open());
    assert!(
        generation_is_current(opening),
        "the lookup itself still counts: the rest of the saved lighting is applied"
    );

    // The lookup lands, naming big.hdr: no decode starts, and nothing clears the cutter's map.
    for wanted in [Some("C:/hdr/big.hdr"), Some("C:/hdr/small.hdr"), None] {
        assert_eq!(
            env_source_for_opening(map_chosen_since_open(), wanted, Some("C:/hdr/small.hdr")),
            EnvSource::ChosenByHand,
            "saved map {wanted:?}"
        );
    }
    // Nor is a decode of the saved map ever started, so there is nothing to land over it.
    assert_eq!(pending_env_path(), None);
}

#[test]
fn clearing_the_map_by_hand_while_the_lookup_waits_is_kept_too() {
    begin_generation();
    env_map_chosen();
    assert_eq!(
        env_source_for_opening(map_chosen_since_open(), Some("C:/hdr/big.hdr"), None),
        EnvSource::ChosenByHand,
        "the cutter cleared the map; the saved one does not come back"
    );
}

#[test]
fn the_saved_map_is_used_when_the_cutter_chose_none_since_the_opening_began() {
    begin_generation();
    assert_eq!(
        env_source_for_opening(
            map_chosen_since_open(),
            Some("C:/hdr/big.hdr"),
            Some("C:/hdr/small.hdr")
        ),
        EnvSource::Decode("C:/hdr/big.hdr".to_string())
    );
    assert_eq!(
        env_source_for_opening(map_chosen_since_open(), None, Some("C:/hdr/small.hdr")),
        EnvSource::Studio
    );
}

#[test]
fn a_map_chosen_before_the_opening_began_is_not_a_choice_since() {
    env_map_chosen();
    begin_generation();
    assert!(
        !map_chosen_since_open(),
        "the earlier choice is what the lighting is applied over"
    );
}

#[test]
fn the_openings_own_decode_is_not_a_choice_of_the_cutters() {
    begin_generation();
    begin_env_decode("C:/hdr/big.hdr");
    assert!(!map_chosen_since_open());
    env_map_chosen();
    assert!(map_chosen_since_open());
}

#[test]
fn a_newer_decode_outlives_the_answer_of_an_older_one() {
    let first = begin_env_decode("C:/hdr/first.hdr");
    let second = begin_env_decode("C:/hdr/second.hdr");
    assert!(!env_decode_is_current(first), "an older decode is stale");
    assert!(env_decode_is_current(second));
    assert_eq!(
        pending_env_path().as_deref(),
        Some("C:/hdr/second.hdr"),
        "the older answer leaves the newer decode's path alone"
    );
    // A map chosen after the newer decode began voids that one as well.
    env_map_chosen();
    assert!(!env_decode_is_current(second));
}

// --- reading the library without waiting ---

#[test]
fn a_busy_library_is_not_waited_for_and_a_free_one_is_read() {
    let db = memory_db();
    store_values(&db, DESIGN, &tent(), 1).expect("stores");
    {
        let _held = db.lock().expect("lock");
        assert_eq!(
            try_read_lookup(&db, DESIGN),
            None,
            "busy: the UI thread asks again later instead of waiting"
        );
    }
    assert_eq!(try_read_lookup(&db, DESIGN), Some(Lookup::Row(tent())));
    assert_eq!(
        try_read_lookup(&db, "1c9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10"),
        Some(Lookup::NoRow)
    );
    assert!(matches!(
        try_read_lookup(&db, "not-a-uuid"),
        Some(Lookup::Unreadable(_))
    ));
}

// --- the settings file never takes a design's lighting ---

#[test]
fn values_written_into_the_settings_read_back_the_same() {
    let mut app = AppSettings::default();
    tent().write_into(&mut app);
    assert_eq!(LightingValues::from_app_settings(&app), tent());
    // No map is stored as an empty path, and reads back as none.
    LightingValues {
        env_map_path: None,
        ..tent()
    }
    .write_into(&mut app);
    assert_eq!(app.env_map_path, "");
    assert_eq!(LightingValues::from_app_settings(&app).env_map_path, None);
}

/// The remaining gap of `landed_w2_light.md`: nudge the exposure while a design's
/// lighting shows, close the app (a flush writes the file), and the settings file must
/// still hold the normal lighting.
#[test]
fn light_control_changes_while_a_designs_lighting_shows_never_reach_the_settings_file() {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix-cut-design-lighting-{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("a temp directory");
    let path = dir.join("settings.toml");
    let persister = SettingsPersister::spawn(path.clone(), SettingsFile::default());
    let normal = LightingValues::from_app_settings(&persister.snapshot().settings);

    // The design's lighting shows: the normal one is pinned over the file's lighting
    // fields, and the controls (still wired to the settings) write what they show.
    persister.set_disk_override(Some(normal.disk_pin()));
    persister.update(|file| tent().write_into(&mut file.settings));
    persister.update(|file| file.settings.exposure = 2.5);
    assert_eq!(
        persister.snapshot().settings.exposure,
        2.5,
        "the running app still has what the controls set"
    );

    // The app closes.
    persister.flush();
    let on_disk =
        LightingValues::from_app_settings(&crate::settings::store::load_or_default(&path).settings);
    assert_eq!(on_disk, normal, "the next start finds the normal lighting");

    // The lighting ends (another design opens): the in-memory settings are the normal
    // lighting again and the pin is let go; the file follows them.
    persister.update(|file| normal.write_into(&mut file.settings));
    persister.set_disk_override(None);
    persister.flush();
    let after =
        LightingValues::from_app_settings(&crate::settings::store::load_or_default(&path).settings);
    assert_eq!(after, normal);
    let _ = std::fs::remove_dir_all(&dir);
}
