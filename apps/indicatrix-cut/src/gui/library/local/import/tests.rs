use super::*;
use crate::{
    gui::library::local::helpers::test_support::{
        VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
    },
    settings::{SettingsFile, SettingsPersister},
};
use indicatrix::geometry::cuts::StandardGemCuts;
use std::sync::atomic::{AtomicU32, Ordering};

/// The strongest available anchor: the built-in standard round brilliant has 16
/// girdle facets, so the outline-based rule must call it Round.
#[test]
fn classify_shape_calls_the_standard_round_brilliant_round() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        classify_shape(&planes, 1.0).as_deref(),
        Some("Round"),
        "16 girdle facets at lw == 1.0 must classify as Round"
    );
}

/// The regression this rule exists for. A fold-count rule called "Round
/// Trichecker-12" a Hexagon, because its schedule declares 6-fold symmetry while
/// the cut is round. Classification keys on the girdle OUTLINE instead, so a
/// round outline stays Round no matter what fold count the schedule declares --
/// `classify_shape` no longer receives `symmetry_order` at all, which is what
/// makes that misreading unrepresentable rather than merely unlikely.
#[test]
fn classify_shape_ignores_fold_count_entirely() {
    let planes = StandardGemCuts::standard_round_brilliant();
    // Same planes, and no symmetry_order is threaded in from anywhere: the only
    // inputs are the outline and the measured ratio.
    assert_eq!(classify_shape(&planes, 1.0).as_deref(), Some("Round"));
}

/// An elongated stone is never guessed at, however round-looking its outline:
/// Oval/Marquise/Pear cannot be told apart by side count alone, so the honest
/// answer is no shape rather than a confident wrong one.
#[test]
fn classify_shape_refuses_to_guess_for_an_elongated_outline() {
    let planes = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        classify_shape(&planes, 1.6),
        None,
        "a 1.6 length/width ratio must not be classified Round"
    );
}

/// No girdle facets at all (or too few to be confident) yields no shape rather
/// than a panic or a default.
#[test]
fn classify_shape_returns_none_without_a_usable_girdle() {
    assert_eq!(classify_shape(&[], 1.0), None);
}

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

/// Re-importing a `.asc` over an existing row (a filename collision) must not
/// silently wipe hand-entered metadata `local::import_asc` never produces
/// (`designer_info`, here), and must invalidate the stale preview image describing
/// the old geometry. Both are asserted here.
#[test]
fn import_path_merges_hand_entered_metadata_and_invalidates_stale_preview_on_collision() {
    let dir = temp_dir_for_test("reimport_merge");
    std::fs::write(dir.join("reimport.asc"), VALID_ASC).expect("write reimport.asc");

    let db_path = temp_db_path_for_test("reimport_merge");
    let db = open_temp_db(&db_path);

    let first = import_path(&db, &dir, false, |_, _| {});
    assert_eq!(
        first.imported_ids.len(),
        1,
        "first import must create one row"
    );
    let id = first.imported_ids[0];

    {
        let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Simulate a cutter hand-typing a designer name in the metadata editor, and a
        // preview batch having already generated an image for the old geometry.
        let full = conn
            .get_diagram_full(id)
            .expect("query must succeed")
            .expect("row must exist");
        let update = indicatrix_vault::model::metadata_update::MetadataUpdate {
            designer_info: Some("Test Designer".to_string()),
            shape: full.shape,
            refractive_index: full.refractive_index,
            index_gear: full.index_gear,
            facets_count: full.facets_count,
            symmetry_order: full.symmetry_order,
            mirror_symmetry: full.mirror_symmetry,
            lw_ratio: full.lw_ratio,
            hw_ratio: full.hw_ratio,
            cw_ratio: full.cw_ratio,
            pw_ratio: full.pw_ratio,
            volume: full.volume,
        };
        conn.update_diagram_metadata(id, &update)
            .expect("metadata update must succeed");
        conn.save_preview_images(id, Some(b"stale-front-png"), None, 111_111)
            .expect("preview save must succeed");
    }

    let second = import_path(&db, &dir, false, |_, _| {});
    assert!(
        second.summary.contains("replaced"),
        "summary must report the collision: {}",
        second.summary
    );
    assert_eq!(
        second.imported_ids,
        vec![id],
        "re-importing the same file must update the SAME row, not create a new one"
    );

    let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let full = conn
        .get_diagram_full(id)
        .expect("query must succeed")
        .expect("row must exist");
    assert_eq!(
        full.designer_info.as_deref(),
        Some("Test Designer"),
        "hand-typed designer_info must survive a re-import collision"
    );

    let preview = conn
        .get_preview_images(id)
        .expect("preview query must succeed");
    assert!(
        preview.front.is_none() && preview.generated_at.is_none(),
        "the stale preview must be invalidated by a re-import collision"
    );
    drop(conn);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

/// The depth backstop in [`collect_asc_files_recursive`], exercised directly with
/// a small `max_depth` rather than building 32 real nested folders to hit
/// [`MAX_RECURSE_DEPTH`].
#[test]
fn collect_asc_files_recursive_stops_at_max_depth() {
    let root = temp_dir_for_test("depth");
    let level1 = root.join("level1");
    let level2 = level1.join("level2");
    std::fs::create_dir_all(&level2).expect("create nested folders");
    std::fs::write(level1.join("shallow.asc"), VALID_ASC).expect("write shallow.asc");
    std::fs::write(level2.join("deep.asc"), VALID_ASC).expect("write deep.asc");

    let mut visited = HashSet::new();
    let mut out = Vec::new();
    let mut gem_gcs_skipped = 0usize;
    // Entering at depth 1 with max_depth 1: `level1` itself is walked (its own
    // depth is within budget), but `level2` one level further down is not.
    collect_asc_files_recursive(
        &level1,
        true,
        1,
        1,
        &mut visited,
        &mut out,
        &mut gem_gcs_skipped,
    );

    assert_eq!(
        out.len(),
        1,
        "only the depth-1 file should be found, not the depth-2 one: {out:?}"
    );
    assert!(out[0].ends_with("shallow.asc"));

    let _ = std::fs::remove_dir_all(&root);
}

/// The symlink-loop backstop in [`collect_asc_files_recursive`]: a directory
/// symlink pointing back to an ancestor must not be followed forever, and must
/// not make `real.asc` (reachable both directly and through the loop) show up
/// more than once. Best-effort -- creating a directory symlink/junction on
/// Windows normally needs Developer Mode or an elevated process, so this skips
/// itself (rather than failing) wherever that isn't available, same tolerance
/// this crate's own real-catalogue perf probe gives a missing prerequisite.
#[test]
fn collect_asc_files_recursive_guards_against_a_symlink_loop() {
    let root = temp_dir_for_test("symlink_loop");
    std::fs::write(root.join("real.asc"), VALID_ASC).expect("write real.asc");
    let link_path = root.join("loop_back");

    #[cfg(windows)]
    let symlink_result = std::os::windows::fs::symlink_dir(&root, &link_path);
    #[cfg(not(windows))]
    let symlink_result = std::os::unix::fs::symlink(&root, &link_path);

    if let Err(e) = symlink_result {
        eprintln!(
            "skipping collect_asc_files_recursive_guards_against_a_symlink_loop: \
                 could not create a directory symlink on this machine ({e}) -- \
                 likely needs Developer Mode or an elevated process on Windows"
        );
        let _ = std::fs::remove_dir_all(&root);
        return;
    }

    let mut visited = HashSet::new();
    let mut out = Vec::new();
    let mut gem_gcs_skipped = 0usize;
    collect_asc_files_recursive(
        &root,
        true,
        0,
        MAX_RECURSE_DEPTH,
        &mut visited,
        &mut out,
        &mut gem_gcs_skipped,
    );

    assert_eq!(
        out.len(),
        1,
        "the symlink loop must not cause real.asc to be found more than once: {out:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

// --- count_pending_collisions ---

#[test]
fn count_pending_collisions_finds_no_collisions_in_an_empty_catalogue() {
    let dir = temp_dir_for_test("collisions_none");
    std::fs::write(dir.join("fresh.asc"), VALID_ASC).expect("write fresh.asc");
    let db_path = temp_db_path_for_test("collisions_none");
    let db = open_temp_db(&db_path);

    assert_eq!(count_pending_collisions(&db, &dir, false), (0, 1));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn count_pending_collisions_flags_a_filename_already_in_the_catalogue() {
    let dir = temp_dir_for_test("collisions_some");
    std::fs::write(dir.join("existing.asc"), VALID_ASC).expect("write existing.asc");
    std::fs::write(dir.join("new.asc"), VALID_ASC).expect("write new.asc");
    let db_path = temp_db_path_for_test("collisions_some");
    let db = open_temp_db(&db_path);

    // Seed the catalogue with a row already named `existing.asc`, via the exact
    // machinery a real import uses -- from a DIFFERENT folder, since the collision
    // test is filename-only, not path-based (see `save_imported_design`'s own doc
    // comment).
    let seed_dir = temp_dir_for_test("collisions_some_seed");
    std::fs::write(seed_dir.join("existing.asc"), VALID_ASC).expect("write seed existing.asc");
    let seeded = import_path(&db, &seed_dir, false, |_, _| {});
    assert_eq!(seeded.imported_ids.len(), 1, "seed import must succeed");

    // Nothing was written by the scan itself: re-running it gives the identical
    // answer, and the seeded row above is still the only row in the catalogue.
    assert_eq!(count_pending_collisions(&db, &dir, false), (1, 2));
    assert_eq!(count_pending_collisions(&db, &dir, false), (1, 2));

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&seed_dir);
    let _ = std::fs::remove_file(&db_path);
}

#[test]
fn count_pending_collisions_is_zero_zero_for_an_unreadable_path() {
    let db_path = temp_db_path_for_test("collisions_unreadable");
    let db = open_temp_db(&db_path);
    let missing = std::env::temp_dir().join("indicatrix_cut_test_does_not_exist_at_all");

    assert_eq!(count_pending_collisions(&db, &missing, false), (0, 0));

    let _ = std::fs::remove_file(&db_path);
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

/// Manual perf probe: how long a single file's parse + geometry actually takes, to
/// judge whether "parse -> geometry -> write" sub-steps would be visible to a
/// human (roughly 100ms is the usual perceptible threshold) or just UI noise.
/// `#[ignore]`d for the same reason as `perf_probe_refresh_after_library_change_cost`
/// in `super::helpers`: a timing measurement, not a correctness test, doesn't
/// belong in a normal CI run. Run explicitly with
/// `cargo test -p indicatrix-cut -- --ignored perf_probe --nocapture`.
// --- The collision-confirm decision ---

#[test]
fn no_collisions_never_needs_confirmation() {
    assert!(!collisions_need_confirmation(0));
}

#[test]
fn a_single_collision_needs_confirmation() {
    assert!(collisions_need_confirmation(1));
}

#[test]
fn many_collisions_still_need_only_one_confirmation() {
    assert!(collisions_need_confirmation(50));
}

#[test]
#[ignore = "manual perf probe, not for CI"]
fn perf_probe_single_file_parse_and_measure_cost() {
    const ITERATIONS: u32 = 500;
    let t0 = std::time::Instant::now();
    for _ in 0..ITERATIONS {
        let mut parsed = local::import_asc("trichecker.asc", VALID_ASC, None).expect("valid .asc");
        apply_measured_metadata(&mut parsed.detail);
    }
    let elapsed = t0.elapsed();
    eprintln!(
        "parse + apply_measured_metadata: {elapsed:?} total over {ITERATIONS} iterations, \
             {:?} average",
        elapsed / ITERATIONS
    );
}
