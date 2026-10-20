//! Tests of the command lines that name several designs: the batch they make, the names of
//! the files they write and the lines that are refused.

use super::{Command, ExportFormat, MaterialArg, ReportFormat, ReportStyle, parse};
use std::path::PathBuf;

fn argv(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

fn parsed(line: &str) -> Command {
    parse(&argv(line)).unwrap_or_else(|e| panic!("{line:?} should parse: {e}"))
}

fn error(line: &str) -> String {
    parse(&argv(line)).expect_err("this line should not parse")
}

/// The single-design commands of a batch, in order.
fn batch_items(line: &str) -> Vec<Command> {
    match parsed(line) {
        Command::Batch(batch) => batch.items,
        other => panic!("{line:?} should be a batch, got {other:?}"),
    }
}

#[test]
fn several_designs_make_a_batch_of_the_same_command_for_each() {
    let Command::Batch(batch) = parsed("info a.asc b.gem c.isdd --json --db lib.sqlite") else {
        panic!("expected a batch");
    };
    assert_eq!(batch.items.len(), 3);
    assert_eq!(batch.out_dir, None);
    assert_eq!(batch.style, ReportStyle::Json);
    let designs: Vec<PathBuf> = batch
        .items
        .iter()
        .map(|item| item.design().expect("a design command").to_path_buf())
        .collect();
    assert_eq!(
        designs,
        [
            PathBuf::from("a.asc"),
            PathBuf::from("b.gem"),
            PathBuf::from("c.isdd")
        ]
    );
    for item in &batch.items {
        let Command::Info(args) = item else {
            panic!("expected info");
        };
        assert!(args.json);
        assert_eq!(args.db, Some(PathBuf::from("lib.sqlite")));
        assert_eq!(args.out, None);
    }
    // One design is the plain command, never a batch of one.
    assert!(matches!(parsed("info a.asc"), Command::Info(_)));
}

#[test]
fn every_design_command_takes_several_designs() {
    for line in [
        "info a.asc b.asc",
        "solve a.asc b.asc",
        "metrics a.asc b.asc --tilt",
        "validate a.asc b.asc",
        "optimize a.asc b.asc --budget 10",
        "retarget a.asc b.asc --ri 1.76",
        "sweep a.asc b.asc --tier P1 --from 40 --to 41 --step 1",
        "export a.asc b.asc --format html --out-dir out",
    ] {
        assert_eq!(batch_items(line).len(), 2, "{line}");
    }
    // The job file of a render command is one file, not a design.
    assert!(error("render a.job.json b.job.json").contains("unexpected extra argument"));
}

#[test]
fn designs_may_be_mixed_with_flags_in_any_order() {
    let items = batch_items("metrics --ri 1.76 a.asc --csv b.asc");
    let Command::Metrics(first) = &items[1] else {
        panic!("expected metrics");
    };
    assert_eq!(first.design, PathBuf::from("b.asc"));
    assert_eq!(first.format, ReportFormat::Csv);
    assert_eq!(first.material, Some(MaterialArg::Ri(1.76)));
}

#[test]
fn an_explicit_output_file_is_refused_with_several_designs() {
    for (line, flag) in [
        ("info a.asc b.asc --out r.txt", "--out"),
        ("solve a.asc b.asc --out r.isdd", "--out"),
        ("metrics a.asc b.asc --out r.txt", "--out"),
        ("validate a.asc b.asc --out r.txt", "--out"),
        ("optimize a.asc b.asc --out r.isdd", "--out"),
        ("retarget a.asc b.asc --ri 1.7 --out r.isdd", "--out"),
        (
            "sweep a.asc b.asc --tier P1 --from 1 --to 2 --step 1 --csv r.csv",
            "--csv",
        ),
        ("export a.asc b.asc --out r.asc", "--out"),
    ] {
        let message = error(line);
        assert!(
            message.starts_with(&format!("{flag} names one file, but 2 design files")),
            "{line}: {message}"
        );
        assert!(message.contains("--out-dir DIR"), "{line}: {message}");
    }
}

#[test]
fn out_dir_names_one_file_for_each_design_by_its_stem() {
    let items = batch_items("solve a.asc sub/b.gem --out-dir out");
    let outs: Vec<Option<PathBuf>> = items
        .iter()
        .map(|item| match item {
            Command::Solve(args) => args.out.clone(),
            other => panic!("expected solve, got {other:?}"),
        })
        .collect();
    assert_eq!(
        outs,
        [
            Some(PathBuf::from("out").join("a.isdd")),
            Some(PathBuf::from("out").join("b.isdd"))
        ]
    );
    let Command::Batch(batch) = parsed("solve a.asc b.gem --out-dir out") else {
        panic!("expected a batch");
    };
    assert_eq!(batch.out_dir, Some(PathBuf::from("out")));
    // Without it the reports are printed and nothing is written.
    for item in batch_items("solve a.asc b.gem") {
        let Command::Solve(args) = item else {
            panic!("expected solve");
        };
        assert_eq!(args.out, None);
    }
}

#[test]
fn out_dir_with_one_design_names_its_file_too() {
    let Command::Solve(args) = parsed("solve sub/a.asc --out-dir out") else {
        panic!("expected solve");
    };
    assert_eq!(args.out, Some(PathBuf::from("out").join("a.isdd")));
}

#[test]
fn the_output_extension_follows_the_command_and_its_format() {
    let file_of = |line: &str| -> Vec<String> {
        let items = batch_items(line);
        let mut names = Vec::new();
        for item in &items {
            let out = match item {
                Command::Info(args) => args.out.clone(),
                Command::Metrics(args) => args.out.clone(),
                Command::Validate(args) => args.out.clone(),
                Command::Optimize(args) => args.out.clone(),
                Command::Retarget(args) => args.out.clone(),
                Command::Sweep(args) => args.csv.clone(),
                Command::Export(args) => Some(args.out.clone()),
                other => panic!("unexpected {other:?}"),
            };
            let path = out.expect("an output file");
            names.push(
                path.file_name()
                    .expect("a file name")
                    .to_string_lossy()
                    .into_owned(),
            );
        }
        names
    };
    let cases = [
        ("info a.asc b.asc --out-dir o", "a.txt"),
        ("info a.asc b.asc --json --out-dir o", "a.json"),
        ("metrics a.asc b.asc --out-dir o", "a.txt"),
        ("metrics a.asc b.asc --json --out-dir o", "a.json"),
        ("metrics a.asc b.asc --csv --out-dir o", "a.csv"),
        ("validate a.asc b.asc --out-dir o", "a.txt"),
        ("validate a.asc b.asc --json --out-dir o", "a.json"),
        ("optimize a.asc b.asc --out-dir o", "a.isdd"),
        ("retarget a.asc b.asc --ri 1.7 --out-dir o", "a.isdd"),
        (
            "sweep a.asc b.asc --tier P1 --from 1 --to 2 --step 1 --out-dir o",
            "a.csv",
        ),
        ("export a.asc b.asc --format asc --out-dir o", "a.asc"),
        ("export a.asc b.asc --format isdd --out-dir o", "a.isdd"),
        (
            "export a.asc b.asc --format indicatrix --out-dir o",
            "a.isdd",
        ),
        ("export a.asc b.asc --format gcs --out-dir o", "a.gcs"),
        ("export a.asc b.asc --format html --out-dir o", "a.html"),
    ];
    for (line, first) in cases {
        let names = file_of(line);
        assert_eq!(names.len(), 2, "{line}");
        assert_eq!(names[0], first, "{line}");
        assert_eq!(names[1], first.replace("a.", "b."), "{line}");
    }
}

#[test]
fn two_designs_with_the_same_stem_would_collide_and_are_refused() {
    for line in [
        "solve a/x.asc b/x.gem --out-dir out",
        "solve x.asc x.gem --out-dir out",
        "solve x.asc X.ASC --out-dir out",
        "solve x.asc x.asc --out-dir out",
    ] {
        let message = error(line);
        assert!(
            message.contains("would both be written to"),
            "{line}: {message}"
        );
        assert!(message.contains("\"solve\""), "{line}: {message}");
    }
    // Without --out-dir nothing is written, so equal stems are no problem.
    assert_eq!(batch_items("solve a/x.asc b/x.gem").len(), 2);
    // A different command may use the same stems in another folder.
    assert_eq!(batch_items("info a/x.asc b/y.asc --out-dir out").len(), 2);
}

#[test]
fn out_and_out_dir_are_not_taken_together() {
    assert_eq!(
        error("solve a.asc --out r.isdd --out-dir out"),
        "give one of --out FILE and --out-dir DIR, not both"
    );
    assert_eq!(
        error("sweep a.asc --tier P1 --from 1 --to 2 --step 1 --csv r.csv --out-dir out"),
        "give one of --csv FILE and --out-dir DIR, not both"
    );
}

#[test]
fn export_with_an_out_dir_needs_a_format_and_one_of_the_two_outputs() {
    assert!(error("export a.asc b.asc").contains("needs --out FILE, or --out-dir DIR"));
    assert!(error("export a.asc b.asc --out-dir out").contains("needs --format"));
    let Command::Export(args) = parsed("export a.asc --format html --out-dir out") else {
        panic!("expected export");
    };
    assert_eq!(args.out, PathBuf::from("out").join("a.html"));
    assert_eq!(args.format, ExportFormat::Html);
}

#[test]
fn the_report_style_of_a_batch_follows_the_format() {
    for (line, style) in [
        ("info a.asc b.asc", ReportStyle::Text),
        ("info a.asc b.asc --json", ReportStyle::Json),
        ("metrics a.asc b.asc --csv", ReportStyle::Csv),
        ("metrics a.asc b.asc --json", ReportStyle::Json),
        (
            "export a.asc b.asc --format asc --out-dir o",
            ReportStyle::Text,
        ),
    ] {
        let Command::Batch(batch) = parsed(line) else {
            panic!("expected a batch");
        };
        assert_eq!(batch.style, style, "{line}");
    }
}
