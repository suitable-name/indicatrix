//! Manual perf and crash-hunt probes (all `#[ignore]`d): the per-file parse-and-measure
//! cost, and the stress imports against an adversarial corpus, a fresh database and a
//! real-layout scratch catalogue.

use super::{
    super::{apply_measured_metadata, confirm::count_pending_collisions},
    run_import_catching_panics, verify_every_imported_id_reads_back,
};
use crate::gui::library::local::helpers::test_support::{
    VALID_ASC, open_temp_db, temp_db_path_for_test, temp_dir_for_test,
};
use indicatrix_vault::{db::sqlite::Database, local, model::filter::RangeFilter};
use std::sync::{Arc, Mutex};

/// Manual perf probe: how long a single file's parse + geometry actually takes, to
/// judge whether "parse -> geometry -> write" sub-steps would be visible to a
/// human (roughly 100ms is the usual perceptible threshold) or just UI noise.
/// `#[ignore]`d for the same reason as `perf_probe_refresh_after_library_change_cost`
/// in `super::helpers`: a timing measurement, not a correctness test, doesn't
/// belong in a normal CI run. Run explicitly with
/// `cargo test -p indicatrix-cut -- --ignored perf_probe --nocapture`.
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

// --- Manual stress probes against a real-layout scratch catalogue (crash hunt) ---
//
// The tests below are NOT part of the ordinary `cargo test -p indicatrix-cut --lib`
// run (each is `#[ignore]`d) -- they read a real-layout copy of the user's own
// catalogue from the `INDICATRIX_SCRATCH_DB` environment variable (never the real
// `facet_diagrams.sqlite` itself -- see this crate's own domain rules) and exercise
// the whole `import_path` batch loop against real, migrated data. Run explicitly
// with, e.g.:
// `INDICATRIX_SCRATCH_DB=C:\path\to\scratch.sqlite cargo test -p indicatrix-cut --lib \
//  -- --ignored --nocapture manual_import`

/// Every one of `indicatrix_formats::asc::tests::adversarial`'s 17 inputs, copied
/// here verbatim as importable files -- that module's own consts are `#[cfg(test)]`-
/// private to `indicatrix-formats` and not reachable from this crate.
const ADVERSARIAL_ASC_INPUTS: &[(&str, &str)] = &[
    (
        "adversarial_01_bom_crlf.asc",
        "\u{feff}GemCad 5.0\r\ng 96 0.0\r\ny 8 y\r\nI 1.54\r\na 41 0.5 0 12\r\n",
    ),
    (
        "adversarial_02_nan_inf.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na NaN inf 0 12\n",
    ),
    (
        "adversarial_03_garbage_index.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 1 2 x3 4\n",
    ),
    (
        "adversarial_04_comma_index.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 1 2,5 4\n",
    ),
    (
        "adversarial_05_trailing_text.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 0 12\nDesigned 1999 by X\n",
    ),
    (
        "adversarial_06_symmetry_zero.asc",
        "GemCad 5.0\ng 96 0.0\ny 0 n\nI 1.54\na 41 0.5 0\n",
    ),
    (
        "adversarial_07_ri_negative.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 n\nI -1\na 41 0.5 0\n",
    ),
    (
        "adversarial_08_index_out_of_range.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 96 200 -5\n",
    ),
    (
        "adversarial_09_comma_decimal_angle.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41,5 0,5 0\n",
    ),
    (
        "adversarial_10_lone_cr.asc",
        "GemCad 5.0\rg 96 0.0\ry 8 y\rI 1.54\ra 41 0.5 0\r",
    ),
    (
        "adversarial_11_trailing_n.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 0 n\n",
    ),
    (
        "adversarial_12_empty_notes.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na 41 0.5 G\n",
    ),
    (
        "adversarial_13_empty_tier.asc",
        "GemCad 5.0\ng 96 0.0\ny 8 y\nI 1.54\na\n",
    ),
    (
        "adversarial_14_gear_nan.asc",
        "GemCad 5.0\ng NaN 0\ny 8 y\nI 1.5\na 1 1\n",
    ),
    (
        "adversarial_15_gear_angle_nan.asc",
        "GemCad 5.0\ng 96 NaN\ny 8 y\nI 1.5\na 1 1\n",
    ),
    (
        "adversarial_16_ri_nan.asc",
        "GemCad 5.0\ng 96 0\ny 8 y\nI NaN\na 1 1\n",
    ),
    (
        "adversarial_17_duplicate_gear.asc",
        "GemCad 5.0\ng 96 0\ng 64 0\ny 8 y\nI 1.5\na 1 1 0\n",
    ),
];

/// Writes every `(name, content)` pair in `files` into `dir`.
fn write_named_files(dir: &std::path::Path, files: &[(&str, &str)]) {
    for (name, content) in files {
        std::fs::write(dir.join(name), content)
            .unwrap_or_else(|e| panic!("write manual-probe fixture '{name}': {e}"));
    }
}

/// Reconstructs up to ~300 `.asc` files spread across `db`'s own id range, straight
/// from its stored angle-settings tables (`local::reconstruct_asc_schedule` +
/// `indicatrix_formats::asc::to_asc_string`), skipping any entry that reconstructs to
/// `Ok(None)` (no angle settings) or `Err` (unparsable stored data) -- exactly the
/// corpus the crash hunt in this module's own doc history calls for. Returns how many
/// files were actually written.
fn write_catalogue_derived_corpus(db: &Arc<Mutex<Database>>, dir: &std::path::Path) -> usize {
    let items = {
        let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        conn.search_diagrams_page("", "All", "All", &RangeFilter::default(), None, 5000)
            .expect("search_diagrams_page must succeed against the migrated catalogue")
    };
    if items.is_empty() {
        return 0;
    }
    let stride = (items.len() / 300).max(1);
    let mut written = 0usize;
    for item in items.iter().step_by(stride) {
        let meta = {
            let conn = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            conn.get_diagram_full_meta(item.id)
                .unwrap_or_else(|e| panic!("get_diagram_full_meta({}) failed: {e}", item.id))
        };
        let Some(meta) = meta else { continue };
        let Ok(Some(schedule)) = local::reconstruct_asc_schedule(
            &meta.title,
            meta.refractive_index.as_deref(),
            meta.index_gear.as_deref(),
            &meta.angle_settings,
        ) else {
            continue;
        };
        let Ok(text) = indicatrix_formats::asc::to_asc_string(&schedule) else {
            continue;
        };
        std::fs::write(dir.join(format!("catalogue_entry_{}.asc", item.id)), text)
            .unwrap_or_else(|e| panic!("write catalogue-derived corpus file: {e}"));
        written += 1;
    }
    written
}

/// Writes a couple of real `.asc` + older `.indicatrix.toml` sidecar pairs (the format
/// earlier builds saved): a solved
/// [`indicatrix_cut_core::design::Design`] from one of `indicatrix_cut_core::templates::TEMPLATES`,
/// written out via `indicatrix_cut_core::native::save_paired`.
fn write_sidecar_pairs(dir: &std::path::Path) {
    for (i, spec) in indicatrix_cut_core::templates::TEMPLATES
        .iter()
        .take(2)
        .enumerate()
    {
        let design = indicatrix_cut_core::design::Design::new(
            indicatrix_cut_core::preform::PreformSpec::cylinder(
                spec.gear_teeth.unsigned_abs() as usize,
                1.5,
                1.0,
                1.5,
            ),
            spec.schedule_meta(),
            spec.tiers(),
        );
        design
            .solve()
            .unwrap_or_else(|e| panic!("template {:?} failed to solve: {e:?}", spec.name));
        let asc_filename = format!("template_{i}.asc");
        let paired = indicatrix_cut_core::native::save_paired(
            &design,
            asc_filename.clone(),
            None,
            None,
            None,
        )
        .unwrap_or_else(|e| panic!("save_paired for template {:?}: {e}", spec.name));
        std::fs::write(dir.join(&asc_filename), &paired.asc_text)
            .unwrap_or_else(|e| panic!("write {asc_filename}: {e}"));
        std::fs::write(
            dir.join(format!("template_{i}.indicatrix.toml")),
            &paired.native_toml,
        )
        .unwrap_or_else(|e| panic!("write template_{i}.indicatrix.toml: {e}"));
    }
}

/// Probe (main): copies the real-layout scratch catalogue named by
/// `INDICATRIX_SCRATCH_DB` to a throwaway temp file (never opens or writes the
/// original), opens it with [`Database::new`] (running the real
/// `migrate_blob_columns_last` 12-step rebuild against real pre-migration data),
/// builds a corpus of catalogue-derived `.asc` files plus every adversarial input
/// plus a couple of older sidecar pairs plus one garbage sidecar plus one 0-byte
/// `.asc`, imports the whole directory recursively, verifies every imported row
/// reads back through every path the library itself uses, then imports the SAME
/// directory again to exercise the collision/merge path.
///
/// Skips itself (does not fail) when `INDICATRIX_SCRATCH_DB` is unset or names a
/// missing file -- this workstation-specific probe must never fail CI or another
/// developer's machine.
#[test]
#[ignore = "manual: needs a real-layout scratch catalogue named by INDICATRIX_SCRATCH_DB"]
fn manual_import_stress_against_the_migrated_scratch_catalogue() {
    let Ok(scratch_path) = std::env::var("INDICATRIX_SCRATCH_DB") else {
        eprintln!(
            "skipping manual_import_stress_against_the_migrated_scratch_catalogue: \
             INDICATRIX_SCRATCH_DB is not set"
        );
        return;
    };
    if !std::path::Path::new(&scratch_path).is_file() {
        eprintln!(
            "skipping manual_import_stress_against_the_migrated_scratch_catalogue: \
             '{scratch_path}' does not exist"
        );
        return;
    }

    let db_path = temp_db_path_for_test("scratch_migrated");
    std::fs::copy(&scratch_path, &db_path)
        .unwrap_or_else(|e| panic!("copy the scratch catalogue to a scratch temp file: {e}"));
    // `Database::new` runs every migration, including `migrate_blob_columns_last`,
    // against this real pre-migration-layout copy.
    let db = open_temp_db(&db_path);

    let corpus_dir = temp_dir_for_test("scratch_corpus");
    let catalogue_written = write_catalogue_derived_corpus(&db, &corpus_dir);
    write_named_files(&corpus_dir, ADVERSARIAL_ASC_INPUTS);
    write_sidecar_pairs(&corpus_dir);
    std::fs::write(corpus_dir.join("garbage_sidecar.asc"), VALID_ASC)
        .expect("write garbage_sidecar.asc");
    std::fs::write(
        corpus_dir.join("garbage_sidecar.indicatrix.toml"),
        "\u{0}not valid toml {{{",
    )
    .expect("write garbage_sidecar.indicatrix.toml");
    std::fs::write(corpus_dir.join("zero_byte.asc"), []).expect("write zero_byte.asc");

    eprintln!(
        "manual scratch-catalogue import stress: {catalogue_written} catalogue-derived .asc + \
         {} adversarial inputs + 2 template sidecar pairs + 1 garbage sidecar + 1 zero-byte file, \
         corpus at {}",
        ADVERSARIAL_ASC_INPUTS.len(),
        corpus_dir.display()
    );

    let outcome = run_import_catching_panics(&db, &corpus_dir).unwrap_or_else(|panic_msg| {
        panic!(
            "import_path PANICKED against the migrated scratch catalogue (this is exactly the \
             crash this probe exists to catch): {panic_msg}"
        )
    });
    eprintln!(
        "first import: {}\nimported_ids.len()={} had_failures={} had_collision={}",
        outcome.summary,
        outcome.imported_ids.len(),
        outcome.had_failures,
        outcome.had_collision
    );
    verify_every_imported_id_reads_back(&db, &outcome.imported_ids);

    let (collisions, total) = count_pending_collisions(&db, &corpus_dir, true);
    eprintln!("count_pending_collisions after first import: {collisions}/{total}");

    let second = run_import_catching_panics(&db, &corpus_dir).unwrap_or_else(|panic_msg| {
        panic!("import_path PANICKED on the second (collision) pass: {panic_msg}")
    });
    eprintln!(
        "second import (collision pass): {}\nimported_ids.len()={} had_failures={} had_collision={}",
        second.summary,
        second.imported_ids.len(),
        second.had_failures,
        second.had_collision
    );
    verify_every_imported_id_reads_back(&db, &second.imported_ids);

    let _ = std::fs::remove_dir_all(&corpus_dir);
    let _ = std::fs::remove_file(&db_path);
}

/// Probe (second): the exact same adversarial/sidecar/garbage/zero-byte
/// corpus [`manual_import_stress_against_the_migrated_scratch_catalogue`] builds
/// (minus the catalogue-derived files, since a fresh database has no rows to
/// reconstruct them from), imported into a brand-new empty database instead of a
/// real-layout catalogue -- so a crash specific to the migrated schema versus one
/// present even on a fresh database can be told apart. Unlike the scratch-catalogue
/// probes, this needs no external file and always runs when un-`--ignore`d; it stays
/// `#[ignore]`d anyway to keep it out of the default fast test loop (template solving
/// plus a worker-thread round trip is not free).
#[test]
#[ignore = "manual: template-solving + worker-thread stress probe, not for the default run"]
fn manual_import_stress_against_a_fresh_empty_db() {
    let db_path = temp_db_path_for_test("empty_stress");
    let db = open_temp_db(&db_path);

    let corpus_dir = temp_dir_for_test("empty_stress_corpus");
    write_named_files(&corpus_dir, ADVERSARIAL_ASC_INPUTS);
    write_sidecar_pairs(&corpus_dir);
    std::fs::write(corpus_dir.join("garbage_sidecar.asc"), VALID_ASC)
        .expect("write garbage_sidecar.asc");
    std::fs::write(
        corpus_dir.join("garbage_sidecar.indicatrix.toml"),
        "\u{0}not valid toml {{{",
    )
    .expect("write garbage_sidecar.indicatrix.toml");
    std::fs::write(corpus_dir.join("zero_byte.asc"), []).expect("write zero_byte.asc");

    let outcome = run_import_catching_panics(&db, &corpus_dir).unwrap_or_else(|panic_msg| {
        panic!("import_path PANICKED against a fresh empty database: {panic_msg}")
    });
    eprintln!(
        "fresh-db import: {}\nimported_ids.len()={} had_failures={} had_collision={}",
        outcome.summary,
        outcome.imported_ids.len(),
        outcome.had_failures,
        outcome.had_collision
    );
    verify_every_imported_id_reads_back(&db, &outcome.imported_ids);

    let _ = std::fs::remove_dir_all(&corpus_dir);
    let _ = std::fs::remove_file(&db_path);
}

/// Probe (third): a migrated scratch catalogue opened
/// [`Database::open_read_only`] (the same connection shape the app's own
/// `immutable=1` fallback in `Database::new` can produce for a database on a
/// read-only file) must surface every write failure as an ordinary
/// [`super::super::pipeline::ImportOutcome::had_failures`] entry, never a panic and never a silently-empty
/// success.
#[test]
#[ignore = "manual: needs a real-layout scratch catalogue named by INDICATRIX_SCRATCH_DB"]
fn manual_import_against_a_read_only_scratch_catalogue_surfaces_errors_not_panics() {
    let Ok(scratch_path) = std::env::var("INDICATRIX_SCRATCH_DB") else {
        eprintln!(
            "skipping manual_import_against_a_read_only_scratch_catalogue_surfaces_errors_not_panics: \
             INDICATRIX_SCRATCH_DB is not set"
        );
        return;
    };
    if !std::path::Path::new(&scratch_path).is_file() {
        eprintln!(
            "skipping manual_import_against_a_read_only_scratch_catalogue_surfaces_errors_not_panics: \
             '{scratch_path}' does not exist"
        );
        return;
    }

    let db_path = temp_db_path_for_test("scratch_readonly");
    std::fs::copy(&scratch_path, &db_path)
        .unwrap_or_else(|e| panic!("copy the scratch catalogue to a scratch temp file: {e}"));
    {
        // Open read-write once, purely so every migration (including
        // `migrate_blob_columns_last`) has already run before the read-only
        // connection below opens the same file.
        drop(open_temp_db(&db_path));
    }

    // `Database::open_read_only` (SQLITE_OPEN_READ_ONLY, no SQLITE_OPEN_CREATE) is
    // the exact connection shape the app's own `immutable=1` fallback in
    // `Database::new` can produce for a database sitting on a read-only file --
    // exercised directly here rather than via the OS's own read-only file
    // attribute, which this probe would then have to clear again before it could
    // clean up its own temp file afterward.
    let db_path_str = db_path
        .to_str()
        .expect("temp path is valid UTF-8")
        .to_string();
    let db = Arc::new(Mutex::new(
        Database::open_read_only(&db_path_str).unwrap_or_else(|e| {
            panic!("open_read_only must succeed against a read-only file: {e}")
        }),
    ));

    let corpus_dir = temp_dir_for_test("scratch_readonly_corpus");
    std::fs::write(corpus_dir.join("one.asc"), VALID_ASC).expect("write one.asc");

    let outcome = run_import_catching_panics(&db, &corpus_dir).unwrap_or_else(|panic_msg| {
        panic!("import_path PANICKED against a read-only database: {panic_msg}")
    });
    eprintln!("read-only db import: {}", outcome.summary);
    assert!(
        outcome.had_failures,
        "importing into a read-only database must fail cleanly (a save error), not silently \
         succeed: {}",
        outcome.summary
    );
    assert!(
        outcome.imported_ids.is_empty(),
        "a read-only database must not report anything as imported"
    );

    let _ = std::fs::remove_dir_all(&corpus_dir);
    let _ = std::fs::remove_file(&db_path);
}
