//! Thumbnails: a small solid raster of each design, cached on disk as raw RGBA.
//!
//! The picture is the Solid view of the design from the default camera (refitted when the
//! stone is much larger or smaller than the default), drawn by the same worker as the
//! viewport. The cache file is `<cache>/thumbs/<id>-<edge>-<stamp>.rgba` with a 12-byte
//! header (`IRTH`, width, height as little-endian `u32`); `stamp` is the entry's
//! `updated_at`, so an edited design draws again. A file that does not match its header is
//! drawn again and replaced. The cache is disposable: the system may clear it.
//!
//! There is no PNG decoder on Android, which is why the cache is raw pixels.

use std::{
    collections::BTreeSet,
    fs,
    io::Write,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::{Arc, atomic::AtomicBool},
};

use indicatrix_solid::preview::{
    CameraPose, DEFAULT_MESH_BOUNDING_RADIUS,
    camera::{RESET_POSE, fit_distance_for_radius},
};
use indicatrix_web_core::solve::{SolveResponse, solve_design, to_solved_tiers};

use super::{
    EntryId, LibraryHandle, Thumbnail, design_source::design_of, handle::db_error,
    import::panic_text,
};
use crate::viewport::{
    ViewMode,
    frame::{FrameKind, RasterFrame, RasterJob, RasterWorker},
};

/// The file header: magic, width, height.
const MAGIC: &[u8; 4] = b"IRTH";
/// The size of the header in bytes.
const HEADER_LEN: usize = 12;
/// The smallest thumbnail edge, in pixels.
const MIN_EDGE: u32 = 32;
/// The largest thumbnail edge, in pixels.
const MAX_EDGE: u32 = 1024;

impl LibraryHandle {
    /// A straight-alpha RGBA8 picture of entry `id`, `edge_px` wide and high (clamped to
    /// 32..=1024), from the cache when it holds one for the current state of the entry.
    ///
    /// Drawing solves the design, which takes tens to hundreds of milliseconds; call it
    /// from a worker thread. The database lock is held only to read the entry.
    ///
    /// # Errors
    /// A sentence when the entry is gone, holds no readable design, or has no closed
    /// solid to draw.
    pub fn thumbnail_rgba(&self, id: EntryId, edge_px: u32) -> Result<Thumbnail, String> {
        let edge = edge_px.clamp(MIN_EDGE, MAX_EDGE);
        let stamp = self
            .lock()
            .entry_updated_at(id)
            .map_err(|_| "That design is no longer in the library.".to_owned())?
            .unwrap_or(0);
        let dir = self.thumbs_dir();
        let name = format!("{id}-{edge}-{stamp}.rgba");
        let path = dir.join(&name);
        if let Some(cached) = fs::read(&path).ok().and_then(|bytes| decode(&bytes)) {
            return Ok(cached);
        }
        let record = self
            .lock()
            .get_diagram_full(id)
            .map_err(db_error("The design could not be read"))?
            .ok_or_else(|| "That design is no longer in the library.".to_owned())?;
        let design = design_of(&record.attached_files)?;
        let thumbnail = match catch_unwind(AssertUnwindSafe(|| draw(design, edge))) {
            Ok(result) => result?,
            Err(payload) => {
                return Err(format!(
                    "The design could not be drawn ({}).",
                    panic_text(payload.as_ref())
                ));
            }
        };
        if let Err(error) = store(&dir, id, edge, &name, &thumbnail) {
            tracing::warn!("Library: could not cache the thumbnail of entry #{id}: {error}");
        }
        Ok(thumbnail)
    }

    /// Deletes every cached thumbnail of entry `id`.
    pub(super) fn remove_thumbs(&self, id: EntryId) {
        remove_matching(&self.thumbs_dir(), &format!("{id}-"), None);
    }
}

/// Draws `design`'s Solid view at `edge` by `edge` pixels.
fn draw(design: indicatrix_cut_core::Design, edge: u32) -> Result<Thumbnail, String> {
    if design.tiers.is_empty() {
        return Err("The design has no facets to draw.".to_owned());
    }
    let cancel = AtomicBool::new(false);
    let tiers = match solve_design(&design, &cancel) {
        SolveResponse::Solved(outcome) => match outcome.solved {
            Some(solved) => to_solved_tiers(&solved),
            None => return Err(outcome.error.unwrap_or(outcome.status_text)),
        },
        SolveResponse::InvalidDesign { message } => return Err(message),
        _ => return Err("The design could not be solved.".to_owned()),
    };
    let mut job = RasterJob {
        token: 1,
        design: Arc::new(design),
        generation: 1,
        solved: Some(tiers),
        chain: None,
        camera: RESET_POSE,
        size: (edge, edge),
        mode: ViewMode::Solid,
        selected_tier: None,
        multi_selected: BTreeSet::new(),
        selected_facet: None,
        provisional_facets: Vec::new(),
        moved_facets: Vec::new(),
        enlarged_panel: -1,
        remembered_facet: None,
        want_handles: false,
        provisional: false,
    };
    let mut worker = RasterWorker::new();
    let mut frame = run_job(&mut worker, &job)?;
    if !frame.has_solid {
        return Err("The design has no closed solid to draw.".to_owned());
    }
    // The default camera frames a standard brilliant; refit a stone of another size.
    let radius = frame.bounding_radius;
    if (radius - DEFAULT_MESH_BOUNDING_RADIUS).abs() > 0.1 * DEFAULT_MESH_BOUNDING_RADIUS {
        job.token += 1;
        job.camera = CameraPose {
            distance: fit_distance_for_radius(radius),
            ..RESET_POSE
        };
        if let Ok(refitted) = run_job(&mut worker, &job) {
            frame = refitted;
        }
    }
    let (width, height) = frame.size;
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4));
    if width == 0 || height == 0 || expected != Some(frame.rgba.len()) {
        return Err("The design could not be drawn.".to_owned());
    }
    Ok(Thumbnail {
        width,
        height,
        rgba: frame.rgba,
    })
}

/// Runs `job` and returns the last frame it produced.
fn run_job(worker: &mut RasterWorker, job: &RasterJob) -> Result<RasterFrame, String> {
    let mut last: Option<RasterFrame> = None;
    worker.run(job, &|| true, &mut |frame| last = Some(frame));
    match last {
        Some(frame) if frame.kind == FrameKind::Image && !frame.rgba.is_empty() => Ok(frame),
        Some(_) => Err("The design is too complex to draw here.".to_owned()),
        None => Err("The design could not be drawn.".to_owned()),
    }
}

/// The cache file's bytes for `thumbnail`.
pub(super) fn encode(thumbnail: &Thumbnail) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(HEADER_LEN + thumbnail.rgba.len());
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&thumbnail.width.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.height.to_le_bytes());
    bytes.extend_from_slice(&thumbnail.rgba);
    bytes
}

/// The thumbnail a cache file holds, `None` when the file is not a whole, well-formed one.
fn decode(bytes: &[u8]) -> Option<Thumbnail> {
    let header = bytes.get(..HEADER_LEN)?;
    if header.get(..4)? != MAGIC {
        return None;
    }
    let width = u32::from_le_bytes(header.get(4..8)?.try_into().ok()?);
    let height = u32::from_le_bytes(header.get(8..12)?.try_into().ok()?);
    let pixels = (width as usize).checked_mul(height as usize)?;
    let expected = pixels.checked_mul(4)?.checked_add(HEADER_LEN)?;
    if width == 0 || height == 0 || bytes.len() != expected {
        return None;
    }
    Some(Thumbnail {
        width,
        height,
        rgba: bytes[HEADER_LEN..].to_vec(),
    })
}

/// Writes the cache file `name` (temp file, then rename) and drops the entry's older
/// files of the same edge.
fn store(
    dir: &Path,
    id: EntryId,
    edge: u32,
    name: &str,
    thumbnail: &Thumbnail,
) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    let temp = dir.join(format!("{name}.tmp"));
    let mut file = fs::File::create(&temp)?;
    file.write_all(&encode(thumbnail))?;
    file.flush()?;
    drop(file);
    fs::rename(&temp, dir.join(name))?;
    remove_matching(dir, &format!("{id}-{edge}-"), Some(name));
    Ok(())
}

/// Deletes the files of `dir` whose names start with `prefix`, except `keep`. A missing
/// folder or an undeletable file is ignored: the cache is disposable.
fn remove_matching(dir: &Path, prefix: &str, keep: Option<&str>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name.starts_with(prefix) && Some(file_name) != keep {
            let _ = fs::remove_file(entry.path());
        }
    }
}
