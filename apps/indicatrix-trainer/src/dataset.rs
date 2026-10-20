//! Training data: the SQLite design library, the train / validation / test split, and the mineral
//! spectrum the retarget task samples its target material from.
//!
//! Nothing here invents data. A missing or unreadable database, an unparsable `.asc` file, a
//! design with too many tiers and an unknown material name are all reported as errors or as
//! logged skips; the only stand-in design is the explicit [`DatasetSplit::fixture`], which the
//! CLI selects with `--fixture`.

use std::{collections::BTreeMap, fmt, path::PathBuf};

use indicatrix::geometry::meet_solver::MeetConstraint;
use indicatrix_ai::training::MAX_TIERS_LIMIT;
use indicatrix_cut_core::{Design, PreformSpec};

#[path = "dataset_spectrum.rs"]
mod spectrum;
pub use spectrum::{
    MINERAL_SPECTRUM, find_spectrum_entry, resolve_entry, resolve_material, sample_target,
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

/// The data set split: designs sharing an entry id or normalized geometry are connected into one
/// group, and every group goes to a single split.
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
        let mut normalized_contents = Vec::new();
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
                    normalized_contents.push(normalized_geometry_key(
                        &entries.last().expect("just inserted").design,
                    ));
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

        let groups = connected_design_groups(&entries, &normalized_contents);
        let group_count = groups.len();
        let (mut train, mut val, mut test) = (Vec::new(), Vec::new(), Vec::new());
        let mut group_counts = [0usize; 3];
        let mut assignments = Vec::with_capacity(group_count);
        for (key, members) in groups {
            let bucket = split_bucket(key, split_seed);
            let target = if bucket < SPLIT_TRAIN_PERCENT {
                0
            } else if bucket < SPLIT_TRAIN_PERCENT + SPLIT_VAL_PERCENT {
                1
            } else {
                2
            };
            group_counts[target] += 1;
            assignments.push((target, members));
        }
        rebalance_splits(&mut assignments, &mut group_counts, group_count);
        for (part, members) in assignments {
            match part {
                0 => train.extend(members),
                1 => val.extend(members),
                _ => test.extend(members),
            }
        }

        let max_tiers = entries
            .iter()
            .map(|e| e.design.tiers.len())
            .max()
            .unwrap_or(1)
            .max(1);
        Ok(Self {
            entries,
            train: {
                train.sort_unstable();
                train
            },
            val: {
                val.sort_unstable();
                val
            },
            test: {
                test.sort_unstable();
                test
            },
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

/// Canonical geometry identity, independent of entry id, title, material, comments and tier
/// labels. Named meet references are encoded by their resolved tier positions, not their text.
/// Keeps at least one complete group for training. Held-out splits remain empty when there
/// are too few independent groups; validation/test never borrow sibling records.
fn rebalance_splits(
    assignments: &mut [(usize, Vec<usize>)],
    group_counts: &mut [usize; 3],
    group_count: usize,
) {
    if group_counts[0] == 0 {
        let donor = (1..=2)
            .filter(|&part| group_counts[part] > 0)
            .max_by_key(|&part| group_counts[part]);
        if let Some(donor) = donor {
            let candidate = assignments
                .iter()
                .enumerate()
                .filter(|(_, (part, _))| *part == donor)
                .min_by_key(|(_, (_, members))| members.len())
                .map(|(index, _)| index)
                .expect("a donor split contains a group");
            assignments[candidate].0 = 0;
            group_counts[donor] -= 1;
            group_counts[0] += 1;
        }
    }
    if group_count >= 3 {
        for target in 0..3 {
            if group_counts[target] == 0 {
                let donor = (0..3)
                    .filter(|&part| group_counts[part] > 1)
                    .max_by_key(|&part| group_counts[part])
                    .expect("three groups leave one donor for each missing split");
                let candidate = assignments
                    .iter()
                    .enumerate()
                    .filter(|(_, (part, _))| *part == donor)
                    .min_by_key(|(_, (_, members))| members.len())
                    .map(|(index, _)| index)
                    .expect("a donor split contains a group");
                assignments[candidate].0 = target;
                group_counts[donor] -= 1;
                group_counts[target] += 1;
            }
        }
    }
}

fn normalized_geometry_key(design: &Design) -> Vec<u8> {
    let tiers = design
        .tiers
        .iter()
        .map(|tier| {
            let mut indices: Vec<_> = tier.indices.iter().map(|&v| normalized_bits(v)).collect();
            indices.sort_unstable();
            let mut detached: Vec<_> = tier.detached.iter().map(|&v| normalized_bits(v)).collect();
            detached.sort_unstable();
            let constraint = normalized_constraint(&tier.constraint, design);
            let imported = tier
                .imported_meet
                .as_ref()
                .map(|c| normalized_constraint(c, design));
            (
                normalized_bits(tier.angle_deg),
                indices,
                detached,
                constraint,
                imported,
            )
        })
        .collect::<Vec<_>>();
    let cheaters: Vec<_> = design
        .cheater_offsets_deg
        .iter()
        .map(|(&i, &value)| (i, normalized_bits(value)))
        .collect();
    format!(
        "{:?}|{}|{:?}|{:?}|{:?}|{:?}|{:?}",
        design.preform,
        normalized_bits(design.preform_y_offset),
        (
            design.meta.gear_teeth,
            normalized_bits(design.meta.gear_reference_angle),
            design.meta.symmetry_order,
            design.meta.mirror
        ),
        cheaters,
        tiers,
        design.girdle_diameter_mm.map(normalized_bits),
        design.tiers.len(),
    )
    .into_bytes()
}

fn normalized_bits(value: f64) -> u64 {
    if value == 0.0 { 0 } else { value.to_bits() }
}

fn normalized_constraint(constraint: &MeetConstraint, design: &Design) -> String {
    match constraint {
        MeetConstraint::MeetExisting => "E".to_owned(),
        MeetConstraint::ScaleReference(value) => format!("S:{}", value.to_bits()),
        MeetConstraint::MeetNamed(names) => {
            let mut targets = names
                .iter()
                .map(|name| {
                    design
                        .tiers
                        .iter()
                        .position(|tier| tier.names().contains(&name.as_str()))
                        .or_else(|| {
                            design.tiers.iter().position(|tier| {
                                tier.names()
                                    .iter()
                                    .any(|candidate| candidate.eq_ignore_ascii_case(name))
                            })
                        })
                        .map_or_else(
                            || format!("?{}", name.trim().to_ascii_lowercase()),
                            |index| index.to_string(),
                        )
                })
                .collect::<Vec<_>>();
            targets.sort_unstable();
            format!("M:{}", targets.join(","))
        }
    }
}

/// Transitive closure of shared entry ids and normalized geometry keys.
fn connected_design_groups(
    entries: &[DesignEntry],
    contents: &[Vec<u8>],
) -> BTreeMap<i64, Vec<usize>> {
    let mut parent: Vec<usize> = (0..entries.len()).collect();
    let mut by_id = BTreeMap::<i64, usize>::new();
    let mut by_content = BTreeMap::<Vec<u8>, usize>::new();
    for (index, entry) in entries.iter().enumerate() {
        for previous in [
            by_id.insert(entry.id, index),
            by_content.insert(contents[index].clone(), index),
        ]
        .into_iter()
        .flatten()
        {
            union(&mut parent, index, previous);
        }
    }
    let mut members = BTreeMap::<usize, Vec<usize>>::new();
    for index in 0..entries.len() {
        let root = find_root(&mut parent, index);
        members.entry(root).or_default().push(index);
    }
    members
        .into_values()
        .map(|indices| {
            let key = indices
                .iter()
                .map(|&i| entries[i].id)
                .min()
                .unwrap_or_default();
            (key, indices)
        })
        .collect()
}

fn find_root(parent: &mut [usize], index: usize) -> usize {
    if parent[index] != index {
        parent[index] = find_root(parent, parent[index]);
    }
    parent[index]
}

fn union(parent: &mut [usize], left: usize, right: usize) {
    let left_root = find_root(parent, left);
    let right_root = find_root(parent, right);
    if left_root != right_root {
        parent[right_root] = left_root;
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn normalized_content_connects_same_cut_across_ids_and_material_metadata() {
        let design = Design::concave_fixture();
        let mut duplicate = design.clone();
        duplicate.material.name = Some("Quartz".to_owned());
        let entries = vec![
            DesignEntry {
                id: 11,
                title: "first".into(),
                filename: "a.asc".into(),
                design,
            },
            DesignEntry {
                id: 99,
                title: "renamed".into(),
                filename: "b.asc".into(),
                design: duplicate,
            },
        ];
        let contents: Vec<_> = entries
            .iter()
            .map(|e| normalized_geometry_key(&e.design))
            .collect();
        let groups = connected_design_groups(&entries, &contents);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups.values().next().unwrap(), &vec![0, 1]);
    }

    #[test]
    fn shared_entry_ids_connect_changed_attachments_transitively() {
        let design = Design::concave_fixture();
        let mut changed = design.clone();
        changed.tiers[0].angle_deg += 1.0;
        let entries = vec![
            DesignEntry {
                id: 7,
                title: "original".into(),
                filename: "a.asc".into(),
                design,
            },
            DesignEntry {
                id: 7,
                title: "attachment".into(),
                filename: "b.asc".into(),
                design: changed,
            },
        ];
        let contents: Vec<_> = entries
            .iter()
            .map(|e| normalized_geometry_key(&e.design))
            .collect();
        assert_ne!(contents[0], contents[1]);
        assert_eq!(connected_design_groups(&entries, &contents).len(), 1);
    }

    #[test]
    fn facet_index_order_and_signed_zero_do_not_change_geometry_identity() {
        let design = Design::concave_fixture();
        let mut reordered = design.clone();
        reordered.tiers[0].indices.reverse();
        reordered.tiers[0].angle_deg = -0.0;
        let mut zero = design;
        zero.tiers[0].angle_deg = 0.0;
        assert_eq!(
            normalized_geometry_key(&zero),
            normalized_geometry_key(&reordered)
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
}
