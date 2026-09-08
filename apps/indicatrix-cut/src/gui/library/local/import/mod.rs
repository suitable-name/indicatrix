//! Importing `.asc` files into the local database, including optional recursive
//! subfolder import, off the UI thread and panic-isolated per file -- the "Import"
//! side of `gui::library::local` (see this group's own `mod.rs`).
//!
//! Nothing here talks to the network or parses HTML/PDF; it operates only on files
//! the user handed the app directly (`import_path`) or on rows already in the local
//! SQLite catalogue. See `indicatrix_vault::local`'s doc comment for the underlying
//! parse/reconstruct logic this module wires up to the UI.

use super::helpers::refresh_after_library_change;
use crate::{
    LibraryModel, MainWindow,
    bridge::library::source::LibrarySource,
    gui::{library::detail::reconstruct_planes, show_toast},
};
use glam::DVec3;
use indicatrix::geometry::{GpuFacetPlane, cuts::FacetSpec, girdle, stone_metrics};
use indicatrix_vault::{
    db::sqlite::{DEFAULT_SHAPES, Database},
    local,
    model::detail::FacetDiagramDetail,
};
use slint::{ComponentHandle, Weak};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};
use tracing::warn;

/// Fills in the proportion fields `indicatrix_vault::local::import_asc` always leaves
/// `None` -- it only reads the `.asc` file's header/tiers, which don't carry these.
/// Reconstructs the same 3D facet planes the viewport shows ([`reconstruct_planes`],
/// reused from `gui::detail::load_diagram_detail` rather than re-parsed) and measures
/// them with `stone_metrics::measure_solid`, measured at ~0.01% median error on L/W and
/// ~0.09% on Vol/W^3 against the real `.asc` corpus. `measure_solid` returns `None` for
/// a degenerate/unbounded arrangement, and every field below stays `None` rather than
/// fabricated when that happens -- never store a number this couldn't measure.
///
/// Values are written with plain `f64::to_string()`, matching `local::import_asc`'s own
/// convention one call site up. The real `facet_diagrams.sqlite`'s ratio/volume columns
/// are all REAL-affinity, so SQLite normalises whatever numeric text is written here
/// regardless of decimal-place convention -- it only has to parse as a plain number.
fn apply_measured_metadata(detail: &mut FacetDiagramDetail) {
    // `reconstruct_planes` falls back to `standard_round_brilliant()` for no facet
    // specs -- right for the viewport, but measuring that fallback here would
    // attribute a fabricated design's proportions to this one.
    if detail.angle_settings_table.is_empty() {
        return;
    }

    let facet_specs: Vec<FacetSpec> = detail
        .angle_settings_table
        .iter()
        .map(|a| FacetSpec {
            facet: a.facet.clone(),
            angle: a.angle.clone(),
            index: a.index.clone(),
            notes: a.notes.clone(),
        })
        .collect();
    let planes = reconstruct_planes(None, detail.index_gear.as_deref(), &facet_specs);
    let dvec_planes: Vec<(DVec3, f64)> = planes
        .iter()
        .map(|p| {
            (
                DVec3::new(
                    f64::from(p.normal[0]),
                    f64::from(p.normal[1]),
                    f64::from(p.normal[2]),
                ),
                // `measure_solid` wants unit outward normals with `n.x <= m`;
                // `GpuFacetPlane`'s convention is `n.x + d = 0` (see
                // `optics::raytracer::intersect`'s ray-plane test), i.e. `m = -d`.
                -f64::from(p.d),
            )
        })
        .collect();

    let Some(m) = stone_metrics::measure_solid(&dvec_planes) else {
        return;
    };
    let w = m.width_axis;
    if w < 1e-9 {
        return;
    }

    detail.lw_ratio = Some((m.length_axis / w).to_string());
    detail.hw_ratio = Some((m.total_height / w).to_string());
    detail.cw_ratio = m.crown_height.map(|c| (c / w).to_string());
    detail.pw_ratio = m.pavilion_depth.map(|p| (p / w).to_string());
    // Stored dimensionless (`Vol/W^3`, matching a printed diagram sheet), not the raw
    // mast-unit volume.
    detail.volume = Some((m.volume / (w * w * w)).to_string());

    // Classified from the SAME planes, so the girdle outline is measured rather than
    // inferred from the schedule's fold count.
    detail.shape = classify_shape(&planes, m.length_axis / w);
}

/// Assigns [`FacetDiagramDetail::shape`], but only where the design's own girdle
/// outline makes the call unambiguous. Returns `None` otherwise -- a blank shape is
/// correctable by the user, whereas a confidently wrong one looks authoritative and
/// would quietly poison the library's shape filter.
///
/// Classifies by girdle facet count, not schedule fold count: a shape name describes
/// the silhouette, and `classify_girdle_plane_indices` returns every near-vertical
/// plane, so its length is the side count directly. Fold count fails here -- e.g. a
/// "Round Trichecker-12" fixture parses to `symmetry_order = 6` while being a round cut
/// built from six repeats, which a fold-count rule would call a Hexagon; its girdle has
/// far more than six facets, so this rule reads it correctly.
///
/// The rule, with `sides` the girdle facet count and `lw` the measured length/width
/// ratio (>= 1.0): `sides >= ROUND_MIN_SIDES` and `lw <= ROUND_LW_MAX` -> Round; exactly
/// 3/4/6/8 sides -> Triangle/Square/Hexagon/Octagon; anything else -> `None`. Square and
/// Octagon (side count a multiple of 4) measure `lw == 1.0` by construction, so they
/// get the tight `ROUND_LW_MAX`; Triangle and Hexagon carry an inherent
/// `2/sqrt(3) ~= 1.1547`, hence the looser `POLYGON_LW_MAX`. Elongated designs (Oval,
/// Rectangle, Marquise, Pear) are never guessed among -- distinguishing those needs
/// outline curvature this does not measure.
///
/// Looks the chosen name up in [`DEFAULT_SHAPES`] rather than returning the literal, so
/// a rename there can't leave this returning a label the vocabulary no longer
/// recognises.
fn classify_shape(planes: &[GpuFacetPlane], lw: f64) -> Option<String> {
    const ROUND_LW_MAX: f64 = 1.03;
    const POLYGON_LW_MAX: f64 = 1.20;
    /// Below this, a many-sided outline is not confidently "round" -- a 10-sided
    /// outline is as plausibly a decagon as a coarse circle, so it gets no shape.
    const ROUND_MIN_SIDES: usize = 12;

    let sides = girdle::classify_girdle_plane_indices(planes).len();
    let name = match sides {
        3 if lw <= POLYGON_LW_MAX => "Triangle",
        4 if lw <= ROUND_LW_MAX => "Square",
        6 if lw <= POLYGON_LW_MAX => "Hexagon",
        8 if lw <= ROUND_LW_MAX => "Octagon",
        n if n >= ROUND_MIN_SIDES && lw <= ROUND_LW_MAX => "Round",
        _ => return None,
    };
    DEFAULT_SHAPES
        .iter()
        .find(|s| **s == name)
        .map(|s| (*s).to_string())
}

/// Runs `f`, converting a panic into an error message instead of letting it unwind
/// past this call. Wraps the one per-file step (`local::import_asc` +
/// `apply_measured_metadata`) that reaches into `indicatrix`'s geometry code -- a crate
/// this module doesn't own and can't guarantee is panic-free on every
/// malformed-but-parseable `.asc`. Without this, one bad file would kill the whole
/// worker thread, silently dropping every file after it and leaving `is_busy` stuck
/// (see [`spawn_import`]'s doc comment). With this, it's just one more `failed` entry
/// and the batch keeps going.
fn catch_file_panic<T>(f: impl FnOnce() -> T + std::panic::UnwindSafe) -> Result<T, String> {
    std::panic::catch_unwind(f).map_err(|payload| {
        payload
            .downcast_ref::<&str>()
            .map(|s| (*s).to_string())
            .or_else(|| payload.downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "unknown panic".to_string())
    })
}

/// Backstop against a pathological filesystem when [`import_path`]'s optional
/// subfolder recursion is enabled -- a symlink/junction loop is already caught by
/// `collect_asc_files_recursive`'s `visited` set regardless of depth, so this only
/// guards an absurdly deep (but non-cyclic) real tree.
const MAX_RECURSE_DEPTH: usize = 32;

/// Walks one directory for `.asc` files, descending into subdirectories when
/// `recurse` is true. `visited` records the canonicalized (symlink-resolved) path of
/// every directory already entered this call tree -- a symlink or junction looping
/// back to an ancestor canonicalizes to a path already in that set, so the second
/// visit is skipped rather than recursing forever (comparing raw paths wouldn't catch
/// this, since a symlink's own path text never repeats even though its target does).
/// `depth` is capped at [`MAX_RECURSE_DEPTH`] as a second, independent backstop. Both
/// guards fail open (skip and log via `warn!`) -- one bad subfolder shouldn't stop
/// `.asc` files elsewhere in the tree from being found.
fn collect_asc_files_recursive(
    dir: &Path,
    recurse: bool,
    depth: usize,
    max_depth: usize,
    visited: &mut HashSet<PathBuf>,
    out: &mut Vec<PathBuf>,
) {
    if depth > max_depth {
        warn!(
            "Import: not descending into '{}' -- exceeded max recursion depth ({max_depth})",
            dir.display()
        );
        return;
    }
    let canon = std::fs::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !visited.insert(canon) {
        warn!(
            "Import: skipping '{}' -- already visited (symlink loop?)",
            dir.display()
        );
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("asc")) {
            out.push(p);
        } else if recurse && p.is_dir() {
            collect_asc_files_recursive(&p, recurse, depth + 1, max_depth, visited, out);
        }
    }
}

/// Resolves what `import_path` should actually import: the single file `path` names,
/// or every `.asc` directly inside it (plus, when `recurse`, its subfolders; see
/// [`collect_asc_files_recursive`] for the symlink-loop/depth guards).
///
/// `Err` carries the user-facing message `import_path` returns verbatim -- an
/// unreadable folder, a path that is neither file nor folder, or a folder with no
/// `.asc` in it. Split out purely so `import_path` stays under clippy's
/// `too_many_lines` limit.
fn collect_import_candidates(path: &Path, recurse: bool) -> Result<Vec<PathBuf>, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if path.is_dir() {
        match std::fs::read_dir(path) {
            Ok(entries) => {
                // The top-level folder the user picked isn't itself loop-guarded (it
                // can't be reached via a symlink pointing back to itself before this
                // point) -- only directories walked below it are, via `visited`.
                let mut visited: HashSet<PathBuf> = HashSet::new();
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.is_file() && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("asc")) {
                        candidates.push(p);
                    } else if recurse && p.is_dir() {
                        collect_asc_files_recursive(
                            &p,
                            recurse,
                            1,
                            MAX_RECURSE_DEPTH,
                            &mut visited,
                            &mut candidates,
                        );
                    }
                }
            }
            Err(e) => return Err(format!("Could not read folder '{}': {e}", path.display())),
        }
    } else if path.is_file() {
        candidates.push(path.to_path_buf());
    } else {
        return Err(format!("'{}' is not a file or folder.", path.display()));
    }

    if candidates.is_empty() {
        return Err(format!("No .asc files found at '{}'.", path.display()));
    }
    Ok(candidates)
}

/// [`import_path`]'s return value: the human-readable summary shown in the toast/status
/// line, plus the `diagram_entries.id` of every design this call actually saved (fresh
/// or a filename-collision replacement) -- the post-import preview-generation offer
/// (`gui::batch::preview::offer_batch_confirmation`, called from [`spawn_import`]'s
/// completion closure) needs exactly this list.
struct ImportOutcome {
    summary: String,
    imported_ids: Vec<i64>,
}

/// Imports every `.asc` file at `path` (see [`collect_import_candidates`]),
/// reporting progress via `on_progress(done, total)` after each file so a caller can
/// keep the UI honest during a long folder import (always runs off the UI thread --
/// see [`spawn_import`]).
///
/// `db` is locked only around each file's actual database work (the collision check
/// and the two writes), never around the file I/O, `.asc` parsing, or plane
/// reconstruction/`measure_solid` geometry that happens first -- so a UI-thread
/// callback that also needs `db` can only ever block for a single row's write, not
/// the whole import.
fn import_path(
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

        // Parsing and measuring run outside the database lock and inside
        // `catch_file_panic` -- see both functions' doc comments.
        let parse_result = catch_file_panic(std::panic::AssertUnwindSafe(|| {
            local::import_asc(&file_name, &content).map(|mut parsed| {
                // Fills the measured proportions AND shape from the same
                // reconstructed planes, measured once, not twice.
                apply_measured_metadata(&mut parsed.detail);
                parsed
            })
        }));

        let parsed = match parse_result {
            Ok(Ok(parsed)) => parsed,
            Ok(Err(e)) => {
                failed.push(format!("{file_name} (parse error: {e})"));
                continue;
            }
            Err(panic_msg) => {
                warn!("Import panicked while processing '{file_name}': {panic_msg}");
                failed.push(format!("{file_name} (internal error: {panic_msg})"));
                continue;
            }
        };

        let save_result = {
            let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            let is_collision =
                seen_before_in_batch || db.has_detail_for_entry_url(&url).unwrap_or(false);
            db.save_diagram_entry(&parsed.entry, local::LOCAL_SOURCE_ID)
                .and_then(|id| db.save_diagram_detail(&parsed.detail, id).map(|()| id))
                .map(|id| (id, is_collision))
        };
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
        let _ = write!(
            summary,
            " {} design(s) replaced an existing entry of the same name ({}) -- \
             filename-only dedup, see this app's import report.",
            replaced.len(),
            replaced.join(", ")
        );
    }
    ImportOutcome {
        summary,
        imported_ids,
    }
}

/// Runs [`import_path`] on its own worker thread and marshals every UI update back
/// through `upgrade_in_event_loop` -- the same idiom
/// `bridge::export_thread::spawn_export` + `gui::render::render_export` use for a
/// long-running operation with progress. `db`/`source` are cloned `Arc`s, cheap to
/// move onto the thread.
///
/// A `BusyGuard` local to this closure clears `is_busy` in its `Drop` impl,
/// unconditionally, rather than only from the success closure below: that guarantees
/// `is_busy` is cleared even if a panic somewhere in this thread skips past the normal
/// completion path, so the UI can never get stuck busy forever. A `Drop` guard is used
/// instead of `catch_unwind`-ing the whole closure or resetting on every return path
/// because it can't be skipped by a `continue`/early `return`/panic added later --
/// there is exactly one exit path. (`catch_file_panic` inside `import_path` is a
/// separate, complementary fix: it stops a bad file from reaching this point as a
/// panic at all, so the batch keeps processing the rest of the files too.)
fn spawn_import(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    source: Arc<Mutex<LibrarySource>>,
    path: PathBuf,
    recurse: bool,
) {
    thread::spawn(move || {
        struct BusyGuard(Weak<MainWindow>);
        impl Drop for BusyGuard {
            fn drop(&mut self) {
                let ui_weak = self.0.clone();
                let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                    ui.global::<LibraryModel>().set_is_busy(false);
                });
            }
        }
        let _busy_guard = BusyGuard(ui_weak.clone());

        let progress_ui_weak = ui_weak.clone();
        let ImportOutcome {
            summary: message,
            imported_ids,
        } = import_path(&db, &path, recurse, move |done, total| {
            let _ = progress_ui_weak.upgrade_in_event_loop(move |ui| {
                ui.global::<LibraryModel>().set_import_done(done as i32);
                ui.global::<LibraryModel>().set_import_total(total as i32);
                ui.global::<LibraryModel>()
                    .set_import_progress(done as f32 / total as f32);
                ui.global::<LibraryModel>()
                    .set_status_message(format!("Importing {done} / {total}...").into());
            });
        });
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            ui.global::<LibraryModel>()
                .set_import_result_text(message.clone().into());
            ui.global::<LibraryModel>()
                .set_status_message(message.clone().into());
            let toast_kind = if message.starts_with("Imported 0") || !message.contains("Imported") {
                "error"
            } else {
                "success"
            };
            show_toast(&ui, &message, toast_kind);
            refresh_after_library_change(&ui, &db, &source);
            // Asks whether to generate previews for what was just imported -- shares
            // the same confirm-step dialog as the missing-previews library scan; see
            // `preview::offer_batch_confirmation`'s doc comment. A no-op when nothing
            // was actually imported.
            crate::gui::batch::preview::offer_batch_confirmation(&ui, &imported_ids);
        });
        // `_busy_guard` drops here: on a normal return, right after the completion
        // closure above is enqueued (not necessarily run yet), so the `is_busy` reset
        // is queued right behind it; on an unwind, it drops during that unwind instead.
    });
}

/// Wires up the "Import" popup's native pickers (`import_dialog.slint`'s "Choose
/// file..."/"Choose folder..." buttons): picking a target runs the import
/// immediately, off the UI thread (see [`spawn_import`]) -- no separate path field or
/// "Import" button to confirm through. Cancelling the native picker does nothing:
/// no import, no error, the popup stays open (`pick_file`/`pick_folder` return `None`,
/// and both handlers just fall through). The folder picker also carries a `bool` --
/// whether to recurse into subfolders -- from `import_dialog.slint`'s toggle at the
/// moment the button was clicked.
///
/// Always writes to the LOCAL database regardless of which library is currently being
/// browsed -- import is inherently a local-only operation, so it needs no `source`
/// guard the way rename/delete do; `source` is only threaded through so
/// [`super::helpers::refresh_after_library_change`] keeps showing whichever library
/// was already on screen afterwards.
pub fn setup_import_callback(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
) {
    let db_file = Arc::clone(db);
    let source_file = Arc::clone(source);
    let ui_weak_file = ui.as_weak();
    ui.global::<LibraryModel>().on_pick_asc_file(move || {
        let Some(ui) = ui_weak_file.upgrade() else {
            return;
        };
        // Blocking `rfd::FileDialog`, invoked directly on the Slint UI thread (see
        // Cargo.toml's `rfd` dependency comment) -- only the picker itself blocks;
        // the import that follows runs on its own thread.
        let dialog = rfd::FileDialog::new().add_filter(".asc design", &["asc"]);
        let Some(path) = dialog.pick_file() else {
            return;
        };
        ui.global::<LibraryModel>().set_is_busy(true);
        ui.global::<LibraryModel>()
            .set_status_message("Importing...".into());
        reset_import_progress(&ui);
        spawn_import(
            ui_weak_file.clone(),
            Arc::clone(&db_file),
            Arc::clone(&source_file),
            path,
            false,
        );
    });

    let db_folder = Arc::clone(db);
    let source_folder = Arc::clone(source);
    let ui_weak_folder = ui.as_weak();
    // `recurse` is `import_dialog.slint`'s "Include subfolders" toggle, read at the
    // moment "Choose folder..." was clicked -- it must be set before the click since
    // the picker opens and the import starts in this same callback.
    ui.global::<LibraryModel>()
        .on_pick_asc_folder(move |recurse: bool| {
            let Some(ui) = ui_weak_folder.upgrade() else {
                return;
            };
            let dialog = rfd::FileDialog::new();
            let Some(path) = dialog.pick_folder() else {
                return;
            };
            ui.global::<LibraryModel>().set_is_busy(true);
            ui.global::<LibraryModel>()
                .set_status_message("Importing...".into());
            reset_import_progress(&ui);
            spawn_import(
                ui_weak_folder.clone(),
                Arc::clone(&db_folder),
                Arc::clone(&source_folder),
                path,
                recurse,
            );
        });
}

/// Clears the previous import's progress readout before starting a new one -- without
/// this, `import_done`/`import_total`/`import_progress` would keep showing the last
/// import's finished state briefly, before this import's first progress callback fires.
fn reset_import_progress(ui: &MainWindow) {
    ui.global::<LibraryModel>().set_import_done(0);
    ui.global::<LibraryModel>().set_import_total(0);
    ui.global::<LibraryModel>().set_import_progress(0.0);
}

#[cfg(test)]
mod tests;
