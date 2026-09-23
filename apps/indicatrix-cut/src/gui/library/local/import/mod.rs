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
    settings::SettingsPersister,
};
use glam::DVec3;
use indicatrix::geometry::{GpuFacetPlane, cuts::FacetSpec, girdle, stone_metrics};
use indicatrix_vault::{
    db::sqlite::{DEFAULT_SHAPES, Database},
    local,
    model::{detail::FacetDiagramDetail, entry::FullDiagramRecord},
};
use slint::{ComponentHandle, ModelRc, VecModel, Weak};
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
pub fn apply_measured_metadata(detail: &mut FacetDiagramDetail) {
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

/// `Database::save_diagram_detail` fully REPLACES a design's
/// `diagram_details` row (see that method's own doc comment), so re-importing a
/// `.asc` whose filename collides with an existing row would, without merging, silently
/// wipe every hand-entered field the fresh parse doesn't itself produce -- `designer_info`,
/// a manually corrected `shape`, the competition-entry columns, and any proportion the
/// cutter typed in over `apply_measured_metadata`'s own measurement.
///
/// # The merge rule
///
/// For every field below: the freshly imported/measured value wins when it is
/// present (`Some`/non-empty) -- a re-import is the cutter saying "this file is the
/// current version", so a fresh geometry-derived shape or proportion should win over
/// a stale one. Only when the fresh parse left a field blank does the EXISTING row's
/// value survive. Since `local::import_asc` never populates `designer_info`,
/// `designer`, `source_citation`, `competition_diagram`, `pdf_file`, `gem_file`,
/// `shape_category`, `diagram_image_name`/`diagram_image_data` or `page_url` at all
/// (those are remote-scrape-only or hand-typed fields), this rule always preserves
/// them across a local re-import -- exactly the "never silently discard a cutter's
/// typed metadata" contract this function exists to uphold. `angle_settings_table` and
/// `attached_files` are deliberately NOT touched here: those two are the whole point
/// of a re-import and must always come from the fresh file.
pub fn merge_reimport_metadata(fresh: &mut FacetDiagramDetail, existing: &FullDiagramRecord) {
    if fresh.page_url.is_empty() {
        fresh.page_url.clone_from(&existing.page_url);
    }
    fresh.diagram_image_name = fresh
        .diagram_image_name
        .take()
        .or_else(|| existing.diagram_image_name.clone());
    fresh.diagram_image_data = fresh
        .diagram_image_data
        .take()
        .or_else(|| existing.diagram_image_data.clone());
    fresh.competition_diagram = fresh
        .competition_diagram
        .take()
        .or_else(|| existing.competition_diagram.clone());
    fresh.lw_ratio = fresh.lw_ratio.take().or_else(|| existing.lw_ratio.clone());
    fresh.refractive_index = fresh
        .refractive_index
        .take()
        .or_else(|| existing.refractive_index.clone());
    fresh.index_gear = fresh
        .index_gear
        .take()
        .or_else(|| existing.index_gear.clone());
    fresh.volume = fresh.volume.take().or_else(|| existing.volume.clone());
    fresh.facets_count = fresh
        .facets_count
        .take()
        .or_else(|| existing.facets_count.clone());
    fresh.shape = fresh.shape.take().or_else(|| existing.shape.clone());
    fresh.designer_info = fresh
        .designer_info
        .take()
        .or_else(|| existing.designer_info.clone());
    fresh.hw_ratio = fresh.hw_ratio.take().or_else(|| existing.hw_ratio.clone());
    fresh.tw_ratio = fresh.tw_ratio.take().or_else(|| existing.tw_ratio.clone());
    fresh.uw_ratio = fresh.uw_ratio.take().or_else(|| existing.uw_ratio.clone());
    fresh.pw_ratio = fresh.pw_ratio.take().or_else(|| existing.pw_ratio.clone());
    fresh.cw_ratio = fresh.cw_ratio.take().or_else(|| existing.cw_ratio.clone());
    fresh.symmetry_order = fresh
        .symmetry_order
        .take()
        .or_else(|| existing.symmetry_order.clone());
    fresh.mirror_symmetry = fresh.mirror_symmetry.or(existing.mirror_symmetry);
    fresh.designer = fresh.designer.take().or_else(|| existing.designer.clone());
    fresh.source_citation = fresh
        .source_citation
        .take()
        .or_else(|| existing.source_citation.clone());
    fresh.pdf_file = fresh.pdf_file.take().or_else(|| existing.pdf_file.clone());
    fresh.gem_file = fresh.gem_file.take().or_else(|| existing.gem_file.clone());
    fresh.shape_category = fresh
        .shape_category
        .take()
        .or_else(|| existing.shape_category.clone());
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
    gem_gcs_skipped: &mut usize,
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
        } else if p.is_file() && is_gem_or_gcs(&p) {
            *gem_gcs_skipped += 1;
        } else if recurse && p.is_dir() {
            collect_asc_files_recursive(
                &p,
                recurse,
                depth + 1,
                max_depth,
                visited,
                out,
                gem_gcs_skipped,
            );
        }
    }
}

/// `true` for a `.gem` (`GemCAD` native save) or `.gcs` file -- this import flow
/// only ever reads `.asc` (see this module's own doc comment). Silently skipping a
/// folder's `GemCAD`-native files with no count and no explanation would read as
/// "my files were not there" rather than "this importer does not read that format."
/// [`collect_import_candidates`] counts these (never opens or parses them -- neither
/// reader lives in this crate, see `indicatrix_formats::gem`/`gcs`'s own doc
/// comments), and [`import_path`] turns the count into an honest sentence in the
/// summary a cutter actually sees.
#[must_use]
fn is_gem_or_gcs(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("gem") || e.eq_ignore_ascii_case("gcs"))
}

/// The honest explanation
/// [`collect_import_candidates`] gives for a directly-picked `.gem`/`.gcs` file,
/// and [`import_path`]'s own summary note for however many of them a folder
/// scan skipped -- see [`is_gem_or_gcs`]'s own doc comment for why this
/// importer never reads either format. Named `.gem`/`.gcs` explicitly (never
/// just "unsupported format") and says what each format actually yields
/// elsewhere in this app, so a cutter is not left guessing: `.gem` is read
/// only PARTIALLY, by a different reader entirely (no facet geometry --
/// `indicatrix_formats::gem`'s own module doc comment), and `.gcs` is read-only
/// wherever it is read at all. Neither is available to Import specifically.
const GEM_GCS_EXPLANATION: &str = ".gem is GemCAD's own binary format -- this app reads only \
     partial data from it (no facet geometry) elsewhere, never through Import; .gcs is read-only \
     wherever it is read. Import a .asc export of the same design instead.";

/// Resolves what `import_path` should actually import: the single file `path` names,
/// or every `.asc` directly inside it (plus, when `recurse`, its subfolders; see
/// [`collect_asc_files_recursive`] for the symlink-loop/depth guards).
///
/// Returns the `.asc` candidates alongside how many `.gem`/`.gcs` files a folder
/// scan skipped along the way -- `0` for a single-file pick, which instead rejects a `.gem`/`.gcs` file
/// outright (see [`GEM_GCS_EXPLANATION`]) rather than silently counting one.
///
/// `Err` carries the user-facing message `import_path` returns verbatim -- an
/// unreadable folder, a path that is neither file nor folder, or a folder with no
/// `.asc` in it. Split out purely so `import_path` stays under clippy's
/// `too_many_lines` limit.
fn collect_import_candidates(path: &Path, recurse: bool) -> Result<(Vec<PathBuf>, usize), String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    let mut gem_gcs_skipped = 0usize;
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
                    } else if p.is_file() && is_gem_or_gcs(&p) {
                        gem_gcs_skipped += 1;
                    } else if recurse && p.is_dir() {
                        collect_asc_files_recursive(
                            &p,
                            recurse,
                            1,
                            MAX_RECURSE_DEPTH,
                            &mut visited,
                            &mut candidates,
                            &mut gem_gcs_skipped,
                        );
                    }
                }
            }
            Err(e) => return Err(format!("Could not read folder '{}': {e}", path.display())),
        }
    } else if path.is_file() {
        // A directly picked Indicatrix native sidecar (current
        // `.indicatrix.toml` or legacy `.gemcut.toml`) is not a `.asc` this import
        // path can do anything useful with -- reaching `local::import_asc` with it
        // would come back as an opaque "parse error", with nothing pointing the
        // cutter at the button that actually opens this kind of file. Detected via
        // `indicatrix_cut_core::native::asc_path_for_native`'s own suffix check (a
        // naming guess, not a parse -- see that function's own doc comment) since no
        // file has been read yet at this point.
        if indicatrix_cut_core::native::asc_path_for_native(path).is_some() {
            return Err(format!(
                "'{}' is an Indicatrix native design file, not a .asc -- open it with \"Open \
                 Native\" in the Edit tab instead of Import.",
                path.display()
            ));
        }
        // Same honesty note as the
        // folder-scan case above, for the single-file pick -- see
        // `GEM_GCS_EXPLANATION`'s own doc comment.
        if is_gem_or_gcs(path) {
            return Err(format!(
                "'{}' is not a .asc file. {GEM_GCS_EXPLANATION}",
                path.display()
            ));
        }
        candidates.push(path.to_path_buf());
    } else {
        return Err(format!("'{}' is not a file or folder.", path.display()));
    }

    if candidates.is_empty() {
        return Err(if gem_gcs_skipped > 0 {
            format!(
                "No .asc files found at '{}' ({gem_gcs_skipped} .gem/.gcs file(s) found but not \
                 imported -- {GEM_GCS_EXPLANATION})",
                path.display()
            )
        } else {
            format!("No .asc files found at '{}'.", path.display())
        });
    }
    Ok((candidates, gem_gcs_skipped))
}

/// Looks for a native sidecar sitting beside `asc_path` -- the current
/// `<stem>.indicatrix.toml` suffix first, falling back to the legacy
/// `<stem>.gemcut.toml` suffix -- and reads its bytes when one exists. `None` when
/// neither file is present, which is the ordinary case for a bare `.asc` with no
/// Indicatrix-authored history.
///
/// `Path::set_extension` is used the same way
/// `indicatrix_formats::native::path::native_path_for_asc` builds the current-suffix
/// path (a multi-segment extension like `"indicatrix.toml"` replaces everything
/// after the LAST dot in the file name, giving `stem.indicatrix.toml`, not
/// `stem.asc.indicatrix.toml`).
fn find_native_sidecar(asc_path: &Path) -> Option<(String, Vec<u8>)> {
    let mut current = asc_path.to_path_buf();
    current.set_extension(indicatrix_cut_core::native::NATIVE_EXTENSION_SUFFIX);
    let mut legacy = asc_path.to_path_buf();
    legacy.set_extension(indicatrix_cut_core::native::LEGACY_NATIVE_EXTENSION_SUFFIX);

    [current, legacy].into_iter().find_map(|candidate| {
        let bytes = std::fs::read(&candidate).ok()?;
        let name = candidate.file_name()?.to_string_lossy().into_owned();
        Some((name, bytes))
    })
}

/// [`import_path`]'s return value: the human-readable summary shown in the toast/status
/// line, plus the `diagram_entries.id` of every design this call actually saved (fresh
/// or a filename-collision replacement) -- the post-import preview-generation offer
/// (`gui::batch::preview::offer_batch_confirmation`, called from [`spawn_import`]'s
/// completion closure) needs exactly this list.
struct ImportOutcome {
    summary: String,
    imported_ids: Vec<i64>,
    /// Whether at least one candidate file failed to read, parse
    /// or save. `spawn_import` derives the completion toast's kind from THIS, not
    /// from sniffing the summary text -- a message like "Imported 3 .asc file(s); 12
    /// skipped (...)" would otherwise read as a plain success because it starts with
    /// "Imported" and isn't "Imported 0", even though 12 of the 15 files failed.
    had_failures: bool,
    /// Whether at least one imported file replaced an
    /// existing catalogue row (filename-only dedup). Together with `had_failures`,
    /// this drives `LibraryModel.import_should_stay_open`, which keeps the import
    /// popup open on completion instead of auto-closing.
    had_collision: bool,
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
            had_failures,
            had_collision,
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
            // Derived from the actual failure count, not from
            // sniffing `message`'s text -- see `ImportOutcome::had_failures`'s own
            // doc comment for why sniffing the text would misreport a partly-failed
            // batch as a plain success.
            let toast_kind = if had_failures { "error" } else { "success" };
            show_toast(&ui, &message, toast_kind);
            // Keep the import popup open on completion (instead
            // of auto-closing into the toast, where a failure list becomes
            // unreadable within 3.5s) whenever there is something worth reading --
            // a failure list or a collision warning, both already in `message`/
            // `import_result_text` above -- driving `import_dialog.slint`'s
            // `changed importing` handler.
            ui.global::<LibraryModel>()
                .set_import_should_stay_open(had_failures || had_collision);
            // For more than one imported id, set
            // `recent_import_filter` to exactly those ids BEFORE
            // `refresh_after_library_change` runs, so the very refresh this import
            // triggers already shows only the just-imported rows -- no separate
            // "show these N" click needed, and no race against the async refresh
            // that a later, separate mutation would have (see
            // `gui::library::search::read_id_filter`'s own doc comment for how this
            // property is cleared again the moment the cutter makes any real
            // search/filter change). The single-id case is left alone: the full
            // list stays visible and `invoke_select_diagram` below opens the one
            // new row directly, which is more useful than narrowing the list to a
            // single row.
            let imported_id_items: Vec<i32> = imported_ids
                .iter()
                .filter_map(|&id| i32::try_from(id).ok())
                .collect();
            if imported_id_items.len() > 1 {
                ui.global::<LibraryModel>()
                    .set_recent_import_filter(ModelRc::new(VecModel::from(imported_id_items)));
            } else {
                ui.global::<LibraryModel>()
                    .set_recent_import_filter(ModelRc::new(VecModel::from(Vec::<i32>::new())));
            }
            refresh_after_library_change(&ui, &db, &source);
            // The common case (a single `.asc` picked via "Choose file...")
            // knows exactly which row it just created; without this, the cutter would
            // have to scroll a possibly-large list looking for their own
            // title. `invoke_select_diagram` re-runs the exact same path a click on
            // the row itself takes (`setup_diagram_selection_and_export_callbacks`,
            // `diagram_list.rs`), so the detail pane opens on it immediately.
            if let [only_id] = imported_ids.as_slice()
                && let Ok(id) = i32::try_from(*only_id)
            {
                ui.global::<LibraryModel>().invoke_select_diagram(id);
            }
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
/// file..."/"Choose folder..." buttons).
///
/// Picking a target runs [`count_pending_collisions`] first: with no filename
/// collision against the existing catalogue, [`begin_import`] starts immediately, off
/// the UI thread (see [`spawn_import`]). When at
/// least one candidate WOULD replace an existing row, [`confirm_collisions_if_any`]
/// shows a synchronous native Yes/No dialog ("up-front confirmation before replacing")
/// before anything is imported; declining leaves the
/// catalogue untouched, same as cancelling the picker itself does. Cancelling the
/// native picker does nothing at all: no import, no error, the popup stays open
/// (`pick_file`/`pick_folder` return `None`, and both handlers just fall through).
/// The folder picker also carries a `bool` -- whether to recurse into subfolders --
/// from `import_dialog.slint`'s toggle at the moment the button was clicked.
///
/// Also kicks off [`spawn_save_folder_rescan`] --
/// "once per session" falls out of this function itself only ever being called once,
/// from `gui::build_main_window`, the same way `gui::batch::preview::setup_preview_batch_callbacks`'s
/// own once-per-session missing-previews scan does.
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
    settings_store: &Arc<SettingsPersister>,
) {
    let db_file = Arc::clone(db);
    let source_file = Arc::clone(source);
    let settings_file = Arc::clone(settings_store);
    let ui_weak_file = ui.as_weak();
    ui.global::<LibraryModel>().on_pick_asc_file(move || {
        let Some(ui) = ui_weak_file.upgrade() else {
            return;
        };
        // The picker runs off the UI thread via `gui::pickers::pick` -- only its own
        // continuation (collision pre-scan, confirm dialog, import dispatch)
        // runs after; the import itself runs on its own thread regardless.
        let db_file = Arc::clone(&db_file);
        let source_file = Arc::clone(&source_file);
        let settings_file = Arc::clone(&settings_file);
        let ui_weak_file = ui_weak_file.clone();
        crate::gui::pickers::pick(
            &ui,
            crate::gui::pickers::PickerRequest {
                kind: crate::gui::pickers::PickerKind::OpenFile,
                title: None,
                filters: vec![crate::gui::pickers::PickerFilter {
                    label: ".asc design".to_string(),
                    extensions: vec!["asc".to_string()],
                }],
                default_file_name: None,
                starting_dir: last_import_directory(&settings_file),
            },
            move |ui, path| {
                let Some(path) = path else {
                    return;
                };
                remember_import_directory(&settings_file, path.parent());
                let (collisions, total) = count_pending_collisions(&db_file, &path, false);
                let db_file = Arc::clone(&db_file);
                let source_file = Arc::clone(&source_file);
                confirm_collisions_then(ui, collisions, total, move |ui| {
                    begin_import(ui, &db_file, &source_file, ui_weak_file, path, false);
                });
            },
        );
    });

    let db_folder = Arc::clone(db);
    let source_folder = Arc::clone(source);
    let settings_folder = Arc::clone(settings_store);
    let ui_weak_folder = ui.as_weak();
    // `recurse` is `import_dialog.slint`'s "Include subfolders" toggle, read at the
    // moment "Choose folder..." was clicked -- it must be set before the click since
    // the picker opens (and, absent a collision, the import starts) in this same
    // callback.
    ui.global::<LibraryModel>()
        .on_pick_asc_folder(move |recurse: bool| {
            let Some(ui) = ui_weak_folder.upgrade() else {
                return;
            };
            // Same off-UI-thread picker as `on_pick_asc_file` above.
            let db_folder = Arc::clone(&db_folder);
            let source_folder = Arc::clone(&source_folder);
            let settings_folder = Arc::clone(&settings_folder);
            let ui_weak_folder = ui_weak_folder.clone();
            crate::gui::pickers::pick(
                &ui,
                crate::gui::pickers::PickerRequest {
                    kind: crate::gui::pickers::PickerKind::PickFolder,
                    title: None,
                    filters: Vec::new(),
                    default_file_name: None,
                    starting_dir: last_import_directory(&settings_folder),
                },
                move |ui, path| {
                    let Some(path) = path else {
                        return;
                    };
                    // The chosen folder itself, not its parent: the next import is far
                    // more likely to be another file from inside it than a sibling
                    // folder.
                    remember_import_directory(&settings_folder, Some(path.as_path()));
                    let (collisions, total) = count_pending_collisions(&db_folder, &path, recurse);
                    let db_folder = Arc::clone(&db_folder);
                    let source_folder = Arc::clone(&source_folder);
                    confirm_collisions_then(ui, collisions, total, move |ui| {
                        begin_import(
                            ui,
                            &db_folder,
                            &source_folder,
                            ui_weak_folder,
                            path,
                            recurse,
                        );
                    });
                },
            );
        });

    spawn_save_folder_rescan(ui.as_weak(), Arc::clone(db), Arc::clone(settings_store));
}

/// Filename-only pre-scan of what [`import_path`] would find at
/// `path` -- how many of its candidate `.asc` files already share a name with a row
/// already in `db` (returned first), out of how many candidates total (returned
/// second). Uses the same `local://<file_name>` collision test
/// [`save_imported_design`] itself applies later, since that url is derivable from the
/// file name alone -- no file is read or parsed here, so this costs only the
/// directory listing [`collect_import_candidates`] needs regardless.
///
/// `(0, 0)` for an unreadable path/empty folder: [`import_path`] reports that failure
/// itself once the import actually runs, so this pre-scan does not need to duplicate
/// it.
fn count_pending_collisions(
    db: &Arc<Mutex<Database>>,
    path: &Path,
    recurse: bool,
) -> (usize, usize) {
    let (candidates, _gem_gcs_skipped) =
        collect_import_candidates(path, recurse).unwrap_or_default();
    let db = db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let collisions = candidates
        .iter()
        .filter(|p| {
            let file_name = p.file_name().map_or_else(
                || "unknown.asc".to_string(),
                |n| n.to_string_lossy().into_owned(),
            );
            db.has_detail_for_entry_url(&format!("local://{file_name}"))
                .unwrap_or(false)
        })
        .count();
    (collisions, candidates.len())
}

/// When `collisions` is non-zero, opens the SAME in-window write-confirm dialog
/// `gui::editor::native_io::ask_write_confirm` already gives Save Native's own
/// prompts, naming how many of `total` candidate files would replace an existing
/// catalogue row, and runs `proceed` once the cutter accepts. Declining runs
/// nothing at all -- the catalogue stays untouched, exactly like cancelling the
/// picker itself does. Runs `proceed` immediately (synchronously) when
/// `collisions == 0`, the common case with nothing to ask about.
///
/// Reuses `EditorModel.write_confirm_*` (`ui/models/editor.slint`), a plain global
/// with no notion of which part of the app is asking, so no new Slint callback is
/// needed -- only `native_io::ask_write_confirm` itself becoming reachable from here
/// (see that function's own doc comment).
fn confirm_collisions_then(
    ui: &MainWindow,
    collisions: usize,
    total: usize,
    proceed: impl FnOnce(&MainWindow) + 'static,
) {
    if !collisions_need_confirmation(collisions) {
        proceed(ui);
        return;
    }
    crate::gui::editor::native_io::ask_write_confirm(
        ui,
        "Replace existing design(s)?",
        format!(
            "{collisions} of {total} file(s) have the same name as a design already in \
             the catalogue and will REPLACE it (filename-only match, not a content \
             comparison). Continue?"
        ),
        "Continue",
        // Not suppressible -- unlike
        // `native_io::confirm_keys`'s two prompts (which only ever change whether a
        // header note gets written), silencing this one would let a cutter
        // permanently stop being warned before a catalogue row gets overwritten.
        None,
        proceed,
    );
}

/// [`confirm_collisions_then`]'s pure decision -- whether ANY collision means the
/// dialog is needed at all. Pulled out so a test can call it directly, matching
/// `gui::editor::native_io::decide_write_status`'s own "pure decision, `ui`-touching
/// action kept separate" split -- `confirm_collisions_then` itself is not
/// exercised end-to-end here, since `ask_write_confirm` needs a live
/// `slint::ComponentHandle` (a real `MainWindow`) this crate's test environment
/// cannot start (the same constraint `solve_service`/`edit_intent`'s own test
/// modules document).
#[must_use]
const fn collisions_need_confirmation(collisions: usize) -> bool {
    collisions > 0
}

/// The actual "start importing now" step -- [`setup_import_callback`]'s pickers call
/// this once [`confirm_collisions_if_any`] has cleared.
fn begin_import(
    ui: &MainWindow,
    db: &Arc<Mutex<Database>>,
    source: &Arc<Mutex<LibrarySource>>,
    ui_weak: Weak<MainWindow>,
    path: PathBuf,
    recurse: bool,
) {
    ui.global::<LibraryModel>().set_is_busy(true);
    ui.global::<LibraryModel>()
        .set_status_message("Importing...".into());
    reset_import_progress(ui);
    spawn_import(ui_weak, Arc::clone(db), Arc::clone(source), path, recurse);
}

/// The folder the cutter's most recent native
/// save/open used (`AppSettings::recent_native_files`'s first, most-recent entry),
/// non-recursively. `None` when nothing has been saved/opened natively this app knows
/// of yet, or that file's own path has no parent directory (should not happen for a
/// real saved file, but never assumed).
fn last_save_folder(settings_store: &Arc<SettingsPersister>) -> Option<PathBuf> {
    settings_store
        .snapshot()
        .settings
        .recent_native_files
        .first()
        .map(PathBuf::from)
        .and_then(|p| p.parent().map(Path::to_path_buf))
}

/// The library never rescans the save folder on its own; once per session, this
/// quietly checks [`last_save_folder`] for `.asc` files not yet in
/// the local catalogue at all (the `total - collisions` half of
/// [`count_pending_collisions`]'s own count) and, if it finds any, toasts a pointer at
/// the existing "Choose folder..." picker rather than importing anything itself.
///
/// Deliberately informational, not automatic: this app's own precedent for "something
/// found at startup" (`setup_startup_restore`, `gui::editor::mod`) is an explicit
/// OFFER, never a silent action, for the same "must not surprise the cutter with an
/// unasked-for write" reasoning -- an automatic
/// import here would also have to decide, unasked, what to do about any filename
/// COLLISION it found, which this module treats as something that must never happen
/// silently (see [`confirm_collisions_then`]).
///
/// Runs off the UI thread (the folder listing plus one `has_detail_for_entry_url`
/// point lookup per candidate is cheap, but this still follows
/// `gui::batch::preview::scan::spawn_missing_preview_scan`'s own "never block startup
/// on catalogue I/O" convention) and posts its toast back via
/// `Weak::upgrade_in_event_loop`.
fn spawn_save_folder_rescan(
    ui_weak: Weak<MainWindow>,
    db: Arc<Mutex<Database>>,
    settings_store: Arc<SettingsPersister>,
) {
    thread::spawn(move || {
        let Some(folder) = last_save_folder(&settings_store) else {
            return;
        };
        let (collisions, total) = count_pending_collisions(&db, &folder, false);
        let new_files = total - collisions;
        if new_files == 0 {
            return;
        }
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            show_toast(
                &ui,
                &format!(
                    "{new_files} new .asc file(s) found in your last save folder -- open \
                     Import > Choose folder to bring {} in.",
                    if new_files == 1 { "it" } else { "them" }
                ),
                "info",
            );
        });
    });
}

/// The folder the last import came from, as a
/// [`super::super::super::pickers::PickerRequest::starting_dir`] -- `None`
/// (leaving the OS's own default in place) when nothing has been imported yet,
/// or the remembered folder has since been moved or deleted (checked here
/// rather than handed to `rfd` unchecked, since it silently ignores a missing
/// directory on some platforms and falls back on others).
fn last_import_directory(settings_store: &Arc<SettingsPersister>) -> Option<PathBuf> {
    let remembered = settings_store.snapshot().settings.last_import_directory;
    if remembered.is_empty() {
        return None;
    }
    let path = PathBuf::from(remembered);
    path.is_dir().then_some(path)
}

/// Records where the cutter just imported from, so the next picker opens there.
/// A `None` directory (a path with no parent, which a picked file should never
/// have) leaves the previous value alone rather than clearing it.
fn remember_import_directory(settings_store: &Arc<SettingsPersister>, directory: Option<&Path>) {
    let Some(directory) = directory else {
        return;
    };
    let as_string = directory.to_string_lossy().into_owned();
    settings_store.update(|s| s.settings.last_import_directory.clone_from(&as_string));
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
