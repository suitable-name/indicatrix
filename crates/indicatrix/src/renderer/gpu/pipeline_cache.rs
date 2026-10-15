//! On-disk persistence for the shared [`wgpu::PipelineCache`] (review item GPU-1).
//!
//! Without it every process start recompiles the 5,000-line megakernel once per material
//! class before the first pixel. The driver blob is keyed by
//! [`wgpu::util::pipeline_cache_key`] (Vulkan vendor/device only; every other backend
//! yields no key and therefore no file), validated by wgpu on load (a stale or corrupt
//! blob is discarded by the `fallback: true` descriptor, not by this module), and written
//! atomically (`.tmp` then rename) so a crash mid-write never leaves a half file.
//!
//! The directory is process-global state set once by the application through
//! [`set_pipeline_cache_dir`]; this crate has no cache-directory helper of its own. With
//! no directory set, and always on wasm32, every function here is a no-op. All I/O
//! failures are logged at debug level and ignored: the cache only ever saves time.

use std::{path::PathBuf, sync::OnceLock};

/// The directory holding `<adapter-key>.bin` blobs, set once by the application.
static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Enables pipeline-cache persistence under `dir` (the application passes
/// `<cache_dir>/indicatrix/pipeline_cache`; it is created on first write).
///
/// Call before the first `GpuFrameRenderer::new`. Returns `false` when a directory was
/// already set (the first one wins). Has no effect on wasm32.
pub fn set_pipeline_cache_dir(dir: PathBuf) -> bool {
    CACHE_DIR.set(dir).is_ok()
}

/// The blob path for `info`, or `None` when persistence is off or the backend has no key.
#[cfg(not(target_arch = "wasm32"))]
fn cache_file(info: &wgpu::AdapterInfo) -> Option<PathBuf> {
    let dir = CACHE_DIR.get()?;
    let key = wgpu::util::pipeline_cache_key(info)?;
    Some(dir.join(format!("{key}.bin")))
}

/// The previously persisted blob for this adapter, if any.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_blob(info: &wgpu::AdapterInfo) -> Option<Vec<u8>> {
    let path = cache_file(info)?;
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            tracing::debug!(%error, path = %path.display(), "no pipeline cache blob loaded");
            None
        }
    }
}

/// The previously persisted blob for this adapter: always `None` on wasm32.
#[cfg(target_arch = "wasm32")]
pub(crate) const fn load_blob(_info: &wgpu::AdapterInfo) -> Option<Vec<u8>> {
    None
}

/// Writes `cache`'s current driver blob to disk atomically; failures are only logged.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn persist(cache: &wgpu::PipelineCache, info: &wgpu::AdapterInfo) {
    let Some(path) = cache_file(info) else {
        return;
    };
    let Some(data) = cache.get_data() else {
        return;
    };
    let write = || -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, &data)?;
        std::fs::rename(&tmp, &path)
    };
    if let Err(error) = write() {
        tracing::debug!(%error, path = %path.display(), "pipeline cache not persisted");
    }
}

/// No-op on wasm32.
#[cfg(target_arch = "wasm32")]
pub(crate) const fn persist(_cache: &wgpu::PipelineCache, _info: &wgpu::AdapterInfo) {}
