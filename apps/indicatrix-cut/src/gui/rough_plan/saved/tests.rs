//! Tests of the plan file format: the round trip and the loader's refusals. The layouts
//! come from [`super::fixtures`], where every figure follows from first principles.

use super::{
    convert::CandidateSource,
    dto::SavedPlanDto,
    fixtures::{
        IDENTITY, LIBRARY_STAMP, MINIMAL_HEADER, TURN_3_4_5, block, designs, exact_fit_layout_in,
        expect_error, layout_with_axes, plain_block, sample_layout, sample_layout_in, settings,
        write, write_with,
    },
    format::{
        LoadedPlan, RankedLayout, SerializeInput, parse_and_validate_plan, payload_version_of,
        payload_with_name, plan_summary, serialize_checked_plan, serialize_plan_to_toml,
    },
};
use indicatrix_cut_core::rough_plan::{
    Axis, BoxFace, RoughBase, RoughCut, RoughLayout, RoughModel,
};

fn assert_round_trip(model: &RoughModel) {
    let layouts = vec![sample_layout_in(model), layout_with_axes(model, TURN_3_4_5)];
    let text = write(model, &layouts);
    let loaded: LoadedPlan = parse_and_validate_plan(&text).expect("the plan loads");
    assert_eq!(&loaded.model, model);
    assert_eq!(loaded.layouts, layouts, "poses, slabs and totals survive");
    assert_eq!(loaded.settings, settings());
    assert_eq!(
        loaded.candidate_source,
        CandidateSource::Library,
        "the source is not the default, so it was read from the file"
    );
    assert_eq!(loaded.material_name, "Aquamarine");
    assert_eq!(loaded.weighed_ct, Some(8.9));
    assert_eq!(loaded.designs, designs());
    assert_eq!(loaded.name, "Test plan");
    assert_eq!(loaded.created_at, 1_790_000_000);
    assert_eq!(loaded.library_id, Some(LIBRARY_STAMP));
    assert_eq!(loaded.version, 1);
}

#[test]
fn a_block_with_every_kind_of_cut_round_trips_with_its_layouts() {
    assert_round_trip(&block(vec![
        RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [3.0, 2.0],
        },
        RoughCut::Corner {
            faces: [BoxFace::Bottom, BoxFace::Back, BoxFace::Left],
            setbacks_mm: [2.5, 2.5, 2.5],
        },
        RoughCut::Face {
            normal: [0.6, 0.0, 0.8],
            depth_mm: 1.0,
        },
    ]));
}

#[test]
fn a_cylinder_and_a_pebble_round_trip_with_their_layouts() {
    assert_round_trip(&RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 12.0,
            length_mm: 30.0,
            axis: Axis::Z,
        },
        vec![RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 2.0,
        }],
    ));
    assert_round_trip(&RoughModel::new(
        RoughBase::Pebble {
            x_mm: 18.4,
            y_mm: 11.0,
            z_mm: 9.6,
        },
        vec![RoughCut::Face {
            normal: [1.0, 0.0, 0.0],
            depth_mm: 3.0,
        }],
    ));
}

#[test]
fn the_specific_gravity_comes_from_the_file_and_the_rank_is_kept() {
    let layout = sample_layout();
    let ranked = [RankedLayout {
        rank: 3,
        layout: &layout,
    }];
    let text = write_with(&plain_block(), &ranked, &designs());
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert!((loaded.settings.specific_gravity - 3.51).abs() < 1e-12);
    let dto: SavedPlanDto = toml::from_str(&text).expect("the document parses");
    assert_eq!(dto.layouts[0].rank, 3);
}

#[test]
fn layouts_are_ordered_by_rank_whatever_order_the_file_lists_them() {
    let first = sample_layout();
    let mut second = sample_layout();
    second.stones.truncate(1);
    second.cut_plan.slabs.truncate(1);
    second.cut_plan.slabs[0].bars[0].pieces_mm.truncate(1);
    second.total_carat = second.stones[0].carat;
    second.total_volume_mm3 = second.stones[0].volume_mm3;
    second.yield_fraction = second.total_volume_mm3 / 2400.0;
    // The file lists rank 2 before rank 1.
    let ranked = [
        RankedLayout {
            rank: 2,
            layout: &second,
        },
        RankedLayout {
            rank: 1,
            layout: &first,
        },
    ];
    let text = write_with(&plain_block(), &ranked, &designs());
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert_eq!(loaded.layouts, vec![first, second]);
}

#[test]
fn a_rank_used_twice_is_refused() {
    let layout = sample_layout();
    let ranked = [
        RankedLayout {
            rank: 4,
            layout: &layout,
        },
        RankedLayout {
            rank: 4,
            layout: &layout,
        },
    ];
    let error = expect_error(&write_with(&plain_block(), &ranked, &designs()));
    assert!(error.contains("layouts[1].rank 4 appears twice"), "{error}");
}

#[test]
fn a_plan_without_layouts_is_a_plan() {
    let text = write(&plain_block(), &[]);
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert_eq!(loaded.layouts, Vec::<RoughLayout>::new());
}

#[test]
fn a_design_that_no_stone_uses_is_refused() {
    let layout = sample_layout();
    let ranked = [RankedLayout {
        rank: 1,
        layout: &layout,
    }];
    let mut listed = designs();
    listed.push(listed[0].clone());
    listed[2].entry_id = 3;
    let error = expect_error(&write_with(&plain_block(), &ranked, &listed));
    assert!(error.contains("designs[2].entry_id 3"), "{error}");
    assert!(error.contains("not used by any stone"), "{error}");
}

#[test]
fn axes_that_are_not_unit_vectors_name_the_field() {
    let model = plain_block();
    let mut axes = IDENTITY;
    axes[0] = [2.0, 0.0, 0.0];
    let error = expect_error(&write(&model, &[layout_with_axes(&model, axes)]));
    assert!(error.contains("layouts[0].stones[1].axes[0]"), "{error}");
    assert!(error.contains("unit vector"), "{error}");

    // 1e-5 off unit length is beyond the 1e-6 tolerance.
    axes[0] = [1.000_01, 0.0, 0.0];
    let error = expect_error(&write(&model, &[layout_with_axes(&model, axes)]));
    assert!(error.contains("axes[0]"), "{error}");

    // 1e-8 off is fine.
    axes[0] = [1.000_000_01, 0.0, 0.0];
    parse_and_validate_plan(&write(&model, &[layout_with_axes(&model, axes)]))
        .expect("within tolerance");
}

#[test]
fn left_handed_and_skewed_axes_are_refused() {
    let model = plain_block();
    let left_handed = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0]];
    let error = expect_error(&write(&model, &[layout_with_axes(&model, left_handed)]));
    assert!(error.contains("right-handed"), "{error}");
    assert!(error.contains("axes"), "{error}");

    let skewed = [[1.0, 0.0, 0.0], [0.6, 0.8, 0.0], [0.0, 0.0, 1.0]];
    let error = expect_error(&write(&model, &[layout_with_axes(&model, skewed)]));
    assert!(error.contains("perpendicular"), "{error}");
}

#[test]
fn nan_and_infinity_are_refused_with_the_field_path() {
    let mut layout = sample_layout();
    layout.total_carat = f64::NAN;
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(error.contains("layouts[0].total_carat"), "{error}");

    let mut layout = sample_layout();
    layout.stones[2].pose.center_mm[1] = f64::INFINITY;
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(
        error.contains("layouts[0].stones[2].center_mm[1]"),
        "{error}"
    );

    let mut layout = sample_layout();
    layout.cut_plan.slabs[0].bars[0].pieces_mm[1] = f64::NAN;
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(error.contains("slabs[0].bars[0].pieces_mm[1]"), "{error}");

    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: f64::INFINITY,
            y_mm: 12.0,
            z_mm: 10.0,
        },
        Vec::new(),
    );
    let error = expect_error(&write(&model, &[]));
    assert!(error.contains("rough.x_mm"), "{error}");

    let mut bad_designs = designs();
    bad_designs[0].fingerprint[1] = f64::NAN;
    let error = expect_error(&write_with(&plain_block(), &[], &bad_designs));
    assert!(error.contains("designs[0].fingerprint[1]"), "{error}");

    let mut bad_width = designs();
    bad_width[1].width_caliper = Some(0.0);
    let error = expect_error(&write_with(&plain_block(), &[], &bad_width));
    assert!(error.contains("designs[1].width_caliper"), "{error}");
}

#[test]
fn a_missing_field_is_named() {
    let text = write(&plain_block(), &[sample_layout()]);
    for field in ["specific_gravity", "kerf_mm", "material", "total_carat"] {
        let cut: String = text
            .lines()
            .filter(|line| !line.trim_start().starts_with(&format!("{field} =")))
            .collect::<Vec<_>>()
            .join("\n");
        let error = expect_error(&cut);
        assert!(error.contains(field), "{field}: {error}");
    }
}

#[test]
fn unknown_keys_and_tables_are_ignored() {
    let model = block(vec![RoughCut::Face {
        normal: [0.0, 1.0, 0.0],
        depth_mm: 1.0,
    }]);
    let text = write(&model, &[sample_layout_in(&model)]);
    let extended = format!("future_key = \"x\"\n{text}\n[future]\nkey = 2\n")
        .replacen("[rough]\n", "[rough]\nfuture_field = 1\n", 1)
        .replacen("[settings]\n", "[settings]\nfuture_setting = true\n", 1);
    let plain = parse_and_validate_plan(&text).expect("plain loads");
    let loaded = parse_and_validate_plan(&extended).expect("extended loads");
    assert_eq!(loaded, plain);
}

#[test]
fn a_newer_version_is_reported_before_the_body_is_read() {
    let error = expect_error("format = \"indicatrix-rough-plan\"\nversion = 3\n");
    assert!(error.contains("newer Indicatrix"), "{error}");

    // A body this build could not read is not a reason to hide that.
    let error = expect_error(
        "format = \"indicatrix-rough-plan\"\nversion = 99\n[rough]\nshape = \"torus\"\n",
    );
    assert!(error.contains("newer Indicatrix"), "{error}");
    assert!(error.contains("99"), "{error}");

    let error = expect_error("format = \"indicatrix-rough-plan\"\nversion = 0\n");
    assert!(error.contains("version"), "{error}");
}

#[test]
fn the_header_version_is_what_a_stored_plan_records() {
    assert_eq!(payload_version_of(MINIMAL_HEADER), Some(1));
    assert_eq!(
        payload_version_of(&write(&plain_block(), &[])),
        Some(1),
        "a written plan declares the current schema"
    );
    assert_eq!(payload_version_of("not a plan"), None);
    assert_eq!(payload_version_of("format = \"x\"\n"), None);
}

#[test]
fn a_wrong_format_is_reported_even_when_the_file_is_truncated() {
    let error = expect_error(
        "format = \"other-app-plan\"\nversion = 1\nname = \"x\"\n[rough]\nbase = \"blo",
    );
    assert!(error.contains("other-app-plan"), "{error}");
    assert!(error.contains("format"), "{error}");

    let error = expect_error("version = 1\nname = \"x\"\n");
    assert!(error.contains("'format' key is missing"), "{error}");

    let error = expect_error(&format!("{MINIMAL_HEADER}[rough"));
    assert!(
        error.starts_with("The rough plan is incomplete or malformed"),
        "{error}"
    );

    let error = expect_error("format = \"indicatrix-rough-plan\"\nname = \"x\"\n");
    assert!(error.contains("'version' key is missing"), "{error}");
}

#[test]
fn the_stone_count_is_reported_never_clamped() {
    for count in [0_u8, 100, 255] {
        let mut wide = settings();
        wide.count = count;
        let layouts: [RankedLayout<'_>; 0] = [];
        let text = serialize_plan_to_toml(&SerializeInput {
            name: "n",
            created_at: 0,
            library_id: None,
            model: &plain_block(),
            material_name: "Quartz",
            weighed_ct: None,
            settings: &wide,
            candidate_source: CandidateSource::Filter,
            designs: &[],
            layouts: &layouts,
        })
        .expect("serialises");
        let error = expect_error(&text);
        assert!(error.contains("settings.count"), "{error}");
        assert!(error.contains(&count.to_string()), "{error}");
    }
    for count in [1_u8, 99] {
        let text =
            write_with(&plain_block(), &[], &[]).replace("count = 6", &format!("count = {count}"));
        assert_eq!(
            parse_and_validate_plan(&text)
                .expect("loads")
                .settings
                .count,
            count
        );
    }
}

#[test]
fn the_losses_are_bounded_by_what_the_planner_form_accepts() {
    let text = write_with(&plain_block(), &[], &[]);
    for (line, replacement, path) in [
        ("kerf_mm = 0.3", "kerf_mm = 50.5", "settings.kerf_mm"),
        (
            "allowance_mm = 0.2",
            "allowance_mm = 51.0",
            "settings.allowance_mm",
        ),
        ("skin_mm = 0.0", "skin_mm = 1000.0", "settings.skin_mm"),
        (
            "min_width_mm = 1.0",
            "min_width_mm = 1000.5",
            "settings.min_width_mm",
        ),
    ] {
        assert!(text.contains(line), "the fixture writes {line}:\n{text}");
        let error = expect_error(&text.replace(line, replacement));
        assert!(error.contains(path), "{path}: {error}");
        assert!(error.contains("at most"), "{path}: {error}");
    }
    // The limits themselves are allowed.
    let edge = text
        .replace("kerf_mm = 0.3", "kerf_mm = 50.0")
        .replace("min_width_mm = 1.0", "min_width_mm = 1000.0");
    parse_and_validate_plan(&edge).expect("the limits are inside the range");
}

#[test]
fn the_candidate_source_is_saved_and_defaults_to_the_filter_for_older_files() {
    let line = "candidate_source = \"library\"\n";
    let text = write(&plain_block(), &[]);
    assert!(text.contains(line), "the key is written:\n{text}");

    let older = text.replace(line, "");
    assert_ne!(older, text);
    let loaded = parse_and_validate_plan(&older).expect("a file without the key loads");
    assert_eq!(loaded.candidate_source, CandidateSource::Filter);

    let filtered = text.replace(line, "candidate_source = \"filter\"\n");
    let loaded = parse_and_validate_plan(&filtered).expect("loads");
    assert_eq!(loaded.candidate_source, CandidateSource::Filter);
    assert!(loaded.candidate_source.uses_filter());
    assert!(!CandidateSource::Library.uses_filter());
    assert_eq!(
        CandidateSource::from_use_filter(false),
        CandidateSource::Library
    );
}

#[test]
fn an_unknown_candidate_source_is_refused_with_the_field_named() {
    let text = write(&plain_block(), &[]).replace(
        "candidate_source = \"library\"",
        "candidate_source = \"mirror\"",
    );
    let error = expect_error(&text);
    assert!(error.contains("settings.candidate_source"), "{error}");
    assert!(error.contains("mirror"), "{error}");
}

#[test]
fn cuts_that_do_not_fit_are_refused_with_the_cut_named() {
    let same_axis = block(vec![RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Bottom],
        setbacks_mm: [1.0, 1.0],
    }]);
    let error = expect_error(&write(&same_axis, &[]));
    assert!(error.contains("rough.cuts[0].faces[1]"), "{error}");

    let zero_setback = block(vec![RoughCut::Corner {
        faces: [BoxFace::Top, BoxFace::Front, BoxFace::Right],
        setbacks_mm: [1.0, 0.0, 1.0],
    }]);
    let error = expect_error(&write(&zero_setback, &[]));
    assert!(error.contains("rough.cuts[0].setbacks_mm[1]"), "{error}");

    let too_long = block(vec![RoughCut::Edge {
        faces: [BoxFace::Top, BoxFace::Front],
        setbacks_mm: [50.0, 1.0],
    }]);
    let error = expect_error(&write(&too_long, &[]));
    assert!(error.contains("Cut 1"), "{error}");

    let through = block(vec![RoughCut::Face {
        normal: [0.0, 1.0, 0.0],
        depth_mm: 30.0,
    }]);
    let error = expect_error(&write(&through, &[]));
    assert!(error.contains("depth"), "{error}");

    let on_cylinder = RoughModel::new(
        RoughBase::Cylinder {
            diameter_mm: 10.0,
            length_mm: 20.0,
            axis: Axis::X,
        },
        vec![RoughCut::Edge {
            faces: [BoxFace::Top, BoxFace::Front],
            setbacks_mm: [1.0, 1.0],
        }],
    );
    let error = expect_error(&write(&on_cylinder, &[]));
    assert!(error.contains("block"), "{error}");
}

#[test]
fn layouts_must_agree_with_their_saw_plan_and_their_designs() {
    let mut layout = sample_layout();
    layout.stones.pop();
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(error.contains("layouts[0].stones"), "{error}");

    let mut layout = sample_layout();
    layout.stones[2].entry_id = 77;
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(error.contains("no entry in designs"), "{error}");

    let mut layout = sample_layout();
    layout.stones[0].pose.mm_per_unit = 0.0;
    let error = expect_error(&write(&plain_block(), &[layout]));
    assert!(error.contains("mm_per_unit"), "{error}");
}

#[test]
fn no_prefix_of_a_plan_and_no_junk_makes_the_loader_panic() {
    let model = block(vec![RoughCut::Face {
        normal: [0.0, 1.0, 0.0],
        depth_mm: 1.0,
    }]);
    let text = write(&model, &[sample_layout_in(&model)]);
    assert!(parse_and_validate_plan(&text).is_ok());
    for end in 0..text.len() {
        if text.is_char_boundary(end) {
            let _ = parse_and_validate_plan(&text[..end]);
        }
    }
    let deep = "[".repeat(5_000);
    for junk in [
        "",
        "\0\0\0",
        "[[[[",
        "format =",
        "format = \"indicatrix-rough-plan\"\nversion = 1\n[[layouts]]\n[[layouts.slabs]]",
        "\u{feff}format = 1",
        deep.as_str(),
    ] {
        assert!(parse_and_validate_plan(junk).is_err(), "{junk:?}");
    }
    let oversized = "#".repeat(17 * 1024 * 1024);
    assert!(expect_error(&oversized).contains("too large"));
}

#[test]
fn the_checked_writer_refuses_what_could_not_be_opened_again() {
    let layouts = [RankedLayout {
        rank: 1,
        layout: &sample_layout(),
    }];
    let input = SerializeInput {
        name: "ok",
        created_at: 0,
        library_id: None,
        model: &plain_block(),
        material_name: "Quartz",
        weighed_ct: None,
        settings: &settings(),
        candidate_source: CandidateSource::Filter,
        designs: &designs(),
        layouts: &layouts,
    };
    let text = serialize_checked_plan(&input).expect("a good plan is written");
    assert!(parse_and_validate_plan(&text).is_ok());

    let mut broken = sample_layout();
    broken.total_carat = f64::NAN;
    let layouts = [RankedLayout {
        rank: 1,
        layout: &broken,
    }];
    let error = serialize_checked_plan(&SerializeInput {
        layouts: &layouts,
        ..input
    })
    .expect_err("a NaN total is refused at save time");
    assert!(error.contains("would not open again"), "{error}");
    assert!(error.contains("total_carat"), "{error}");
}

#[test]
fn the_list_summary_and_the_renamed_payload_read_the_stored_text() {
    let text = write(&plain_block(), &[sample_layout(), sample_layout()]);
    assert_eq!(
        plan_summary(&text),
        "Block \u{b7} Aquamarine \u{b7} 2 results"
    );
    let one = write(&plain_block(), &[sample_layout()]);
    assert_eq!(
        plan_summary(&one),
        "Block \u{b7} Aquamarine \u{b7} 1 result"
    );
    assert_eq!(plan_summary("not a plan"), "Saved plan");

    let renamed = payload_with_name(&text, "Better name");
    let loaded = parse_and_validate_plan(&renamed).expect("still a plan");
    assert_eq!(loaded.name, "Better name");
    assert_eq!(loaded.layouts.len(), 2);
    assert_eq!(payload_with_name("junk", "x"), "junk");
}

/// The lines of `text` with the top-level `name` line taken out, and that line.
fn without_name_line(text: &str) -> (Vec<&str>, &str) {
    let mut name_line = "";
    let rest = text
        .lines()
        .filter(|line| {
            if line.starts_with("name =") && name_line.is_empty() {
                name_line = *line;
                false
            } else {
                true
            }
        })
        .collect();
    (rest, name_line)
}

#[test]
fn a_rename_changes_the_name_line_and_nothing_else_of_the_text() {
    let text = write(&plain_block(), &[sample_layout()]);
    // Something this build does not know, a comment, and a comment on the name line itself.
    let annotated = format!("# written by a newer tool\nfuture_key = [1, 2]\n{text}").replacen(
        "name = \"Test plan\"",
        "name = \"Test plan\" # the label",
        1,
    );
    let renamed = payload_with_name(&annotated, "Better \"quoted\" name");
    let (before, old_line) = without_name_line(&annotated);
    let (after, new_line) = without_name_line(&renamed);
    assert_eq!(before, after, "every other line is untouched");
    assert_eq!(old_line, "name = \"Test plan\" # the label");
    assert!(new_line.ends_with(" # the label"), "{new_line}");
    let loaded = parse_and_validate_plan(&renamed).expect("still a plan");
    assert_eq!(loaded.name, "Better \"quoted\" name");
}

#[test]
fn every_form_of_the_name_line_is_patched_in_place() {
    let text = write(&plain_block(), &[]);
    let new_value = toml::Value::String("Back\\slash".to_string()).to_string();
    // (the whole line as it is written, the string value on it)
    for (line, value) in [
        ("name = \"Test plan\"", "\"Test plan\""),
        ("name='Test plan'", "'Test plan'"),
        ("  name   =   \"Test plan\"", "\"Test plan\""),
    ] {
        let edited = text.replacen("name = \"Test plan\"", line, 1);
        assert!(edited.contains(line));
        let renamed = payload_with_name(&edited, "Back\\slash");
        assert_eq!(
            parse_and_validate_plan(&renamed).expect("loads").name,
            "Back\\slash",
            "{line}"
        );
        // Byte for byte: only the value on that line changed.
        assert_eq!(renamed, edited.replacen(value, &new_value, 1), "{line}");
    }
    // Windows line endings survive.
    let crlf = text.replace('\n', "\r\n");
    let renamed = payload_with_name(&crlf, "New");
    assert_eq!(
        renamed.matches("\r\n").count(),
        crlf.matches("\r\n").count()
    );
    assert_eq!(
        parse_and_validate_plan(&renamed).expect("loads").name,
        "New"
    );
}

#[test]
fn a_name_line_of_another_form_is_rewritten_through_the_document() {
    let text = write(&plain_block(), &[]);
    let multiline = text.replacen("name = \"Test plan\"", "name = \"\"\"Test plan\"\"\"", 1);
    let renamed = payload_with_name(&multiline, "Plain");
    assert_eq!(
        parse_and_validate_plan(&renamed).expect("loads").name,
        "Plain"
    );
}

#[test]
fn a_name_longer_than_200_characters_is_refused() {
    let text = write(&plain_block(), &[]);
    let long = "n".repeat(201);
    let error = expect_error(&text.replacen("Test plan", &long, 1));
    assert!(error.contains("name is longer than 200"), "{error}");
    let fits = "n".repeat(200);
    let loaded = parse_and_validate_plan(&text.replacen("Test plan", &fits, 1)).expect("loads");
    assert_eq!(loaded.name.chars().count(), 200);
}

#[test]
fn a_plan_with_more_layouts_or_designs_than_a_planner_makes_is_refused() {
    let layout = sample_layout();
    let ranked: Vec<RankedLayout<'_>> = (1..=11)
        .map(|rank| RankedLayout {
            rank,
            layout: &layout,
        })
        .collect();
    let error = expect_error(&write_with(&plain_block(), &ranked, &designs()));
    assert!(error.contains("layouts lists 11 layouts"), "{error}");
    parse_and_validate_plan(&write_with(&plain_block(), &ranked[..10], &designs()))
        .expect("ten layouts are a full list");

    let many: Vec<_> = (1..=991)
        .map(|entry_id| {
            let mut design = designs()[0].clone();
            design.entry_id = entry_id;
            design
        })
        .collect();
    let error = expect_error(&write_with(&plain_block(), &[], &many));
    assert!(error.contains("designs lists 991 designs"), "{error}");
}

#[test]
fn saw_plans_and_stone_lists_beyond_the_planners_limits_are_refused() {
    let model = plain_block();
    let mut stones = sample_layout();
    let extra = stones.stones[0];
    stones.stones = vec![extra; 100];
    let error = expect_error(&write(&model, &[stones]));
    assert!(
        error.contains("layouts[0].stones lists 100 stones"),
        "{error}"
    );

    // 100 slabs, all but one empty: the stone list is short, the slab list is not.
    let mut slabs = sample_layout();
    slabs.stones.truncate(1);
    slabs.cut_plan.slabs.truncate(1);
    slabs.cut_plan.slabs[0].bars[0].pieces_mm.truncate(1);
    let empty = indicatrix_cut_core::rough_plan::SlabCut {
        thickness_mm: 1.0,
        bars: Vec::new(),
    };
    slabs.cut_plan.slabs.extend(vec![empty; 99]);
    let error = expect_error(&write(&model, &[slabs.clone()]));
    assert!(
        error.contains("layouts[0].slabs lists 100 slabs"),
        "{error}"
    );

    // 100 bars in a slab.
    let bar = indicatrix_cut_core::rough_plan::BarCut {
        width_mm: 1.0,
        pieces_mm: Vec::new(),
    };
    slabs.cut_plan.slabs.truncate(1);
    slabs.cut_plan.slabs[0].bars.extend(vec![bar; 99]);
    let error = expect_error(&write(&model, &[slabs]));
    assert!(
        error.contains("layouts[0].slabs[0].bars lists 100 bars"),
        "{error}"
    );
}

#[test]
fn an_exact_fit_layout_round_trips_and_keeps_its_marker() {
    let model = plain_block();
    let exact = exact_fit_layout_in(&model);
    let text = write(&model, &[exact.clone(), sample_layout()]);
    let loaded = parse_and_validate_plan(&text).expect("loads");
    assert_eq!(loaded.layouts[0], exact);
    assert!(loaded.layouts[0].exact_fit);
    assert!(!loaded.layouts[1].exact_fit);
}

#[test]
fn a_rough_the_cuts_leave_nothing_of_is_refused_even_when_each_cut_is_fine_alone() {
    // Two face cuts take 7 mm off the top and 7 mm off the bottom of a 12 mm thick block:
    // each is shallower than the block, together they remove all of it.
    let hollow = block(vec![
        RoughCut::Face {
            normal: [0.0, 1.0, 0.0],
            depth_mm: 7.0,
        },
        RoughCut::Face {
            normal: [0.0, -1.0, 0.0],
            depth_mm: 7.0,
        },
    ]);
    let error = expect_error(&write(&hollow, &[]));
    assert!(error.starts_with("rough:"), "{error}");
}
