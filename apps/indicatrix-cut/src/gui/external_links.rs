//! Opening the user manual (in the in-app help window), the manual's folder, catalogue
//! links and the Edit tab's last-saved folder through the platform's own file/URL opener.
//!
//! Every launch goes through [`open_external_url`] or [`open_local_path`], which hand
//! the target to the opener as ONE argument of a directly spawned program -- never
//! through `cmd.exe` or any other shell, where `&`, `|`, `^` and `%` in a link would be
//! read as shell syntax.

use crate::{EditorModel, MainWindow, gui::show_toast};
use slint::ComponentHandle;
use std::{
    ffi::OsStr,
    io,
    path::{Path, PathBuf},
    process::Command,
};
use tracing::{info, warn};

/// Candidate `docs/manual/README.md` locations for [`locate_user_manual`], relative to
/// a base directory (the running executable's own directory in production; see that
/// function for how each candidate is built from it).
///
/// Order matters: earlier candidates are preferred, and the first one that exists on
/// disk wins. The first three match how the manual is actually laid out relative to
/// the installed/built binary (bundled alongside it, one level up, or in the `cargo
/// build` target-directory layout); the last is a dev-only fallback for running
/// straight out of a `cargo run` checkout, where none of those relative layouts apply.
pub(super) fn user_manual_candidates(exe_dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    vec![
        exe_dir.join("docs/manual/README.md"),
        exe_dir.join("../docs/manual/README.md"),
        // `cargo build` layout: `<target>/<profile>/<exe>` -> the crate root is two
        // directories up from the executable's directory.
        exe_dir.join("../../apps/indicatrix-cut/docs/manual/README.md"),
        // Dev fallback: running via `cargo run` from within this crate's own checkout,
        // where the exe-relative candidates above don't line up with the source tree.
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual/README.md"),
    ]
}

/// Resolves the bundled user manual's path at runtime: tries each of
/// [`user_manual_candidates`], relative to the running executable's own directory, in
/// order, and returns the first one that exists on disk. `None` if none of them do
/// (e.g. a build that never bundled the manual at all).
///
/// Runtime-resolved rather than the `CARGO_MANIFEST_DIR` compile-time path this used
/// to be: that path is baked in at compile time and only ever resolves correctly for a
/// build run from within this crate's own checkout, never for a distributed binary.
fn locate_user_manual() -> Option<std::path::PathBuf> {
    let exe_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf))?;
    user_manual_candidates(&exe_dir)
        .into_iter()
        .find(|candidate| candidate.is_file())
}

/// The folder with the manual's plain `.md` files, when the program is installed next to
/// them (see [`locate_user_manual`] for the search order). The manual itself is built into
/// the program (`gui::help`), so a missing folder costs nothing but the help window's
/// "Open manual folder" button, which is for reading the files in an editor.
pub(super) fn locate_manual_folder() -> Option<PathBuf> {
    locate_user_manual().and_then(|readme| readme.parent().map(Path::to_path_buf))
}

/// Help menu: opens the in-app help window at the manual's contents (`gui::help`). The
/// manual is compiled into the program, so this works in an installed build whether or
/// not the plain files are there; if the window cannot be created, an error toast says so.
pub(super) fn setup_user_manual_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.on_open_user_manual(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        if let Err(message) = super::help::open_topic(&ui, super::help::topics::CONTENTS) {
            warn!("{message}");
            show_toast(&ui, &message, "error");
        }
    });
}

/// Opens `url` in the default browser.
///
/// Only `http://` and `https://` links are accepted (see [`validated_url`]): a
/// catalogue link is data that can come from a remote library, so it must never reach
/// the opener as a local path, a `file:` URL or anything else the shell would execute.
///
/// # Errors
///
/// A ready-to-toast message when the link is refused or the opener cannot be started.
pub(super) fn open_external_url(url: &str) -> Result<(), String> {
    let launch_url = validated_url(url).inspect_err(|_| {
        warn!("Refused to open the link {url:?}");
    })?;
    info!("Opening URL: {launch_url}");
    launch(OsStr::new(&launch_url)).map_err(|e| format!("Could not open {launch_url}: {e}"))
}

/// Opens an existing file or folder with the platform's default handler. Only for
/// paths this application itself produced (the bundled manual, the folder a design was
/// saved into), never for text that came from a catalogue or a remote worker -- those
/// go through [`open_external_url`].
///
/// # Errors
///
/// A ready-to-toast message when `path` does not exist or the opener cannot be
/// started.
pub(super) fn open_local_path(path: &Path) -> Result<(), String> {
    let path = validated_local_path(path)?;
    launch(path.as_os_str()).map_err(|e| format!("Could not open {}: {e}", path.display()))
}

/// Accepts `url` only if it is an `http://` or `https://` address with something after
/// the scheme and no control characters, and returns the text to hand to the opener:
/// the same URL with spaces and double quotes percent-encoded, so the single argument
/// never contains a character the opener's own command-line parsing could treat as
/// quoting. A `&` (or any other URL character) is legal and stays untouched, because
/// the URL is never parsed by a shell.
///
/// # Errors
///
/// A ready-to-toast message naming why the link was refused.
fn validated_url(url: &str) -> Result<String, String> {
    let url = url.trim();
    if url.chars().any(char::is_control) {
        return Err("Refused to open a link that contains control characters.".to_owned());
    }
    let is_web_link = url.split_once("://").is_some_and(|(scheme, rest)| {
        (scheme.eq_ignore_ascii_case("http") || scheme.eq_ignore_ascii_case("https"))
            && !rest.is_empty()
    });
    if !is_web_link {
        return Err("Refused to open a link that is not an http or https address.".to_owned());
    }
    Ok(url.replace(' ', "%20").replace('"', "%22"))
}

/// Accepts `path` only if it exists, and returns it made absolute so a relative name
/// can never be read as an option by the opener.
///
/// # Errors
///
/// A ready-to-toast message when `path` does not exist.
fn validated_local_path(path: &Path) -> Result<PathBuf, String> {
    if !path.exists() {
        return Err(format!("{} does not exist.", path.display()));
    }
    Ok(std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf()))
}

/// The opener command for `target`, which is passed as a single argument:
/// `rundll32 url.dll,FileProtocolHandler` on Windows (the shell's own "open" verb, with
/// no `cmd.exe` in between), `open` on macOS and `xdg-open` elsewhere.
fn opener_command(target: &OsStr) -> Command {
    #[cfg(target_os = "windows")]
    {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler").arg(target);
        command
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = Command::new("open");
        command.arg(target);
        command
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let mut command = Command::new("xdg-open");
        command.arg(target);
        command
    }
}

/// Starts the opener for `target` and returns without waiting for it to finish.
///
/// Only a failure to start the opener is reported: there is no portable way to tell
/// "no handler registered" from "the handler launched and exited", and every caller has
/// already put the target on screen, so the cutter is never left with nothing.
fn launch(target: &OsStr) -> io::Result<()> {
    let mut child = opener_command(target).spawn()?;
    // Reaps the opener when it exits, so it never lingers as a zombie process on Unix.
    std::thread::spawn(move || {
        let _ = child.wait();
    });
    Ok(())
}

/// Reveals whatever the Edit tab last saved or exported, by
/// opening its containing FOLDER (not the file -- opening a `.asc` would launch
/// whatever text editor is registered for it, which is not what "where did it
/// go?" is asking). The path itself is pushed by
/// `gui::editor::native_io::record_last_saved_path`; this is a no-op until the
/// first save of the session, which is also when the strip segment that invokes
/// it first appears.
pub(super) fn setup_reveal_last_saved_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_reveal_last_saved_path(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let saved = ui.global::<EditorModel>().get_last_saved_path();
            if saved.is_empty() {
                return;
            }
            let path = std::path::PathBuf::from(saved.as_str());
            let target = path.parent().unwrap_or(&path);
            if let Err(message) = open_local_path(target) {
                warn!("{message}");
                show_toast(&ui, &message, "error");
            }
        });
}

#[cfg(test)]
mod tests {
    use super::{
        locate_manual_folder, opener_command, user_manual_candidates, validated_local_path,
        validated_url,
    };
    use std::{ffi::OsStr, path::Path};

    /// From a checkout the manual's folder is always found (the last candidate is the
    /// source tree), and it is the folder that holds the contents page.
    #[test]
    fn the_manual_folder_is_found_in_a_checkout() {
        let folder = locate_manual_folder().expect("the checkout's manual folder");
        assert!(folder.join("README.md").is_file());
    }

    /// The candidate list is built entirely from the given base directory (plus one
    /// fixed dev-only `CARGO_MANIFEST_DIR` fallback) -- no hidden dependence on the
    /// current working directory or environment. Also pins down the order (exe dir,
    /// one level up, two levels up under the `cargo build` layout, then the dev
    /// fallback) since [`locate_user_manual`](super::locate_user_manual) relies on
    /// earlier candidates being preferred.
    #[test]
    fn user_manual_candidates_are_relative_to_exe_dir_in_order() {
        let exe_dir = std::path::Path::new("/opt/indicatrix-cut/bin");
        let candidates = user_manual_candidates(exe_dir);

        assert_eq!(
            candidates[0],
            std::path::Path::new("/opt/indicatrix-cut/bin/docs/manual/README.md")
        );
        assert_eq!(
            candidates[1],
            std::path::Path::new("/opt/indicatrix-cut/bin/../docs/manual/README.md")
        );
        assert_eq!(
            candidates[2],
            std::path::Path::new(
                "/opt/indicatrix-cut/bin/../../apps/indicatrix-cut/docs/manual/README.md"
            )
        );
        // Last candidate is the fixed dev-only fallback, independent of `exe_dir`.
        assert_eq!(
            candidates[3],
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/manual/README.md")
        );
        assert_eq!(candidates.len(), 4);
    }

    /// An `&` is ordinary URL syntax (query strings are full of them) and must reach
    /// the opener untouched -- the fix is never handing the URL to a shell, not
    /// rejecting or escaping the character.
    #[test]
    fn an_http_url_with_shell_metacharacters_is_accepted_verbatim() {
        for url in [
            "http://example.com/?a=1&calc",
            "https://example.com/path?x=1&y=2|3^4%5",
            "https://example.com/?a&echo%20pwned>%TEMP%\\x",
        ] {
            assert_eq!(validated_url(url).as_deref(), Ok(url));
        }
    }

    #[test]
    fn the_scheme_check_ignores_case_and_surrounding_whitespace() {
        assert_eq!(
            validated_url("  HTTPS://Example.com/a \n").as_deref(),
            Ok("HTTPS://Example.com/a")
        );
    }

    /// Spaces and double quotes inside a link are percent-encoded so the single
    /// argument holds no character an opener's own command-line parsing could read as
    /// quoting.
    #[test]
    fn spaces_and_quotes_are_percent_encoded() {
        assert_eq!(
            validated_url("http://example.com/a b\"c").as_deref(),
            Ok("http://example.com/a%20b%22c")
        );
    }

    #[test]
    fn anything_but_an_http_or_https_url_is_refused() {
        for refused in [
            "",
            "   ",
            "http://",
            "https://",
            "example.com",
            "ftp://example.com/file",
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "calc.exe",
            "C:\\Windows\\System32\\calc.exe",
            "\\\\server\\share\\tool.exe",
            "/usr/bin/xterm",
            "ms-settings:network",
        ] {
            assert!(
                validated_url(refused).is_err(),
                "{refused:?} must be refused"
            );
        }
    }

    #[test]
    fn a_url_with_control_characters_is_refused() {
        for refused in [
            "http://example.com/\nnext",
            "http://example.com/\r\ncalc",
            "http://example.com/\0",
            "http://example.com/\tx",
            "http://example.com/\u{1b}[0m",
        ] {
            assert!(
                validated_url(refused).is_err(),
                "{refused:?} must be refused"
            );
        }
    }

    /// The injection this module exists to prevent: the validated URL, `&` and all,
    /// travels as the final single argument of a directly spawned opener, and no shell
    /// is involved.
    #[test]
    fn the_opener_receives_the_url_as_one_argument_and_no_shell_is_involved() {
        let url = validated_url("http://x/?a=1&calc|echo pwned>%TEMP%\\x").unwrap();
        let command = opener_command(OsStr::new(&url));

        let program = Path::new(command.get_program())
            .file_stem()
            .map(|stem| stem.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        assert!(
            !matches!(
                program.as_str(),
                "cmd" | "sh" | "bash" | "zsh" | "powershell" | "pwsh"
            ),
            "the opener must not be a shell: {program}"
        );
        let args: Vec<&OsStr> = command.get_args().collect();
        assert_eq!(args.last().copied(), Some(OsStr::new(&url)));
        assert_eq!(
            args.iter()
                .filter(|arg| arg.to_string_lossy().contains('&'))
                .count(),
            1,
            "the URL must not be split across arguments"
        );
    }

    #[test]
    fn a_local_path_must_exist() {
        let here = Path::new(env!("CARGO_MANIFEST_DIR"));
        let accepted = validated_local_path(here).unwrap();
        assert!(accepted.is_absolute());
        assert!(validated_local_path(&here.join("no-such-file-for-the-opener")).is_err());
    }
}
