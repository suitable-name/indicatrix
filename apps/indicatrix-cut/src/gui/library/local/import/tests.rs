use super::*;
use crate::gui::library::local::helpers::test_support::{
    VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
};
use indicatrix::geometry::cuts::StandardGemCuts;

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

/// The reproduction for this task's BUG 1: a batch with one good file and one
/// file that fails to parse must still import the good one, record the bad one in
/// `failed` rather than abort, and report progress for both -- not just silently
/// stop partway (which is what the OLD whole-loop `db.lock()` plus an uncaught
/// panic anywhere downstream would risk turning into a wedged `is_busy`, per this
/// task's write-up).
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

/// FEATURE 3's ordering contract at the `import_path` level: with `recurse:
/// false`, a file that only exists one level down is invisible (matching the old,
/// always-flat behaviour); with `recurse: true`, the exact same folder yields it
/// too.
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
    // Entering at depth 1 with max_depth 1: `level1` itself is walked (its own
    // depth is within budget), but `level2` one level further down is not.
    collect_asc_files_recursive(&level1, true, 1, 1, &mut visited, &mut out);

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
    collect_asc_files_recursive(&root, true, 0, MAX_RECURSE_DEPTH, &mut visited, &mut out);

    assert_eq!(
        out.len(),
        1,
        "the symlink loop must not cause real.asc to be found more than once: {out:?}"
    );

    let _ = std::fs::remove_dir_all(&root);
}

/// Manual perf probe for FEATURE 2's "per-file step indicator" question: how long
/// a single file's parse + geometry actually takes, to judge whether "parse ->
/// geometry -> write" sub-steps would be visible to a human (roughly 100ms is the
/// usual perceptible threshold) or just UI noise. `#[ignore]`d for the same reason
/// as `perf_probe_refresh_after_library_change_cost` in `super::helpers`: a timing
/// measurement, not a correctness test, doesn't belong in a normal CI run. Run
/// explicitly with `cargo test -p indicatrix-cut -- --ignored perf_probe --nocapture`.
#[test]
#[ignore = "manual perf probe, not for CI"]
fn perf_probe_single_file_parse_and_measure_cost() {
    const ITERATIONS: u32 = 500;
    let t0 = std::time::Instant::now();
    for _ in 0..ITERATIONS {
        let mut parsed = local::import_asc("trichecker.asc", VALID_ASC).expect("valid .asc");
        apply_measured_metadata(&mut parsed.detail);
    }
    let elapsed = t0.elapsed();
    eprintln!(
        "parse + apply_measured_metadata: {elapsed:?} total over {ITERATIONS} iterations, \
             {:?} average",
        elapsed / ITERATIONS
    );
}
