//! The core parse-measure-save pipeline: turns one candidate `.asc` (plus its
//! optional native sidecar), or one `.gem`/`.gcs` design converted to `.asc`, into
//! a saved catalogue row, and drives that over an entire batch of candidates.

use super::{
    foreign::{ForeignFormat, convert_foreign_design, converted_asc_file_name},
    measure::{apply_measured_metadata, merge_reimport_metadata},
    scan::{collect_import_candidates, find_native_sidecar},
};
use indicatrix_vault::{db::sqlite::Database, local, model::file::AttachedFile};
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
///
/// Public (not `pub(super)`) because `gui::editor::native_io::catalogue::write_back_to_catalogue`
/// reuses it around the identical `local::import_asc` + `apply_measured_metadata` step
/// on Save Native's own background thread -- see that function's own doc comment for
/// why: it deliberately runs the SAME parse-and-measure path this module's per-file
/// loop does, and until this reuse, only THIS caller (not that one) turned a panic
/// there into an error instead of silently killing the save thread mid-write with no
/// toast and no completion callback ever firing.
pub fn catch_file_panic<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
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
    /// from sniffing the summary text -- a message like "Imported 3 file(s); 12
    /// skipped (...)" would otherwise read as a plain success because it starts with
    /// "Imported" and isn't "Imported 0", even though 12 of the 15 files failed.
    pub(super) had_failures: bool,
    /// Whether at least one imported file replaced an
    /// existing catalogue row (filename-only dedup). Together with `had_failures`,
    /// this drives `LibraryModel.import_should_stay_open`, which keeps the import
    /// popup open on completion instead of auto-closing.
    pub(super) had_collision: bool,
    /// Whether at least one imported `.gem`/`.gcs` file came with reader or
    /// converter warnings (a preform section that is not converted, a hidden or
    /// guide tier, a missing refractive index, ...). Listed in `summary`; keeps
    /// the import popup open the same way `had_collision` does, so the list can
    /// be read.
    pub(super) had_notes: bool,
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
/// The entry and detail are written together by `Database::save_design`, one
/// transaction, so a failure between the two cannot leave a detail-less entry that the
/// next import would read as "no collision". A failure of the collision check itself
/// is an error for this file, not silently "no collision": treating it as none would
/// skip the metadata merge and the cache invalidation for a design that is replaced.
///
/// # Errors
///
/// Returns the underlying `Database` error if the collision check or the entry/detail
/// write fails.
fn save_imported_design(
    db: &Arc<Mutex<Database>>,
    url: &str,
    seen_before_in_batch: bool,
    parsed: local::ImportedAsc,
    file_name: &str,
) -> anyhow::Result<(i64, bool)> {
    let local::ImportedAsc {
        entry,
        mut detail,
        derived_from_entry_id,
    } = parsed;
    let (id, is_collision) = {
        let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let is_collision = seen_before_in_batch || db.has_detail_for_entry_url(url)?;
        // The replaced row is read BEFORE the write, while its old detail still exists; the
        // url-keyed upsert in `save_design` then reuses this same row.
        if is_collision
            && let Some(existing_id) = db.diagram_entry_id_for_url(&entry.url)?
            && let Ok(Some(existing)) = db.get_diagram_full(existing_id)
        {
            merge_reimport_metadata(&mut detail, &existing);
        }
        let id = db.save_design(&entry, &detail, local::LOCAL_SOURCE_ID)?;
        if is_collision {
            invalidate_stale_caches(&db, id, file_name);
        } else if let Some(source_id) = derived_from_entry_id {
            record_provenance(&db, id, source_id, file_name);
        }
        drop(db);
        (id, is_collision)
    };
    Ok((id, is_collision))
}

/// Deletes `id`'s cached previews, tilt curves and solid extents after a re-import
/// replaced its geometry, so a regenerate pass rebuilds them from the new design
/// rather than describing the old one. Logged, not propagated: the import itself
/// succeeded.
fn invalidate_stale_caches(db: &Database, id: i64, file_name: &str) {
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
    if let Err(e) = db.delete_solid_extents(id) {
        warn!(
            "Import: failed to invalidate stale solid-extents cache for entry #{id} \
             ('{file_name}'): {e}"
        );
    }
}

/// Records `id` as derived from `source_id`, the catalogue row a footnote inside the
/// imported file names -- but only once that row is confirmed to still exist.
///
/// Never a title/filename heuristic: `source_id` came from a footnote
/// `gui::editor::native_io` itself wrote into this exact file, recording exactly which
/// catalogue row this design was exported/saved from. A failure is logged, not
/// propagated: the import itself succeeded.
fn record_provenance(db: &Database, id: i64, source_id: i64, file_name: &str) {
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
            // The recorded source row is gone -- leave provenance unset rather than
            // pointing at a row that no longer exists.
        }
        Err(e) => warn!(
            "Import: could not verify source entry #{source_id} for '{file_name}' \
             before recording provenance: {e}"
        ),
    }
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
    bytes: &[u8],
    sidecar: Option<&(String, Vec<u8>)>,
) -> Result<local::ImportedAsc, String> {
    parse_guarded(file_name, || {
        local::import_asc_bytes(
            file_name,
            bytes,
            sidecar.map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
        )
    })
}

/// Runs `import` inside [`catch_file_panic`], fills the measured proportions and shape
/// from the reconstructed planes (measured once), and turns a parse error or panic into
/// a ready-to-list failure line naming `file_name`.
///
/// # Errors
///
/// The failure line described above.
fn parse_guarded(
    file_name: &str,
    import: impl FnOnce() -> Result<local::ImportedAsc, String>,
) -> Result<local::ImportedAsc, String> {
    let parse_result = catch_file_panic(std::panic::AssertUnwindSafe(|| {
        import().map(|mut parsed| {
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

/// How many of one file's reader/converter warnings the import summary quotes
/// before it says "+N more" (the log always gets every one).
const WARNINGS_PER_FILE: usize = 3;

/// Converts one `.gem`/`.gcs` design to `.asc` cutting instructions
/// ([`convert_foreign_design`], inside [`catch_file_panic`]) and hands the text to
/// [`parse_guarded`] -- the SAME `local::import_asc` + `apply_measured_metadata`
/// step a `.asc` file takes. No native sidecar is looked for: a sidecar's
/// fingerprint describes a `.asc`, never a `.gem`/`.gcs`.
///
/// `local::import_asc` names its one attachment after `file_name`, but its bytes
/// are the generated `.asc` text, so that attachment is renamed to
/// [`converted_asc_file_name`] (the `.asc` the editor loads and "Export .asc"
/// writes) and the ORIGINAL file's bytes are attached beside it under `file_name`.
/// A `.gem` also fills `gem_file`. The title falls back to the file stem, not the
/// full `file_name`, when the design carries no title header.
///
/// Returns the parsed design plus the converter's warnings for the summary.
///
/// # Errors
///
/// A ready-to-list failure line naming the file and the typed reader error (or
/// the panic message), exactly like [`parse_one_import`]'s.
fn parse_foreign_import(
    file_name: &str,
    format: ForeignFormat,
    bytes: &[u8],
) -> Result<(local::ImportedAsc, Vec<String>), String> {
    let converted = match catch_file_panic(std::panic::AssertUnwindSafe(|| {
        convert_foreign_design(format, bytes)
    })) {
        Ok(Ok(converted)) => converted,
        Ok(Err(e)) => return Err(format!("{file_name} (parse error: {e})")),
        Err(panic_msg) => {
            warn!("Import panicked while converting '{file_name}': {panic_msg}");
            return Err(format!("{file_name} (internal error: {panic_msg})"));
        }
    };
    let mut parsed = parse_guarded(file_name, || {
        local::import_asc(file_name, &converted.asc_text, None)
    })?;
    let asc_name = converted_asc_file_name(file_name);
    if let Some(generated) = parsed.detail.attached_files.first_mut() {
        generated.name.clone_from(&asc_name);
    }
    parsed.detail.attached_files.push(AttachedFile {
        name: file_name.to_string(),
        url: String::new(),
        content: bytes.to_vec(),
    });
    if format == ForeignFormat::Gem {
        parsed.detail.gem_file = Some(file_name.to_string());
    }
    if parsed.entry.title == file_name {
        parsed.entry.title = asc_name.trim_end_matches(".asc").to_string();
    }
    Ok((parsed, converted.warnings))
}

/// One summary line for a converted file's warnings: the file name, the first
/// [`WARNINGS_PER_FILE`] warnings, and how many more there were.
fn warning_line(file_name: &str, warnings: &[String]) -> String {
    let shown = warnings
        .iter()
        .take(WARNINGS_PER_FILE)
        .cloned()
        .collect::<Vec<_>>()
        .join("; ");
    let rest = warnings.len().saturating_sub(WARNINGS_PER_FILE);
    if rest > 0 {
        format!("{file_name}: {shown} (+{rest} more)")
    } else {
        format!("{file_name}: {shown}")
    }
}

/// Reads and parses one candidate: a `.gem`/`.gcs` through
/// [`parse_foreign_import`], anything else as `.asc` text (plus its native
/// sidecar) through [`parse_one_import`]. Split out of [`import_path`] purely to
/// keep that function under clippy's `too_many_lines` limit.
///
/// # Errors
///
/// A ready-to-list failure line (read, parse or internal error) naming the file.
fn read_and_parse_candidate(
    file_path: &Path,
    file_name: &str,
) -> Result<(local::ImportedAsc, Vec<String>), String> {
    let bytes = std::fs::read(file_path).map_err(|e| format!("{file_name} (read error: {e})"))?;
    if let Some(format) = ForeignFormat::from_path(file_path) {
        return parse_foreign_import(file_name, format, &bytes);
    }
    // The raw bytes go to the vault, which decodes them for parsing (Windows-1252 and
    // byte-order marks included) and stores the file exactly as received.
    // A design saved through Save Native writes a `.asc` PLUS
    // a native sidecar carrying everything the bare `.asc` can't (authored meet
    // constraints, preform, detached facets, material/RI override -- see
    // `indicatrix_formats::native`'s module doc comment). Importing only the
    // `.asc` would silently discard all of that. `find_native_sidecar` looks beside
    // the `.asc` itself, independent of what `collect_import_candidates`
    // collected, and `local::import_asc_bytes` attaches it as a second file when found;
    // `gui::editor::loading::design_from_full_record` already prefers
    // `indicatrix_cut_core::load_paired` whenever both attachments are present.
    let sidecar = find_native_sidecar(file_path);
    parse_one_import(file_name, &bytes, sidecar.as_ref()).map(|parsed| (parsed, Vec::new()))
}

/// Imports every `.asc`, `.gem` and `.gcs` file at `path` (see
/// [`collect_import_candidates`]), reporting progress via `on_progress(done,
/// total)` after each file so a caller can keep the UI honest during a long folder
/// import (always runs off the UI thread -- see `super::wiring::spawn_import`).
///
/// `db` is locked only around each file's actual database work (the collision check
/// and the two writes), never around the file I/O, parsing, `.gem`/`.gcs`
/// conversion, or plane reconstruction/`measure_solid` geometry that happens first
/// -- so a UI-thread callback that also needs `db` can only ever block for a single
/// row's write, not the whole import.
pub(super) fn import_path(
    db: &Arc<Mutex<Database>>,
    path: &Path,
    recurse: bool,
    mut on_progress: impl FnMut(usize, usize),
) -> ImportOutcome {
    let candidates = match collect_import_candidates(path, recurse) {
        Ok(c) => c,
        Err(message) => {
            return ImportOutcome {
                summary: message,
                imported_ids: Vec::new(),
                had_failures: true,
                had_collision: false,
                had_notes: false,
            };
        }
    };

    let total = candidates.len();
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
    let mut noted: Vec<String> = Vec::new();

    for (i, file_path) in candidates.into_iter().enumerate() {
        let file_name = file_path.file_name().map_or_else(
            || "unknown.asc".to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        on_progress(i + 1, total);

        let url = format!("local://{file_name}");
        // Recorded before parsing, unconditionally, so a batch-internal name
        // collision is still caught even if this occurrence fails to parse or panics.
        let seen_before_in_batch = !seen_in_batch.insert(file_name.clone());

        let (parsed, warnings) = match read_and_parse_candidate(&file_path, &file_name) {
            Ok(result) => result,
            Err(message) => {
                failed.push(message);
                continue;
            }
        };

        match save_imported_design(db, &url, seen_before_in_batch, parsed, &file_name) {
            Ok((id, is_collision)) => {
                imported_ids.push(id);
                if !warnings.is_empty() {
                    tracing::info!("Import: '{file_name}' converted with notes: {warnings:?}");
                    noted.push(warning_line(&file_name, &warnings));
                }
                if is_collision {
                    replaced.push(file_name);
                }
            }
            Err(e) => failed.push(format!("{file_name} (save error: {e})")),
        }
    }

    ImportOutcome {
        summary: import_summary(path, imported_ids.len(), &failed, &replaced, &noted),
        imported_ids,
        had_failures: !failed.is_empty(),
        had_collision: !replaced.is_empty(),
        had_notes: !noted.is_empty(),
    }
}

/// [`import_path`]'s summary text: how many files were imported, which were
/// skipped and why, which replaced an existing entry by file name, and which
/// `.gem`/`.gcs` files came with converter notes -- each list capped by
/// [`name_preview`], the full lists going to the log.
fn import_summary(
    path: &Path,
    imported: usize,
    failed: &[String],
    replaced: &[String],
    noted: &[String],
) -> String {
    use std::fmt::Write as _;
    let mut summary = if failed.is_empty() {
        format!("Imported {imported} file(s).")
    } else {
        warn!(
            "Import from {}: {} failed: {:?}",
            path.display(),
            failed.len(),
            failed
        );
        format!(
            "Imported {imported} file(s); {} skipped ({}).",
            failed.len(),
            name_preview(failed, "; ")
        )
    };
    if !replaced.is_empty() {
        // The full list goes to the log; the popup gets a capped preview (a
        // 300-file re-import used to produce a 300-name toast).
        tracing::info!(
            "Import from {}: {} replaced by file name: {:?}",
            path.display(),
            replaced.len(),
            replaced
        );
        // Named right here, since the popup that shows this text
        // (`import_dialog.slint`'s own `result_text` block) is the only place this
        // information is ever shown.
        let _ = write!(
            summary,
            " {} design(s) replaced an existing entry with the same file name ({}) -- \
             filename-only matching, not a content comparison.",
            replaced.len(),
            name_preview(replaced, ", ")
        );
    }
    if !noted.is_empty() {
        let _ = write!(
            summary,
            " Converted with notes: {}.",
            name_preview(noted, " | ")
        );
    }
    summary
}

/// How many file names the import summary lists before it says "and N more".
///
/// The popup that shows the summary is a fixed-size dialog; the complete list
/// always goes to the log (`warn!`/`info!` next to the two call sites).
const SUMMARY_NAME_PREVIEW: usize = 8;

/// Joins at most [`SUMMARY_NAME_PREVIEW`] of `names` with `separator`, appending
/// "and N more" for the rest, so a several-hundred-file batch cannot turn the
/// summary into a wall of file names.
fn name_preview(names: &[String], separator: &str) -> String {
    let shown = names
        .iter()
        .take(SUMMARY_NAME_PREVIEW)
        .cloned()
        .collect::<Vec<_>>();
    let mut text = shown.join(separator);
    let rest = names.len().saturating_sub(SUMMARY_NAME_PREVIEW);
    if rest > 0 {
        use std::fmt::Write as _;
        let _ = write!(text, "{separator}and {rest} more");
    }
    text
}

#[cfg(test)]
mod summary_tests {
    use super::{SUMMARY_NAME_PREVIEW, name_preview};

    #[test]
    fn a_short_list_is_joined_in_full() {
        let names = vec!["a.asc".to_string(), "b.asc".to_string()];
        assert_eq!(name_preview(&names, ", "), "a.asc, b.asc");
    }

    #[test]
    fn a_long_list_is_capped_with_a_count_of_the_rest() {
        let names: Vec<String> = (0..300).map(|i| format!("f{i}.asc")).collect();
        let text = name_preview(&names, ", ");
        assert!(text.starts_with("f0.asc, f1.asc"));
        assert!(text.ends_with(&format!("and {} more", 300 - SUMMARY_NAME_PREVIEW)));
        assert_eq!(text.matches(".asc").count(), SUMMARY_NAME_PREVIEW);
    }
}
