//! Exporting a stored plan to a file and importing a file as a new stored plan.
//!
//! The pickers are the app's shared off-thread ones (`gui::pickers`); reading and
//! writing the file and the database work run on a worker thread as well.

use super::{
    announce,
    dto::MAX_PAYLOAD_BYTES,
    format::{parse_and_validate_plan, payload_with_name},
    list::refresh_list,
    naming::{EXPORT_SUFFIX, clean_plan_name, current_unix_time, export_file_name},
    open::{OpenedPlan, PLAN_RUNNING_MESSAGE, begin_open, check_staleness, finish_open},
    show_error, spawn_task, store,
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

/// The file filter of the export and import dialogs.
///
/// The extension list holds `toml`, not `indicatrix-rough.toml`: the dialogs match on the
/// last dot-separated segment of the file name only. Exported files are still named
/// `<name>.indicatrix-rough.toml` (see [`export_file_name`]), and any `.toml` file can be
/// picked for an import; the loader decides whether it is a rough plan.
fn plan_filter() -> PickerFilter {
    PickerFilter {
        label: "Indicatrix rough plan".to_string(),
        extensions: vec!["toml".to_string()],
    }
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

/// `path`, with the export suffix appended unless it already ends in `.toml`.
fn with_toml_extension(path: PathBuf) -> PathBuf {
    let is_toml = path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("toml"));
    if is_toml {
        path
    } else {
        let mut name = path.into_os_string();
        name.push(EXPORT_SUFFIX);
        PathBuf::from(name)
    }
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

/// Writes stored plan `id` to `path` (worker thread). The stored name goes into the
/// file; only that line of the stored text changes, so what this build does not know in
/// it is exported as it is.
fn export_to_file(db: &Mutex<Database>, id: i64, path: PathBuf) -> Result<PathBuf, String> {
    let stored = store::load_saved(db, id)?;
    let text = payload_with_name(&stored.payload, &stored.name);
    let target = with_toml_extension(path);
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
        title: Some("Export rough plan".to_string()),
        filters: vec![plan_filter()],
        default_file_name: Some(default_name),
        starting_dir,
    };
    pick(&main, request, move |_main, chosen| {
        let Some(path) = chosen else {
            return;
        };
        let target = with_toml_extension(path.clone());
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
    let plan = parse_and_validate_plan(&text)
        .map_err(|e| format!("{file} is not a usable rough plan: {e}"))?;
    let name = match clean_plan_name(&plan.name).as_str() {
        "" => "Imported rough plan".to_string(),
        cleaned => cleaned.to_string(),
    };
    let plan_id = store::save_plan(db, &name, &text)?;
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
        title: Some("Import rough plan".to_string()),
        filters: vec![plan_filter()],
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
    fn a_toml_path_is_kept_and_anything_else_gets_the_export_suffix() {
        let kept = with_toml_extension(PathBuf::from("plans").join("a.indicatrix-rough.toml"));
        assert_eq!(kept, PathBuf::from("plans").join("a.indicatrix-rough.toml"));
        let upper = with_toml_extension(PathBuf::from("b.TOML"));
        assert_eq!(upper, PathBuf::from("b.TOML"));
        let bare = with_toml_extension(PathBuf::from("plans").join("c"));
        assert_eq!(bare, PathBuf::from("plans").join("c.indicatrix-rough.toml"));
        let other = with_toml_extension(PathBuf::from("d.txt"));
        assert_eq!(other, PathBuf::from("d.txt.indicatrix-rough.toml"));
    }

    #[test]
    fn an_export_replaces_the_old_file_whole_and_leaves_no_temporary_file() {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-rough-export-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("the test directory is created");
        let target = dir.join("plan.indicatrix-rough.toml");
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
        assert_eq!(
            leftovers,
            vec![std::ffi::OsString::from("plan.indicatrix-rough.toml")]
        );
        std::fs::remove_dir_all(&dir).expect("the test directory is removed");
    }

    #[test]
    fn an_export_into_a_missing_directory_fails_with_the_target_named() {
        let target = std::env::temp_dir()
            .join("indicatrix-rough-no-such-dir")
            .join("plan.indicatrix-rough.toml");
        let message = write_replacing(&target, "x").expect_err("the directory does not exist");
        assert!(message.contains("plan.indicatrix-rough.toml"), "{message}");
    }
}
