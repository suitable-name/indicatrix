//! Tests of the import pipeline: the batch loop and its per-file panic guard, sidecar
//! pairing, the recursive folder scan and the remembered save folder. The topic-specific
//! groups live in the submodules.

mod collisions;
mod foreign_formats;
mod measure;
mod old_layout;
mod perf;

use super::{
    folder_memory::last_save_folder,
    pipeline::{ImportOutcome, catch_file_panic, import_path},
    scan::{MAX_RECURSE_DEPTH, collect_design_files_recursive, find_native_sidecar},
};
use crate::{
    gui::library::local::helpers::test_support::{
        VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
    },
    settings::{SettingsFile, SettingsPersister},
};
use indicatrix_vault::{db::sqlite::Database, model::filter::RangeFilter};
use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
};

/// The exact mechanism [`import_path`] leans on to keep one bad file from taking
/// the whole worker thread down -- proven directly, independent
/// of `indicatrix`'s actual geometry code (which this module doesn't own and can't
/// force to panic on demand for a test).
#[test]
fn catch_file_panic_converts_a_panic_into_an_error_message_instead_of_unwinding() {
    // The panic is deliberate and caught -- suppress the default hook's stderr
    // noise for it so this doesn't look like an unhandled test failure in the log.
    let prev_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let result = catch_file_panic(std::panic::AssertUnwindSafe(|| -> i32 {
        panic!("deliberate test panic");
    }));
    std::panic::set_hook(prev_hook);
    assert_eq!(result, Err("deliberate test panic".to_string()));
}

#[test]
fn catch_file_panic_returns_ok_when_the_closure_does_not_panic() {
    assert_eq!(
        catch_file_panic(std::panic::AssertUnwindSafe(|| 42)),
        Ok(42)
    );
}

/// A batch with one good file and one file that fails to parse must still import
/// the good one, record the bad one in `failed` rather than abort, and report
/// progress for both -- not silently stop partway, which a whole-loop `db.lock()`
/// plus an uncaught panic anywhere downstream would risk turning into a wedged
/// `is_busy`.
#[test]
fn import_path_continues_after_one_file_fails_to_parse() {
    let dir = temp_dir_for_test("parse_fail");
    std::fs::write(dir.join("good.asc"), VALID_ASC).expect("write good.asc");
    std::fs::write(dir.join("bad.asc"), "not an asc file").expect("write bad.asc");

    let db_path = temp_db_path_for_test("parse_fail");
    let db = open_temp_db(&db_path);

    let mut progress_calls = 0usize;
    let outcome = import_path(&db, &dir, false, |_done, _total| {
        progress_calls += 1;
    });
    let summary = outcome.summary;

    assert!(summary.contains("Imported 1"), "summary was: {summary}");
    assert!(summary.contains("1 skipped"), "summary was: {summary}");
    assert!(summary.contains("bad.asc"), "summary was: {summary}");
    assert_eq!(
        outcome.imported_ids.len(),
        1,
        "exactly the one successfully-imported file's id must be reported"
    );
    assert_eq!(
        progress_calls, 2,
        "on_progress must still fire once per candidate file, good AND bad"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// The `import_path`-level recursion contract: with `recurse: false`, a file that
/// only exists one level down is invisible; with `recurse: true`, the exact same
/// folder yields it too.
#[test]
fn import_path_recurses_into_subfolders_only_when_requested() {
    let dir = temp_dir_for_test("recurse");
    std::fs::write(dir.join("top.asc"), VALID_ASC).expect("write top.asc");
    let sub = dir.join("sub");
    std::fs::create_dir_all(&sub).expect("create subfolder");
    std::fs::write(sub.join("nested.asc"), VALID_ASC).expect("write nested.asc");

    let flat_db_path = temp_db_path_for_test("recurse_flat");
    let flat_db = open_temp_db(&flat_db_path);
    let flat_summary = import_path(&flat_db, &dir, false, |_, _| {}).summary;
    assert!(
        flat_summary.contains("Imported 1"),
        "non-recursive import must see only top.asc: {flat_summary}"
    );

    let deep_db_path = temp_db_path_for_test("recurse_deep");
    let deep_db = open_temp_db(&deep_db_path);
    let deep_summary = import_path(&deep_db, &dir, true, |_, _| {}).summary;
    assert!(
        deep_summary.contains("Imported 2"),
        "recursive import must see both top.asc and sub/nested.asc: {deep_summary}"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&flat_db_path);
    let _ = std::fs::remove_file(&deep_db_path);
}

/// A `.asc` saved by Save Native has a `<stem>.indicatrix.toml`
/// sidecar sitting right beside it on disk. Importing that pair must attach the
/// sidecar as a second file on the saved row, not just the bare `.asc`, so
/// `gui::editor::loading::design_from_full_record`'s existing `load_paired`
/// preference actually has a sidecar to find on Load Selected.
#[test]
fn import_path_attaches_a_sibling_native_sidecar_found_beside_the_asc() {
    let dir = temp_dir_for_test("sidecar");
    std::fs::write(dir.join("paired.asc"), VALID_ASC).expect("write paired.asc");
    std::fs::write(dir.join("paired.indicatrix.toml"), "format_version = 1\n")
        .expect("write paired.indicatrix.toml");
    // A lone `.asc` with no sidecar must still import with just the one attachment.
    std::fs::write(dir.join("lonely.asc"), VALID_ASC).expect("write lonely.asc");

    let db_path = temp_db_path_for_test("sidecar");
    let db = open_temp_db(&db_path);
    let outcome = import_path(&db, &dir, false, |_, _| {});
    assert!(
        outcome.summary.contains("Imported 2"),
        "summary was: {}",
        outcome.summary
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut paired_files = None;
    let mut lonely_files = None;
    for id in &outcome.imported_ids {
        let full = conn
            .get_diagram_full(*id)
            .expect("query must succeed")
            .expect("row must exist");
        if full.url.contains("paired.asc") {
            paired_files = Some(full.attached_files.len());
        } else if full.url.contains("lonely.asc") {
            lonely_files = Some(full.attached_files.len());
        }
    }
    assert_eq!(
        paired_files,
        Some(2),
        "the .asc plus its native sidecar must both be attached"
    );
    assert_eq!(
        lonely_files,
        Some(1),
        "a .asc with no sidecar on disk must attach only itself"
    );
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// The legacy `.gemcut.toml` suffix must be found too, when there is no current
/// `.indicatrix.toml` sidecar beside the `.asc`.
#[test]
fn find_native_sidecar_falls_back_to_the_legacy_suffix() {
    let dir = temp_dir_for_test("sidecar_legacy");
    std::fs::create_dir_all(&dir).expect("create dir");
    let asc_path = dir.join("legacy.asc");
    std::fs::write(&asc_path, VALID_ASC).expect("write legacy.asc");
    std::fs::write(dir.join("legacy.gemcut.toml"), "format_version = 1\n")
        .expect("write legacy.gemcut.toml");

    let sidecar = find_native_sidecar(&asc_path).expect("legacy sidecar must be found");
    assert_eq!(sidecar.0, "legacy.gemcut.toml");

    let _ = std::fs::remove_dir_all(&dir);
}

/// The depth backstop in [`collect_design_files_recursive`], exercised directly with
/// a small `max_depth` rather than building 32 real nested folders to hit
/// [`MAX_RECURSE_DEPTH`].
#[test]
fn collect_design_files_recursive_stops_at_max_depth() {
    let root = temp_dir_for_test("depth");
    let level1 = root.join("level1");
    let level2 = level1.join("level2");
    std::fs::create_dir_all(&level2).expect("create nested folders");
    std::fs::write(level1.join("shallow.asc"), VALID_ASC).expect("write shallow.asc");
    std::fs::write(level2.join("deep.asc"), VALID_ASC).expect("write deep.asc");

    let mut visited = HashSet::new();
    let mut out = Vec::new();
    // Entering at depth 1 with max_depth 1: `level1` itself is walked (its own
    // depth is within budget), but `level2` one level further down is not.
    collect_design_files_recursive(&level1, true, 1, 1, &mut visited, &mut out);

    assert_eq!(
        out.len(),
        1,
        "only the depth-1 file should be found, not the depth-2 one: {out:?}"
    );
    assert!(out[0].ends_with("shallow.asc"));

    let _ = std::fs::remove_dir_all(&root);
}

/// The symlink-loop backstop in [`collect_design_files_recursive`]: a directory
/// symlink pointing back to an ancestor must not be followed forever, and must
/// not make `real.asc` (reachable both directly and through the loop) show up
/// more than once. Best-effort -- creating a directory symlink/junction on
/// Windows normally needs Developer Mode or an elevated process, so this skips
/// itself (rather than failing) wherever that isn't available, same tolerance
/// this crate's own real-catalogue perf probe gives a missing prerequisite.
#[test]
fn collect_design_files_recursive_guards_against_a_symlink_loop() {
    let root = temp_dir_for_test("symlink_loop");
    std::fs::write(root.join("real.asc"), VALID_ASC).expect("write real.asc");
    let link_path = root.join("loop_back");

    #[cfg(windows)]
    let symlink_result = std::os::windows::fs::symlink_dir(&root, &link_path);
    #[cfg(not(windows))]
    let symlink_result = std::os::unix::fs::symlink(&root, &link_path);

    if let Err(e) = symlink_result {
        eprintln!(
            "skipping collect_design_files_recursive_guards_against_a_symlink_loop: \
                 could not create a directory symlink on this machine ({e}) -- \
                 likely needs Developer Mode or an elevated process on Windows"
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let mut visited = HashSet::new();
    let mut out = Vec::new();
    collect_design_files_recursive(&root, true, 0, MAX_RECURSE_DEPTH, &mut visited, &mut out);

    assert_eq!(
        out.len(),
        1,
        "the symlink loop must not cause real.asc to be found more than once: {out:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// --- last_save_folder ---

fn temp_settings_store(label: &str) -> Arc<SettingsPersister> {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!(
        "indicatrix_cut_import_test_settings_{label}_{n}_{}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp settings dir");
    Arc::new(SettingsPersister::spawn(
        dir.join("settings.toml"),
        SettingsFile::default(),
    ))
}

#[test]
fn last_save_folder_returns_none_when_nothing_recorded() {
    let store = temp_settings_store("none");
    assert!(last_save_folder(&store).is_none());
}

#[test]
fn last_save_folder_returns_the_most_recent_entrys_parent_directory() {
    let store = temp_settings_store("some");
    let saved_dir = temp_dir_for_test("last_save_folder");
    let saved_file = saved_dir.join("design.indicatrix.toml");
    store.update(|s| {
        s.settings.recent_native_files = vec![
            saved_file.display().to_string(),
            "/some/older/design.indicatrix.toml".to_string(),
        ];
    });

    assert_eq!(
        last_save_folder(&store),
        Some(saved_dir.clone()),
        "must use the FIRST (most recent) entry, not an older one"
    );

    let _ = std::fs::remove_dir_all(&saved_dir);
}

/// Runs [`import_path`] recursively over `dir` on its own thread, wrapped in
/// `catch_unwind` around the WHOLE call -- the manual crash-hunt probe itself, as
/// opposed to `pipeline::catch_file_panic`'s narrower per-file guard this is checking
/// up on. `Err` carries the panic's message; `Ok` is the ordinary [`ImportOutcome`].
fn run_import_catching_panics(
    db: &Arc<Mutex<Database>>,
    dir: &std::path::Path,
) -> Result<ImportOutcome, String> {
    let db = Arc::clone(db);
    let dir = dir.to_path_buf();
    let handle = std::thread::spawn(move || {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            import_path(&db, &dir, true, |_, _| {})
        }))
    });
    handle
        .join()
        .expect("the import thread itself must not panic while being joined")
        .map_err(|payload| {
            payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "unknown panic".to_string())
        })
}

/// Reads every one of `ids` back through [`Database::get_diagram_full`],
/// [`Database::get_diagram_full_meta`], [`Database::get_preview_material`] and a
/// keyset [`Database::search_diagrams_page`] lookup landing exactly on that id
/// (unlike an unfiltered [`Database::search_diagrams`], which is capped and would
/// otherwise let a newly-imported high id slip past the assertion silently) -- the
/// full "does this row actually come back the way the library reads it" check that the
/// crash-hunt probes rely on.
fn verify_every_imported_id_reads_back(db: &Arc<Mutex<Database>>, ids: &[i64]) {
    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    for &id in ids {
        let full = conn
            .get_diagram_full(id)
            .unwrap_or_else(|e| panic!("get_diagram_full({id}) failed: {e}"));
        assert!(
            full.is_some(),
            "imported id {id} must read back via get_diagram_full"
        );
        let meta = conn
            .get_diagram_full_meta(id)
            .unwrap_or_else(|e| panic!("get_diagram_full_meta({id}) failed: {e}"));
        assert!(
            meta.is_some(),
            "imported id {id} must read back via get_diagram_full_meta"
        );
        conn.get_preview_material(id)
            .unwrap_or_else(|e| panic!("get_preview_material({id}) failed: {e}"));
        let page = conn
            .search_diagrams_page("", "All", "All", &RangeFilter::default(), Some(id - 1), 1)
            .unwrap_or_else(|e| panic!("search_diagrams_page around id {id} failed: {e}"));
        assert_eq!(
            page.first().map(|item| item.id),
            Some(id),
            "the library search/list query must find imported id {id} immediately after id - 1"
        );
    }
    // Also exercise the exact unfiltered query `gui::library::local::helpers`'s own
    // `refresh_after_library_change` runs after every import, purely to prove it
    // still runs without error/panic against this now-larger catalogue.
    conn.search_diagrams("", "All", "All", &RangeFilter::default())
        .expect("the library's own unfiltered search_diagrams must still succeed");
}
