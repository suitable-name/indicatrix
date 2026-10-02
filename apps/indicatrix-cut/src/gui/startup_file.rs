//! A design named on the command line: `indicatrix-cut <path>` opens that file once the
//! window is up.
//!
//! That is what an operating-system "open with" or double-click association does (the
//! shell starts the program with the file as its argument).
//!
//! `main` hands the arguments to [`path_from_args`] and the result to
//! [`request_open_at_startup`]; the editor's startup sequence collects it with
//! [`take_requested_open`] and opens it through the same path File > Open Recent uses,
//! so a `.indicatrix` design file, an older `.indicatrix.toml` sidecar, a `.asc` and a
//! `.gem`/`.gcs` all work. A second launch does not forward its file to a running
//! instance; each launch opens its own window.

use std::{ffi::OsString, path::PathBuf, sync::Mutex};

/// The file the command line asked to open, until the editor takes it.
static REQUESTED: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The file to open from a program's arguments (without the program name): the first
/// argument that is not a flag.
///
/// Flags (`--log`, or anything else starting with `-`) are skipped; after a bare `--`
/// the next argument is a file even if it starts with `-`. `None` when no file was
/// named.
#[must_use]
pub fn path_from_args(args: impl IntoIterator<Item = OsString>) -> Option<PathBuf> {
    let mut only_files = false;
    for arg in args {
        if arg.is_empty() {
            continue;
        }
        if !only_files {
            if arg == "--" {
                only_files = true;
                continue;
            }
            if arg.to_string_lossy().starts_with('-') {
                continue;
            }
        }
        return Some(PathBuf::from(arg));
    }
    None
}

/// Asks the editor to open `path` once it has started.
///
/// The path is made absolute first (the recent-files list must not hold a path relative
/// to a working directory that is gone next launch). Meant to be called once, before
/// the window is built.
pub fn request_open_at_startup(path: PathBuf) {
    let path = std::path::absolute(&path).unwrap_or(path);
    *REQUESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(path);
}

/// Takes the file [`request_open_at_startup`] recorded, if any.
pub(crate) fn take_requested_open() -> Option<PathBuf> {
    REQUESTED
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<OsString> {
        list.iter().map(OsString::from).collect()
    }

    #[test]
    fn no_arguments_name_no_file() {
        assert_eq!(path_from_args(args(&[])), None);
        assert_eq!(path_from_args(args(&["--log"])), None);
    }

    #[test]
    fn the_first_argument_that_is_not_a_flag_is_the_file() {
        assert_eq!(
            path_from_args(args(&["design.indicatrix"])),
            Some(PathBuf::from("design.indicatrix"))
        );
        assert_eq!(
            path_from_args(args(&[
                "--log",
                "C:\\designs\\round.indicatrix",
                "other.asc"
            ])),
            Some(PathBuf::from("C:\\designs\\round.indicatrix"))
        );
        assert_eq!(
            path_from_args(args(&["old.asc", "--log"])),
            Some(PathBuf::from("old.asc"))
        );
    }

    #[test]
    fn a_path_with_spaces_stays_one_argument() {
        assert_eq!(
            path_from_args(args(&["my designs/round brilliant.indicatrix"])),
            Some(PathBuf::from("my designs/round brilliant.indicatrix"))
        );
    }

    #[test]
    fn after_a_double_dash_a_leading_dash_is_part_of_the_file_name() {
        assert_eq!(
            path_from_args(args(&["--", "-odd.indicatrix"])),
            Some(PathBuf::from("-odd.indicatrix"))
        );
        assert_eq!(path_from_args(args(&["--"])), None);
    }

    #[test]
    fn empty_arguments_are_ignored() {
        assert_eq!(
            path_from_args(args(&["", "a.indicatrix"])),
            Some(PathBuf::from("a.indicatrix"))
        );
    }

    #[test]
    fn a_requested_file_is_taken_exactly_once() {
        request_open_at_startup(PathBuf::from("take_once_test.indicatrix"));
        let taken = take_requested_open().expect("a request was recorded");
        assert!(taken.is_absolute());
        assert!(taken.ends_with("take_once_test.indicatrix"));
        assert_eq!(take_requested_open(), None);
    }
}
