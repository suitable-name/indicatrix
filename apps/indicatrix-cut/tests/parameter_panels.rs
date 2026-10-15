//! Guards three promises of the Parameters dock that no single unit test can see, because they
//! are properties of the whole source tree:
//!
//! - nothing but the explicit "Preview as..." handler ever sets the viewport material to
//!   "not linked to the design" (a silent unlink would show a picture that is not the design);
//! - the Stone tab and the Preform tab edit one stone width (`EditorModel.girdle_diameter_mm`),
//!   and the old app-level `SettingsModel.stone_width_mm` is read-only for the dock;
//! - the old "Linked to design" toggle is gone.
//!
//! It only reads the sources as text, like `guide_highlights.rs` and `theme_tokens.rs`.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// Every file with the extension `extension` under `dir`, as (path relative to the crate, text).
fn collect(dir: &Path, extension: &str, found: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, extension, found);
        } else if path.extension().is_some_and(|ext| ext == extension) {
            let text = fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            found.push((relative(&path), text));
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn relative(path: &Path) -> String {
    path.strip_prefix(root())
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The text of every file with `extension` under `folder` of this crate.
fn sources(folder: &str, extension: &str) -> Vec<(String, String)> {
    let mut found = Vec::new();
    collect(&root().join(folder), extension, &mut found);
    assert!(!found.is_empty(), "no .{extension} files under {folder}/");
    found
}

fn read(path: &str) -> String {
    fs::read_to_string(root().join(path))
        .unwrap_or_else(|error| panic!("cannot read {path}: {error}"))
}

/// The lines of `source` that are code: not a `//` comment (nor a `///` / `//!` doc line).
fn code_lines(source: &str) -> impl Iterator<Item = &str> {
    source
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
}

#[test]
fn only_the_preview_handler_unlinks_the_viewport_material() {
    let mut setters = Vec::new();
    for (path, text) in sources("src", "rs") {
        // The statement may be wrapped over several lines (`false,\n)` included), so drop all
        // whitespace from the code and read the argument right after the call.
        let code: String = code_lines(&text)
            .flat_map(str::chars)
            .filter(|c| !c.is_whitespace())
            .collect();
        for (at, _) in code.match_indices("set_viewport_material_linked(") {
            let argument = &code[at + "set_viewport_material_linked(".len()..];
            if argument.starts_with("false)") || argument.starts_with("false,)") {
                setters.push(path.clone());
            }
        }
    }
    assert_eq!(
        setters,
        ["src/gui/render/material.rs"],
        "only `on_preview_material_chosen` may set the viewport material to not linked"
    );
}

#[test]
fn no_slint_file_unlinks_the_material_or_keeps_the_old_toggle() {
    for (path, text) in sources("ui", "slint") {
        for line in code_lines(&text) {
            // `viewport_material_linked =` is a write; the `=` of `changed viewport_material_linked =>`
            // is a read handler, so a needle directly followed by `>` does not count.
            let assigns = line.contains("viewport_material_linked <=>")
                || line.contains("<=> ViewportModel.viewport_material_linked")
                || line
                    .match_indices("viewport_material_linked =")
                    .any(|(at, needle)| !line[at + needle.len()..].starts_with('>'));
            assert!(!assigns, "{path} writes viewport_material_linked: {line}");
            assert!(
                !line.contains("LinkedToggle") && !line.contains("Linked to design"),
                "{path} still carries the Linked toggle: {line}"
            );
        }
    }
    // The render pipeline relies on this read handler; a later sweep must not drop it silently.
    let viewport = read("ui/models/viewport.slint");
    assert!(
        code_lines(&viewport).any(|line| line.contains("changed viewport_material_linked =>")),
        "ui/models/viewport.slint must keep `changed viewport_material_linked =>`"
    );
}

#[test]
fn the_stone_tab_and_the_preform_tab_share_one_stone_width() {
    for path in [
        "ui/components/parameters_panel/stone_tab.slint",
        "ui/components/editor_inspector/preform_tab.slint",
    ] {
        let text = read(path);
        assert!(
            code_lines(&text).any(|line| line.contains("<=> EditorModel.girdle_diameter_mm")),
            "{path} must edit EditorModel.girdle_diameter_mm"
        );
    }
}

#[test]
fn the_panels_last_tab_is_seeded_once_not_bound_live() {
    let text = read("ui/components/parameters_panel/parameters_panel.slint");
    assert!(
        code_lines(&text).any(|line| line.contains("init => { root.last_tab")),
        "parameters_panel.slint must seed `last_tab` in `init`"
    );
    for line in code_lines(&text) {
        assert!(
            !line.contains("last_tab: ParametersModel"),
            "`last_tab` must not be a live binding to the active tab: {line}"
        );
    }
}

#[test]
fn the_old_app_level_stone_width_is_read_only_for_the_dock() {
    let settings = read("ui/models/settings.slint");
    assert!(
        code_lines(&settings).any(|line| line.contains("in property <float> stone_width_mm")),
        "SettingsModel.stone_width_mm must stay an `in` property (a legacy value Rust feeds)"
    );
    for (path, text) in sources("ui/components/parameters_panel", "slint") {
        for line in code_lines(&text) {
            assert!(
                !line.contains("SettingsModel.stone_width_mm"),
                "{path} reads the legacy app-level width: {line}"
            );
        }
    }
}
