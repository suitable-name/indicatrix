//! The imported HDR environment map: decoding a Radiance `.hdr` file, the downscaled copy
//! in the app's private files, and the shared map the render scene lights the stone with.
//!
//! # Files
//!
//! Everything lives in `<files>/hdr/`:
//!
//! - `current.envmap`: the map, downscaled to at most 2048 x 1024 texels (see [`file`]'s
//!   format). One map at a time; importing another replaces it.
//! - `selected`: an empty flag file. It exists while "HDR Map" is the chosen lighting
//!   (settings are not ours to extend, so the choice is a sidecar file).
//!
//! # Threads
//!
//! Decoding and downscaling take seconds on a phone, so the front end runs [`prepare`] on a
//! worker thread and hands the result to [`install`] on the UI thread. [`import`] does both
//! in one call for tests and scripts. The stored map is reloaded lazily: [`HdrState::loader`]
//! gives a worker the means to fill the shared slot, and [`HdrState::active`] loads
//! synchronously as a fallback when nothing did.
//!
//! # Memory
//!
//! Files larger than 4096 x 2048 texels are refused before any pixel is decoded: the
//! decoder holds 12 bytes per texel and the sampler another 8, and a phone cannot hold an
//! 8k map next to the renderer.

mod file;
mod scale;

#[cfg(test)]
mod tests;

use std::{
    fmt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use indicatrix::renderer::env_map::{
    EnvMapError, EnvironmentMap, HdrLimits, environment_from_hdr_bytes,
};

use crate::{
    app_state::AppState,
    platform::{AppDirs, PickedFile},
};

pub use scale::{MAX_HEIGHT, MAX_WIDTH};

/// The widest `.hdr` file accepted, in texels.
const DECODE_MAX_WIDTH: u32 = 4096;
/// The tallest `.hdr` file accepted, in texels.
const DECODE_MAX_HEIGHT: u32 = 2048;
/// The map's file name inside `<files>/hdr/`.
const MAP_FILE: &str = "current.envmap";
/// The flag file that marks the HDR map as the chosen lighting.
const SELECTED_FILE: &str = "selected";

/// What a loaded map and its slot hold; shared with the loader thread.
#[derive(Default)]
struct Slot {
    /// The decoded map, once loaded.
    map: Option<Arc<EnvironmentMap>>,
    /// The size recorded in the stored file, known without loading it.
    size: Option<(u32, u32)>,
    /// A load of the stored file was attempted (successfully or not): do not try again.
    loaded: bool,
}

/// What the Light tab shows about the HDR map.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HdrView {
    /// A map is imported (so "Use HDR Map" can be chosen).
    pub available: bool,
    /// "HDR Map" is the chosen lighting and a map exists.
    pub selected: bool,
    /// One line about the map: "2048 x 1024 HDR map", or what to do when there is none.
    pub description: String,
}

/// The current HDR map, if one was imported.
pub struct HdrState {
    /// `<files>/hdr`, known after [`Self::attach`].
    dir: Option<PathBuf>,
    slot: Arc<Mutex<Slot>>,
    selected: bool,
}

impl Default for HdrState {
    fn default() -> Self {
        Self {
            dir: None,
            slot: Arc::new(Mutex::new(Slot::default())),
            selected: false,
        }
    }
}

impl fmt::Debug for HdrState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The map holds millions of texels: print its size, never its contents.
        f.debug_struct("HdrState")
            .field("dir", &self.dir)
            .field("size", &self.size())
            .field("selected", &self.selected)
            .finish_non_exhaustive()
    }
}

/// Everything a worker needs to fill the state's slot from the stored file.
pub struct HdrLoader {
    path: PathBuf,
    slot: Arc<Mutex<Slot>>,
}

impl HdrLoader {
    /// Loads the stored map into the slot unless something loaded it already. Slow: run
    /// on a worker thread.
    pub fn run(self) {
        load_into(&self.slot, &self.path);
    }
}

/// A freshly imported map: written to disk and decoded, waiting to be installed.
pub struct PreparedHdr {
    map: Arc<EnvironmentMap>,
    width: u32,
    height: u32,
}

impl PreparedHdr {
    /// The line the Light tab shows for this map.
    #[must_use]
    pub fn description(&self) -> String {
        describe(self.width, self.height)
    }
}

/// "2048 x 1024 HDR map".
fn describe(width: u32, height: u32) -> String {
    format!("{width} x {height} HDR map")
}

/// The slot, ignoring a poisoned lock (the slot holds plain data).
fn lock(slot: &Mutex<Slot>) -> MutexGuard<'_, Slot> {
    slot.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Builds the sampler from stored texels.
fn build_map(width: u32, height: u32, texels: Vec<[f32; 3]>) -> Result<EnvironmentMap, String> {
    EnvironmentMap::from_rgb(width as usize, height as usize, texels)
        .map_err(|error| format!("The HDR map could not be used: {error}."))
}

/// Reads the stored file and fills the slot (once). A damaged file is ignored with a
/// warning, as if there were none.
fn load_into(slot: &Mutex<Slot>, path: &Path) {
    if lock(slot).loaded {
        return;
    }
    let result = file::read(path)
        .map_err(|error| error.to_string())
        .and_then(|(width, height, texels)| {
            build_map(width, height, texels).map(|map| (width, height, map))
        });
    let mut guard = lock(slot);
    if guard.loaded {
        // Another thread (or an install) got there first.
        return;
    }
    guard.loaded = true;
    match result {
        Ok((width, height, map)) => {
            guard.size = Some((width, height));
            guard.map = Some(Arc::new(map));
        }
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "the stored HDR map is ignored");
            guard.size = None;
        }
    }
}

impl HdrState {
    /// Tells the state where the app's files live and reads what the directory says: the
    /// stored map's size (the header only) and the chosen-lighting flag. Cheap; safe to call
    /// again (it does nothing after the first call).
    pub fn attach(&mut self, dirs: &AppDirs) {
        if self.dir.is_some() {
            return;
        }
        let dir = dirs.files.join("hdr");
        let size = file::read_size(&dir.join(MAP_FILE)).ok();
        lock(&self.slot).size = size;
        self.selected = size.is_some() && dir.join(SELECTED_FILE).is_file();
        self.dir = Some(dir);
    }

    /// The map the render scene lights the stone with: `Some` only when a map is imported
    /// AND "HDR Map" is the chosen lighting; `None` means the lighting preset applies. Loads
    /// the stored file synchronously if no loader has done so yet (see [`Self::loader`]).
    #[must_use]
    pub fn active(&self) -> Option<Arc<EnvironmentMap>> {
        if self.selected { self.map() } else { None }
    }

    /// The imported map whether or not it is the chosen lighting. Loads the stored file
    /// synchronously if nothing loaded it yet.
    #[must_use]
    pub fn map(&self) -> Option<Arc<EnvironmentMap>> {
        {
            let guard = lock(&self.slot);
            if guard.map.is_some() || guard.loaded {
                return guard.map.clone();
            }
        }
        let dir = self.dir.as_ref()?;
        load_into(&self.slot, &dir.join(MAP_FILE));
        lock(&self.slot).map.clone()
    }

    /// A loader for a worker thread, or `None` when there is nothing to load (no map was
    /// imported, or it is loaded already).
    #[must_use]
    pub fn loader(&self) -> Option<HdrLoader> {
        let dir = self.dir.as_ref()?;
        let nothing_to_load = {
            let guard = lock(&self.slot);
            guard.loaded || guard.size.is_none()
        };
        if nothing_to_load {
            return None;
        }
        Some(HdrLoader {
            path: dir.join(MAP_FILE),
            slot: Arc::clone(&self.slot),
        })
    }

    /// A number that changes whenever the lighting map does, without loading anything:
    /// 0 while "HDR Map" is not the chosen lighting, 1 while it is chosen but not loaded
    /// yet, otherwise the loaded map's address. Lets a cache notice a map change cheaply.
    #[must_use]
    pub fn identity(&self) -> usize {
        if !self.is_selected() {
            return 0;
        }
        lock(&self.slot)
            .map
            .as_ref()
            .map_or(1, |map| Arc::as_ptr(map) as usize)
    }

    /// The imported map's size in texels, `None` when there is none.
    #[must_use]
    pub fn size(&self) -> Option<(u32, u32)> {
        lock(&self.slot).size
    }

    /// Whether a map is imported.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.size().is_some()
    }

    /// Whether "HDR Map" is the chosen lighting (and a map exists).
    #[must_use]
    pub fn is_selected(&self) -> bool {
        self.selected && self.is_available()
    }

    /// What the Light tab shows.
    #[must_use]
    pub fn view(&self) -> HdrView {
        let size = self.size();
        HdrView {
            available: size.is_some(),
            selected: self.is_selected(),
            description: size.map_or_else(
                || "Import a Radiance .hdr panorama to light the stone.".to_owned(),
                |(width, height)| describe(width, height),
            ),
        }
    }
}

/// The directory of the stored map, creating it.
fn ensure_dir(state: &mut AppState) -> Result<PathBuf, String> {
    let dirs = state.platform().app_dirs().clone();
    state.hdr.attach(&dirs);
    let dir = dirs.files.join("hdr");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("The HDR folder could not be created: {error}."))?;
    Ok(dir)
}

/// Reads what the app's files say about the HDR map (size and chosen-lighting flag). Call
/// once at start-up; cheap.
pub fn restore(state: &mut AppState) {
    let dirs = state.platform().app_dirs().clone();
    state.hdr.attach(&dirs);
}

/// A sentence for a decoder refusal.
fn decode_message(error: &EnvMapError) -> String {
    match error {
        EnvMapError::TooLarge { .. } => format!(
            "That map is larger than {DECODE_MAX_WIDTH} x {DECODE_MAX_HEIGHT} texels. \
             Pick a smaller one (a 2k or 4k version of the same panorama)."
        ),
        EnvMapError::ZeroSized => "That map is empty.".to_owned(),
        _ => "That file is not a Radiance HDR panorama (.hdr).".to_owned(),
    }
}

/// Decodes `file`, downscales it to at most 2048 x 1024, writes the stored copy in
/// `dirs.files/hdr` and builds the sampler. Slow: run it on a worker thread, then hand the
/// result to [`install`] on the UI thread.
///
/// # Errors
///
/// A sentence for the user: not a Radiance file, too large, completely black, or the copy
/// could not be written.
pub fn prepare(dirs: &AppDirs, file: &PickedFile) -> Result<PreparedHdr, String> {
    let limits = HdrLimits {
        max_width: DECODE_MAX_WIDTH,
        max_height: DECODE_MAX_HEIGHT,
        max_decoded_bytes: u64::from(DECODE_MAX_WIDTH) * u64::from(DECODE_MAX_HEIGHT) * 12,
    };
    let decoded =
        environment_from_hdr_bytes(&file.bytes, limits).map_err(|error| decode_message(&error))?;
    let (width, height) = scale::target_size(decoded.width(), decoded.height());
    let texels = scale::resample_map(&decoded, width, height);
    drop(decoded);
    let mean = scale::mean_radiance(&texels);
    if !mean.iter().all(|channel| channel.is_finite()) || mean.iter().sum::<f64>() <= 0.0 {
        return Err("That map is completely black, so it cannot light the stone.".to_owned());
    }
    let (width, height) = (
        u32::try_from(width).map_err(|_| "That map is too large.".to_owned())?,
        u32::try_from(height).map_err(|_| "That map is too large.".to_owned())?,
    );
    let dir = dirs.files.join("hdr");
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("The HDR folder could not be created: {error}."))?;
    file::write(&dir.join(MAP_FILE), width, height, &texels)?;
    // The active map is always built from the stored texels, so a fresh import and a
    // reload after a restart light the stone identically.
    let map = build_map(width, height, texels)?;
    Ok(PreparedHdr {
        map: Arc::new(map),
        width,
        height,
    })
}

/// Makes a [`prepare`]d map the current one and the chosen lighting. Returns the line to
/// show ("2048 x 1024 HDR map").
pub fn install(state: &mut AppState, prepared: PreparedHdr) -> String {
    let description = prepared.description();
    let dirs = state.platform().app_dirs().clone();
    state.hdr.attach(&dirs);
    {
        let mut slot = lock(&state.hdr.slot);
        slot.map = Some(prepared.map);
        slot.size = Some((prepared.width, prepared.height));
        slot.loaded = true;
    }
    if let Err(message) = set_selected(state, true) {
        tracing::warn!(%message, "the HDR map is imported but could not be chosen");
    }
    description
}

/// Decodes `file` and installs it, in one call on the calling thread. See [`prepare`] and
/// [`install`] for the split the app uses.
///
/// # Errors
///
/// See [`prepare`].
pub fn import(state: &mut AppState, file: &PickedFile) -> Result<String, String> {
    let dirs = state.platform().app_dirs().clone();
    let prepared = prepare(&dirs, file)?;
    Ok(install(state, prepared))
}

/// Forgets the imported map: deletes the stored copy and the chosen-lighting flag.
pub fn clear(state: &mut AppState) {
    let dirs = state.platform().app_dirs().clone();
    state.hdr.attach(&dirs);
    let dir = dirs.files.join("hdr");
    for name in [MAP_FILE, SELECTED_FILE] {
        if let Err(error) = std::fs::remove_file(dir.join(name))
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, name, "an HDR file could not be removed");
        }
    }
    {
        let mut slot = lock(&state.hdr.slot);
        slot.map = None;
        slot.size = None;
        // Nothing left to load; a later import sets the slot itself.
        slot.loaded = true;
    }
    state.hdr.selected = false;
}

/// Chooses (or stops choosing) "HDR Map" as the lighting.
///
/// # Errors
///
/// A sentence when `on` is true and no map is imported, or the flag file cannot be written.
pub fn set_selected(state: &mut AppState, on: bool) -> Result<(), String> {
    if on && !state.hdr.is_available() {
        return Err("Import an HDR map first.".to_owned());
    }
    if state.hdr.selected == on {
        return Ok(());
    }
    let dir = ensure_dir(state)?;
    let flag = dir.join(SELECTED_FILE);
    if on {
        std::fs::write(&flag, b"")
            .map_err(|error| format!("The choice could not be saved: {error}."))?;
    } else if let Err(error) = std::fs::remove_file(&flag)
        && error.kind() != std::io::ErrorKind::NotFound
    {
        return Err(format!("The choice could not be saved: {error}."));
    }
    state.hdr.selected = on;
    Ok(())
}
