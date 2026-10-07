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
//! # Layout
//!
//! `types` holds the parsed shapes and the flag words, `values` the cursor along the words and
//! the check of one flag's value, and `parsers` one parser per command.

mod parsers;
#[cfg(test)]
mod tests;
mod types;
mod values;

use parsers::{
    parse_export, parse_info, parse_metrics, parse_optimize, parse_render_job, parse_retarget,
    parse_solve, parse_sweep, parse_validate,
};
pub use types::{
    Command, ExportArgs, ExportFormat, InfoArgs, JobCommandKind, MaterialArg, MetricsArgs, Mode,
    OptimizeArgs, RenderJobArgs, ReportFormat, RetargetArgs, SolveArgs, SweepArgs, Topic,
    ValidateArgs, lighting_name, preset_name,
};

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
        "info" => parse_info(rest).map(Command::Info),
        "solve" => parse_solve(rest).map(Command::Solve),
        "metrics" => parse_metrics(rest).map(Command::Metrics),
        "validate" => parse_validate(rest).map(Command::Validate),
        "optimize" => parse_optimize(rest).map(Command::Optimize),
        "retarget" => parse_retarget(rest).map(Command::Retarget),
        "sweep" => parse_sweep(rest).map(Command::Sweep),
        "export" => parse_export(rest).map(Command::Export),
        "render" => parse_render_job(JobCommandKind::Render, rest).map(Command::Render),
        "tilt-video" => parse_render_job(JobCommandKind::TiltVideo, rest).map(Command::TiltVideo),
        other => Err(format!(
            "unknown command {other:?} (expected info, solve, metrics, validate, optimize, \
             retarget, sweep, export, render or tilt-video; see --help)"
        )),
    }
}
