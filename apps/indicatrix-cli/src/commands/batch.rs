//! Several design files on one command line: each runs as the command it would have been
//! alone, and their results are put together.
//!
//! # What each file gets
//!
//! A file's command is exactly the single-file command, so its report, its notes and its output
//! file are the ones the same command prints for that file alone. Nothing about one file
//! changes what another prints, and a file that fails does not stop the run.
//!
//! # Putting the results together
//!
//! - **Standard output.** Text reports follow one another, each under a `==> FILE <==` line and
//!   separated by a blank line, as `head` does for several files. JSON reports follow one
//!   another as complete documents with no line between them (a stream of values, which
//!   `jq` and `serde_json::StreamDeserializer` read as it is). `metrics --csv` keeps its header
//!   line once and adds the row of every design, whose first column names it. A file whose
//!   command printed nothing (a report sent to its `--out-dir` file) adds nothing.
//! - **Standard error.** What a file printed there (notes, `error: ...`) comes under the same
//!   `==> FILE <==` line, so it is clear whose it is. When any file failed, the last line is
//!   `error: N of M design files failed`.
//! - **Files.** The files of the files that worked, in order. [`crate::run`] writes them even
//!   when another file failed: a file that was refused never leaves one behind, but its
//!   neighbours' stay.
//! - **Exit code.** 0 when every file worked. Otherwise the code of the first file that failed
//!   (1, 2, 3 or 5), else 4 when no file failed but one is a `validate` Problem.

use super::dispatch;
use crate::{
    args::{Batch, ReportStyle},
    outcome::{CliError, CommandResult, EXIT_OK, EXIT_PROBLEM, Outcome},
};
use std::fmt::Write as _;

/// `text` with a line feed at its end, unless it is empty or has one already.
fn with_line_feed(text: &str) -> String {
    if text.is_empty() || text.ends_with('\n') {
        text.to_string()
    } else {
        format!("{text}\n")
    }
}

/// Adds the standard output of one file to `all`; `shown` counts the files already added.
fn add_stdout(all: &mut String, label: &str, text: &str, style: ReportStyle, shown: usize) {
    if text.is_empty() {
        return;
    }
    let text = with_line_feed(text);
    match style {
        ReportStyle::Text => {
            if shown > 0 {
                all.push('\n');
            }
            let _ = writeln!(all, "==> {label} <==");
            all.push_str(&text);
        }
        ReportStyle::Json => all.push_str(&text),
        ReportStyle::Csv => {
            if shown == 0 {
                all.push_str(&text);
            } else if let Some((_, rows)) = text.split_once('\n') {
                all.push_str(rows);
            }
        }
    }
}

/// Adds what one file printed on standard error to `all`, under its `==> FILE <==` line.
fn add_stderr(all: &mut String, label: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    let _ = writeln!(all, "==> {label} <==");
    all.push_str(&with_line_feed(text));
}

/// Runs every file of `batch` and puts the results together (see the module documentation).
///
/// # Errors
///
/// [`CliError`] (exit 3) when `--out-dir` names a folder that does not exist. A file that
/// fails is not an error here: it is part of the outcome.
pub fn run(batch: &Batch) -> CommandResult {
    if let Some(dir) = &batch.out_dir
        && !dir.is_dir()
    {
        return Err(CliError::io(format!(
            "--out-dir {} is not a folder that exists",
            dir.display()
        )));
    }
    let total = batch.items.len();
    let mut all = Outcome::default();
    let mut failed = 0_usize;
    let mut first_failure = None;
    let mut problem = false;
    let mut shown = 0_usize;
    for item in &batch.items {
        let label = item
            .design()
            .map_or_else(String::new, |path| path.display().to_string());
        let result = dispatch(item).unwrap_or_else(|error| Outcome::failure(&error));
        match result.exit {
            EXIT_OK => {}
            EXIT_PROBLEM => problem = true,
            code => {
                failed += 1;
                first_failure.get_or_insert(code);
            }
        }
        if !result.stdout.is_empty() {
            add_stdout(&mut all.stdout, &label, &result.stdout, batch.style, shown);
            shown += 1;
        }
        add_stderr(&mut all.stderr, &label, &result.stderr);
        all.files.extend(result.files);
    }
    if failed > 0 {
        let _ = writeln!(all.stderr, "error: {failed} of {total} design files failed");
    }
    all.exit = first_failure.unwrap_or(if problem { EXIT_PROBLEM } else { EXIT_OK });
    Ok(all)
}

#[cfg(test)]
mod tests {
    use crate::{
        outcome::{EXIT_IO, EXIT_OK, EXIT_PROBLEM},
        testing,
    };

    /// Two good designs, written under names of their own so tests running side by side never
    /// write the same file.
    fn two_designs(tag: &str) -> (String, String) {
        (
            testing::write_design(&format!("batch-{tag}-a.isdd"), &testing::template()),
            testing::write_design(&format!("batch-{tag}-b.isdd"), &testing::brilliant_at_172()),
        )
    }

    fn missing_file(name: &str) -> String {
        testing::temp_path(name).to_string_lossy().into_owned()
    }

    #[test]
    fn the_text_reports_of_several_designs_come_under_their_file_names() {
        let (a, b) = two_designs("text");
        let outcome = testing::run(&["info", &a, &b]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let first = testing::run(&["info", &a]);
        let second = testing::run(&["info", &b]);
        assert_eq!(
            outcome.stdout,
            format!(
                "==> {a} <==\n{}\n==> {b} <==\n{}",
                first.stdout, second.stdout
            )
        );
    }

    #[test]
    fn the_json_reports_of_several_designs_are_a_stream_of_documents() {
        let (a, b) = two_designs("json");
        let outcome = testing::run(&["info", &a, &b, "--json"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let first = testing::run(&["info", &a, "--json"]);
        let second = testing::run(&["info", &b, "--json"]);
        assert_eq!(outcome.stdout, format!("{}{}", first.stdout, second.stdout));
        let documents = serde_json::Deserializer::from_str(&outcome.stdout)
            .into_iter::<serde_json::Value>()
            .count();
        assert_eq!(documents, 2);
    }

    #[test]
    fn metrics_csv_keeps_one_header_line_and_a_row_for_each_design() {
        let (a, b) = two_designs("csv");
        let line = |text: &str, n: usize| text.lines().nth(n).map(str::to_string);
        let outcome = testing::run(&["metrics", &a, &b, "--ri", "1.76", "--csv"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        let first = testing::run(&["metrics", &a, "--ri", "1.76", "--csv"]);
        let second = testing::run(&["metrics", &b, "--ri", "1.76", "--csv"]);
        assert_eq!(outcome.stdout.lines().count(), 3, "{}", outcome.stdout);
        assert_eq!(line(&outcome.stdout, 0), line(&first.stdout, 0));
        assert_eq!(line(&outcome.stdout, 1), line(&first.stdout, 1));
        assert_eq!(line(&outcome.stdout, 2), line(&second.stdout, 1));
    }

    #[test]
    fn a_failing_file_is_reported_and_the_others_still_run_and_write() {
        let (a, b) = two_designs("fail");
        let missing = missing_file("batch-fail-no-such-file.isdd");
        let folder = testing::temp_path("batch-fail-out");
        let _ = std::fs::create_dir_all(&folder);
        let dir = folder.to_string_lossy().into_owned();
        let outcome = testing::run(&["solve", &a, &missing, &b, "--out-dir", &dir]);
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        assert_eq!(outcome.files.len(), 2, "the two good designs are saved");
        assert!(outcome.files[0].path.ends_with("batch-fail-a.isdd"));
        assert!(outcome.files[1].path.ends_with("batch-fail-b.isdd"));
        assert!(outcome.files[0].path.starts_with(&folder));
        assert!(
            outcome
                .stderr
                .contains(&format!("==> {missing} <==\nerror: ")),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome
                .stderr
                .ends_with("error: 1 of 3 design files failed\n"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn the_exit_code_is_the_first_failure_else_zero() {
        let (a, _) = two_designs("exit");
        let missing = missing_file("batch-exit-gone.isdd");
        let broken = testing::temp_path("batch-exit-broken.isdd");
        std::fs::write(&broken, "this is not a design").expect("the folder is writable");
        let broken = broken.to_string_lossy().into_owned();
        let outcome = testing::run(&["info", &missing, &broken, &a]);
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        let outcome = testing::run(&["info", &broken, &missing, &a]);
        assert_ne!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_ne!(outcome.exit, EXIT_PROBLEM, "{}", outcome.stderr);
        let outcome = testing::run(&["info", &a, &a]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
    }

    #[test]
    fn a_folder_that_does_not_exist_is_an_io_error_before_anything_runs() {
        let (a, b) = two_designs("folder");
        let no_folder = missing_file("batch-folder-no-such-folder");
        let outcome = testing::run(&["solve", &a, &b, "--out-dir", &no_folder]);
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        assert!(
            outcome.stderr.starts_with("error: --out-dir "),
            "{}",
            outcome.stderr
        );
        assert_eq!(outcome.stdout, "");
        assert_eq!(outcome.files, [] as [crate::outcome::OutFile; 0]);
    }
}
