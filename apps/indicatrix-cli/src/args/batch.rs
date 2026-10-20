//! Several design files on one command line: [`fan_out`] turns the files a parser collected
//! into the single-design command of each, and decides where each one's output goes.
//!
//! # The naming rule
//!
//! One design keeps what it always did: the explicit file (`--out FILE`, or `--csv FILE` for
//! `sweep`) is where its output goes. Several designs cannot share one file, so a line with
//! several designs and an explicit file is refused. `--out-dir DIR` writes one file for each
//! design instead, named `<stem of the design>.<extension of the output>` in `DIR`
//! (`round.asc` and `--out-dir out` give `out/round.isdd` for `solve`). Two designs that would
//! get the same name (`a/x.asc` and `b/x.asc`, or `x.asc` and `x.gem`) are refused before
//! anything runs, rather than the second silently replacing the first. Names are compared
//! without regard to case, because the folder may be on a file system that ignores it.
//!
//! Nothing here touches the disk: a folder that does not exist is found when the batch runs.

use super::types::{Batch, Command, ReportStyle};
use std::path::{Path, PathBuf};

/// What a parser hands to [`fan_out`].
pub(super) struct Fan<'a> {
    /// The command word, for the messages.
    pub(super) command: &'static str,
    /// Every design file the line named, in order (at least one).
    pub(super) designs: Vec<PathBuf>,
    /// The one explicit output file the line gave, with the flag that gave it.
    pub(super) out: Option<(&'static str, PathBuf)>,
    /// The folder `--out-dir` named.
    pub(super) out_dir: Option<PathBuf>,
    /// The extension of the output file of this command, without the dot.
    pub(super) extension: &'a str,
    /// How the reports of several designs are put together.
    pub(super) style: ReportStyle,
}

/// The file `--out-dir` gives `design`: its stem and `extension` in `dir`.
fn target_in(dir: &Path, design: &Path, extension: &str) -> PathBuf {
    let stem = design.file_stem().map_or_else(
        || "design".to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    dir.join(format!("{stem}.{extension}"))
}

/// The name of an output file as compared for collisions: lower case, because the folder may
/// be on a file system that ignores case.
fn name_key(target: Option<&PathBuf>) -> Option<String> {
    target
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().to_ascii_lowercase())
}

/// An error naming two designs that would be written to the same file.
fn collision(command: &str, designs: &[PathBuf], targets: &[Option<PathBuf>]) -> Option<String> {
    for (later, target) in targets.iter().enumerate() {
        let Some(name) = name_key(target.as_ref()) else {
            continue;
        };
        let Some(earlier) = targets
            .iter()
            .take(later)
            .position(|other| name_key(other.as_ref()).as_deref() == Some(name.as_str()))
        else {
            continue;
        };
        return Some(format!(
            "\"{command}\": {} and {} would both be written to {}; rename one of them, or run \
             them separately",
            designs.get(earlier)?.display(),
            designs.get(later)?.display(),
            target.as_ref()?.display(),
        ));
    }
    None
}

/// The command line as a command: the plain command for one design, a [`Batch`] for several.
///
/// `build` makes the single-design command for one file and its output file (`None`: the
/// command prints its report, or writes no file).
///
/// # Errors
///
/// A sentence when both `--out`-style file and `--out-dir` are given, when several designs
/// are given with an explicit file, or when two of them would get the same output name.
pub(super) fn fan_out(
    fan: Fan<'_>,
    build: impl Fn(PathBuf, Option<PathBuf>) -> Command,
) -> Result<Command, String> {
    let Fan {
        command,
        designs,
        out,
        out_dir,
        extension,
        style,
    } = fan;
    if let (Some((flag, _)), Some(_)) = (&out, &out_dir) {
        return Err(format!(
            "give one of {flag} FILE and --out-dir DIR, not both"
        ));
    }
    if let [only] = designs.as_slice() {
        let target = match (out, &out_dir) {
            (Some((_, file)), _) => Some(file),
            (None, Some(dir)) => Some(target_in(dir, only, extension)),
            (None, None) => None,
        };
        return Ok(build(only.clone(), target));
    }
    if let Some((flag, _)) = &out {
        return Err(format!(
            "{flag} names one file, but {} design files were given: use --out-dir DIR to write \
             one file for each",
            designs.len()
        ));
    }
    let targets: Vec<Option<PathBuf>> = designs
        .iter()
        .map(|design| {
            out_dir
                .as_deref()
                .map(|dir| target_in(dir, design, extension))
        })
        .collect();
    if let Some(message) = collision(command, &designs, &targets) {
        return Err(message);
    }
    let items = designs
        .into_iter()
        .zip(targets)
        .map(|(design, target)| build(design, target))
        .collect();
    Ok(Command::Batch(Batch {
        items,
        out_dir,
        style,
    }))
}
