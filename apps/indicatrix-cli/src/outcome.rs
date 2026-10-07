//! What a command hands back to `main`: text for stdout and stderr, files to write, and the
//! process exit code.
//!
//! A command never prints and never touches the disk for its output. It returns an
//! [`Outcome`], and [`crate::run`] writes the files and `main` prints the text. That keeps
//! every command a pure function of its arguments and the files it reads, which is what lets
//! the tests run them in-process and compare their text byte for byte.
//!
//! The two render commands (`render`, `tilt-video`) are the documented exception: they write
//! their pictures themselves and stream progress to a writer they are given.

use std::path::PathBuf;

/// Success.
pub const EXIT_OK: i32 = 0;

/// The command line is wrong: an unknown command or flag, a missing or unreadable value.
pub const EXIT_USAGE: i32 = 1;

/// The design cannot be used: it does not parse, does not solve or close, or a result was
/// refused (an invalid retarget, a search with nothing to offer).
pub const EXIT_DESIGN: i32 = 2;

/// A file could not be read or written.
pub const EXIT_IO: i32 = 3;

/// `validate` only: the overall verdict is Problem.
pub const EXIT_PROBLEM: i32 = 4;

/// `render` and `tilt-video` only: the render failed or was stopped (no reachable remote
/// worker, a remote failure, a frame that could not be rendered).
pub const EXIT_RENDER: i32 = 5;

/// A failure with the exit code it ends the process with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    /// One of the `EXIT_*` codes.
    pub code: i32,
    /// The sentence printed after `error: `. May span several lines.
    pub message: String,
}

impl CliError {
    /// A command-line mistake ([`EXIT_USAGE`]).
    pub fn usage(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_USAGE,
            message: message.into(),
        }
    }

    /// A design that cannot be used, or a refused result ([`EXIT_DESIGN`]).
    pub fn design(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_DESIGN,
            message: message.into(),
        }
    }

    /// A file that cannot be read or written ([`EXIT_IO`]).
    pub fn io(message: impl Into<String>) -> Self {
        Self {
            code: EXIT_IO,
            message: message.into(),
        }
    }
}

/// One file a command wants written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutFile {
    /// Where to write it.
    pub path: PathBuf,
    /// Its whole text.
    pub text: String,
}

/// The result of one command.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Outcome {
    /// Text for standard output.
    pub stdout: String,
    /// Text for standard error (progress notes, the error line).
    pub stderr: String,
    /// Files to write, in order. Written only when `exit` is not a failure of the design:
    /// a command that refuses its result returns none.
    pub files: Vec<OutFile>,
    /// The process exit code.
    pub exit: i32,
}

impl Outcome {
    /// A successful outcome carrying `stdout`.
    #[must_use]
    pub fn text(stdout: String) -> Self {
        Self {
            stdout,
            ..Self::default()
        }
    }

    /// An outcome that is only the error line for `error`.
    #[must_use]
    pub fn failure(error: &CliError) -> Self {
        Self {
            stderr: format!("error: {}\n", error.message),
            exit: error.code,
            ..Self::default()
        }
    }

    /// Adds a file to write.
    #[must_use]
    pub fn with_file(mut self, path: PathBuf, text: String) -> Self {
        self.files.push(OutFile { path, text });
        self
    }

    /// Adds a line to standard error.
    #[must_use]
    pub fn with_note(mut self, note: &str) -> Self {
        self.stderr.push_str(note);
        self.stderr.push('\n');
        self
    }

    /// Sets the exit code.
    #[must_use]
    pub const fn with_exit(mut self, exit: i32) -> Self {
        self.exit = exit;
        self
    }
}

/// What a command function returns: an outcome, or an error that becomes one.
pub type CommandResult = Result<Outcome, CliError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_carry_their_exit_codes() {
        assert_eq!(CliError::usage("x").code, EXIT_USAGE);
        assert_eq!(CliError::design("x").code, EXIT_DESIGN);
        assert_eq!(CliError::io("x").code, EXIT_IO);
    }

    #[test]
    fn the_exit_codes_are_distinct() {
        let codes = [
            EXIT_OK,
            EXIT_USAGE,
            EXIT_DESIGN,
            EXIT_IO,
            EXIT_PROBLEM,
            EXIT_RENDER,
        ];
        for (index, code) in codes.iter().enumerate() {
            assert_eq!(
                codes.iter().filter(|other| *other == code).count(),
                1,
                "{index}"
            );
        }
        assert_eq!(EXIT_RENDER, 5);
    }

    #[test]
    fn a_failure_is_one_error_line_and_its_code() {
        let outcome = Outcome::failure(&CliError::design("does not solve"));
        assert_eq!(outcome.stderr, "error: does not solve\n");
        assert_eq!(outcome.exit, EXIT_DESIGN);
        assert_eq!(outcome.stdout, "");
        assert_eq!(outcome.files.len(), 0);
    }

    #[test]
    fn builders_keep_order() {
        let outcome = Outcome::text("a\n".to_string())
            .with_file(PathBuf::from("one"), "1".to_string())
            .with_file(PathBuf::from("two"), "2".to_string())
            .with_note("first")
            .with_note("second")
            .with_exit(EXIT_PROBLEM);
        assert_eq!(outcome.files[0].path, PathBuf::from("one"));
        assert_eq!(outcome.files[1].path, PathBuf::from("two"));
        assert_eq!(outcome.stderr, "first\nsecond\n");
        assert_eq!(outcome.exit, EXIT_PROBLEM);
    }
}
