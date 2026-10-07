//! HDR environment maps as protocol assets (v14), viewer side.
//!
//! Every HDR map the viewer loads goes through [`load_hdr_file`]: it reads the file,
//! hashes the exact bytes (SHA-256, `indicatrix_net::messages::content_hash`), decodes
//! them with the one shared builder (`indicatrix::renderer::env_map::
//! environment_from_hdr_bytes`, the same function a render worker uses on the bytes it
//! is sent), and registers an [`HdrAsset`] for the decoded map. From then on:
//!
//! - [`scene_environment`] names the map in an outgoing `SceneState` by hash;
//! - when a server answers a request with `NEED_ASSET`, the connection thread finds the
//!   asset by hash ([`asset_by_hash`]) and sends its bytes ([`HdrAsset::upload_bytes`]):
//!   kept in memory up to [`KEEP_IN_MEMORY_MAX`], otherwise re-read from the file and
//!   re-verified -- a file that changed since it was loaded fails the request clearly
//!   rather than sending a different map under the old hash.
//!
//! The registry maps a decoded map (by `Arc` identity) to its asset, so the render
//! context, scene snapshots and export paths keep passing the plain
//! `Arc<EnvironmentMap>` they always did.

use indicatrix::renderer::env_map::{EnvironmentMap, HdrLimits, environment_from_hdr_bytes};
use indicatrix_net::{
    messages::{ContentHash, MAX_ASSET_LEN, content_hash, hash_hex},
    scene::{HdrEnvironment, SceneEnvironment},
};
use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, PoisonError, Weak},
};

/// Files up to this size keep their bytes in memory for uploads (64 MiB); larger ones
/// are re-read (and re-verified) when a server asks.
pub const KEEP_IN_MEMORY_MAX: usize = 64 * 1024 * 1024;

/// One loaded HDR map's identity as an asset: its hash, size and where its bytes are.
#[derive(Debug)]
pub struct HdrAsset {
    content_hash: ContentHash,
    width: u32,
    height: u32,
    path: PathBuf,
    len: usize,
    bytes: Option<Arc<[u8]>>,
}

impl HdrAsset {
    /// The SHA-256 of the file's bytes.
    #[must_use]
    pub const fn content_hash(&self) -> &ContentHash {
        &self.content_hash
    }

    /// The file this map was loaded from.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The `SceneState` environment naming this map.
    #[must_use]
    pub const fn environment(&self) -> HdrEnvironment {
        HdrEnvironment {
            content_hash: self.content_hash,
            width: self.width,
            height: self.height,
        }
    }

    /// Whether the protocol can carry this map at all (`MAX_ASSET_LEN`).
    #[must_use]
    pub fn transferable(&self) -> bool {
        u32::try_from(self.len).is_ok_and(|len| len <= MAX_ASSET_LEN)
    }

    /// The exact bytes this asset was loaded from, for an `ASSET` upload: from memory,
    /// else re-read from the file and verified against the hash.
    ///
    /// # Errors
    ///
    /// A human-readable reason when the file cannot be read, or no longer has the
    /// content it was loaded with (edited or replaced since).
    pub fn upload_bytes(&self) -> Result<Arc<[u8]>, String> {
        if let Some(bytes) = &self.bytes {
            return Ok(Arc::clone(bytes));
        }
        let bytes = std::fs::read(&self.path)
            .map_err(|e| format!("cannot re-read the HDR map {}: {e}", self.path.display()))?;
        if content_hash(&bytes) != self.content_hash {
            return Err(format!(
                "the HDR map {} changed on disk since it was loaded; reload it to render remotely",
                self.path.display()
            ));
        }
        Ok(Arc::from(bytes))
    }
}

/// Every loaded map with its asset. A dead map's entry is pruned on the next load.
static REGISTRY: Mutex<Vec<(Weak<EnvironmentMap>, Arc<HdrAsset>)>> = Mutex::new(Vec::new());

/// Reads, hashes and decodes the `.hdr` file at `path` (see the module doc comment) and
/// registers its asset.
///
/// # Errors
///
/// A human-readable message when the file cannot be read or does not decode (malformed,
/// or over `HdrLimits::DEFAULT`).
pub fn load_hdr_file(path: &Path) -> Result<(Arc<EnvironmentMap>, Arc<HdrAsset>), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("failed to decode HDR image: {e}"))?;
    let map = Arc::new(
        environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).map_err(|e| e.to_string())?,
    );
    let asset = Arc::new(HdrAsset {
        content_hash: content_hash(&bytes),
        width: map.width() as u32,
        height: map.height() as u32,
        path: path.to_path_buf(),
        len: bytes.len(),
        bytes: (bytes.len() <= KEEP_IN_MEMORY_MAX).then(|| Arc::from(bytes)),
    });
    tracing::debug!(
        "HDR map {} is asset {}",
        path.display(),
        hash_hex(asset.content_hash())
    );
    register(&map, Arc::clone(&asset));
    Ok((map, asset))
}

/// Records `asset` as the source of `map`, pruning entries whose map is gone.
fn register(map: &Arc<EnvironmentMap>, asset: Arc<HdrAsset>) {
    let mut registry = REGISTRY.lock().unwrap_or_else(PoisonError::into_inner);
    registry.retain(|(weak, _)| weak.strong_count() > 0);
    registry.push((Arc::downgrade(map), asset));
}

/// The asset `map` was loaded from, if it was loaded by [`load_hdr_file`] (a synthetic
/// map built in memory has none and cannot be sent to a server).
#[must_use]
pub fn asset_for(map: &Arc<EnvironmentMap>) -> Option<Arc<HdrAsset>> {
    let target = Arc::downgrade(map);
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .find(|(weak, _)| weak.ptr_eq(&target))
        .map(|(_, asset)| Arc::clone(asset))
}

/// The asset with `hash` among the loaded maps still alive -- what the connection
/// thread uploads when a server sends `NEED_ASSET`.
#[must_use]
pub fn asset_by_hash(hash: &ContentHash) -> Option<Arc<HdrAsset>> {
    REGISTRY
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|(weak, asset)| weak.strong_count() > 0 && asset.content_hash() == hash)
        .map(|(_, asset)| Arc::clone(asset))
}

/// The environment an outgoing `SceneState` carries for a scene lit by `env_map`
/// (`None` = the studio rig).
///
/// A map with no asset (built in memory, never loaded from a file) is named by an
/// all-zero hash no server can resolve: such a scene must be refused before dispatch
/// (`super::guard::remote_can_render`), and if it ever reaches a server the request
/// fails with `ASSET_FAILED` -- never renders with the studio rig in its place.
#[must_use]
pub fn scene_environment(env_map: Option<&Arc<EnvironmentMap>>) -> SceneEnvironment {
    let Some(map) = env_map else {
        return SceneEnvironment::Studio;
    };
    SceneEnvironment::Hdr(asset_for(map).map_or_else(
        || HdrEnvironment {
            content_hash: [0; 32],
            width: map.width() as u32,
            height: map.height() as u32,
        },
        |asset| asset.environment(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_temp_hdr(name: &str, salt: f32) -> PathBuf {
        let pixels: Vec<image::Rgb<f32>> = (0..64)
            .map(|i| image::Rgb([(i as f32).mul_add(0.01, salt), 0.5, 0.25]))
            .collect();
        let mut bytes = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut bytes)
            .encode(&pixels, 16, 4)
            .unwrap();
        let path = std::env::temp_dir().join(format!(
            "indicatrix-cut-hdr-asset-{name}-{}.hdr",
            std::process::id()
        ));
        std::fs::write(&path, &bytes).unwrap();
        path
    }

    #[test]
    fn a_loaded_map_is_named_by_its_file_hash_and_found_by_it() {
        let path = write_temp_hdr("named", 0.1);
        let (map, asset) = load_hdr_file(&path).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(asset.content_hash(), &content_hash(&bytes));
        let SceneEnvironment::Hdr(env) = scene_environment(Some(&map)) else {
            panic!("an HDR map must be named as one");
        };
        assert_eq!(env, asset.environment());
        assert_eq!((env.width, env.height), (16, 4));
        assert!(Arc::ptr_eq(
            &asset_by_hash(&env.content_hash).unwrap(),
            &asset
        ));
        assert_eq!(&*asset.upload_bytes().unwrap(), bytes.as_slice());
        assert!(asset.transferable());
        assert_eq!(scene_environment(None), SceneEnvironment::Studio);
        let _ = std::fs::remove_file(&path);
    }

    /// A large file's bytes are re-read for the upload; if the file changed since it
    /// was loaded, the upload fails clearly instead of sending other bytes.
    #[test]
    fn a_changed_file_fails_the_upload_instead_of_sending_other_bytes() {
        let path = write_temp_hdr("changed", 0.2);
        let (_map, asset) = load_hdr_file(&path).unwrap();
        let reread = HdrAsset {
            content_hash: asset.content_hash,
            width: asset.width,
            height: asset.height,
            path: asset.path.clone(),
            len: asset.len,
            bytes: None,
        };
        assert!(reread.upload_bytes().is_ok());
        std::fs::write(&path, b"#?RADIANCE\nsomething else").unwrap();
        let err = reread.upload_bytes().unwrap_err();
        assert!(err.contains("changed on disk"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    /// A map built in memory has no asset and is named by the unresolvable zero hash.
    #[test]
    fn a_synthetic_map_has_no_asset() {
        let map = Arc::new(EnvironmentMap::uniform(4, 2, [1.0, 1.0, 1.0]));
        assert!(asset_for(&map).is_none());
        let SceneEnvironment::Hdr(env) = scene_environment(Some(&map)) else {
            panic!("still an HDR scene");
        };
        assert_eq!(env.content_hash, [0; 32]);
    }
}
