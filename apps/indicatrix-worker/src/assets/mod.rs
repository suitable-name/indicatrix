//! HDR environment maps over the protocol (v14), server side.
//!
//! A viewer's `SceneState` names an HDR panorama by the SHA-256 of its `.hdr` file
//! (`SceneEnvironment::Hdr`). Before such a request is served, the request loop resolves
//! the map ([`ensure_environment`]): already decoded in this process, else from the
//! on-disk [`AssetCache`], else by asking the viewer (`NEED_ASSET` -> `ASSET`), verifying
//! the bytes' hash, caching them and decoding them with the viewer's own builder
//! (`indicatrix::renderer::env_map::environment_from_hdr_bytes`). The tracer then lights
//! the scene with that map ([`resolved_hdr_map`]/[`environment_source`]), bit-identical
//! to the viewer's.
//!
//! A coordinator resolves the map through its OWN cache before a job starts
//! ([`ensure_held`]: asking the viewer only if the cache lacks it, decoding only when
//! its own lane renders) and answers each joined worker's `NEED_ASSET` from that copy
//! (`crate::coordinator`'s lanes, via [`HeldAsset`]); every joined worker keeps its own
//! cache, so it asks at most once per map.
//!
//! - [`cache`]: the bounded on-disk LRU (hash-named files, atomic writes, verified).
//! - `decoded`: the process-wide decoded-map registry and the tracer's environment.
//! - `fetch`: the `NEED_ASSET`/`ASSET` exchange.
//! - `held`: the map a coordinator holds for one job.
//! - [`policy`]: which requests may be served with an HDR map at all, and what a
//!   coordinator advertises.
//!
//! # Configuration
//!
//! The cache directory is `$INDICATRIX_ASSET_CACHE_DIR` if set, else `asset-cache` next
//! to an anchor path: the served library database for `serve` (the coordinator's data
//! directory; opened with `--render` or a worker port), the certificate directory
//! (`--cert-dir`, default `worker-cert` in the working directory) for `join`. Its size
//! cap is `$INDICATRIX_ASSET_CACHE_MIB` MiB if set, else [`DEFAULT_CACHE_BYTES`] (2 GiB).
//! A cache that cannot be opened disables HDR on that node (a joined worker then
//! advertises `hdr: false` and gets no HDR chunks).

pub mod cache;
mod decoded;
mod fetch;
mod held;
pub mod policy;
#[cfg(test)]
mod tests;

pub use cache::{AssetCache, CacheError, DEFAULT_CACHE_BYTES};
pub use decoded::{decode_and_register, environment_source, lookup, resolved_hdr_map};
pub use fetch::{ASSET_WAIT, Fetched, Pending, discard_asset, ensure_environment, ensure_held};
pub use held::HeldAsset;
pub use policy::{HdrRoute, coordinator_advertises_hdr, hdr_route};

use std::path::{Path, PathBuf};

/// Environment variable overriding the cache directory.
pub const CACHE_DIR_ENV: &str = "INDICATRIX_ASSET_CACHE_DIR";

/// Environment variable overriding the cache size cap, in MiB.
pub const CACHE_MIB_ENV: &str = "INDICATRIX_ASSET_CACHE_MIB";

/// The directory name used next to the anchor path (see the module doc comment) when
/// [`CACHE_DIR_ENV`] is unset.
pub const DEFAULT_DIR_NAME: &str = "asset-cache";

/// The cache directory and byte cap from the environment (see the module doc comment).
///
/// `anchor` is the library database `serve` serves, or `join`'s certificate directory;
/// the default directory is [`DEFAULT_DIR_NAME`] beside it.
#[must_use]
pub fn configured_location(anchor: &Path) -> (PathBuf, u64) {
    let dir = std::env::var_os(CACHE_DIR_ENV).map_or_else(
        || {
            anchor
                .parent()
                .unwrap_or_else(|| Path::new(""))
                .join(DEFAULT_DIR_NAME)
        },
        PathBuf::from,
    );
    let cap = std::env::var(CACHE_MIB_ENV)
        .ok()
        .and_then(|mib| mib.trim().parse::<u64>().ok())
        .map_or(DEFAULT_CACHE_BYTES, |mib| mib.saturating_mul(1024 * 1024));
    (dir, cap)
}

/// Opens the configured cache (see [`configured_location`] for `anchor`), logging where
/// it lives.
///
/// `None` -- HDR scenes then refused, never rendered wrongly -- when the
/// directory cannot be created or listed.
#[must_use]
pub fn open_configured(anchor: &Path) -> Option<std::sync::Arc<AssetCache>> {
    let (dir, cap) = configured_location(anchor);
    match AssetCache::open(&dir, cap) {
        Ok(cache) => {
            tracing::info!(
                "indicatrix-worker: HDR asset cache at {} (cap {} MiB, {} MiB in use)",
                dir.display(),
                cap / (1024 * 1024),
                cache.total_bytes() / (1024 * 1024)
            );
            Some(std::sync::Arc::new(cache))
        }
        Err(e) => {
            tracing::warn!(
                "indicatrix-worker: cannot open the HDR asset cache at {} ({e}); HDR scenes will be \
                 refused (set {CACHE_DIR_ENV} to a writable directory)",
                dir.display()
            );
            None
        }
    }
}
