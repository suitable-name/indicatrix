//! [`AssetCache`]: the bounded on-disk LRU of asset bytes, keyed by content hash.
//!
//! - **Naming.** An asset lives at `<dir>/<64 lower-case hex digits>.hdr`, the name
//!   derived only from its SHA-256 ([`indicatrix_net::messages::hash_hex`]). A peer never
//!   supplies a path or a name, so nothing is ever written outside `dir`.
//! - **Integrity.** [`AssetCache::put`] refuses bytes whose SHA-256 is not the key; a
//!   file read back by [`AssetCache::get`] is re-hashed, and a corrupt one is deleted
//!   and reported as absent (the viewer is simply asked again).
//! - **Atomicity.** Bytes are written to a unique `*.tmp` file in `dir`, flushed, then
//!   renamed onto the final name, so a crash never leaves a truncated `*.hdr` behind.
//!   Leftover `*.tmp` files are removed when the cache is opened.
//! - **Bound.** The total size of the cached files stays at or below the byte cap: a
//!   `put` evicts least-recently-used entries (never the one being inserted) until the
//!   new one fits; an asset larger than the whole cap is refused.
//! - **Held.** The cache also remembers every hash whose verified bytes passed through
//!   it in this process ([`AssetCache::has_held`]), evicted or not. The decoded-map
//!   registry is process-wide; this is what scopes its fast path to the node (the
//!   cache) that actually obtained the asset, so several nodes sharing one process
//!   (tests: a coordinator and its joined workers) each follow the protocol.

use indicatrix_net::messages::{ContentHash, content_hash, hash_hex};
use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

/// The default byte cap of the cache (2 GiB).
pub const DEFAULT_CACHE_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The file-name extension of a cached asset.
const ASSET_EXTENSION: &str = "hdr";

/// The file-name extension of an in-progress write.
const TEMP_EXTENSION: &str = "tmp";

/// Why [`AssetCache::put`] refused or failed.
#[derive(Debug)]
pub enum CacheError {
    /// The bytes' SHA-256 is not the key they were offered under.
    HashMismatch,
    /// The asset alone is larger than the cache's whole byte cap.
    TooLarge {
        /// The asset's size.
        len: u64,
        /// The cache's cap.
        cap: u64,
    },
    /// Writing or renaming the file failed.
    Io(std::io::Error),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::HashMismatch => write!(f, "asset bytes do not match their SHA-256 key"),
            Self::TooLarge { len, cap } => {
                write!(
                    f,
                    "asset of {len} bytes exceeds the cache cap of {cap} bytes"
                )
            }
            Self::Io(e) => write!(f, "asset cache I/O error: {e}"),
        }
    }
}

impl std::error::Error for CacheError {}

/// One cached file's bookkeeping.
#[derive(Debug, Clone, Copy)]
struct Entry {
    /// File size in bytes.
    len: u64,
    /// Recency stamp: larger is more recently used.
    used: u64,
}

/// Every entry plus the running total, under one lock.
#[derive(Debug, Default)]
struct Index {
    entries: HashMap<ContentHash, Entry>,
    total: u64,
    clock: u64,
}

impl Index {
    const fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    /// Marks `hash` most recently used; `false` if it is not indexed.
    fn touch(&mut self, hash: &ContentHash) -> bool {
        let used = self.tick();
        if let Some(entry) = self.entries.get_mut(hash) {
            entry.used = used;
            return true;
        }
        false
    }

    /// Indexes `hash` (`len` bytes) as most recently used.
    fn insert(&mut self, hash: ContentHash, len: u64) {
        let used = self.tick();
        if let Some(previous) = self.entries.insert(hash, Entry { len, used }) {
            self.total -= previous.len;
        }
        self.total += len;
    }

    /// Forgets `hash`.
    fn remove(&mut self, hash: &ContentHash) {
        if let Some(entry) = self.entries.remove(hash) {
            self.total -= entry.len;
        }
    }
}

/// The bounded on-disk LRU of asset bytes -- see the module doc comment.
#[derive(Debug)]
pub struct AssetCache {
    dir: PathBuf,
    cap_bytes: u64,
    index: Mutex<Index>,
    /// Every hash whose verified bytes this cache stored or returned (see the module
    /// doc comment's "Held").
    held: Mutex<HashSet<ContentHash>>,
}

/// Distinguishes concurrent temp files of one process.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

impl AssetCache {
    /// Opens (creating it if needed) the cache in `dir` with a total byte cap of
    /// `cap_bytes`, indexing the `*.hdr` files already there (oldest modification first
    /// in LRU order), deleting leftover `*.tmp` files and anything over the cap.
    ///
    /// # Errors
    ///
    /// The I/O error when `dir` cannot be created or listed.
    pub fn open(dir: &Path, cap_bytes: u64) -> std::io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut found: Vec<(ContentHash, u64, std::time::SystemTime)> = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            let extension = path.extension().and_then(|e| e.to_str());
            if extension == Some(TEMP_EXTENSION) {
                let _ = fs::remove_file(&path);
                continue;
            }
            let Some(hash) = parse_asset_name(&path) else {
                continue;
            };
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_file() {
                let modified = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
                found.push((hash, meta.len(), modified));
            }
        }
        found.sort_by_key(|(_, _, modified)| *modified);
        let cache = Self {
            dir: dir.to_path_buf(),
            cap_bytes,
            index: Mutex::new(Index::default()),
            held: Mutex::new(HashSet::new()),
        };
        cache.with_index(|index| {
            for (hash, len, _) in found {
                index.insert(hash, len);
            }
            cache.evict_to_fit(index, None);
        });
        Ok(cache)
    }

    /// The directory this cache stores its files in.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The total byte cap.
    #[must_use]
    pub const fn cap_bytes(&self) -> u64 {
        self.cap_bytes
    }

    /// The bytes currently cached, in total.
    #[must_use]
    pub fn total_bytes(&self) -> u64 {
        self.lock().total
    }

    /// Whether `hash` is cached (without touching its recency).
    #[must_use]
    pub fn contains(&self, hash: &ContentHash) -> bool {
        self.lock().entries.contains_key(hash)
    }

    /// Whether verified bytes of `hash` passed through this cache in this process
    /// ([`Self::put`] accepted their hash, or [`Self::get`] returned them) -- even if the
    /// file has since been evicted or could not be stored.
    #[must_use]
    pub fn has_held(&self, hash: &ContentHash) -> bool {
        self.lock_held().contains(hash)
    }

    fn note_held(&self, hash: &ContentHash) {
        self.lock_held().insert(*hash);
    }

    fn lock_held(&self) -> std::sync::MutexGuard<'_, HashSet<ContentHash>> {
        self.held.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The path an asset with `hash` is stored at -- always directly inside
    /// [`Self::dir`], named only by the hash's hex digits.
    #[must_use]
    pub fn path_for(&self, hash: &ContentHash) -> PathBuf {
        self.dir
            .join(format!("{}.{ASSET_EXTENSION}", hash_hex(hash)))
    }

    /// The cached bytes of `hash`, marking it most recently used; `None` if absent,
    /// unreadable, or corrupt (a file whose SHA-256 no longer matches is deleted).
    #[must_use]
    pub fn get(&self, hash: &ContentHash) -> Option<Vec<u8>> {
        if !self.with_index(|index| index.touch(hash)) {
            return None;
        }
        let path = self.path_for(hash);
        let bytes = fs::read(&path).ok();
        if let Some(bytes) = bytes.filter(|b| content_hash(b) == *hash) {
            self.note_held(hash);
            return Some(bytes);
        }
        tracing::warn!(
            "asset cache: {} is unreadable or corrupt; dropping it",
            path.display()
        );
        let _ = fs::remove_file(&path);
        self.with_index(|index| index.remove(hash));
        None
    }

    /// Stores `bytes` under `hash` (atomically, see the module doc comment), then evicts
    /// least-recently-used entries until the total fits the cap. Storing an asset that
    /// is already cached only refreshes its recency.
    ///
    /// # Errors
    ///
    /// [`CacheError::HashMismatch`] when `bytes` do not hash to `hash`;
    /// [`CacheError::TooLarge`] when `bytes` alone exceed the cap; [`CacheError::Io`]
    /// when writing fails (nothing partial is left behind).
    pub fn put(&self, hash: &ContentHash, bytes: &[u8]) -> Result<(), CacheError> {
        if content_hash(bytes) != *hash {
            return Err(CacheError::HashMismatch);
        }
        self.note_held(hash);
        let len = bytes.len() as u64;
        if len > self.cap_bytes {
            return Err(CacheError::TooLarge {
                len,
                cap: self.cap_bytes,
            });
        }
        if self.path_for(hash).is_file() && self.with_index(|index| index.touch(hash)) {
            return Ok(());
        }
        self.write_atomically(hash, bytes).map_err(CacheError::Io)?;
        self.with_index(|index| {
            index.insert(*hash, len);
            self.evict_to_fit(index, Some(hash));
        });
        Ok(())
    }

    /// Runs `f` on the index under its lock.
    fn with_index<R>(&self, f: impl FnOnce(&mut Index) -> R) -> R {
        f(&mut self.lock())
    }

    /// Writes `bytes` to a unique temp file in [`Self::dir`], flushes it to disk, and
    /// renames it onto the asset's final path ([`Self::path_for`]).
    fn write_atomically(&self, hash: &ContentHash, bytes: &[u8]) -> std::io::Result<()> {
        let temp = self.dir.join(format!(
            "{}.{}-{}.{TEMP_EXTENSION}",
            hash_hex(hash),
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let written =
            write_synced(&temp, bytes).and_then(|()| fs::rename(&temp, self.path_for(hash)));
        if written.is_err() {
            let _ = fs::remove_file(&temp);
        }
        written
    }

    /// Evicts least-recently-used entries (never `keep`) until `index.total` fits the
    /// cap, deleting their files.
    fn evict_to_fit(&self, index: &mut Index, keep: Option<&ContentHash>) {
        while index.total > self.cap_bytes {
            let victim = index
                .entries
                .iter()
                .filter(|(hash, _)| Some(*hash) != keep)
                .min_by_key(|(_, entry)| entry.used)
                .map(|(hash, _)| *hash);
            let Some(victim) = victim else {
                break;
            };
            if let Some(entry) = index.entries.remove(&victim) {
                index.total -= entry.len;
            }
            let path = self.path_for(&victim);
            if let Err(e) = fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!("asset cache: could not evict {}: {e}", path.display());
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Index> {
        self.index.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Creates `path`, writes `bytes` and flushes them to disk.
fn write_synced(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// The hash a cached file's name encodes, if `path` is named `<64 hex digits>.hdr`.
fn parse_asset_name(path: &Path) -> Option<ContentHash> {
    if path.extension().and_then(|e| e.to_str()) != Some(ASSET_EXTENSION) {
        return None;
    }
    let stem = path.file_stem()?.to_str()?;
    if stem.len() != 64 || !stem.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let mut hash = [0u8; 32];
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&stem[2 * i..2 * i + 2], 16).ok()?;
    }
    Some(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, empty directory under the system temp dir, unique to this test.
    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-asset-cache-{name}-{}-{}",
            std::process::id(),
            TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn blob(tag: u8, len: usize) -> (ContentHash, Vec<u8>) {
        let bytes = vec![tag; len];
        (content_hash(&bytes), bytes)
    }

    #[test]
    fn put_then_get_round_trips_and_names_the_file_by_hex_hash_inside_the_dir() {
        let dir = temp_dir("roundtrip");
        let cache = AssetCache::open(&dir, 1024).unwrap();
        let (hash, bytes) = blob(1, 100);
        cache.put(&hash, &bytes).unwrap();
        assert_eq!(cache.get(&hash).unwrap(), bytes);
        let path = cache.path_for(&hash);
        assert_eq!(path.parent(), Some(dir.as_path()));
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("{}.hdr", hash_hex(&hash))
        );
        assert_eq!(parse_asset_name(&path), Some(hash));
        // Nothing but the one asset file (no temp leftovers).
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn bytes_offered_under_the_wrong_hash_are_refused_and_not_stored() {
        let dir = temp_dir("mismatch");
        let cache = AssetCache::open(&dir, 1024).unwrap();
        let (hash, _) = blob(1, 10);
        assert!(matches!(
            cache.put(&hash, b"other bytes"),
            Err(CacheError::HashMismatch)
        ));
        assert!(!cache.contains(&hash));
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_least_recently_used_entry_is_evicted_to_stay_under_the_cap() {
        let dir = temp_dir("lru");
        let cache = AssetCache::open(&dir, 250).unwrap();
        let (a, a_bytes) = blob(1, 100);
        let (b, b_bytes) = blob(2, 100);
        let (c, c_bytes) = blob(3, 100);
        cache.put(&a, &a_bytes).unwrap();
        cache.put(&b, &b_bytes).unwrap();
        // Touch `a`, so `b` is now the least recently used.
        assert!(cache.get(&a).is_some());
        cache.put(&c, &c_bytes).unwrap();
        assert!(cache.contains(&a) && cache.contains(&c));
        assert!(!cache.contains(&b));
        assert!(!cache.path_for(&b).exists());
        assert!(cache.total_bytes() <= cache.cap_bytes());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_asset_larger_than_the_whole_cap_is_refused() {
        let dir = temp_dir("cap");
        let cache = AssetCache::open(&dir, 50).unwrap();
        let (hash, bytes) = blob(4, 51);
        assert!(matches!(
            cache.put(&hash, &bytes),
            Err(CacheError::TooLarge { len: 51, cap: 50 })
        ));
        assert_eq!(cache.total_bytes(), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn reopening_indexes_existing_files_ignores_foreign_names_and_drops_temp_files() {
        let dir = temp_dir("reopen");
        let (hash, bytes) = blob(5, 40);
        {
            let cache = AssetCache::open(&dir, 1024).unwrap();
            cache.put(&hash, &bytes).unwrap();
        }
        fs::write(dir.join("notes.txt"), b"not an asset").unwrap();
        fs::write(dir.join("abc.hdr"), b"short name").unwrap();
        fs::write(dir.join("partial.tmp"), b"half a write").unwrap();
        let cache = AssetCache::open(&dir, 1024).unwrap();
        assert!(cache.contains(&hash));
        assert_eq!(cache.total_bytes(), 40);
        assert!(!dir.join("partial.tmp").exists());
        assert_eq!(cache.get(&hash).unwrap(), bytes);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_corrupted_file_is_dropped_on_read() {
        let dir = temp_dir("corrupt");
        let cache = AssetCache::open(&dir, 1024).unwrap();
        let (hash, bytes) = blob(6, 30);
        cache.put(&hash, &bytes).unwrap();
        fs::write(cache.path_for(&hash), b"tampered").unwrap();
        assert!(cache.get(&hash).is_none());
        assert!(!cache.contains(&hash));
        assert!(!cache.path_for(&hash).exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn only_exact_64_hex_digit_names_parse() {
        let hash = [0xA5u8; 32];
        let name = format!("{}.hdr", hash_hex(&hash));
        assert_eq!(parse_asset_name(Path::new(&name)), Some(hash));
        for bad in [
            "a5.hdr",
            "../a5a5.hdr",
            &format!("{}.tmp", hash_hex(&hash)),
            &format!("{}x.hdr", &hash_hex(&hash)[..63]),
        ] {
            assert_eq!(parse_asset_name(Path::new(bad)), None, "{bad}");
        }
    }
}
