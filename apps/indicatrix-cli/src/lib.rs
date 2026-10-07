//! `indicatrix-cli`: the Indicatrix engines from a script, without the editor.
//!
//! The binary is a thin shell around this library: [`run`] takes the command line (without the
//! program name), runs the command and writes the files it asks for, and hands back an
//! [`Outcome`] with the text for standard output and standard error and the exit code.
//! [`run_command_line`] is the same without writing those files, which is what the tests use.
//! The two render commands (`render`, `tilt-video`) are the exception: they write their pictures
//! themselves and stream progress while they run.
//!
//! # Commands
//!
//! | Command | What it does |
//! |---|---|
//! | `info` | name, gear, symmetry, tiers and material of a design |
//! | `solve` | solve the design, report closure and warnings, optionally save it |
//! | `metrics` | optical and geometric figures, as text, JSON or CSV |
//! | `validate` | manufacturability warnings and the overall verdict (exit 4 on Problem) |
//! | `optimize` | search the free tier angles, as the Optimize tab does |
//! | `retarget` | new angles for another material, through the dialog's engine and gate |
//! | `sweep` | one tier's angle over a range, every angle scored |
//! | `export` | `.asc`, `.indicatrix`, `.gcs` or the cutting sheet as HTML |
//! | `render` | render a still picture from a render job file (`*.job.json`) |
//! | `tilt-video` | render a tilt performance video from a render job file; resumes from its frames |
//!
//! Designs open from `.indicatrix`, `.asc`, `.gem` and `.gcs` files. Custom materials come from
//! a design library given with `--db`, opened read-only. Render job files are written by the
//! desktop app's render queue. See each command's `--help`.
//!
//! # Exit codes
//!
//! | Code | Meaning |
//! |---|---|
//! | 0 | success |
//! | 1 | the command line is wrong |
//! | 2 | the design cannot be used, or a result was refused |
//! | 3 | a file could not be read or written |
//! | 4 | `validate`: the verdict is Problem |
//! | 5 | `render`, `tilt-video`: the render failed or was stopped |
//!
//! For `render` and `tilt-video`, code 2 means the job file cannot be used and code 3 that a
//! file cannot be read or written.
//!
//! # Determinism
//!
//! The same command on the same files prints the same bytes: numbers have a fixed number of
//! decimals, lists a fixed order, and nothing reads the clock or mints an id. Only the thread
//! count of a sweep depends on the machine: it sizes its worker pool from the processor count,
//! but each row is a pure function of its angle and the rows come back in angle order.
//!
//! `render` and `tilt-video` keep standard output deterministic (one line, the path written),
//! while they print progress and an estimated time left to standard error.

mod args;
mod commands;
mod format;
mod help;
mod load;
mod materials;
mod outcome;
mod stone;
#[cfg(test)]
mod testing;

pub use outcome::{
    EXIT_DESIGN, EXIT_IO, EXIT_OK, EXIT_PROBLEM, EXIT_RENDER, EXIT_USAGE, OutFile, Outcome,
};

use outcome::CliError;
use std::{fmt::Write as _, io::IsTerminal as _};

/// Parses and runs the command line, streaming the progress of `render` and `tilt-video`
/// to `progress`. Every other command writes nothing to it.
fn execute(args: &[String], progress: &mut dyn std::io::Write, terminal: bool) -> Outcome {
    let command = match args::parse(args) {
        Ok(command) => command,
        Err(message) => {
            let mut outcome = Outcome::failure(&CliError::usage(message));
            outcome.stderr.push_str("try: indicatrix-cli --help\n");
            return outcome;
        }
    };
    commands::dispatch_streaming(&command, progress, terminal)
        .unwrap_or_else(|error| Outcome::failure(&error))
}

/// Runs the command line `args` (the words after the program name) without writing any file
/// of its own, except for `render` and `tilt-video`, which write their pictures and frames
/// themselves.
///
/// The files the command wants written are in [`Outcome::files`]; [`run`] writes them. The
/// progress of `render` and `tilt-video` (never a terminal here) is put in front of
/// [`Outcome::stderr`], so a caller sees everything the command printed.
#[must_use]
pub fn run_command_line(args: &[String]) -> Outcome {
    let mut buffer: Vec<u8> = Vec::new();
    let mut outcome = execute(args, &mut buffer, false);
    if !buffer.is_empty() {
        outcome
            .stderr
            .insert_str(0, &String::from_utf8_lossy(&buffer));
    }
    outcome
}

/// Runs the command line `args` (the words after the program name) and writes the files the
/// command asks for.
///
/// Files are written only when the command succeeded (exit code 0), or when it succeeded in
/// finding a Problem (`validate`, exit code 4): a command that refuses its result never leaves
/// a file behind. A file that cannot be written ends the run with exit code 3. Each file that
/// was written is noted on standard error.
///
/// `render` and `tilt-video` write their pictures themselves and stream their progress to
/// standard error while they run (one rewritten line when it is a terminal): that part is
/// already out when this returns, and [`Outcome::stderr`] holds only the rest (notes, the
/// error line), which the caller prints.
#[must_use]
pub fn run(args: &[String]) -> Outcome {
    let mut stderr = std::io::stderr();
    let terminal = stderr.is_terminal();
    let mut outcome = execute(args, &mut stderr, terminal);
    if outcome.exit != EXIT_OK && outcome.exit != EXIT_PROBLEM {
        return outcome;
    }
    for file in &outcome.files {
        match std::fs::write(&file.path, &file.text) {
            Ok(()) => {
                let _ = writeln!(outcome.stderr, "wrote {}", file.path.display());
            }
            Err(error) => {
                let _ = writeln!(
                    outcome.stderr,
                    "error: cannot write {}: {error}",
                    file.path.display()
                );
                outcome.exit = EXIT_IO;
                break;
            }
        }
    }
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing;

    fn words(line: &str) -> Vec<String> {
        line.split_whitespace().map(ToString::to_string).collect()
    }

    #[test]
    fn no_arguments_print_the_command_list() {
        let outcome = run_command_line(&[]);
        assert_eq!(outcome.exit, EXIT_OK);
        assert!(outcome.stdout.contains("info"), "{}", outcome.stdout);
        assert_eq!(outcome.stderr, "");
    }

    #[test]
    fn a_command_line_mistake_is_exit_1_with_a_hint() {
        let outcome = run_command_line(&words("frobnicate x.asc"));
        assert_eq!(outcome.exit, EXIT_USAGE);
        assert_eq!(outcome.stdout, "");
        assert!(
            outcome.stderr.starts_with("error: unknown command"),
            "{}",
            outcome.stderr
        );
        assert!(
            outcome.stderr.ends_with("try: indicatrix-cli --help\n"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn help_and_version_are_exit_0() {
        for line in ["--help", "info --help", "help retarget", "--version"] {
            let outcome = run_command_line(&words(line));
            assert_eq!(outcome.exit, EXIT_OK, "{line}");
            assert!(!outcome.stdout.is_empty(), "{line}");
        }
    }

    #[test]
    fn a_file_that_cannot_be_read_is_exit_3() {
        let outcome = run_command_line(&words("info no-such-folder/no-such-file.indicatrix"));
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        assert!(
            outcome.stderr.starts_with("error: cannot read"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn run_writes_the_files_and_says_so() {
        let input = testing::write_design("lib-run.indicatrix", &testing::template());
        let target = testing::temp_path("lib-run-out.indicatrix");
        let _ = std::fs::remove_file(&target);
        let outcome = run(&[
            "solve".to_string(),
            input,
            "--out".to_string(),
            target.to_string_lossy().into_owned(),
        ]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert!(target.is_file(), "the file is on disk");
        assert!(outcome.stderr.contains("wrote "), "{}", outcome.stderr);
        let text = std::fs::read_to_string(&target).expect("readable");
        assert_ne!(text, "");
    }

    #[test]
    fn run_ends_with_exit_3_when_a_file_cannot_be_written() {
        let input = testing::write_design("lib-run-bad.indicatrix", &testing::template());
        let target = testing::temp_path("no-such-folder").join("out.indicatrix");
        let outcome = run(&[
            "solve".to_string(),
            input,
            "--out".to_string(),
            target.to_string_lossy().into_owned(),
        ]);
        assert_eq!(outcome.exit, EXIT_IO, "{}", outcome.stderr);
        assert!(
            outcome.stderr.contains("error: cannot write"),
            "{}",
            outcome.stderr
        );
    }

    #[test]
    fn a_refused_command_leaves_no_file_behind() {
        let target = testing::temp_path("lib-refused.indicatrix");
        let _ = std::fs::remove_file(&target);
        let outcome = run(&[
            "solve".to_string(),
            "no-such-file.indicatrix".to_string(),
            "--out".to_string(),
            target.to_string_lossy().into_owned(),
        ]);
        assert_eq!(outcome.exit, EXIT_IO);
        assert!(!target.exists());
    }
}
