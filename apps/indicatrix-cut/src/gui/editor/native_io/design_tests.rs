//! Tests of the `.indicatrix` design file's wiring in the editor: its file names, the
//! autosave naming, routing a file by content, the newer-version message and the
//! atomic write.

use super::{
    atomic_write::write_file_atomically,
    autosave::{autosave_path, write_autosave},
    design_paths::{
        autosave_base_name, autosave_file_name, design_file_name_for, design_stem_of_path,
        ensure_design_extension, is_autosave_design_path, is_autosave_file_name,
        legacy_autosave_file_name, path_belongs_to_schedule_name, schedule_name_for_design_path,
    },
    open_picker::design_open_error_message,
    save_helpers::design_file_text,
};
use indicatrix_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta,
    native::{DesignExtras, design_from_str, save_paired},
};
use indicatrix_formats::native::design::{FileKind, detect_kind, is_design_path};
use std::path::{Path, PathBuf};

fn round_brilliant() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta::standard_round_brilliant(),
        ConstraintTier::standard_round_brilliant(),
    )
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "indicatrix_cut_design_tests_{label}_{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// The design's recorded name, the name offered for saving, the file recorded for a
/// design opened from a file and the quick-save check all agree on one stem.
#[test]
fn design_file_names_follow_the_designs_recorded_name() {
    assert_eq!(design_file_name_for("round.asc"), "round.indicatrix");
    assert_eq!(
        design_file_name_for("edited_design.asc"),
        "edited_design.indicatrix"
    );
    assert_eq!(
        design_file_name_for("no_extension"),
        "no_extension.indicatrix"
    );

    let path = Path::new("designs/round.indicatrix");
    assert_eq!(schedule_name_for_design_path(path), "round.asc");
    assert!(path_belongs_to_schedule_name(path, "round.asc"));
    assert!(path_belongs_to_schedule_name(path, "ROUND.asc"));
    assert!(
        !path_belongs_to_schedule_name(path, "other.asc"),
        "a design replaced by another one must not overwrite the first one's file"
    );
    assert!(!path_belongs_to_schedule_name(
        Path::new("designs/round.indicatrix.toml"),
        "round.asc"
    ));
}

#[test]
fn a_save_name_without_the_extension_gets_it() {
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round")),
        PathBuf::from("d/round.indicatrix")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.indicatrix")),
        PathBuf::from("d/round.indicatrix")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.INDICATRIX")),
        PathBuf::from("d/round.INDICATRIX")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.v2")),
        PathBuf::from("d/round.v2.indicatrix")
    );
}

/// An autosave is named so the design-file rule accepts it, its name leads back to the
/// design's own, and the older recovery names are still recognised.
#[test]
fn the_autosave_name_round_trips_and_older_names_are_still_found() {
    let name = autosave_file_name(Some("round.asc"));
    assert_eq!(name, "round.autosave.indicatrix");
    assert!(is_design_path(Path::new(&name)), "{name}");
    assert!(is_autosave_file_name(&name));
    assert!(is_autosave_design_path(Path::new(&name)));
    assert_eq!(autosave_base_name(Some("round.asc")), "round");
    assert_eq!(autosave_file_name(None), "untitled.autosave.indicatrix");

    // The name leads back to the design: recovering the snapshot offers `round.indicatrix`
    // and records the design as `round.asc`.
    let recovered = Path::new("settings").join(&name);
    assert_eq!(design_stem_of_path(&recovered).as_deref(), Some("round"));
    assert_eq!(schedule_name_for_design_path(&recovered), "round.asc");
    assert_eq!(autosave_file_name(Some("round.asc")), name);

    // An ordinary design file is not an autosave, and is not quick-saved over by one.
    assert!(!is_autosave_design_path(Path::new("d/round.indicatrix")));
    assert!(!path_belongs_to_schedule_name(&recovered, "round.asc"));

    // The older naming is recognised for recovery and for cleanup.
    let legacy = legacy_autosave_file_name(Some("round.asc"));
    assert_eq!(legacy, "round.indicatrix.autosave.toml");
    assert!(is_autosave_file_name(&legacy));
    assert!(!is_autosave_file_name("round.indicatrix"));
    assert!(!is_autosave_file_name("round.indicatrix.toml"));
    assert!(autosave_path(Some("round.asc")).ends_with(&name));
}

/// An autosave written the way a tick writes it opens back as the same design through
/// the design-file reader, with no `.asc` anywhere.
#[test]
fn an_autosave_written_and_read_back_is_the_same_design() {
    let dir = temp_dir("autosave_round_trip");
    let design = round_brilliant();
    let text = design_file_text(&design, None, &DesignExtras::default(), false).expect("text");
    let path = dir.join(autosave_file_name(Some("round.asc")));

    write_autosave(&path, &text).expect("write");
    assert!(is_design_path(&path));
    let read = std::fs::read_to_string(&path).expect("read back");
    let loaded = design_from_str(&read).expect("opens as a design file");
    assert_eq!(loaded.design.tiers.len(), design.tiers.len());
    assert!(!loaded.draft);

    let _ = std::fs::remove_dir_all(&dir);
}

/// A save marks a design that did not solve as a draft, and nothing else.
#[test]
fn the_draft_flag_follows_the_save() {
    let design = round_brilliant();
    let draft = design_file_text(&design, None, &DesignExtras::default(), true).expect("text");
    assert!(design_from_str(&draft).expect("opens").draft);
    let ordinary = design_file_text(&design, None, &DesignExtras::default(), false).expect("text");
    assert!(!design_from_str(&ordinary).expect("opens").draft);
}

/// A file is routed by what it holds: a design file under any name is a design file,
/// and an older overlay sidecar stays one even when it carries the new extension.
#[test]
fn a_file_is_routed_by_its_content_not_its_name() {
    let design = round_brilliant();
    let design_text =
        design_file_text(&design, None, &DesignExtras::default(), false).expect("text");
    assert_eq!(detect_kind(design_text.as_bytes()), FileKind::Design);

    let paired = save_paired(&design, "round.asc", None, None, None).expect("older pair saves");
    assert_eq!(
        detect_kind(paired.native_toml.as_bytes()),
        FileKind::OverlaySidecar,
        "an older sidecar, whatever it is named, takes the paired path"
    );
    assert_eq!(detect_kind(b"hello = 1\n"), FileKind::Unknown);
}

/// A design file from a newer version is refused with a message that says so and names
/// both versions.
#[test]
fn a_newer_version_is_refused_with_a_clear_message() {
    let text = design_file_text(&round_brilliant(), None, &DesignExtras::default(), false)
        .expect("text")
        .replacen("version = 1", "version = 99", 1);
    let error = design_from_str(&text).expect_err("a newer version must not open");
    let message = design_open_error_message(Path::new("future.indicatrix"), &error);
    assert!(message.contains("future.indicatrix"), "{message}");
    assert!(message.contains("newer version"), "{message}");
    assert!(message.contains("99"), "{message}");
}

/// The write replaces the file, keeps the previous contents as `.bak`, and leaves no
/// staging file behind.
#[test]
fn the_design_file_write_replaces_atomically_and_keeps_one_backup() {
    let dir = temp_dir("atomic_write");
    let path = dir.join("round.indicatrix");

    write_file_atomically(&path, "first").expect("first write");
    assert!(
        !dir.join("round.indicatrix.bak").exists(),
        "a first save has nothing to back up"
    );
    write_file_atomically(&path, "second").expect("second write");

    assert_eq!(std::fs::read_to_string(&path).expect("read"), "second");
    assert_eq!(
        std::fs::read_to_string(dir.join("round.indicatrix.bak")).expect("read backup"),
        "first"
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["round.indicatrix", "round.indicatrix.bak"]);

    let _ = std::fs::remove_dir_all(&dir);
}
