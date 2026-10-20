//! Importing picked files into the library.
//!
//! The same pipeline as the desktop importer, over bytes instead of paths: classify each
//! file, parse it with the vault's entry points (`.isdd` or legacy `.indicatrix`, `.asc`,
//! `.gem`/`.gcs` converted to `.asc` with the original kept beside it),
//! measure the proportions and shape from the solved planes, and store the row in one
//! transaction. The parse and the measuring run outside the database lock and inside
//! `catch_unwind`: one bad file becomes one `failed` entry, never a dead worker.
//!
//! # Duplicates
//!
//! A file is a duplicate when its bytes were already seen in this call, or when the
//! library already holds an entry with the same `local://<file name>` url whose stored
//! design file has exactly these bytes. The same name with different bytes replaces the
//! entry (the desktop's rule; hand-entered metadata is carried over, the cached previews
//! are dropped) and counts as added.

use std::{
    collections::HashSet,
    panic::{AssertUnwindSafe, catch_unwind},
};

use indicatrix_editor::files::{InputFileKind, convert_foreign_design};
use indicatrix_formats::native::sha256_hex;
use indicatrix_vault::{
    db::sqlite::Database,
    local::{self, ImportedAsc},
    model::{detail::FacetingDiagramDetail, entry::FullDiagramRecord, file::AttachedFile},
};
use indicatrix_web_core::input::MAX_DESIGN_FILE_BYTES;
use tracing::warn;

use super::{
    ImportReport, LibraryHandle,
    design_source::primary_attachment,
    handle::db_error,
    measure::{apply_measured_metadata, fill_designer_info, merge_existing_metadata},
};
use crate::platform::PickedFile;

/// One file to import.
struct Job {
    kind: InputFileKind,
    file: PickedFile,
}

/// What storing one parsed design came to.
enum Stored {
    Added,
    Duplicate,
}

impl LibraryHandle {
    /// Imports the picked files (`.isdd`, legacy `.indicatrix`, `.asc`, `.gem`, `.gcs`).
    /// See the module docs for what a duplicate is. Blocks for as long as the parsing, measuring and storing take.
    #[must_use]
    pub fn import(&self, files: Vec<PickedFile>) -> ImportReport {
        let mut report = ImportReport::default();
        let jobs = plan(files, &mut report.failed);
        let mut seen: HashSet<String> = HashSet::new();
        for job in jobs {
            let name = job.file.display_name.clone();
            if !seen.insert(sha256_hex(&job.file.bytes)) {
                report.duplicates += 1;
                continue;
            }
            let parsed = match catch_unwind(AssertUnwindSafe(|| parse_job(&job))) {
                Ok(Ok(parsed)) => parsed,
                Ok(Err(message)) => {
                    report.failed.push((name, message));
                    continue;
                }
                Err(payload) => {
                    let message = format!(
                        "This file could not be processed ({}).",
                        panic_text(payload.as_ref())
                    );
                    report.failed.push((name, message));
                    continue;
                }
            };
            match self.store_imported(parsed) {
                Ok(Stored::Added) => report.added += 1,
                Ok(Stored::Duplicate) => report.duplicates += 1,
                Err(message) => report.failed.push((name, message)),
            }
        }
        if report.added > 0 {
            self.bump();
        }
        report
    }

    /// Writes one parsed design: a new row, or a replacement of the row with the same
    /// url (a byte-identical file is a duplicate and writes nothing).
    fn store_imported(&self, parsed: ImportedAsc) -> Result<Stored, String> {
        let ImportedAsc {
            entry,
            mut detail,
            derived_from_entry_id,
            extras,
        } = parsed;
        let database = self.lock();
        let existing_id = database
            .diagram_entry_id_for_url(&entry.url)
            .map_err(db_error("The library could not be checked for the file"))?;
        let mut replaced = false;
        if let Some(id) = existing_id {
            let record = database
                .get_diagram_full(id)
                .map_err(db_error("The stored design could not be read"))?;
            if let Some(record) = record {
                if holds_same_file(&detail, &record) {
                    return Ok(Stored::Duplicate);
                }
                merge_existing_metadata(&mut detail, &record);
                replaced = true;
            }
        }
        let id = database
            .save_design(&entry, &detail, local::LOCAL_SOURCE_ID)
            .map_err(db_error("The design could not be stored"))?;
        // Tags, the ignored mark and the planner exclusion a design file carries. Only
        // sets: a re-import never clears what the row already has.
        if let Err(error) = local::apply_imported_extras(&database, id, &extras) {
            warn!("Library import: could not restore the tags and marks of entry #{id}: {error}");
        }
        if replaced {
            invalidate_caches(&database, id);
        } else if let Some(source) = derived_from_entry_id {
            record_provenance(&database, id, source);
        }
        drop(database);
        if replaced {
            self.remove_thumbs(id);
        }
        Ok(Stored::Added)
    }
}

/// Sorts the picked files into import jobs; files that cannot be imported at all are
/// pushed onto `failed` with the reason.
fn plan(files: Vec<PickedFile>, failed: &mut Vec<(String, String)>) -> Vec<Job> {
    const MIB: f64 = 1024.0 * 1024.0;
    let mut designs: Vec<(InputFileKind, PickedFile)> = Vec::new();
    for file in files {
        let name = file.display_name.clone();
        if file.bytes.len() as u64 > MAX_DESIGN_FILE_BYTES {
            failed.push((
                name,
                format!(
                    "This file is {:.1} MiB; design files are limited to {} MiB.",
                    file.bytes.len() as f64 / MIB,
                    MAX_DESIGN_FILE_BYTES / (1024 * 1024)
                ),
            ));
            continue;
        }
        match InputFileKind::classify(&name, &file.bytes) {
            Some(InputFileKind::Hdr) => {
                failed.push((name, "This is an environment map, not a design.".to_owned()));
            }
            Some(kind) => designs.push((kind, file)),
            None => failed.push((
                name,
                "This is not a file type Indicatrix imports (.isdd, .indicatrix, .asc, .gem, .gcs)."
                    .to_owned(),
            )),
        }
    }
    designs
        .into_iter()
        .map(|(kind, file)| Job { kind, file })
        .collect()
}

/// Parses and measures one job.
fn parse_job(job: &Job) -> Result<ImportedAsc, String> {
    let name = job.file.display_name.as_str();
    let bytes = job.file.bytes.as_slice();
    let mut parsed = match job.kind {
        InputFileKind::Design => local::import_native_design(name, bytes)
            .map_err(|error| format!("This design file cannot be read: {error}"))?,
        InputFileKind::Gem | InputFileKind::Gcs => parse_foreign(job)?,
        _ => local::import_asc_bytes(name, bytes, None)
            .map_err(|error| format!("This .asc file cannot be read: {error}"))?,
    };
    apply_measured_metadata(&mut parsed.detail);
    fill_designer_info(&mut parsed.detail);
    Ok(parsed)
}

/// A `.gem` or `.gcs` design: converted to `.asc` cutting instructions, which become the
/// entry's design file, with the original file attached beside them.
fn parse_foreign(job: &Job) -> Result<ImportedAsc, String> {
    let name = &job.file.display_name;
    let converted = convert_foreign_design(name, job.kind, &job.file.bytes)
        .map_err(|error| format!("This file cannot be converted: {error}"))?;
    // `.asc` has no notation for frosting: what a `.gcs` states is stored with the row
    // (`local::apply_imported_extras`) and applied when the entry is read back.
    let mut parsed = local::import_asc(name, &converted.asc_text, None)
        .map_err(|error| format!("The converted schedule cannot be read: {error}"))?
        .with_frosting(&converted.frosting);
    let asc_name = converted.asc_file_name;
    // `import_asc` names its one attachment after the picked file but holds the
    // generated `.asc` text: rename it, and keep the original file beside it.
    if let Some(generated) = parsed.detail.attached_files.first_mut() {
        generated.name.clone_from(&asc_name);
    }
    parsed.detail.attached_files.push(AttachedFile {
        name: name.clone(),
        url: String::new(),
        content: job.file.bytes.clone(),
    });
    if job.kind == InputFileKind::Gem {
        parsed.detail.gem_file = Some(name.clone());
    }
    if parsed.entry.title == *name {
        asc_name
            .trim_end_matches(".asc")
            .clone_into(&mut parsed.entry.title);
    }
    Ok(parsed)
}

/// Whether the stored entry already holds the design file `detail` would store.
fn holds_same_file(detail: &FacetingDiagramDetail, stored: &FullDiagramRecord) -> bool {
    let Some(fresh) = primary_attachment(&detail.attached_files) else {
        return false;
    };
    stored
        .attached_files
        .iter()
        .any(|file| file.name == fresh.name && file.content == fresh.content)
}

/// Deletes the cached previews, tilt curves and solid extents of `id`, which describe
/// the geometry a replacement just overwrote. Logged, not propagated: the import itself
/// succeeded.
pub(super) fn invalidate_caches(database: &Database, id: i64) {
    if let Err(error) = database.delete_preview_images(id) {
        warn!("Library: could not drop the stale previews of entry #{id}: {error}");
    }
    if let Err(error) = database.delete_tilt_curves(id) {
        warn!("Library: could not drop the stale tilt curves of entry #{id}: {error}");
    }
    if let Err(error) = database.delete_solid_extents(id) {
        warn!("Library: could not drop the stale solid extents of entry #{id}: {error}");
    }
}

/// Records `id` as derived from `source`, the entry a footnote inside the imported file
/// names, once that entry is confirmed to still exist.
fn record_provenance(database: &Database, id: i64, source: i64) {
    match database.get_diagram_full(source) {
        Ok(Some(_)) => {
            if let Err(error) = database.set_derived_from_entry_id(id, Some(source)) {
                warn!(
                    "Library import: could not record entry #{id} as derived from #{source}: {error}"
                );
            }
        }
        // The source is gone: leave the provenance unset.
        Ok(None) => {}
        Err(error) => {
            warn!("Library import: could not check source entry #{source}: {error}");
        }
    }
}

/// The text of a caught panic payload.
pub(super) fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|text| (*text).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown error".to_owned())
}
