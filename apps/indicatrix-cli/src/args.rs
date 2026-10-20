//! Hand-rolled argument parsing: [`parse`] turns the words after the program name into a
//! [`Command`] with one typed argument struct per command.
//!
//! # Why `Result<_, String>`, not a typed error
//!
//! Like the worker's parser, every error here is terminal, user-facing text: `main` prints it
//! after `error: ` with the exit code 1 and nothing ever matches on the kind of a parse
//! failure. A typed enum would add a variant per malformed flag for no caller that needs one.
//!
//! Nothing here reads a file or runs a solve. Numbers are checked for being numbers, and for
//! the ranges that need no design to judge (a refractive index above 1, a candidate count of
//! 1 to 5); what only a design can judge (a tier name, a sweep range) is judged by the
//! command.
//!
//! # Several designs
//!
//! Every command that takes a design file takes several. A line with one design is the plain
//! command; a line with several is a [`Command::Batch`] holding the plain command of each, with
//! the naming rule for their output files in `batch`.
//!
//! # Layout
//!
//! `types` holds the parsed shapes and the flag words, `values` the cursor along the words and
//! the check of one flag's value, `parsers` one parser per command, and `batch` the step that
//! turns several design files into one command each.

mod batch;
mod parsers;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_batch;
mod types;
mod values;

use parsers::{
    parse_census, parse_export, parse_info, parse_metrics, parse_optimize, parse_render_job,
    parse_retarget, parse_solve, parse_sweep, parse_validate,
};
pub use types::{
    Batch, CensusArgs, Command, ExportArgs, ExportFormat, InfoArgs, JobCommandKind, MaterialArg,
    MetricsArgs, Mode, OptimizeArgs, RenderJobArgs, ReportFormat, ReportStyle, RetargetArgs,
    SolveArgs, SweepArgs, Topic, ValidateArgs, lighting_name, preset_name,
};
#[cfg(test)]
pub use types::{CensusBuiltins, CensusOutput};

/// Parses `argv` (without the program name).
///
/// `-h` or `--help` anywhere short-circuits to the help page of the first word, even when other
/// required flags are missing.
///
/// # Errors
///
/// A sentence naming the first thing wrong with `argv`.
pub fn parse(argv: &[String]) -> Result<Command, String> {
    let Some(first) = argv.first().map(String::as_str) else {
        return Ok(Command::Help(Topic::Root));
    };
    if argv.iter().any(|a| a == "-h" || a == "--help") {
        return Ok(Command::Help(Topic::of(first)));
    }
    let rest = &argv[1..];
    match first {
        "-V" | "--version" | "version" => Ok(Command::Version),
        "help" => Ok(Command::Help(
            rest.first().map_or(Topic::Root, |w| Topic::of(w)),
        )),
        "info" => parse_info(rest),
        "solve" => parse_solve(rest),
        "metrics" => parse_metrics(rest),
        "validate" => parse_validate(rest),
        "optimize" => parse_optimize(rest),
        "retarget" => parse_retarget(rest),
        "sweep" => parse_sweep(rest),
        "export" => parse_export(rest),
        "census" => parse_census(rest),
        "render" => parse_render_job(JobCommandKind::Render, rest).map(Command::Render),
        "tilt-video" => parse_render_job(JobCommandKind::TiltVideo, rest).map(Command::TiltVideo),
        other => Err(format!(
            "unknown command {other:?} (expected info, solve, metrics, validate, optimize, \
             retarget, sweep, export, census, render or tilt-video; see --help)"
        )),
    }
}
