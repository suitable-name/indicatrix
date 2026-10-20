//! Tests of the census command (follow-ups plan 2026-10-19, section 8.1).

use super::{
    collect,
    load::{SourceRef, Subject, design_from_record, open_subject},
    measure::{Context, panicked, process},
    percentiles::summarize,
    rows::Row,
    run,
};
use crate::{
    args::{CensusArgs, CensusBuiltins, CensusOutput, MaterialArg, Mode},
    load::from_bytes,
    materials::Catalogue,
    outcome::{EXIT_IO, EXIT_OK, EXIT_USAGE},
    testing,
};
use indicatrix_cut_core::{
    meet_preservation::{MeetPreservationBaseline, PreservationPolicy},
    templates::TEMPLATES,
};
use indicatrix_editor::retarget::CrownShift;
use indicatrix_vault::model::{entry::FullDiagramRecord, file::AttachedFile};
use serde_json::Value;
use std::path::PathBuf;

const TINY_ASC: &str = "GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";

/// A `GemCad` 4.51 schedule: its masts have five decimals, which open its designed meets by a
/// few millionths of the stone. The former near-incidence rule refused its capture.
const SHAH_ASC: &[u8] = include_bytes!("../../../tests/fixtures/shah.asc");

/// A census of nothing yet: the tests set what they measure.
fn blank() -> CensusArgs {
    CensusArgs {
        files: Vec::new(),
        builtins: CensusBuiltins::default(),
        db: None,
        all: false,
        ids: Vec::new(),
        limit: None,
        offset: 0,
        targets: vec![MaterialArg::Ri(1.7681)],
        modes: vec![Mode::Shift],
        crown: CrownShift::default(),
        budget: 24,
        output: CensusOutput::default(),
        out: None,
        out_dir: None,
    }
}

/// A census of the 1.72 brilliant, written as a design file, retargeted to index 1.7681.
fn brilliant_census(name: &str) -> CensusArgs {
    let path = testing::write_design(name, &testing::brilliant_at_172());
    CensusArgs {
        files: vec![PathBuf::from(path)],
        ..blank()
    }
}

fn run_text(args: &CensusArgs) -> String {
    let outcome = run(args).expect("the census runs");
    assert_eq!(outcome.exit, EXIT_OK);
    outcome.stdout
}

fn collected(args: &CensusArgs) -> (Vec<Row>, Vec<(String, String)>) {
    collect(args, &mut std::io::sink()).expect("the census runs")
}

/// The fields of one CSV line, honoring the quotes `csv_field` writes.
fn csv_fields(line: &str) -> Vec<String> {
    let (mut fields, mut field, mut quoted) = (Vec::new(), String::new(), false);
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                field.push('"');
                chars.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut field)),
            other => field.push(other),
        }
    }
    fields.push(field);
    fields
}

/// The header and the data lines of a census CSV, comment lines left out.
fn csv_table(text: &str) -> (Vec<String>, Vec<Vec<String>>) {
    let mut lines = text.lines().filter(|line| !line.starts_with('#'));
    let header = csv_fields(lines.next().expect("a header line"));
    (header, lines.map(csv_fields).collect())
}

fn column(header: &[String], name: &str) -> usize {
    header
        .iter()
        .position(|found| found == name)
        .unwrap_or_else(|| panic!("no column {name}"))
}

fn empty_record() -> FullDiagramRecord {
    FullDiagramRecord {
        entry_id: 1,
        title: "Test Design".to_string(),
        url: String::new(),
        design_id: None,
        page_url: String::new(),
        diagram_image_name: None,
        diagram_image_data: None,
        competition_diagram: None,
        lw_ratio: None,
        refractive_index: None,
        index_gear: None,
        volume: None,
        facets_count: None,
        shape: None,
        designer_info: None,
        hw_ratio: None,
        tw_ratio: None,
        uw_ratio: None,
        pw_ratio: None,
        cw_ratio: None,
        symmetry_order: None,
        mirror_symmetry: None,
        designer: None,
        source_citation: None,
        pdf_file: None,
        gem_file: None,
        shape_category: None,
        angle_settings: Vec::new(),
        attached_files: Vec::new(),
        frosting: Vec::new(),
    }
}

#[test]
fn the_same_census_prints_the_same_bytes_twice() {
    let wanted = brilliant_census("census-twice.isdd");
    let first = run_text(&wanted);
    let second = run_text(&wanted);
    assert_eq!(first, second);
    assert!(first.starts_with("# census_version = 2\n"), "{first}");
    assert!(
        !first.contains("t_capture_ms"),
        "no timing column without --timings"
    );
}

#[test]
fn every_template_captures() {
    for number in 1..=TEMPLATES.len() {
        let number = i32::try_from(number).expect("a small number");
        let subject = open_subject(&SourceRef::Template(number), None);
        assert_eq!(subject.source, "template");
        let design = subject.design.expect("a template is a design");
        MeetPreservationBaseline::capture(&design)
            .unwrap_or_else(|error| panic!("template {number} does not capture: {error}"));
    }
}

#[test]
fn a_record_without_a_design_file_is_one_row_that_says_so() {
    assert_eq!(
        design_from_record(&empty_record()).err().as_deref(),
        Some("no design file")
    );
    let mut with_asc = empty_record();
    with_asc.attached_files.push(AttachedFile {
        name: "tiny.asc".to_string(),
        url: String::new(),
        content: TINY_ASC.as_bytes().to_vec(),
    });
    let (design, loaded, decimals) =
        design_from_record(&with_asc).expect("the attached schedule reads");
    assert_eq!(design.tiers.len(), 1);
    assert!(loaded.is_none());
    assert_eq!(
        decimals,
        Some(8),
        "a GemCad 5 schedule writes eight decimals"
    );

    let subject = Subject {
        source: "library",
        key: "1".to_string(),
        title: "Test Design".to_string(),
        design: Err("no design file".to_string()),
        loaded: None,
        mast_decimals: None,
    };
    let args = blank();
    let catalogue = Catalogue::default();
    let output = process(
        &subject,
        &Context {
            args: &args,
            catalogue: &catalogue,
        },
    );
    assert_eq!(output.rows.len(), 1);
    let row = &output.rows[0];
    assert_eq!(row.text("capture"), "no design file");
    assert_eq!(row.text("source"), "library");
    assert_eq!(row.text("mode"), "", "no target and no mode for such a row");
}

#[test]
fn a_missing_file_and_a_panic_are_rows_not_errors() {
    let wanted = CensusArgs {
        files: vec![testing::temp_path("census-absent.isdd")],
        ..blank()
    };
    let (rows, _) = collected(&wanted);
    assert_eq!(rows.len(), 1);
    assert!(rows[0].text("capture").contains("cannot read"));

    let subject = open_subject(&SourceRef::Template(1), None);
    let payload: Box<dyn std::any::Any + Send> = Box::new("boom");
    let output = panicked(&subject, &*payload);
    assert_eq!(output.rows[0].text("capture"), "panic: boom");
}

#[test]
fn the_percentile_table_matches_the_rows() {
    let wanted = CensusArgs {
        targets: vec![MaterialArg::Ri(1.7681), MaterialArg::Ri(1.9)],
        ..brilliant_census("census-percentiles.isdd")
    };
    let text = run_text(&wanted);
    let (header, rows) = csv_table(&text);
    let (mode, status, excess) = (
        column(&header, "mode"),
        column(&header, "census_status"),
        column(&header, "excess_depth"),
    );
    let mut from_csv: Vec<f64> = rows
        .iter()
        .filter(|row| row[mode] == "shift" && row[status] == "valid")
        .filter_map(|row| row[excess].parse().ok())
        .collect();
    from_csv.sort_by(f64::total_cmp);
    assert!(!from_csv.is_empty(), "at least one measurable row:\n{text}");

    let (rows, _) = collected(&wanted);
    let summary = summarize(&rows);
    let depth = summary
        .tables
        .iter()
        .find(|table| table.mode == "shift" && table.figure == "depth")
        .expect("a depth table");
    assert_eq!(depth.n, from_csv.len());
    assert_eq!(depth.p100, from_csv.last().copied());
    assert_eq!(
        depth.p50,
        Some(from_csv[(from_csv.len() * 50).div_ceil(100) - 1])
    );
    assert!(text.contains("# Mode shift against RETARGET"), "{text}");
}

#[test]
fn json_and_csv_agree_row_for_row() {
    let csv_args = brilliant_census("census-formats.isdd");
    let json_args = CensusArgs {
        output: CensusOutput {
            json: true,
            ..csv_args.output
        },
        ..csv_args.clone()
    };
    let (header, csv_rows) = csv_table(&run_text(&csv_args));
    let document: Value = serde_json::from_str(&run_text(&json_args)).expect("valid JSON");
    assert_eq!(document["census_version"], 2);
    let json_rows = document["rows"].as_array().expect("rows");
    assert_eq!(json_rows.len(), csv_rows.len());
    for (json_row, csv_row) in json_rows.iter().zip(&csv_rows) {
        for name in ["key", "mode", "status", "census_status", "branch"] {
            let text = json_row[name].as_str().unwrap_or_default();
            assert_eq!(text, csv_row[column(&header, name)], "{name}");
        }
        for name in ["junction_disp", "excess_depth", "d_aspect"] {
            let cell = &csv_row[column(&header, name)];
            match (json_row[name].as_f64(), cell.parse::<f64>().ok()) {
                (Some(json), Some(csv)) => assert!((json - csv).abs() < 1e-9, "{name}"),
                (None, None) => {}
                other => panic!("{name} differs: {other:?}"),
            }
        }
    }
}

#[test]
fn out_dir_writes_only_valid_rows_and_they_reopen_and_validate() {
    let folder = testing::temp_path("census-stones");
    std::fs::create_dir_all(&folder).expect("a folder");
    let wanted = CensusArgs {
        targets: vec![MaterialArg::Ri(1.7681), MaterialArg::Ri(2.2)],
        out_dir: Some(folder),
        ..brilliant_census("census-stones.isdd")
    };
    let (rows, files) = collected(&wanted);
    let valid = rows
        .iter()
        .filter(|row| row.text("status") == "valid")
        .count();
    assert_eq!(
        files.len(),
        valid,
        "one stone for each Valid row, none for the others"
    );
    assert!(valid > 0, "the known-good sapphire case is valid");
    let baseline = MeetPreservationBaseline::capture(&testing::brilliant_at_172())
        .expect("the original meets are trustworthy");
    for (name, text) in &files {
        assert!(name.ends_with("__shift.isdd"), "{name}");
        let again = from_bytes(name, text.as_bytes()).expect("the stone reopens");
        baseline
            .validate(&again.design, PreservationPolicy::RETARGET)
            .unwrap_or_else(|error| panic!("{name} does not validate: {error}"));
    }
    let outcome = run(&wanted).expect("the census runs");
    assert_eq!(outcome.files.len(), files.len());
    assert!(outcome.files.iter().all(|file| {
        file.path
            .starts_with(wanted.out_dir.as_deref().expect("a folder"))
    }));
}

#[test]
fn bad_targets_and_bad_folders_are_refused_before_anything_runs() {
    let unknown = CensusArgs {
        targets: vec![MaterialArg::Named("Unobtainium".to_string())],
        ..brilliant_census("census-unknown.isdd")
    };
    assert_eq!(
        run(&unknown).expect_err("unknown material").code,
        EXIT_USAGE
    );
    let folder = CensusArgs {
        out_dir: Some(testing::temp_path("census-no-such-folder")),
        ..brilliant_census("census-folder.isdd")
    };
    assert_eq!(run(&folder).expect_err("not a folder").code, EXIT_IO);
}

#[test]
fn the_out_file_gets_the_report_and_the_screen_the_summary() {
    let wanted = CensusArgs {
        out: Some(PathBuf::from("census.csv")),
        ..brilliant_census("census-out.isdd")
    };
    let outcome = run(&wanted).expect("the census runs");
    assert!(
        outcome.stdout.starts_with("Census Summary"),
        "{}",
        outcome.stdout
    );
    assert_eq!(outcome.files.len(), 1);
    assert_eq!(outcome.files[0].path, PathBuf::from("census.csv"));
    assert!(outcome.files[0].text.starts_with("# census_version = 2\n"));
}

#[test]
fn the_fixtures_are_measured_and_the_timing_columns_are_opt_in() {
    let wanted = CensusArgs {
        builtins: CensusBuiltins {
            fixtures: true,
            ..CensusBuiltins::default()
        },
        output: CensusOutput {
            timings: true,
            ..CensusOutput::default()
        },
        ..blank()
    };
    let (header, rows) = csv_table(&run_text(&wanted));
    assert!(header.iter().any(|name| name == "t_projection_ms"));
    let keys: Vec<&str> = rows
        .iter()
        .map(|row| row[column(&header, "key")].as_str())
        .collect();
    assert!(keys.contains(&"concave_fixture"), "{keys:?}");
    assert!(keys.contains(&"brilliant_172"), "{keys:?}");
}

/// The diamond-to-sapphire brilliant of the plan: the ten-degree pavilion shift that deepens
/// the stone by far more than the Retarget constant, which only the turn's travel can admit.
#[test]
fn the_diamond_to_sapphire_brilliant_is_measured_under_the_census_policy() {
    let path = testing::write_design("census-sapphire.isdd", &testing::template());
    let wanted = CensusArgs {
        files: vec![PathBuf::from(path)],
        targets: vec![MaterialArg::Named("Sapphire".to_string())],
        ..blank()
    };
    let (rows, _) = collected(&wanted);
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row.text("mode"), "shift");
    assert!(!row.text("census_status").is_empty(), "the row is measured");
    if row.text("status") == "valid" {
        assert_eq!(row.text("census_status"), "valid");
        let deepening = row.num("d_pavilion").map_or(0.0, f64::abs);
        assert!(
            deepening > PreservationPolicy::RETARGET.max_depth_change,
            "the pavilion moves by more than the constant: {deepening}"
        );
        assert!(
            row.num("travel_width").unwrap_or(0.0) > 0.0,
            "and the turn's travel is what admits it"
        );
    }
}

#[test]
fn a_five_decimal_schedule_captures_and_reports_its_quantisation() {
    let loaded = from_bytes("shah.asc", SHAH_ASC).expect("the schedule reads");
    assert_eq!(loaded.mast_decimals, Some(5));
    let subject = Subject {
        source: "file",
        key: "shah.asc".to_string(),
        title: loaded.name.clone(),
        design: Ok(loaded.design.clone()),
        mast_decimals: loaded.mast_decimals,
        loaded: Some(loaded),
    };
    let args = blank();
    let catalogue = Catalogue::default();
    let output = process(
        &subject,
        &Context {
            args: &args,
            catalogue: &catalogue,
        },
    );
    let row = &output.rows[0];
    assert_eq!(row.text("capture"), "ok");
    assert_eq!(row.text("gemcad_version"), "4.51");
    assert_eq!(row.num("mast_decimals"), Some(5.0));
    // Recorded, and within the identification radius (50 millionths of the stone).
    let slack = row
        .num("max_slack_ppm")
        .expect("a captured network has a slack");
    assert!(slack <= 50.0, "{slack}");
    assert!(row.num("faces").is_some_and(|faces| faces >= 4.0));
    assert!(row.num("valence").is_some_and(|valence| valence >= 3.0));
    assert!(row.num("split_ratio").is_some_and(|ratio| ratio > 0.0));
}
