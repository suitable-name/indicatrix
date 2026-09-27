//! Opening the bundled user manual and revealing the Edit tab's last-saved folder,
//! both through the platform's own file/URL opener.

use crate::{EditorModel, MainWindow, gui::show_toast};
use slint::ComponentHandle;

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

/// Help menu: opens the bundled `docs/manual/README.md` via the same per-platform
/// `xdg-open`/`cmd /C start`/`open` dispatch
/// `library::diagram_list::setup_diagram_selection_and_export_callbacks`'s
/// `on_open_diagram_url` uses for URLs (all three also open a plain file path). The
/// path itself is resolved at runtime by [`locate_user_manual`] (see its doc comment
/// for the candidate search order). If none of those candidates exist on disk, this
/// shows an error toast instead of silently doing nothing.
pub(super) fn setup_user_manual_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.on_open_user_manual(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Some(manual_path) = locate_user_manual() else {
            show_toast(&ui, "User manual not found", "error");
            return;
        };
        open_with_os_handler(&manual_path.to_string_lossy());
    });
}

/// Hands `path` (a file, a folder, or a URL -- all three work on every branch) to
/// the platform's own opener. Failures are deliberately ignored: there is no
/// portable way to tell "no handler registered" from "the handler launched and
/// exited", and every caller here has already put the path on screen, so the
/// cutter is never left with nothing.
fn open_with_os_handler(path: &str) {
    #[cfg(target_os = "linux")]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
    #[cfg(target_os = "windows")]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", path])
        .spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
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
            open_with_os_handler(&target.to_string_lossy());
        });
}

#[cfg(test)]
mod tests {
    use super::user_manual_candidates;

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
}
