//! Saved rough plans: model types for persisting user rough planner sessions and layouts.
//!
//! See `crate::db::sqlite::Database::save_rough_plan`/`get_saved_rough_plan`/
//! `list_saved_rough_plans`/`rename_saved_rough_plan`/`delete_saved_rough_plan` for the
//! storage side.

/// Metadata for a saved rough plan (excluding the payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRoughPlanMeta {
    /// The unique primary key ID of the saved plan.
    pub plan_id: i64,
    /// User-visible name or label of the plan.
    pub name: String,
    /// Unix timestamp (seconds) when the plan was first saved.
    pub created_at: i64,
    /// Unix timestamp (seconds) when the plan was last updated or renamed.
    pub updated_at: i64,
    /// One-line description of the plan for lists, written when the plan is saved so a
    /// list never has to read a payload. `None` for a row saved before summaries were
    /// stored; the reader computes it once and stores it with
    /// `Database::set_saved_rough_plan_summary`.
    pub summary: Option<String>,
}

/// A complete saved rough plan including its serialized payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SavedRoughPlan {
    /// Plan metadata (ID, name, timestamps).
    pub meta: SavedRoughPlanMeta,
    /// Version number of the payload schema (for compatibility checks).
    pub payload_version: u32,
    /// Serialized payload text (e.g. TOML document).
    pub payload: String,
}
