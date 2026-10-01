//! Staleness of the designs a saved plan uses.
//!
//! For every stored design: still in the library with the same shape (unchanged), in
//! the library with another shape (changed), gone (deleted), or gone but found again
//! under its title with the same shape (matched by title, which is how an imported file
//! finds the designs of another library).
//!
//! Entry ids are only trustworthy inside the library that wrote them. A plan records its
//! library's stamp; when it differs from the opening library's (or is missing), an id that
//! exists here but carries another title is not taken for the stored design: the title
//! decides, the way it does for an id that does not exist at all.
//!
//! "Same shape" is the ratio fingerprint and, when the plan recorded one, the absolute
//! width: a design drawn twice as large has the same ratios but cuts different stones.

use super::{
    dto::{DesignShape, SavedDesignDto},
    format::design_shape,
};
use crate::gui::rough_plan::run::DesignStatus;
use indicatrix_vault::{
    db::sqlite::Database,
    model::solid_extents::{SOLID_EXTENTS_VERSION, StoredSolidExtents},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Mutex, PoisonError},
};
use tracing::warn;

/// How far two fingerprint components (or two widths) may differ, relative to the larger.
const FINGERPRINT_TOLERANCE: f64 = 1e-6;

/// Most design names a warning lists before it says "and N more".
const LISTED_NAMES: usize = 5;

/// What the check found.
pub struct Staleness {
    /// The status of every stored design, keyed by the entry id stored in the plan.
    pub statuses: BTreeMap<i64, DesignStatus>,
    /// Things that did not stop the check but made an answer less certain: a title
    /// search that failed (each design concerned is reported as deleted), designs the
    /// library has not measured (their shape could not be compared), designs measured by
    /// another version of the measuring rule.
    pub warnings: Vec<String>,
}

/// What the check reads from the library. Every method is one query; the implementation
/// for the shared database handle locks it per call, so a long check never holds the
/// library.
pub trait LibraryView {
    /// The title of each design of `ids` that exists (ignored designs included).
    ///
    /// # Errors
    ///
    /// Returns the database's message.
    fn titles_of(&self, ids: &[i64]) -> Result<BTreeMap<i64, String>, String>;

    /// The current cached extents row of each of `ids` that has one.
    ///
    /// # Errors
    ///
    /// Returns the database's message.
    fn extents_of(&self, ids: &[i64]) -> Result<BTreeMap<i64, StoredSolidExtents>, String>;

    /// The designs titled like each of `titles` (outer spaces and ASCII case ignored,
    /// ignored designs included), keyed by the trimmed lowercase title.
    ///
    /// # Errors
    ///
    /// Returns the database's message.
    fn ids_titled(&self, titles: &[String]) -> Result<BTreeMap<String, Vec<i64>>, String>;

    /// This library's identity stamp, if it has one or can make one.
    fn stamp(&self) -> Option<u32>;
}

impl LibraryView for Mutex<Database> {
    fn titles_of(&self, ids: &[i64]) -> Result<BTreeMap<i64, String>, String> {
        self.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry_titles_for(ids)
            .map_err(|e| format!("{e:#}"))
    }

    fn extents_of(&self, ids: &[i64]) -> Result<BTreeMap<i64, StoredSolidExtents>, String> {
        self.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .solid_extents_for(ids)
            .map_err(|e| format!("{e:#}"))
    }

    fn ids_titled(&self, titles: &[String]) -> Result<BTreeMap<String, Vec<i64>>, String> {
        self.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry_ids_titled(titles)
            .map_err(|e| format!("{e:#}"))
    }

    fn stamp(&self) -> Option<u32> {
        self.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .library_stamp()
            .inspect_err(|e| warn!("Rough planner: could not read the library stamp: {e}"))
            .ok()
    }
}

/// A current design that shares a stored design's title.
struct TitleCandidate {
    entry_id: i64,
    shape: DesignShape,
}

/// The designs the library holds under one title that have usable cached extents, in id
/// order (a namesake that was never measured cannot be told from the stored design).
struct Namesakes {
    candidates: Vec<TitleCandidate>,
}

/// Whether a fingerprint is the "unknown" all-zero one.
fn is_unknown(fingerprint: [f64; 3]) -> bool {
    fingerprint
        .iter()
        .all(|component| component.abs() < f64::MIN_POSITIVE)
}

/// Whether `a` and `b` agree within [`FINGERPRINT_TOLERANCE`] of the larger.
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() / a.abs().max(b.abs()).max(1e-12) <= FINGERPRINT_TOLERANCE
}

/// Whether two fingerprints agree within [`FINGERPRINT_TOLERANCE`] on every component.
#[must_use]
pub fn fingerprints_match(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(&left, right)| close(left, right))
}

/// Whether two shapes agree: the ratios, and the width when both know it (a shape from an
/// older file has no width, which leaves the size out of the comparison).
#[must_use]
pub fn shapes_match(saved: &DesignShape, current: &DesignShape) -> bool {
    fingerprints_match(saved.fingerprint, current.fingerprint)
        && match (saved.width_caliper, current.width_caliper) {
            (Some(left), Some(right)) => close(left, right),
            _ => true,
        }
}

/// Whether the saved figures came from the measuring rule the library uses now (or from
/// one the file does not name, which is taken as the current one). Figures of another
/// rule cannot be compared: they may differ without the design having changed.
const fn comparable(saved: &DesignShape) -> bool {
    saved.extents_version == 0 || saved.extents_version == SOLID_EXTENTS_VERSION
}

/// Whether the saved shape can tell a changed design from an unchanged one.
fn usable(saved: &DesignShape) -> bool {
    comparable(saved) && !is_unknown(saved.fingerprint)
}

/// The status of a design that is still in the library and is the stored one.
///
/// `current` is its shape from the extents cache. A design without cached extents (never
/// measured, or measured as unusable) cannot be compared; it counts as unchanged. So does
/// a stored fingerprint of all zeros, which the save writes when the design had no
/// extents then, and a stored shape of another measuring rule version.
#[must_use]
pub fn status_for_existing(saved: &DesignShape, current: Option<&DesignShape>) -> DesignStatus {
    match current {
        Some(current) if usable(saved) && !shapes_match(saved, current) => DesignStatus::Changed,
        _ => DesignStatus::Unchanged,
    }
}

/// The ids of the candidates whose shape matches the stored one. None at all when the
/// stored shape cannot confirm a match.
fn matching_ids(candidates: &[TitleCandidate], saved: &DesignShape) -> Vec<i64> {
    if !usable(saved) {
        return Vec::new();
    }
    candidates
        .iter()
        .filter(|candidate| shapes_match(saved, &candidate.shape))
        .map(|candidate| candidate.entry_id)
        .collect()
}

/// The one candidate whose shape matches the stored one, if there is exactly one.
#[cfg(test)]
fn unique_match(candidates: &[TitleCandidate], saved: &DesignShape) -> Option<i64> {
    match matching_ids(candidates, saved).as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The key a title is looked up under: outer spaces and ASCII case ignored.
fn title_key(title: &str) -> String {
    title.trim().to_ascii_lowercase()
}

/// Whether `design` carries the title the save writes when it knows none.
fn is_fallback_title(design: &SavedDesignDto) -> bool {
    design.title == format!("Design {}", design.entry_id)
}

/// How a stored design is bound to the library.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Binding {
    /// Its id is taken as the design: it exists here and is trusted (same library, same
    /// title, or a title the save invented).
    Id,
    /// The title decides. `colliding` is set when the id exists here under another title.
    Title { colliding: bool },
}

/// How `design` is bound, given the title its id carries here (if any).
fn binding_of(
    design: &SavedDesignDto,
    local_title: Option<&String>,
    same_library: bool,
) -> Binding {
    match local_title {
        None => Binding::Title { colliding: false },
        Some(local)
            if same_library
                || is_fallback_title(design)
                || title_key(local) == title_key(&design.title) =>
        {
            Binding::Id
        }
        Some(_) => Binding::Title { colliding: true },
    }
}

/// The status of a design the title decides about.
///
/// One namesake of the stored shape resolves it; several are ambiguous and leave it
/// deleted. With none: an id that does not exist is "changed" when the title is there
/// with another shape and "deleted" otherwise; an id that exists under another title is
/// kept when its own shape is the stored one (the design was renamed) and "deleted"
/// otherwise -- the stored design is not the one that holds the id.
fn status_by_title(
    design: &SavedDesignDto,
    found: Option<&Namesakes>,
    colliding: bool,
    local: Option<&DesignShape>,
) -> DesignStatus {
    let saved = design.shape();
    let candidates = found.map_or(&[][..], |found| found.candidates.as_slice());
    match matching_ids(candidates, &saved).as_slice() {
        [resolved_entry_id] => DesignStatus::MatchedByTitle {
            resolved_entry_id: *resolved_entry_id,
        },
        [] if colliding && usable(&saved) && local.is_some_and(|l| shapes_match(&saved, l)) => {
            DesignStatus::Unchanged
        }
        [] if !colliding && usable(&saved) && !candidates.is_empty() => DesignStatus::Changed,
        _ => DesignStatus::Deleted,
    }
}

/// What the check could not settle, for the warnings.
#[derive(Default)]
struct Notes {
    /// Titles of the designs whose shape could not be compared for want of a measurement.
    unverified: Vec<String>,
    /// Titles of the designs saved under another measuring rule version.
    incomparable: Vec<String>,
    /// The line for a failed title search.
    search: Option<String>,
}

/// The names of `titles` for a warning: quoted, at most [`LISTED_NAMES`] of them.
fn name_list(titles: &[String]) -> String {
    let shown: Vec<String> = titles
        .iter()
        .take(LISTED_NAMES)
        .map(|title| format!("\"{title}\""))
        .collect();
    match titles.len().saturating_sub(LISTED_NAMES) {
        0 => shown.join(", "),
        more => format!("{} and {more} more", shown.join(", ")),
    }
}

impl Notes {
    /// The warning lines, in a fixed order.
    fn into_warnings(self) -> Vec<String> {
        let mut lines = Vec::new();
        lines.extend(self.search);
        if !self.unverified.is_empty() {
            let (count, names) = (self.unverified.len(), name_list(&self.unverified));
            lines.push(if count == 1 {
                format!(
                    "The design {names} has not been re-measured in this library, so its shape could not be checked against the saved plan. Running the plan re-measures it."
                )
            } else {
                format!(
                    "{count} designs have not been re-measured in this library, so their shapes could not be checked against the saved plan: {names}. Running the plan re-measures them."
                )
            });
        }
        if !self.incomparable.is_empty() {
            lines.push(format!(
                "The plan measured {} with an earlier version of the measuring rule, so {} could not be compared with the library now.",
                name_list(&self.incomparable),
                if self.incomparable.len() == 1 { "it" } else { "they" }
            ));
        }
        lines
    }
}

/// The status of a design whose id is trusted.
fn status_by_id(
    design: &SavedDesignDto,
    current: Option<&DesignShape>,
    notes: &mut Notes,
) -> DesignStatus {
    let saved = design.shape();
    if current.is_none() {
        notes.unverified.push(design.title.clone());
    } else if !comparable(&saved) && !is_unknown(saved.fingerprint) {
        notes.incomparable.push(design.title.clone());
    }
    status_for_existing(&saved, current)
}

/// Looks up the designs titled like the designs the title decides about, with their
/// cached extents (three queries however many designs there are). `None` when there is
/// nothing to look up or the search failed (a line for `notes` then says so).
fn find_namesakes(
    library: &impl LibraryView,
    wanted: &[&SavedDesignDto],
    notes: &mut Notes,
) -> Option<BTreeMap<String, Namesakes>> {
    if wanted.is_empty() {
        return None;
    }
    let titles: Vec<String> = wanted
        .iter()
        .map(|design| title_key(&design.title))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let searched = library.ids_titled(&titles).and_then(|by_title| {
        let ids: Vec<i64> = by_title.values().flatten().copied().collect();
        library.extents_of(&ids).map(|stored| (by_title, stored))
    });
    let (by_title, stored) = match searched {
        Ok(found) => found,
        Err(error) => {
            warn!("Rough planner: title search for saved designs failed: {error}");
            notes.search = Some(format!(
                "Could not search the library for the titles of {} design{}: {error}",
                wanted.len(),
                if wanted.len() == 1 { "" } else { "s" }
            ));
            return None;
        }
    };
    let namesakes = by_title
        .into_iter()
        .map(|(title, ids)| {
            let candidates = ids
                .iter()
                .filter_map(|&entry_id| {
                    let extents = stored.get(&entry_id)?.extents?;
                    Some(TitleCandidate {
                        entry_id,
                        shape: design_shape(&extents),
                    })
                })
                .collect();
            (title, Namesakes { candidates })
        })
        .collect();
    Some(namesakes)
}

/// Checks every stored design against the library.
///
/// `plan_library` is the stamp the plan recorded for its library; when it equals the
/// library's own, every id that exists is taken as the stored design. Otherwise an id
/// that exists under another title is checked by title too. The library is queried in a
/// handful of batches, never once per design.
///
/// # Errors
///
/// Returns a message when the library cannot be read at all (the titles or the extents
/// cache); a failed title search only adds a warning.
pub fn resolve_designs(
    library: &impl LibraryView,
    designs: &[SavedDesignDto],
    plan_library: Option<u32>,
) -> Result<Staleness, String> {
    let ids: Vec<i64> = designs.iter().map(|design| design.entry_id).collect();
    let titles = library
        .titles_of(&ids)
        .map_err(|e| format!("Could not read the library: {e}"))?;
    let present: Vec<i64> = titles.keys().copied().collect();
    let stored = library
        .extents_of(&present)
        .map_err(|e| format!("Could not read the design measurements: {e}"))?;
    let same_library = plan_library.is_some() && plan_library == library.stamp();
    let bindings: Vec<Binding> = designs
        .iter()
        .map(|design| binding_of(design, titles.get(&design.entry_id), same_library))
        .collect();

    let mut notes = Notes::default();
    let by_title: Vec<&SavedDesignDto> = designs
        .iter()
        .zip(&bindings)
        .filter(|(_, binding)| matches!(binding, Binding::Title { .. }))
        .map(|(design, _)| design)
        .collect();
    let namesakes = find_namesakes(library, &by_title, &mut notes);

    let mut statuses = BTreeMap::new();
    for (design, binding) in designs.iter().zip(&bindings) {
        let current = stored
            .get(&design.entry_id)
            .and_then(|row| row.extents)
            .map(|extents| design_shape(&extents));
        let status = match *binding {
            Binding::Id => status_by_id(design, current.as_ref(), &mut notes),
            Binding::Title { colliding } => status_by_title(
                design,
                namesakes
                    .as_ref()
                    .and_then(|found| found.get(&title_key(&design.title))),
                colliding,
                current.as_ref(),
            ),
        };
        statuses.insert(design.entry_id, status);
    }
    Ok(Staleness {
        statuses,
        warnings: notes.into_warnings(),
    })
}

/// The severity of a status: what wins when two designs land on one entry id.
const fn severity(status: &DesignStatus) -> u8 {
    match status {
        DesignStatus::Unchanged => 0,
        DesignStatus::MatchedByTitle { .. } => 1,
        DesignStatus::Changed => 2,
        DesignStatus::Deleted => 3,
    }
}

/// Re-keys `statuses` by the entry id the layouts use after `MatchedByTitle` stones were
/// moved to their resolved designs (`remap`, old id to new id). One step is taken, the
/// same one the stones take, so a status always sits under the id its stones now carry.
/// When two designs end on one id, the more severe status is kept.
#[must_use]
pub fn rekey_statuses(
    statuses: BTreeMap<i64, DesignStatus>,
    remap: &BTreeMap<i64, i64>,
) -> BTreeMap<i64, DesignStatus> {
    let mut out: BTreeMap<i64, DesignStatus> = BTreeMap::new();
    for (id, status) in statuses {
        let key = remap.get(&id).copied().unwrap_or(id);
        match out.get(&key) {
            Some(kept) if severity(kept) >= severity(&status) => {}
            _ => {
                out.insert(key, status);
            }
        }
    }
    out
}

/// The old id to new id moves of the `MatchedByTitle` designs.
#[must_use]
pub fn remap_of(statuses: &BTreeMap<i64, DesignStatus>) -> BTreeMap<i64, i64> {
    statuses
        .iter()
        .filter_map(|(&id, status)| match status {
            DesignStatus::MatchedByTitle { resolved_entry_id } => Some((id, *resolved_entry_id)),
            _ => None,
        })
        .collect()
}

#[cfg(test)]
mod tests;
