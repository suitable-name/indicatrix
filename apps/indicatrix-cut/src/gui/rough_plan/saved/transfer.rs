//! Exporting a stored plan to a file and importing a file as a new stored plan.
//!
//! The pickers are the app's shared off-thread ones (`gui::pickers`); reading and
//! writing the file and the database work run on a worker thread as well.

use super::{
    announce,
    dto::MAX_PAYLOAD_BYTES,
    format::{parse_and_validate_plan, payload_with_name},
    list::refresh_list,
    naming::{
        EXPORT_EXTENSION, EXPORT_SUFFIX, IMPORT_TOML_EXTENSION, LEGACY_EXPORT_SUFFIX,
        clean_plan_name, current_unix_time, export_file_name,
    },
    open::{OpenedPlan, PLAN_RUNNING_MESSAGE, begin_open, check_staleness, finish_open},
    sections, show_error, spawn_task, store,
};
use crate::{
    RoughPlanModel,
    gui::{
        pickers::{PickerFilter, PickerKind, PickerRequest, pick},
        rough_plan::{
            host::{Host, on_host},
            run::guard_unsaved_results,
        },
    },
};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Model};
use std::{
    path::{Path, PathBuf},
    rc::Rc,
    sync::Mutex,
};
use tracing::warn;

/// The file filter of the export dialog: `.isrd` only.
fn export_filter() -> PickerFilter {
    PickerFilter {
        label: "Indicatrix rough plan (*.isrd)".to_string(),
        extensions: vec![EXPORT_EXTENSION.to_string()],
    }
}

/// The file filters of the import dialog: `.isrd` first, then `.toml`.
///
/// The `toml` entry covers the `<name>.indicatrix-rough.toml` files older versions wrote (the
/// dialogs match on the last dot-separated segment only) and any other `.toml` file; the
/// loader decides whether a file is a rough plan.
fn import_filters() -> Vec<PickerFilter> {
    vec![
        PickerFilter {
            label: "Indicatrix rough plan (*.isrd)".to_string(),
            extensions: vec![EXPORT_EXTENSION.to_string()],
        },
        PickerFilter {
            label: "Older rough plan (*.toml)".to_string(),
            extensions: vec![IMPORT_TOML_EXTENSION.to_string()],
        },
    ]
}

/// The name of list entry `id`, as the list shows it.
fn listed_name(host: &Host, id: i64) -> Option<String> {
    host.window
        .global::<RoughPlanModel>()
        .get_saved_plans()
        .iter()
        .find(|row| i64::from(row.id) == id)
        .map(|row| row.name.to_string())
}

/// `path` as an export target: a name ending in `.isrd` is kept, a trailing
/// `.indicatrix-rough.toml` is turned into `.isrd`, and anything else gets `.isrd` appended.
fn with_export_extension(path: PathBuf) -> PathBuf {
    let is_export = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case(EXPORT_EXTENSION));
    if is_export {
        return path;
    }
    let text = path.to_string_lossy().into_owned();
    let legacy = LEGACY_EXPORT_SUFFIX.len();
    let cut = text.len().saturating_sub(legacy);
    let stem = if text.len() > legacy
        && text.is_char_boundary(cut)
        && text[cut..].eq_ignore_ascii_case(LEGACY_EXPORT_SUFFIX)
    {
        Some(PathBuf::from(&text[..cut]))
    } else {
        None
    };
    let mut name = stem.unwrap_or(path).into_os_string();
    name.push(EXPORT_SUFFIX);
    PathBuf::from(name)
}

/// Writes `text` to a temporary file beside `target`, then renames it over `target`: a
/// write that fails or is interrupted never leaves half a plan where an earlier export
/// stood.
fn write_replacing(target: &Path, text: &str) -> Result<(), String> {
    let file_name = target.file_name().map_or_else(
        || "plan".to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let temp = target.with_file_name(format!(".{file_name}.tmp"));
    let failed = |e: std::io::Error| format!("Could not write {}: {e}", target.display());
    std::fs::write(&temp, text).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        failed(e)
    })?;
    std::fs::rename(&temp, target).map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        failed(e)
    })
}

/// The text an export writes for stored plan `id` (worker thread).
///
/// The stored name goes into the file. A plan that has a rough color or stone orientation
/// choices is read, given the two sections and written again by the TOML writer as a version 4
/// document; the other plans keep the exact text path: only the name line of the stored text
/// changes, so what this build does not know in it is exported as it is.
///
/// # Errors
///
/// Returns a message when the plan is gone, its color or choices cannot be read, or the
/// document would not open again.
pub(super) fn export_text(db: &Mutex<Database>, id: i64) -> Result<String, String> {
    let stored = store::load_saved(db, id)?;
    if let Some(found) = sections::stored_sections(db, id)? {
        return sections::payload_with_sections(&stored.payload, &stored.name, found);
    }
    Ok(payload_with_name(&stored.payload, &stored.name))
}

/// Writes stored plan `id` to `path` (worker thread); see [`export_text`] for what is written.
fn export_to_file(db: &Mutex<Database>, id: i64, path: PathBuf) -> Result<PathBuf, String> {
    let text = export_text(db, id)?;
    let target = with_export_extension(path);
    write_replacing(&target, &text)?;
    Ok(target)
}

/// `RoughPlanModel.export_saved`: asks for a file, then writes the plan to it.
pub(super) fn export_saved(host: &Rc<Host>, id: i64) {
    let name = listed_name(host, id).unwrap_or_default();
    ask_export_path(host, id, export_file_name(&name), None);
}

/// Asks where plan `id` goes, suggesting `default_name` in `starting_dir`.
///
/// The dialog confirms an overwrite of the file it was given; the export appends the
/// suffix to a name that has none, which can be a different file. When that file exists
/// the dialog opens again on it, so the question is asked about the file that would be
/// replaced.
fn ask_export_path(host: &Rc<Host>, id: i64, default_name: String, starting_dir: Option<PathBuf>) {
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let request = PickerRequest {
        kind: PickerKind::SaveFile,
        title: Some("Export Rough Plan".to_string()),
        filters: vec![export_filter()],
        default_file_name: Some(default_name),
        starting_dir,
    };
    pick(&main, request, move |_main, chosen| {
        let Some(path) = chosen else {
            return;
        };
        let target = with_export_extension(path.clone());
        if target != path && target.exists() {
            let name = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            let dir = target.parent().map(Path::to_path_buf);
            on_host(|host| ask_export_path(host, id, name, dir));
            return;
        }
        on_host(|host| {
            spawn_task(
                host,
                move |db| export_to_file(db, id, path),
                |host, result| match result {
                    Ok(target) => {
                        announce(host, &format!("Exported the plan to {}.", target.display()));
                    }
                    Err(message) => show_error(host, &message),
                },
            );
        });
    });
}

/// Reads, validates and stores the plan file at `path`, then checks its designs
/// (worker thread).
fn prepare_import(db: &Mutex<Database>, path: &Path) -> Result<OpenedPlan, String> {
    let file = path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    );
    let size = std::fs::metadata(path)
        .map_err(|e| format!("Could not read {file}: {e}"))?
        .len();
    if size > u64::try_from(MAX_PAYLOAD_BYTES).unwrap_or(u64::MAX) {
        return Err(format!("{file} is too large to be a rough plan."));
    }
    let text = std::fs::read_to_string(path).map_err(|e| format!("Could not read {file}: {e}"))?;
    import_text(db, &file, &text)
}

/// Deletes the plan an import stored when its color or stone orientations could not follow
/// it, so an import never leaves a plan without the color its file had. A delete that fails
/// is logged: the import fails either way.
fn forget_import(db: &Mutex<Database>, plan_id: i64) {
    if let Err(error) = store::delete_saved(db, plan_id) {
        warn!("Rough planner: plan #{plan_id} of a failed import was not removed: {error}");
    }
}

/// Validates the text of the plan file `file`, stores it as a new plan and checks its designs
/// (worker thread).
///
/// A color and stone orientations in the file go to the new plan's side tables and are not
/// part of the stored payload, which is the file's text itself unless it had them (then the
/// document without them, written again, with the version its other content needs).
///
/// # Errors
///
/// Returns a message naming `file` when it is not a usable rough plan, including a color that
/// cannot be used and an orientation that names no stone of the plan; nothing is stored then.
/// When the color or orientations cannot be stored, the plan is removed again.
pub(super) fn import_text(
    db: &Mutex<Database>,
    file: &str,
    text: &str,
) -> Result<OpenedPlan, String> {
    let plan = parse_and_validate_plan(text)
        .map_err(|e| format!("{file} is not a usable rough plan: {e}"))?;
    let name = match clean_plan_name(&plan.name).as_str() {
        "" => "Imported rough plan".to_string(),
        cleaned => cleaned.to_string(),
    };
    let payload = sections::stored_payload(text, &plan)
        .map_err(|e| format!("{file} could not be imported: {e}"))?;
    let plan_id = store::save_plan(db, &name, &payload)?;
    if let Err(message) = sections::restore_sections(db, plan_id, &plan) {
        forget_import(db, plan_id);
        return Err(format!("{file} could not be imported: {message}"));
    }
    let staleness = check_staleness(db, &plan)?;
    Ok(OpenedPlan {
        plan_id,
        name,
        created_at: current_unix_time(),
        plan,
        staleness,
    })
}

/// Stores the file as a new plan and opens it.
fn start_import(host: &Rc<Host>, path: PathBuf) {
    let Some(seq) = begin_open(host) else {
        return;
    };
    show_error(host, "");
    spawn_task(
        host,
        move |db| prepare_import(db, &path),
        move |host, result| {
            finish_open(host, seq, result);
            // The plan is in the table even if opening it failed afterwards.
            refresh_list(host);
        },
    );
}

/// Imports the file at `path`, after asking whether to replace results that were not saved.
fn import_from(host: &Rc<Host>, path: PathBuf) {
    guard_unsaved_results(host, move |host| start_import(host, path));
}

/// `RoughPlanModel.import_saved`: asks for a file, stores it as a new plan and opens it.
pub(super) fn import_saved(host: &Rc<Host>) {
    if host.window.global::<RoughPlanModel>().get_running() {
        show_error(host, PLAN_RUNNING_MESSAGE);
        return;
    }
    let Some(main) = host.main.upgrade() else {
        return;
    };
    let request = PickerRequest {
        kind: PickerKind::OpenFile,
        title: Some("Import Rough Plan".to_string()),
        filters: import_filters(),
        default_file_name: None,
        starting_dir: None,
    };
    pick(&main, request, |_main, chosen| {
        if let Some(path) = chosen {
            on_host(|host| import_from(host, path));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_isrd_path_is_kept_and_anything_else_gets_the_export_suffix() {
        let kept = with_export_extension(PathBuf::from("plans").join("a.isrd"));
        assert_eq!(kept, PathBuf::from("plans").join("a.isrd"));
        let upper = with_export_extension(PathBuf::from("b.ISRD"));
        assert_eq!(upper, PathBuf::from("b.ISRD"));
        let bare = with_export_extension(PathBuf::from("plans").join("c"));
        assert_eq!(bare, PathBuf::from("plans").join("c.isrd"));
        let other = with_export_extension(PathBuf::from("d.txt"));
        assert_eq!(other, PathBuf::from("d.txt.isrd"));
        let toml = with_export_extension(PathBuf::from("e.toml"));
        assert_eq!(toml, PathBuf::from("e.toml.isrd"));
    }

    #[test]
    fn a_typed_legacy_name_becomes_isrd() {
        let legacy = with_export_extension(PathBuf::from("plans").join("a.indicatrix-rough.toml"));
        assert_eq!(legacy, PathBuf::from("plans").join("a.isrd"));
        let upper = with_export_extension(PathBuf::from("B.Indicatrix-Rough.TOML"));
        assert_eq!(upper, PathBuf::from("B.isrd"));
        // Nothing precedes the suffix: it is kept as a stem, not cut to nothing.
        let only = with_export_extension(PathBuf::from(".indicatrix-rough.toml"));
        assert_eq!(only, PathBuf::from(".indicatrix-rough.toml.isrd"));
    }

    #[test]
    fn the_dialog_filters_offer_isrd_first_then_toml() {
        let export = export_filter();
        assert_eq!(export.label, "Indicatrix rough plan (*.isrd)");
        assert_eq!(export.extensions, vec!["isrd".to_string()]);
        let import = import_filters();
        assert_eq!(import.len(), 2);
        assert_eq!(import[0].extensions, vec!["isrd".to_string()]);
        assert_eq!(import[1].extensions, vec!["toml".to_string()]);
    }

    #[test]
    fn an_old_indicatrix_rough_toml_file_still_imports() {
        use crate::gui::rough_plan::saved::{
            fixtures::{plain_block, sample_layout, write},
            tests_db::library,
        };
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-rough-legacy-import-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("the test directory is created");
        let path = dir.join("old.indicatrix-rough.toml");
        std::fs::write(&path, write(&plain_block(), &[sample_layout()])).expect("written");
        let db = library();
        let opened = prepare_import(&db, &path).expect("an older plan file imports");
        assert!(opened.plan_id > 0);
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn an_export_replaces_the_old_file_whole_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-rough-export-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("the test directory is created");
        let target = dir.join("plan.isrd");
        write_replacing(&target, "first, and longer than the second").expect("first write");
        write_replacing(&target, "second").expect("second write");
        assert_eq!(
            std::fs::read_to_string(&target).expect("readable"),
            "second"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .expect("listable")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .collect();
        assert_eq!(leftovers, vec![std::ffi::OsString::from("plan.isrd")]);
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn an_export_into_a_missing_directory_fails_with_the_target_named() {
        let target = std::env::temp_dir()
            .join("indicatrix-rough-no-such-dir")
            .join("plan.isrd");
        let message = write_replacing(&target, "x").expect_err("the directory does not exist");
        assert!(message.contains("plan.isrd"), "{message}");
    }
}
