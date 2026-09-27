//! The core parse-measure-save pipeline: turns one candidate `.asc` (plus its
//! optional native sidecar) into a saved catalogue row, and drives that over an
//! entire batch of candidates.

use super::{
    measure::{apply_measured_metadata, merge_reimport_metadata},
    scan::{GEM_GCS_EXPLANATION, collect_import_candidates, find_native_sidecar},
};
use indicatrix_vault::{db::sqlite::Database, local};
use std::{
    collections::HashSet,
    path::Path,
    sync::{Arc, Mutex},
};
use tracing::warn;

/// Runs `f`, converting a panic into an error message instead of letting it unwind
/// past this call. Wraps the one per-file step (`local::import_asc` +
/// `apply_measured_metadata`) that reaches into `indicatrix`'s geometry code -- a crate
/// this module doesn't own and can't guarantee is panic-free on every
/// malformed-but-parseable `.asc`. Without this, one bad file would kill the whole
/// worker thread, silently dropping every file after it and leaving `is_busy` stuck
/// (see [`super::wiring::setup_import_callback`]'s doc comment). With this, it's just
/// one more `failed` entry and the batch keeps going.
pub(super) fn catch_file_panic<T>(
    f: impl FnOnce() -> T + std::panic::UnwindSafe,
) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string())
    })
}

/// [`import_path`]'s return value: the human-readable summary shown in the toast/status
/// line, plus the `diagram_entries.id` of every design this call actually saved (fresh
/// or a filename-collision replacement) -- the post-import preview-generation offer
/// (`gui::batch::preview::offer_batch_confirmation`, called from
/// `super::wiring::spawn_import`'s completion closure) needs exactly this list.
pub(super) struct ImportOutcome {
    pub(super) summary: String,
    pub(super) imported_ids: Vec<i64>,
    /// Whether at least one candidate file failed to read, parse
    /// or save. `spawn_import` derives the completion toast's kind from THIS, not
    /// from sniffing the summary text -- a message like "Imported 3 .asc file(s); 12
    /// skipped (...)" would otherwise read as a plain success because it starts with
    /// "Imported" and isn't "Imported 0", even though 12 of the 15 files failed.
    pub(super) had_failures: bool,
    /// Whether at least one imported file replaced an
    /// existing catalogue row (filename-only dedup). Together with `had_failures`,
    /// this drives `LibraryModel.import_should_stay_open`, which keeps the import
    /// popup open on completion instead of auto-closing.
    pub(super) had_collision: bool,
}

/// Saves one already-parsed [`local::ImportedAsc`] into `db` and reports whether the
/// save landed on an existing row -- split out of [`import_path`]'s own loop purely
/// to keep that function under clippy's `too_many_lines` limit.
///
/// On a collision, carries the existing row's hand-entered
/// metadata forward before the full-replace write (see
/// [`merge_reimport_metadata`]'s own doc comment for the exact rule) and invalidates
/// its now-stale preview/tilt cache afterwards so a regenerate pass rebuilds from
/// the new geometry rather than describing the old one. `file_name` is used only for
/// the cache-invalidation warning logs.
///
/// On a fresh (non-collision) row, stamps
/// `diagram_entries.derived_from_entry_id` from `parsed`'s own recovered
/// [`local::ImportedAsc::derived_from_entry_id`] -- but only once the recorded id is
/// confirmed to still name a real row (it may have been deleted since the `.asc` was
/// exported); a stale or missing id is left unstamped rather than pointing the new
/// row at nothing. Never attempted on a collision: that outcome already IS the
/// recorded source row (same url, same id), so there is nothing to derive it from.
///
/// # Errors
///
/// Returns the underlying `Database` error if the entry or detail write fails.
fn save_imported_design(
    db: &Arc<Mutex<Database>>,
    url: &str,
    seen_before_in_batch: bool,
    parsed: local::ImportedAsc,
    file_name: &str,
) -> anyhow::Result<(i64, bool)> {
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let is_collision = seen_before_in_batch || db.has_detail_for_entry_url(url).unwrap_or(false);
    let local::ImportedAsc {
        entry,
        mut detail,
        derived_from_entry_id,
    } = parsed;
    db.save_diagram_entry(&entry, local::LOCAL_SOURCE_ID)
        .and_then(|id| {
            if is_collision && let Ok(Some(existing)) = db.get_diagram_full(id) {
                merge_reimport_metadata(&mut detail, &existing);
            }
            db.save_diagram_detail(&detail, id).map(|()| id)
        })
        .map(|id| {
            if is_collision {
                if let Err(e) = db.delete_preview_images(id) {
                    warn!(
                        "Import: failed to invalidate stale preview cache for entry #{id} \
                         ('{file_name}'): {e}"
                    );
                }
                if let Err(e) = db.delete_tilt_curves(id) {
                    warn!(
                        "Import: failed to invalidate stale tilt-curve cache for entry #{id} \
                         ('{file_name}'): {e}"
                    );
                }
            } else if let Some(source_id) = derived_from_entry_id {
                // Never a title/filename heuristic -- `source_id` came from a footnote
                // `gui::editor::native_io` itself wrote into this exact file, recording
                // exactly which catalogue row this design was exported/saved from.
                match db.get_diagram_full(source_id) {
                    Ok(Some(_)) => {
                        if let Err(e) = db.set_derived_from_entry_id(id, Some(source_id)) {
                            warn!(
                                "Import: failed to record entry #{id} ('{file_name}') as \
                                 derived from #{source_id}: {e}"
                            );
                        }
                    }
                    Ok(None) => {
                        // The recorded source row is gone -- leave provenance unset
                        // rather than pointing at a row that no longer exists.
                    }
                    Err(e) => warn!(
                        "Import: could not verify source entry #{source_id} for '{file_name}' \
                         before recording provenance: {e}"
                    ),
                }
            }
            (id, is_collision)
        })
}

/// Parses one `.asc` (plus its native sidecar, when one sits beside it) and fills in
/// the measured proportions -- [`import_path`]'s per-file parse step.
///
/// Runs outside the database lock and inside [`catch_file_panic`]: a single
/// malformed file in a folder import must not take the whole batch down. See both of
/// those functions' own doc comments.
///
/// # Errors
///
/// A ready-to-list failure line naming the file and what went wrong, so the caller
/// can push it straight onto its failed-files list.
fn parse_one_import(
    file_name: &str,
    content: &str,
    sidecar: Option<&(String, Vec<u8>)>,
) -> Result<local::ImportedAsc, String> {
    let parse_result = catch_file_panic(std::panic::AssertUnwindSafe(|| {
        local::import_asc(
            file_name,
            content,
            sidecar.map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
        )
        .map(|mut parsed| {
            // Fills the measured proportions AND shape from the same reconstructed
            // planes, measured once, not twice.
            apply_measured_metadata(&mut parsed.detail);
            parsed
        })
    }));
    match parse_result {
        Ok(Ok(parsed)) => Ok(parsed),
        Ok(Err(e)) => Err(format!("{file_name} (parse error: {e})")),
        Err(panic_msg) => {
            warn!("Import panicked while processing '{file_name}': {panic_msg}");
            Err(format!("{file_name} (internal error: {panic_msg})"))
        }
    }
}

/// Imports every `.asc` file at `path` (see [`collect_import_candidates`]),
/// reporting progress via `on_progress(done, total)` after each file so a caller can
/// keep the UI honest during a long folder import (always runs off the UI thread --
/// see `super::wiring::spawn_import`).
///
/// `db` is locked only around each file's actual database work (the collision check
/// and the two writes), never around the file I/O, `.asc` parsing, or plane
/// reconstruction/`measure_solid` geometry that happens first -- so a UI-thread
/// callback that also needs `db` can only ever block for a single row's write, not
/// the whole import.
pub(super) fn import_path(
    db: &Arc<Mutex<Database>>,
    path: &Path,
    recurse: bool,
    mut on_progress: impl FnMut(usize, usize),
) -> ImportOutcome {
    let (candidates, gem_gcs_skipped) = match collect_import_candidates(path, recurse) {
        Ok(c) => c,
        Err(message) => {
            return ImportOutcome {
                summary: message,
                imported_ids: Vec::new(),
                had_failures: true,
                had_collision: false,
            };
        }
    };

    let total = candidates.len();
    let mut imported = 0usize;
    let mut imported_ids: Vec<i64> = Vec::new();
    let mut failed: Vec<String> = Vec::new();
    // Known limitation, not fixed here: `indicatrix_vault::local::import_asc` dedupes
    // on the bare filename (`local://<file_name>`), not the source path, and
    // `diagram_entries.url` is UNIQUE -- so importing `round.asc` from two different
    // folders silently replaces the first with the second. That key lives in
    // `indicatrix_vault`, out of scope here, so this loop only detects the collision
    // (against this batch or a past import) and surfaces it in the summary below
    // rather than letting it pass silently.
    let mut seen_in_batch: HashSet<String> = HashSet::new();
    let mut replaced: Vec<String> = Vec::new();

    for (i, file_path) in candidates.into_iter().enumerate() {
        let file_name = file_path.file_name().map_or_else(
            || "unknown.asc".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        on_progress(i + 1, total);

        let content = match std::fs::read_to_string(&file_path) {
            Ok(c) => c,
            Err(e) => {
                failed.push(format!("{file_name} (read error: {e})"));
                continue;
            }
        };

        let url = format!("local://{file_name}");
        // Recorded before parsing, unconditionally, so a batch-internal name
        // collision is still caught even if this occurrence fails to parse or panics.
        let seen_before_in_batch = !seen_in_batch.insert(file_name.clone());

        // A design saved through Save Native writes a `.asc` PLUS
        // a native sidecar carrying everything the bare `.asc` can't (authored meet
        // constraints, preform, detached facets, material/RI override -- see
        // `indicatrix_formats::native`'s module doc comment). Importing only the
        // `.asc` would silently discard all of that. `find_native_sidecar` looks beside
        // the `.asc` itself, independent of what `collect_import_candidates`
        // collected, and `local::import_asc` attaches it as a second file when found;
        // `gui::editor::loading::design_from_full_record` already prefers
        // `indicatrix_cut_core::load_paired` whenever both attachments are present.
        let sidecar = find_native_sidecar(&file_path);

        let parsed = match parse_one_import(&file_name, &content, sidecar.as_ref()) {
            Ok(parsed) => parsed,
            Err(message) => {
                failed.push(message);
                continue;
            }
        };

        let save_result = save_imported_design(db, &url, seen_before_in_batch, parsed, &file_name);
        match save_result {
            Ok((id, is_collision)) => {
                imported += 1;
                imported_ids.push(id);
                if is_collision {
                    replaced.push(file_name);
                }
            }
            Err(e) => failed.push(format!("{file_name} (save error: {e})")),
        }
    }

    let mut summary = if failed.is_empty() {
        format!("Imported {imported} .asc file(s).")
    } else {
        warn!(
            "Import from {}: {} failed: {:?}",
            path.display(),
            failed.len(),
            failed
        );
        format!(
            "Imported {imported} .asc file(s); {} skipped ({}).",
            failed.len(),
            failed.join("; ")
        )
    };
    if !replaced.is_empty() {
        use std::fmt::Write as _;
        // Named right here, since the popup that shows this text
        // (`import_dialog.slint`'s own `result_text` block) is the only place this
        // information is ever shown.
        let _ = write!(
            summary,
            " {} design(s) replaced an existing entry with the same file name ({}) -- \
             filename-only matching, not a content comparison.",
            replaced.len(),
            replaced.join(", ")
        );
    }
    if gem_gcs_skipped > 0 {
        use std::fmt::Write as _;
        // Same honesty note as `collect_import_candidates`'s own no-`.asc`-found
        // case, for the ordinary "some `.asc` files imported too" case -- see
        // `GEM_GCS_EXPLANATION`'s own doc comment.
        let _ = write!(
            summary,
            " {gem_gcs_skipped} .gem/.gcs file(s) were not imported -- {GEM_GCS_EXPLANATION}"
        );
    }
    ImportOutcome {
        summary,
        imported_ids,
        had_failures: !failed.is_empty(),
        had_collision: !replaced.is_empty(),
    }
}
