//! Training data: the SQLite design library, the train / validation / test split, and the mineral
//! spectrum the retarget task samples its target material from.
//!
//! Nothing here invents data. A missing or unreadable database, an unparsable `.asc` file, a
//! design with too many tiers and an unknown material name are all reported as errors or as
//! logged skips; the only stand-in design is the explicit [`DatasetSplit::fixture`], which the
//! CLI selects with `--fixture`.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use indicatrix_ai::{model::Prng, training::MAX_TIERS_LIMIT};
use indicatrix_cut_core::{
    BuiltinMaterials, Design, MaterialLookup, MaterialSelection, PreformSpec, ResolvedMaterial,
};

/// Largest tier count a design may have by default; larger designs are skipped at load time
/// (every training batch is padded to the largest design, so one outlier costs everybody).
pub const DEFAULT_MAX_TIERS_CAP: usize = 32;

/// Default salt of the split hash; recorded in the run statistics.
pub const DEFAULT_SPLIT_SEED: u64 = 0;

/// Share of the library entries that go to the train split, in percent.
pub const SPLIT_TRAIN_PERCENT: u64 = 75;
/// Share of the library entries that go to the validation split, in percent.
pub const SPLIT_VAL_PERCENT: u64 = 15;
/// Share of the library entries that go to the test split, in percent.
#[allow(
    dead_code,
    reason = "shown by the GUI's split text only; this module is shared with the CLI"
)]
pub const SPLIT_TEST_PERCENT: u64 = 100 - SPLIT_TRAIN_PERCENT - SPLIT_VAL_PERCENT;

/// Why the data set could not be built.
#[derive(Debug)]
pub enum DatasetError {
    /// No database file was found; the text says where it was looked for.
    DatabaseNotFound(String),
    /// SQLite reported an error while opening or reading the database.
    Database(String),
    /// The database holds no `.asc` attachments.
    NoDesigns,
    /// Every record was rejected (unparsable, empty, or too many tiers).
    NoLoadableDesigns {
        /// Number of rejected records.
        skipped: usize,
        /// Reason of the first rejection.
        first_reason: String,
    },
    /// A material name matches no entry of the mineral spectrum.
    UnknownMaterial(String),
}

impl fmt::Display for DatasetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DatabaseNotFound(searched) => {
                write!(f, "facet_diagrams.sqlite not found ({searched})")
            }
            Self::Database(msg) => write!(f, "database error: {msg}"),
            Self::NoDesigns => f.write_str("the database holds no .asc designs"),
            Self::NoLoadableDesigns {
                skipped,
                first_reason,
            } => write!(
                f,
                "none of the {skipped} designs could be loaded (first reason: {first_reason})"
            ),
            Self::UnknownMaterial(name) => write!(f, "unknown material '{name}'"),
        }
    }
}

impl std::error::Error for DatasetError {}

/// A design record as stored in the library: one `.asc` attachment of one library entry.
#[derive(Clone, Debug)]
pub struct DbDesignRecord {
    /// Library entry id (several attachments of one entry share it).
    pub id: i64,
    /// Library title.
    pub title: String,
    /// Attachment file name.
    pub filename: String,
    /// Raw attachment bytes (decoded by the `.asc` reader, which handles Windows-1252).
    pub asc_bytes: Vec<u8>,
}

/// A record that was parsed into a design.
#[derive(Clone, Debug)]
pub struct DesignEntry {
    /// Library entry id.
    pub id: i64,
    /// Library title.
    pub title: String,
    /// Attachment file name.
    pub filename: String,
    /// The parsed design.
    pub design: Design,
}

/// A record that was left out, and why.
#[derive(Clone, Debug)]
pub struct SkippedRecord {
    /// Library title.
    pub title: String,
    /// Reason, ready to log.
    pub reason: String,
}

/// Parses one record into a design.
///
/// # Errors
///
/// Text naming why the record is unusable: empty attachment, `.asc` parse error, no tiers, or
/// more than `max_tiers` tiers.
pub fn load_design(record: &DbDesignRecord, max_tiers: usize) -> Result<Design, String> {
    if record.asc_bytes.is_empty() {
        return Err("empty .asc attachment".to_owned());
    }
    let schedule = indicatrix_formats::asc::parse_asc_bytes(&record.asc_bytes)
        .map_err(|e| format!(".asc parse error: {e}"))?;
    let design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    let tiers = design.tiers.len();
    if tiers == 0 {
        return Err("the design has no tiers".to_owned());
    }
    if tiers > max_tiers {
        return Err(format!("{tiers} tiers exceed the limit of {max_tiers}"));
    }
    Ok(design)
}

/// The data set split: designs are grouped by library entry id and each group goes to exactly one
/// of train (75 %), validation (15 %) or test (10 %), so attachments of one entry never straddle
/// the split (except in a corpus too small for the hash to fill every part).
#[derive(Clone, Debug)]
pub struct DatasetSplit {
    /// All loadable designs, in record order.
    pub entries: Vec<DesignEntry>,
    /// Indices into `entries` of the train split.
    pub train: Vec<usize>,
    /// Indices into `entries` of the validation split.
    pub val: Vec<usize>,
    /// Indices into `entries` of the test split.
    pub test: Vec<usize>,
    /// For every input record, its index in `entries` (`None` when it was skipped).
    pub by_record: Vec<Option<usize>>,
    /// Records that were left out.
    pub skipped: Vec<SkippedRecord>,
    /// Tier slots training batches are padded to: the largest tier count of any loaded design.
    pub max_tiers: usize,
    /// Salt of the split hash.
    pub split_seed: u64,
}

/// FNV-1a over the salt and the entry id.
fn split_bucket(id: i64, salt: u64) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in salt.to_le_bytes().into_iter().chain(id.to_le_bytes()) {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash % 100
}

impl DatasetSplit {
    /// Parses the records and splits them deterministically by entry id.
    ///
    /// Designs with more than `max_tiers_cap` tiers (at most [`MAX_TIERS_LIMIT`]) are skipped
    /// and listed in [`Self::skipped`].
    ///
    /// # Errors
    ///
    /// [`DatasetError::NoDesigns`] for an empty record list,
    /// [`DatasetError::NoLoadableDesigns`] when every record was rejected.
    pub fn from_records(
        records: &[DbDesignRecord],
        split_seed: u64,
        max_tiers_cap: usize,
    ) -> Result<Self, DatasetError> {
        if records.is_empty() {
            return Err(DatasetError::NoDesigns);
        }
        let cap = max_tiers_cap.clamp(1, MAX_TIERS_LIMIT);
        let mut entries = Vec::new();
        let mut by_record = Vec::with_capacity(records.len());
        let mut skipped = Vec::new();
        for record in records {
            match load_design(record, cap) {
                Ok(design) => {
                    by_record.push(Some(entries.len()));
                    entries.push(DesignEntry {
                        id: record.id,
                        title: record.title.clone(),
                        filename: record.filename.clone(),
                        design,
                    });
                }
                Err(reason) => {
                    by_record.push(None);
                    skipped.push(SkippedRecord {
                        title: record.title.clone(),
                        reason,
                    });
                }
            }
        }
        if entries.is_empty() {
            return Err(DatasetError::NoLoadableDesigns {
                skipped: skipped.len(),
                first_reason: skipped
                    .first()
                    .map(|s| format!("'{}': {}", s.title, s.reason))
                    .unwrap_or_default(),
            });
        }

        let mut groups: BTreeMap<i64, Vec<usize>> = BTreeMap::new();
        for (index, entry) in entries.iter().enumerate() {
            groups.entry(entry.id).or_default().push(index);
        }
        let (mut train, mut val, mut test) = (Vec::new(), Vec::new(), Vec::new());
        for (id, members) in groups {
            let bucket = split_bucket(id, split_seed);
            let target = if bucket < SPLIT_TRAIN_PERCENT {
                &mut train
            } else if bucket < SPLIT_TRAIN_PERCENT + SPLIT_VAL_PERCENT {
                &mut val
            } else {
                &mut test
            };
            target.extend(members);
        }
        // A corpus too small for the hash to fill every part still gets one design in each.
        if train.is_empty()
            && let Some(i) = val.pop().or_else(|| test.pop())
        {
            train.push(i);
        }
        if val.is_empty() && train.len() > 1 {
            val.extend(train.pop());
        }
        if test.is_empty() && train.len() > 1 {
            test.extend(train.pop());
        }

        let max_tiers = entries
            .iter()
            .map(|e| e.design.tiers.len())
            .max()
            .unwrap_or(1)
            .max(1);
        Ok(Self {
            entries,
            train,
            val,
            test,
            by_record,
            skipped,
            max_tiers,
            split_seed,
        })
    }

    /// The explicit smoke-test data set: the built-in concave fixture used for all three splits.
    #[must_use]
    // This module is compiled into both binaries; only the CLI (`--fixture`) calls this.
    #[allow(dead_code)]
    pub fn fixture() -> Self {
        let design = Design::concave_fixture();
        let max_tiers = design.tiers.len().max(1);
        Self {
            entries: vec![DesignEntry {
                id: -1,
                title: "Concave fixture (smoke test)".to_owned(),
                filename: "concave_fixture".to_owned(),
                design,
            }],
            train: vec![0],
            val: vec![0],
            test: vec![0],
            by_record: vec![Some(0)],
            skipped: Vec::new(),
            max_tiers,
            split_seed: DEFAULT_SPLIT_SEED,
        }
    }
}

/// Databases are looked up here when `DIAGRAM_DB_PATH` is not set.
const DB_CANDIDATES: [&str; 4] = [
    "facet_diagrams.sqlite",
    "../facet_diagrams.sqlite",
    "../../facet_diagrams.sqlite",
    "../../../facet_diagrams.sqlite",
];

/// Resolves the path of `facet_diagrams.sqlite`: the `DIAGRAM_DB_PATH` environment variable when
/// set (and then it must exist), otherwise the first of a few relative locations.
///
/// # Errors
///
/// [`DatasetError::DatabaseNotFound`] naming what was tried.
pub fn find_db_path() -> Result<PathBuf, DatasetError> {
    if let Ok(env_path) = std::env::var("DIAGRAM_DB_PATH") {
        let path = PathBuf::from(&env_path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(DatasetError::DatabaseNotFound(format!(
                "DIAGRAM_DB_PATH='{env_path}' is not a file"
            )))
        };
    }
    DB_CANDIDATES
        .iter()
        .map(PathBuf::from)
        .find(|p| p.is_file())
        .ok_or_else(|| {
            DatasetError::DatabaseNotFound(format!(
                "set DIAGRAM_DB_PATH or place it at one of {}",
                DB_CANDIDATES.join(", ")
            ))
        })
}

/// Loads every `.asc` attachment of the design library (read-only).
///
/// # Errors
///
/// [`DatasetError`] for a missing database, an SQLite failure, or an empty library.
#[allow(
    dead_code,
    reason = "the CLI loads the library itself; the GUI worker uses load_db_designs_from"
)]
pub fn load_db_designs() -> Result<Vec<DbDesignRecord>, DatasetError> {
    load_db_designs_from(&find_db_path()?)
}

/// Loads every `.asc` attachment of the design library at `path` (read-only).
///
/// # Errors
///
/// [`DatasetError`] for a missing database, an SQLite failure, or an empty library.
// The CLI binary shares this module and only calls `load_db_designs`.
#[allow(dead_code)]
pub fn load_db_designs_from(path: &std::path::Path) -> Result<Vec<DbDesignRecord>, DatasetError> {
    if !path.is_file() {
        return Err(DatasetError::DatabaseNotFound(format!(
            "'{}' is not a file",
            path.display()
        )));
    }
    let db_err = |e: rusqlite::Error| DatasetError::Database(format!("{}: {e}", path.display()));
    let conn =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(db_err)?;
    let mut stmt = conn
        .prepare(
            "SELECT de.id, de.title, af.name, af.content \
             FROM diagram_entries de \
             JOIN diagram_details dd ON de.id = dd.entry_id \
             JOIN attached_files af ON dd.id = af.detail_id \
             WHERE af.name LIKE '%.asc' \
             ORDER BY de.id, af.name",
        )
        .map_err(db_err)?;
    let rows = stmt
        .query_map([], |row| {
            let bytes = match row.get_ref(3)? {
                rusqlite::types::ValueRef::Blob(b) | rusqlite::types::ValueRef::Text(b) => {
                    b.to_vec()
                }
                _ => Vec::new(),
            };
            Ok(DbDesignRecord {
                id: row.get(0)?,
                title: row.get(1)?,
                filename: row.get(2)?,
                asc_bytes: bytes,
            })
        })
        .map_err(db_err)?;
    let records = rows.collect::<Result<Vec<_>, _>>().map_err(db_err)?;
    if records.is_empty() {
        return Err(DatasetError::NoDesigns);
    }
    Ok(records)
}

/// Entry in the mineral optical spectrum table with nominal refractive index.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MineralSpectrumEntry {
    /// Species name.
    pub name: &'static str,
    /// Name of the built-in material preset that stands for the species, or `None` when the
    /// library has no preset: the species is then a neutral stone of exactly `refractive_index`.
    pub preset: Option<&'static str>,
    /// Nominal refractive index (sodium D).
    pub refractive_index: f32,
    /// Critical angle in degrees, `asin(1 / n)`.
    pub critical_angle_deg: f32,
}

const fn entry(
    name: &'static str,
    preset: Option<&'static str>,
    refractive_index: f32,
    critical_angle_deg: f32,
) -> MineralSpectrumEntry {
    MineralSpectrumEntry {
        name,
        preset,
        refractive_index,
        critical_angle_deg,
    }
}

/// Gemstone species across the optical refraction spectrum (n from 1.43 to 2.90).
pub const MINERAL_SPECTRUM: &[MineralSpectrumEntry] = &[
    entry("Fluorite", None, 1.434, 44.22),
    entry("Quartz", Some("Quartz"), 1.544, 40.36),
    entry("Beryl", None, 1.577, 39.35),
    entry("Topaz", Some("Topaz"), 1.619, 38.14),
    entry("Tourmaline", Some("Tourmaline"), 1.624, 38.01),
    entry("Spinel", Some("Spinel"), 1.718, 35.60),
    entry("Sapphire", Some("Sapphire"), 1.768, 34.45),
    entry("Garnet", None, 1.830, 33.12),
    entry("Zircon", Some("Zircon"), 1.950, 30.85),
    entry("Cubic Zirconia", Some("Cubic Zirconia"), 2.150, 27.72),
    entry("Diamond", Some("Diamond"), 2.417, 24.44),
    entry(
        "Synthetic Moissanite",
        Some("Synthetic Moissanite"),
        2.650,
        22.17,
    ),
    entry("Rutile", Some("Rutile"), 2.903, 20.15),
];

/// Finds the spectrum entry for `name`. Accepts the UI labels (`Sapphire (n=1.77)`), is case
/// insensitive, and knows the aliases `Corundum`, `Moissanite` and `CZ`.
///
/// # Errors
///
/// [`DatasetError::UnknownMaterial`] when nothing matches; there is no fallback to diamond.
pub fn find_spectrum_entry(name: &str) -> Result<MineralSpectrumEntry, DatasetError> {
    let base = name.split(" (").next().unwrap_or(name).trim();
    let canonical = match base.to_ascii_lowercase().as_str() {
        "corundum" => "sapphire".to_owned(),
        "moissanite" => "synthetic moissanite".to_owned(),
        "cz" => "cubic zirconia".to_owned(),
        other => other.to_owned(),
    };
    MINERAL_SPECTRUM
        .iter()
        .find(|e| e.name.eq_ignore_ascii_case(&canonical))
        .copied()
        .ok_or_else(|| DatasetError::UnknownMaterial(name.to_owned()))
}

/// Resolves a spectrum entry into a material: its preset, or a neutral stone of the entry's
/// refractive index when the library has no preset for the species.
///
/// # Errors
///
/// [`DatasetError::UnknownMaterial`] when the entry names a preset the library lacks (the
/// resolver would otherwise silently substitute diamond).
pub fn resolve_entry(entry: &MineralSpectrumEntry) -> Result<ResolvedMaterial, DatasetError> {
    let selection = match entry.preset {
        Some(preset) => {
            if BuiltinMaterials.lookup(preset).is_none() {
                return Err(DatasetError::UnknownMaterial(preset.to_owned()));
            }
            MaterialSelection {
                name: Some(preset.to_owned()),
                ..MaterialSelection::none()
            }
        }
        None => MaterialSelection {
            refractive_index_override: Some(f64::from(entry.refractive_index)),
            ..MaterialSelection::none()
        },
    };
    Ok(selection.resolve(&BuiltinMaterials))
}

/// Resolves a material name (see [`find_spectrum_entry`]) into a material.
///
/// # Errors
///
/// As [`find_spectrum_entry`] and [`resolve_entry`].
pub fn resolve_material(name: &str) -> Result<ResolvedMaterial, DatasetError> {
    resolve_entry(&find_spectrum_entry(name)?)
}

/// Deterministically draws a target material from the spectrum.
#[must_use]
pub fn sample_target(seed: u64) -> MineralSpectrumEntry {
    let index = Prng::from_stream(seed, 0x7A12_0001).next_index(MINERAL_SPECTRUM.len());
    MINERAL_SPECTRUM[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spectrum_critical_angles_match_the_refractive_indices() {
        for e in MINERAL_SPECTRUM {
            let exact = indicatrix_cut_core::critical_angle_deg(f64::from(e.refractive_index));
            assert!(
                (exact - f64::from(e.critical_angle_deg)).abs() < 0.02,
                "{}: table {} vs exact {exact}",
                e.name,
                e.critical_angle_deg
            );
        }
    }

    #[test]
    fn every_entry_resolves_and_presetless_species_use_their_index() {
        for e in MINERAL_SPECTRUM {
            let material = resolve_entry(e).expect("resolves");
            if e.preset.is_none() {
                assert!((material.n_d - f64::from(e.refractive_index)).abs() < 1e-6);
            }
        }
        let fluorite = resolve_material("Fluorite (n=1.43)").unwrap();
        let diamond = resolve_material("Diamond").unwrap();
        assert!(fluorite.n_d < 1.5 && diamond.n_d > 2.3);
    }

    #[test]
    fn unknown_material_is_an_error_not_diamond() {
        assert!(matches!(
            resolve_material("Unobtainium"),
            Err(DatasetError::UnknownMaterial(_))
        ));
        assert_eq!(find_spectrum_entry("corundum").unwrap().name, "Sapphire");
    }

    #[test]
    fn the_split_hash_is_deterministic_and_salted() {
        assert_eq!(split_bucket(42, 0), split_bucket(42, 0));
        assert!(split_bucket(42, 0) < 100);
        assert_ne!(
            (0..50).map(|i| split_bucket(i, 0)).collect::<Vec<_>>(),
            (0..50).map(|i| split_bucket(i, 1)).collect::<Vec<_>>()
        );
    }

    #[test]
    fn fixture_fills_every_split() {
        let f = DatasetSplit::fixture();
        assert_eq!((f.train.len(), f.val.len(), f.test.len()), (1, 1, 1));
        assert!(f.max_tiers >= 5);
    }

    #[test]
    fn empty_and_unparsable_records_are_rejected_loudly() {
        assert!(matches!(
            DatasetSplit::from_records(&[], 0, 32),
            Err(DatasetError::NoDesigns)
        ));
        let bad = DbDesignRecord {
            id: 1,
            title: "bad".into(),
            filename: "bad.asc".into(),
            asc_bytes: b"not an asc file".to_vec(),
        };
        assert!(matches!(
            DatasetSplit::from_records(&[bad], 0, 32),
            Err(DatasetError::NoLoadableDesigns { skipped: 1, .. })
        ));
    }

    #[test]
    fn sampling_a_target_is_deterministic() {
        assert_eq!(sample_target(5), sample_target(5));
    }
}
