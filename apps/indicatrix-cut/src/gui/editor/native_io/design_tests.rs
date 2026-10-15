//! Tests of the `.isdd` design file's wiring in the editor: its file names (and the
//! legacy `.indicatrix` extension, read but never written), the autosave naming,
//! routing a file by content, the newer-version message and the atomic write.

use super::{
    atomic_write::write_file_atomically,
    autosave::{autosave_path, write_autosave},
    design_paths::{
        autosave_base_name, autosave_file_name, design_file_name_for, design_stem_of_path,
        ensure_design_extension, is_autosave_design_path, is_autosave_file_name,
        legacy_extension_autosave_file_name, path_belongs_to_schedule_name,
        schedule_name_for_design_path,
    },
    open_picker::design_open_error_message,
    save_helpers::design_file_text,
};
use indicatrix_cut_core::{
    ConstraintTier, Design, PreformSpec, ScheduleMeta,
    native::{DesignExtras, design_from_str},
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
    assert_eq!(design_file_name_for("round.asc"), "round.isdd");
    assert_eq!(
        design_file_name_for("edited_design.asc"),
        "edited_design.isdd"
    );
    assert_eq!(design_file_name_for("no_extension"), "no_extension.isdd");
    // A design opened from a legacy file is offered under the new extension.
    assert_eq!(design_file_name_for("round.indicatrix"), "round.isdd");

    let path = Path::new("designs/round.isdd");
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

/// A design opened from a legacy `.indicatrix` file names the same design, but a quick
/// Save never writes back into the old file: the Save dialog proposes `round.isdd`.
#[test]
fn a_legacy_file_names_its_design_but_is_never_quick_saved_over() {
    let legacy = Path::new("designs/round.indicatrix");
    assert_eq!(schedule_name_for_design_path(legacy), "round.asc");
    assert!(is_design_path(legacy), "a legacy file is still read");
    assert!(
        !path_belongs_to_schedule_name(legacy, "round.asc"),
        "the legacy file is left alone; Save asks for an .isdd name"
    );
}

#[test]
fn a_save_name_without_the_extension_gets_it() {
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round")),
        PathBuf::from("d/round.isdd")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.isdd")),
        PathBuf::from("d/round.isdd")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.ISDD")),
        PathBuf::from("d/round.ISDD")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.v2")),
        PathBuf::from("d/round.v2.isdd")
    );
}

/// Nothing writes the legacy extension any more: a typed `.indicatrix` name is saved as
/// `.isdd`.
#[test]
fn a_typed_legacy_name_is_saved_under_the_new_extension() {
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.indicatrix")),
        PathBuf::from("d/round.isdd")
    );
    assert_eq!(
        ensure_design_extension(PathBuf::from("d/round.INDICATRIX")),
        PathBuf::from("d/round.isdd")
    );
}

/// An autosave is named so the design-file rule accepts it, its name leads back to the
/// design's own, and a leftover `.autosave.indicatrix` of an earlier version is still
/// recognised.
#[test]
fn the_autosave_name_round_trips_and_older_names_are_still_found() {
    let name = autosave_file_name(Some("round.asc"));
    assert_eq!(name, "round.autosave.isdd");
    assert!(is_design_path(Path::new(&name)), "{name}");
    assert!(is_autosave_file_name(&name));
    assert!(is_autosave_design_path(Path::new(&name)));
    assert_eq!(autosave_base_name(Some("round.asc")), "round");
    assert_eq!(autosave_file_name(None), "untitled.autosave.isdd");

    // The name leads back to the design: recovering the snapshot offers `round.isdd`
    // and records the design as `round.asc`.
    let recovered = Path::new("settings").join(&name);
    assert_eq!(design_stem_of_path(&recovered).as_deref(), Some("round"));
    assert_eq!(schedule_name_for_design_path(&recovered), "round.asc");
    assert_eq!(autosave_file_name(Some("round.asc")), name);

    // An ordinary design file is not an autosave, and is not quick-saved over by one.
    assert!(!is_autosave_design_path(Path::new("d/round.isdd")));
    assert!(!path_belongs_to_schedule_name(&recovered, "round.asc"));

    // The earlier version's extension is recognised for recovery and for cleanup.
    let legacy = legacy_extension_autosave_file_name(Some("round.asc"));
    assert_eq!(legacy, "round.autosave.indicatrix");
    assert!(is_autosave_file_name(&legacy));
    assert!(is_autosave_design_path(Path::new(&legacy)));
    assert!(!is_autosave_file_name("round.isdd"));
    assert!(!is_autosave_file_name("round.indicatrix"));
    // The `.indicatrix.autosave.toml` naming is gone with the sidecar.
    assert!(!is_autosave_file_name("round.indicatrix.autosave.toml"));
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
/// and anything else (an older overlay sidecar included, which is no longer supported)
/// is not.
#[test]
fn a_file_is_routed_by_its_content_not_its_name() {
    let design = round_brilliant();
    let design_text =
        design_file_text(&design, None, &DesignExtras::default(), false).expect("text");
    assert_eq!(detect_kind(design_text.as_bytes()), FileKind::Design);
    assert_eq!(detect_kind(b"hello = 1\n"), FileKind::Unknown);
    assert_eq!(
        detect_kind(b"format_version = 1\nasc_filename = \"x.asc\"\n"),
        FileKind::Unknown,
        "an older overlay sidecar body is not a design file"
    );
}

/// A design file from a newer version is refused with a message that says so and names
/// both versions.
#[test]
fn a_newer_version_is_refused_with_a_clear_message() {
    let text = design_file_text(&round_brilliant(), None, &DesignExtras::default(), false)
        .expect("text")
        .replacen("version = 1", "version = 99", 1);
    let error = design_from_str(&text).expect_err("a newer version must not open");
    let message = design_open_error_message(Path::new("future.isdd"), &error);
    assert!(message.contains("future.isdd"), "{message}");
    assert!(message.contains("newer version"), "{message}");
    assert!(message.contains("99"), "{message}");
}

/// The write replaces the file, keeps the previous contents as `.bak`, and leaves no
/// staging file behind.
#[test]
fn the_design_file_write_replaces_atomically_and_keeps_one_backup() {
    let dir = temp_dir("atomic_write");
    let path = dir.join("round.isdd");

    write_file_atomically(&path, "first").expect("first write");
    assert!(
        !dir.join("round.isdd.bak").exists(),
        "a first save has nothing to back up"
    );
    write_file_atomically(&path, "second").expect("second write");

    assert_eq!(std::fs::read_to_string(&path).expect("read"), "second");
    assert_eq!(
        std::fs::read_to_string(dir.join("round.isdd.bak")).expect("read backup"),
        "first"
    );
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .expect("read dir")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["round.isdd", "round.isdd.bak"]);

    let _ = std::fs::remove_dir_all(&dir);
}
