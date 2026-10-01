//! The `saved_rough_plans` table: listing, storing, loading, renaming and deleting.
//!
//! Every function takes the shared database handle and holds its lock only for one
//! query, so a long list never blocks the library on the UI thread. All of them are
//! meant to run on a worker thread.

use super::{
    dto::CURRENT_SCHEMA_VERSION,
    format::{payload_version_of, payload_with_name, plan_summary},
    naming::{current_unix_time, format_unix_date},
};
use indicatrix_vault::{db::sqlite::Database, model::saved_rough_plan::SavedRoughPlan};
use std::sync::{Mutex, PoisonError};
use tracing::warn;

/// What the list shows for a plan whose payload cannot be read.
const UNREADABLE_SUMMARY: &str = "Saved plan";

/// One row of the saved list, ready to be shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedPlan {
    /// The plan's id in the table.
    pub id: i64,
    /// The plan's name.
    pub name: String,
    /// "Pebble · Aquamarine · 3 results".
    pub summary: String,
    /// The creation date, "YYYY-MM-DD".
    pub date: String,
}

/// A stored plan with its payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredPlan {
    /// The plan's name in the table (a rename changes it and the payload's own name).
    pub name: String,
    /// When it was stored (Unix seconds).
    pub created_at: i64,
    /// The schema version recorded when the plan was stored; it equals the TOML header's
    /// unless someone edited the row.
    pub payload_version: u32,
    /// The TOML text.
    pub payload: String,
}

/// Runs `f` with the database locked.
fn with_db<T>(db: &Mutex<Database>, f: impl FnOnce(&Database) -> T) -> T {
    f(&db.lock().unwrap_or_else(PoisonError::into_inner))
}

/// The message for a plan that is not in the table (deleted in another window, say).
fn gone_message(id: i64) -> String {
    format!("Saved rough plan #{id} no longer exists.")
}

/// Reads a stored plan for a list row that has no stored summary.
type ReadPlan<'a> = dyn FnMut(i64) -> Result<Option<SavedRoughPlan>, String> + 'a;

/// The summary of plan `id`, read from its payload once and stored so no later list reads
/// the payload again. `None` when the plan is gone; "Saved plan" (not stored, so a later
/// list tries again) when the read failed.
fn fill_summary(db: &Mutex<Database>, read_plan: &mut ReadPlan<'_>, id: i64) -> Option<String> {
    match read_plan(id) {
        Ok(Some(plan)) => {
            let summary = plan_summary(&plan.payload);
            if let Err(e) = with_db(db, |d| d.set_saved_rough_plan_summary(id, &summary)) {
                warn!("Rough planner: could not store the summary of saved plan {id}: {e}");
            }
            Some(summary)
        }
        Ok(None) => None,
        Err(e) => {
            warn!("Rough planner: could not read saved plan {id}: {e}");
            Some(UNREADABLE_SUMMARY.to_string())
        }
    }
}

/// [`list_saved`] with the payload reader spelled out: it is called for a row only when
/// the row has no stored summary.
pub(super) fn list_saved_with(
    db: &Mutex<Database>,
    read_plan: &mut ReadPlan<'_>,
) -> Result<Vec<ListedPlan>, String> {
    let metas = with_db(db, Database::list_saved_rough_plans)
        .map_err(|e| format!("Could not list the saved rough plans: {e}"))?;
    let mut rows = Vec::with_capacity(metas.len());
    for meta in metas {
        // An empty summary is as good as none: it is what a row written without one holds.
        let stored = meta.summary.filter(|summary| !summary.trim().is_empty());
        let Some(summary) = stored.or_else(|| fill_summary(db, read_plan, meta.plan_id)) else {
            continue;
        };
        rows.push(ListedPlan {
            id: meta.plan_id,
            name: meta.name,
            summary,
            date: format_unix_date(meta.created_at),
        });
    }
    Ok(rows)
}

/// Lists the stored plans, newest first, with a one-line summary each. The summaries are
/// stored with the plans, so no payload is read or parsed; only a plan saved before
/// summaries were stored is read once and gets its summary filled in. Call it off the UI
/// thread.
///
/// # Errors
///
/// Returns a message if the table cannot be listed. A plan whose payload cannot be read
/// is listed with the summary "Saved plan".
pub fn list_saved(db: &Mutex<Database>) -> Result<Vec<ListedPlan>, String> {
    list_saved_with(db, &mut |id| {
        with_db(db, |d| d.get_saved_rough_plan(id)).map_err(|e| e.to_string())
    })
}

/// Stores a plan as a new row and returns its id. The row records the payload's schema
/// version and its list summary.
///
/// # Errors
///
/// Returns a message if the insert fails.
pub fn save_plan(db: &Mutex<Database>, name: &str, payload: &str) -> Result<i64, String> {
    let now = current_unix_time();
    let version = payload_version_of(payload).unwrap_or(CURRENT_SCHEMA_VERSION);
    let summary = plan_summary(payload);
    with_db(db, |d| {
        d.save_rough_plan(name, version, payload, &summary, now)
    })
    .map_err(|e| format!("Could not save the rough plan: {e}"))
}

/// Loads a stored plan.
///
/// # Errors
///
/// Returns a message if the query fails or the plan no longer exists.
pub fn load_saved(db: &Mutex<Database>, id: i64) -> Result<StoredPlan, String> {
    let plan = with_db(db, |d| d.get_saved_rough_plan(id))
        .map_err(|e| format!("Could not load saved rough plan #{id}: {e}"))?
        .ok_or_else(|| gone_message(id))?;
    Ok(StoredPlan {
        name: plan.meta.name,
        created_at: plan.meta.created_at,
        payload_version: plan.payload_version,
        payload: plan.payload,
    })
}

/// Renames a stored plan: the table's name and the name inside the payload change in one
/// update, so an export, which writes the payload, always carries the current name. The
/// stored summary stays.
///
/// # Errors
///
/// Returns a message if the update fails or the plan no longer exists.
pub fn rename_saved(db: &Mutex<Database>, id: i64, new_name: &str) -> Result<(), String> {
    let now = current_unix_time();
    let changed = with_db(db, |d| {
        let payload = d
            .get_saved_rough_plan(id)?
            .map(|plan| payload_with_name(&plan.payload, new_name));
        d.rename_saved_rough_plan(id, new_name, payload.as_deref(), now)
    })
    .map_err(|e| format!("Could not rename rough plan #{id}: {e}"))?;
    if changed == 0 {
        Err(gone_message(id))
    } else {
        Ok(())
    }
}

/// Deletes a stored plan.
///
/// # Errors
///
/// Returns a message if the delete fails or the plan no longer exists.
pub fn delete_saved(db: &Mutex<Database>, id: i64) -> Result<(), String> {
    let deleted = with_db(db, |d| d.delete_saved_rough_plan(id))
        .map_err(|e| format!("Could not delete rough plan #{id}: {e}"))?;
    if deleted == 0 {
        Err(gone_message(id))
    } else {
        Ok(())
    }
}

/// This library's identity stamp (created on first use), or `None` when it cannot be read
/// or written; the plan is then saved without one and opens by the title check.
#[must_use]
pub fn library_stamp(db: &Mutex<Database>) -> Option<u32> {
    with_db(db, Database::library_stamp)
        .inspect_err(|e| warn!("Rough planner: could not read the library stamp: {e}"))
        .ok()
}
