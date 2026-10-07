//! The commands: one module each, and [`dispatch`], which runs a parsed [`Command`].
//!
//! Each command module has a `run` function taking its argument struct and returning a
//! [`CommandResult`]: the text for stdout, the files to write and the exit code, never a print
//! and never a write. The pieces every command shares are here.

mod export;
mod info;
mod metrics;
mod optimize;
mod render_job;
mod retarget;
mod solve;
mod sweep;
mod validate;

use crate::{
    args::Command,
    help,
    load::{Loaded, load_path},
    materials::Catalogue,
    outcome::{CliError, CommandResult, Outcome},
};
use std::{io::Write, path::Path};

/// Runs a parsed command.
///
/// # Errors
///
/// Whatever the command refuses: see each command's own documentation.
pub fn dispatch(command: &Command) -> CommandResult {
    match command {
        Command::Help(topic) => Ok(Outcome::text(help::text(*topic))),
        Command::Version => Ok(Outcome::text(format!(
            "indicatrix-cli {}\n",
            env!("CARGO_PKG_VERSION")
        ))),
        Command::Info(args) => info::run(args),
        Command::Solve(args) => solve::run(args),
        Command::Metrics(args) => metrics::run(args),
        Command::Validate(args) => validate::run(args),
        Command::Optimize(args) => optimize::run(args),
        Command::Retarget(args) => retarget::run(args),
        Command::Sweep(args) => sweep::run(args),
        Command::Export(args) => export::run(args),
        Command::Render(_) | Command::TiltVideo(_) => Err(CliError::usage(
            "internal: render commands need dispatch_streaming",
        )),
    }
}

/// Runs a parsed command, streaming progress to `progress` for the two render commands.
///
/// `render` and `tilt-video` write their pictures themselves and report progress as they go
/// (`terminal` says `progress` is a terminal, which rewrites one line); every other command
/// is [`dispatch`].
///
/// # Errors
///
/// Whatever the command refuses: see each command's own documentation.
pub fn dispatch_streaming(
    command: &Command,
    progress: &mut dyn Write,
    terminal: bool,
) -> CommandResult {
    match command {
        Command::Render(args) | Command::TiltVideo(args) => {
            render_job::run(args, progress, terminal)
        }
        other => dispatch(other),
    }
}

/// Opens the design file and the catalogue its commands resolve materials against.
fn open(design: &Path, db: Option<&Path>) -> Result<(Loaded, Catalogue), CliError> {
    let loaded = load_path(design)?;
    let catalogue = loaded.catalogue(db)?;
    Ok((loaded, catalogue))
}

/// A report for stdout, or for the file `out` names. The note on stderr saying a file was
/// written comes from [`crate::run`], which writes the files.
fn deliver(report: String, out: Option<&Path>) -> Outcome {
    match out {
        None => Outcome::text(report),
        Some(path) => Outcome::default().with_file(path.to_path_buf(), report),
    }
}

/// `lines` as a list under a heading; empty text when there are none.
fn list_block(heading: &str, lines: &[String]) -> String {
    if lines.is_empty() {
        return String::new();
    }
    let mut text = format!("{heading} ({}):\n", lines.len());
    for line in lines {
        text.push_str("  - ");
        text.push_str(line);
        text.push('\n');
    }
    text
}

/// The `Label:  value` lines of a report header, the labels padded to the longest.
fn key_values(pairs: &[(&str, String)]) -> String {
    let width = pairs
        .iter()
        .map(|(label, _)| label.chars().count())
        .max()
        .unwrap_or(0);
    let mut text = String::new();
    for (label, value) in pairs {
        let pad = width.saturating_sub(label.chars().count());
        text.push_str(label);
        text.push(':');
        text.push_str(&" ".repeat(pad + 2));
        text.push_str(value);
        text.push('\n');
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{args::Topic, outcome::EXIT_OK};

    #[test]
    fn help_and_version_are_plain_text() {
        let help = dispatch(&Command::Help(Topic::Root)).expect("help");
        assert!(help.stdout.starts_with("indicatrix-cli:"));
        assert_eq!(help.exit, EXIT_OK);
        let version = dispatch(&Command::Version).expect("version");
        assert_eq!(
            version.stdout,
            format!("indicatrix-cli {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn the_render_commands_need_the_streaming_dispatch() {
        let command = crate::args::parse(&["render".to_string(), "x.job.json".to_string()])
            .expect("a render command line");
        let error = dispatch(&command).expect_err("dispatch cannot stream");
        assert!(
            error.message.contains("dispatch_streaming"),
            "{}",
            error.message
        );
        let mut progress: Vec<u8> = Vec::new();
        let help = dispatch_streaming(&Command::Help(Topic::Render), &mut progress, false)
            .expect("other commands fall back to dispatch");
        assert!(help.stdout.contains("render"));
        assert_eq!(progress.len(), 0);
    }

    #[test]
    fn a_report_goes_to_stdout_or_to_a_file() {
        let printed = deliver("text\n".to_string(), None);
        assert_eq!(printed.stdout, "text\n");
        assert_eq!(printed.files.len(), 0);
        let written = deliver("text\n".to_string(), Some(Path::new("out.txt")));
        assert_eq!(written.stdout, "");
        assert_eq!(written.files[0].text, "text\n");
        assert!(
            written.stderr.is_empty(),
            "the notes of a written file come from run()"
        );
    }

    #[test]
    fn lists_and_key_values_are_laid_out_for_reading() {
        assert_eq!(list_block("Warnings", &[]), "");
        assert_eq!(
            list_block("Warnings", &["one".to_string(), "two".to_string()]),
            "Warnings (2):\n  - one\n  - two\n"
        );
        assert_eq!(
            key_values(&[("Name", "A".to_string()), ("Material", "B".to_string())]),
            "Name:      A\nMaterial:  B\n"
        );
    }
}
