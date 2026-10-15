//! The design engine around the editor session: the solve state and the rules for running
//! a solve, the tier-list rows, tier and angle editing, undo and redo, and the numeric
//! keypad model.
//!
//! Everything here is plain Rust on top of [`crate::app_state::AppState`], so it is
//! tested natively; the Slint side (`apps/indicatrix-mobile-android/src/design/{tiers,solve}.rs`)
//! only moves values between these functions and the screen.
//!
//! - `solve`: [`SolveState`] (the last solve, stamped with the design generation it
//!   describes), the job/install pair a worker thread uses, and the auto-solve policy.
//! - `rows`: [`MobileTierRow`], the tier list's rows with their badges, and the status
//!   line.
//! - `edit`: every tier-list edit (angle, name, indices, add, remove, move, orbit tools,
//!   batch actions, undo and redo) with selection renumbering and stale-mast tracking.
//! - `keypad`: [`KeypadState`], the model behind the app's own numeric keypad.
//!
//! Callers that change `state.session` through these functions get back an
//! [`Edited`]; when the session's generation moved they call `design::after_edit` in the
//! app crate.

mod edit;
mod keypad;
mod rows;
mod solve;

#[cfg(test)]
mod tests;

pub use edit::{
    EditOutcome, EditRefusal, Edited, QuickAdd, add_index, add_tier, adopt, adopt_all,
    adopt_selected, can_redo, can_undo, clear_multi, complete_orbit, duplicate, mirror_indices,
    move_tier, nudge_angle, offset_multi, quick_add, redo, redo_hint, remove, remove_index,
    remove_multi, rotate_indices, select_all, select_tier, set_angle_from_text, set_angle_value,
    set_indices_from_text, set_name, toggle_detach, toggle_multi, undo, undo_hint,
};
pub use indicatrix_web_core::solve::SolveResponse;
pub use keypad::{Key, KeypadState};
pub use rows::{
    Badge, BadgeLevel, IndexChipView, MobileTierRow, StatusKind, StatusLine, TierBadges,
    TierDetail, status_line, tier_detail, tier_rows,
};
pub use solve::{
    DirtyScope, FailedSolve, Installed, SolveJob, SolveState, SolvedSnapshot, begin_solve, install,
    install_failure, run_solve, should_auto_solve, should_solve_synchronously,
};
