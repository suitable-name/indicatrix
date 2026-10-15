//! Where the library-lesson scenarios get their designs from, and the seeded shuffle that
//! picks which ones.
//!
//! "A database is connected" means the environment variable `INDICATRIX_HARNESS_LIBRARY` names
//! a `facet_diagrams.sqlite`. The harness never opens that file: [`relaunch_with_database`]
//! copies it (and its `-wal`/`-shm` siblings) into the throw-away profile, after the profile's
//! own data was wiped and before the app starts, and the app and the harness only ever touch
//! the copy. Without the variable the fixture library of `fixtures::sample_designs` is used.
//!
//! Every random choice of the tutorial scenarios comes from [`SeededRng`], seeded by
//! `INDICATRIX_HARNESS_SEED`, so a failing run can be repeated.
#![allow(dead_code)]

use crate::support::{Ctx, HarnessApp, ProfileExt};
use indicatrix_cut::harness_support;
use indicatrix_editor::loading::design_from_asc_text;
use indicatrix_vault::db::sqlite::Database;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The environment variable that connects an existing library database.
pub const ENV_LIBRARY: &str = "INDICATRIX_HARNESS_LIBRARY";
/// The environment variable that seeds every random choice of the tutorial scenarios.
pub const ENV_SEED: &str = "INDICATRIX_HARNESS_SEED";
/// The seed used when [`ENV_SEED`] is not set.
pub const DEFAULT_SEED: u64 = 20_261_013;
/// Suffixes of the files that belong to one SQLite database in WAL mode.
const SIBLING_SUFFIXES: [&str; 2] = ["-wal", "-shm"];

/// Where the designs of the library lessons come from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LibrarySource {
    /// The designs of `fixtures::sample_designs`, written into a fresh library.
    Fixtures,
    /// This database file, copied into the profile before launch.
    Database(PathBuf),
}

/// The configured source: the database named by [`ENV_LIBRARY`] when that file exists, else the
/// fixtures. The second value is a note for the report when the variable was set but unusable.
#[must_use]
pub fn configured_source() -> (LibrarySource, Option<String>) {
    let Some(raw) = std::env::var_os(ENV_LIBRARY).filter(|v| !v.is_empty()) else {
        return (LibrarySource::Fixtures, None);
    };
    let path = PathBuf::from(raw);
    if path.is_file() {
        (LibrarySource::Database(path), None)
    } else {
        let note = format!(
            "{ENV_LIBRARY} is set to {} but that is not a file; the fixture library is used instead",
            path.display()
        );
        (LibrarySource::Fixtures, Some(note))
    }
}

/// The seed of this run: [`ENV_SEED`] as a number, a hash of it when it is any other text, else
/// [`DEFAULT_SEED`].
#[must_use]
pub fn harness_seed() -> u64 {
    let Ok(text) = std::env::var(ENV_SEED) else {
        return DEFAULT_SEED;
    };
    let text = text.trim();
    if text.is_empty() {
        return DEFAULT_SEED;
    }
    text.parse::<u64>().unwrap_or_else(|_| {
        // FNV-1a over the bytes: any text is a usable, repeatable seed.
        text.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
        })
    })
}

/// A small deterministic generator (splitmix64). `salt` separates the independent choices made
/// from one seed (which tutorials, which library designs) so they do not mirror each other.
pub struct SeededRng(u64);

impl SeededRng {
    /// A generator for `seed` and `salt`.
    #[must_use]
    pub const fn new(seed: u64, salt: u64) -> Self {
        Self(seed ^ salt.wrapping_mul(0x9e37_79b9_7f4a_7c15))
    }

    /// The next 64 random bits.
    pub const fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// Shuffles `items` in place (Fisher-Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let bound = u64::try_from(i + 1).unwrap_or(u64::MAX);
            let j = usize::try_from(self.next_u64() % bound).unwrap_or(0);
            items.swap(i, j);
        }
    }
}

/// `items` in a seeded random order.
#[must_use]
pub fn shuffled<T>(mut items: Vec<T>, seed: u64, salt: u64) -> Vec<T> {
    SeededRng::new(seed, salt).shuffle(&mut items);
    items
}

/// Shuts the app down, wipes what it persisted, seeds the settings the scenario started with,
/// copies `original` (with its `-wal`/`-shm` files) over the profile's library database and
/// launches the app on the copy. Returns the path of the copy.
///
/// # Errors
///
/// The database path would lie outside the profile, the file cannot be copied, or the app
/// does not start.
pub fn relaunch_with_database(ctx: &mut Ctx<'_>, original: &Path) -> Result<PathBuf, String> {
    let target = harness_support::database_path();
    if !ctx.profile.contains(&target) {
        return Err(format!(
            "the library database would be {} which is outside the profile",
            target.display()
        ));
    }
    ctx.app.shutdown();
    ctx.profile.wipe_app_data();
    ctx.profile.seed_settings(&ctx.seed);
    harness_support::clear_picker_answers();
    copy_database(original, &target)?;
    *ctx.app = HarnessApp::launch()?;
    Ok(target)
}

/// Copies the database file and its `-wal`/`-shm` siblings from `original` to `target`.
fn copy_database(original: &Path, target: &Path) -> Result<(), String> {
    if let (Ok(a), Ok(b)) = (fs::canonicalize(original), fs::canonicalize(target))
        && a == b
    {
        return Err("the library copy would overwrite its own source".to_owned());
    }
    fs::copy(original, target).map_err(|e| {
        format!(
            "cannot copy {} to {}: {e}",
            original.display(),
            target.display()
        )
    })?;
    for suffix in SIBLING_SUFFIXES {
        let from = with_suffix(original, suffix);
        if from.is_file() {
            let to = with_suffix(target, suffix);
            fs::copy(&from, &to)
                .map_err(|e| format!("cannot copy {} to {}: {e}", from.display(), to.display()))?;
        }
    }
    Ok(())
}

/// `path` with `suffix` appended to the file name.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// How many tier rows the design of fixture or attachment text `asc` has when the editor opens
/// it (flat plus concave), or `None` when it does not parse.
#[must_use]
pub fn tier_rows_of_asc(file_name: &str, asc: &str, lw_ratio: Option<&str>) -> Option<usize> {
    let loaded = design_from_asc_text(file_name, asc, lw_ratio).ok()?;
    Some(loaded.design.tiers.len() + loaded.design.concave_tiers.len())
}

/// The tier-row count of the first `.asc` attachment of library entry `entry_id`, read from the
/// profile's COPY of the database (never the original) through a read-only connection.
#[must_use]
pub fn tier_rows_in_database(copy: &Path, entry_id: i64) -> Option<usize> {
    let db = Database::open_read_only(copy.to_str()?).ok()?;
    let full = db.get_diagram_full(entry_id).ok()??;
    let file = full
        .attached_files
        .iter()
        .find(|file| file.name.to_ascii_lowercase().ends_with(".asc"))?;
    let text = String::from_utf8_lossy(&file.content);
    tier_rows_of_asc(&file.name, &text, full.lw_ratio.as_deref())
}
