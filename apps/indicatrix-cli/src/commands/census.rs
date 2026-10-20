//! `census`: what the meet-preserving gate does to many designs, as data.
//!
//! The measurement behind the calibration of the Retarget and Optimize constants and behind
//! the decision whether support bindings are worth building (follow-ups plan 2026-10-19,
//! sections 4.1 to 4.4). For every design (the built-in templates and fixtures, design files,
//! the designs of a library opened read-only), every target material and every mode it writes
//! one row: the verdict under the Retarget bounds, the raw shape changes under bounds no stone
//! reaches, the turn travel the gate grants, and for a refused shift whether the refusal is a
//! branch mismatch or a real infeasibility. The report ends with the percentile tables of
//! section 4.4 and the branch summary.
//!
//! # Read-only and deterministic
//!
//! The command changes no design and no library. A design that cannot be read, captured or
//! measured is a row (or, for a panic, a row saying so), never an error: the exit code is 0
//! whenever the census ran. Rows come in a fixed order (templates, fixtures, files, library by
//! ascending entry id; each design's targets in the order given, shift before optimize), numbers
//! have six decimals and nothing reads the clock unless `--timings` adds the timing columns, so
//! the same census prints the same bytes.
//!
//! `--out-dir` writes the retargeted `.isdd` of every Valid row, so any row can be opened in
//! the editor and the Compare window. Progress (one line per design) goes to standard error.

mod load;
mod measure;
mod percentiles;
mod report;
mod rows;
#[cfg(test)]
mod tests;

use crate::{
    args::CensusArgs,
    materials::{Catalogue, resolve_scoring},
    outcome::{CliError, CommandResult, Outcome},
};
use indicatrix_editor::templates::gallery_design;
use measure::{Context, Output, panicked, process};
use rows::Row;
use std::{
    io::Write,
    panic::{AssertUnwindSafe, catch_unwind},
};

/// Runs `census`, printing no progress.
///
/// # Errors
///
/// As [`run_streaming`].
pub fn run(args: &CensusArgs) -> CommandResult {
    run_streaming(args, &mut std::io::sink())
}

/// `(file name, text)` of every retargeted stone to write.
type StoneFiles = Vec<(String, String)>;

/// Measures every design `args` names: the rows and the stones to write.
///
/// # Errors
///
/// [`CliError::usage`] for an unknown target material, [`CliError::io`] for a library that
/// cannot be opened or listed or an `--out-dir` that is not a folder.
fn collect(
    args: &CensusArgs,
    progress: &mut dyn Write,
) -> Result<(Vec<Row>, StoneFiles), CliError> {
    let catalogue = Catalogue::open(args.db.as_deref(), None)?;
    if let Some(dir) = &args.out_dir
        && !dir.is_dir()
    {
        return Err(CliError::io(format!(
            "--out-dir {} is not an existing folder",
            dir.display()
        )));
    }
    let probe = gallery_design(1);
    for target in &args.targets {
        resolve_scoring(&probe, &catalogue, Some(target))?;
    }
    let library = args.db.as_deref().map(load::open_library).transpose()?;
    let sources = load::plan_sources(args, library.as_ref())?;
    let context = Context {
        args,
        catalogue: &catalogue,
    };
    let (mut rows, mut files) = (Vec::new(), Vec::new());
    for (number, source) in sources.iter().enumerate() {
        let subject = load::open_subject(source, library.as_ref());
        let output = catch_unwind(AssertUnwindSafe(|| process(&subject, &context)))
            .unwrap_or_else(|payload| panicked(&subject, &*payload));
        let Output {
            rows: new_rows,
            files: new_files,
        } = output;
        rows.extend(new_rows);
        files.extend(new_files);
        let _ = writeln!(
            progress,
            "census: {}/{} {} {}",
            number + 1,
            sources.len(),
            subject.source,
            subject.key
        );
    }
    Ok((rows, files))
}

/// Runs `census`, writing one progress line per design to `progress`.
///
/// # Errors
///
/// As [`collect`].
pub fn run_streaming(args: &CensusArgs, progress: &mut dyn Write) -> CommandResult {
    let (rows, files) = collect(args, progress)?;
    let summary = percentiles::summarize(&rows);
    let body = if args.output.json {
        report::json(&rows, args.output.timings, &summary)
    } else {
        report::csv(&rows, args.output.timings, &summary)
    };
    let mut outcome = match &args.out {
        Some(path) => Outcome::text(report::summary_text(&summary)).with_file(path.clone(), body),
        None => Outcome::text(body),
    };
    if let Some(dir) = &args.out_dir {
        for (name, text) in files {
            outcome = outcome.with_file(dir.join(name), text);
        }
    }
    Ok(outcome)
}
