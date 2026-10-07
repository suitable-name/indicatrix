//! The UI events the solving, checking, optimizing and comparing tutorials wait for.
//!
//! Most of what these lessons check can be read from the design (a tier moved, the material
//! changed, a retarget or a sweep applied) or from the solve verdict. The rest happens only on
//! screen: the Solve button was pressed, the auto-solve list was changed, the verdict list was
//! opened, a Deep Solve ran, a result list appeared, a snapshot was taken, a comparison opened.
//! The desktop reports each of those once, by name, through `GuideModel.event` (a Slint call or
//! `gui::tutorial_events::raise`), at the place the action succeeds; a step then waits for the
//! name with `Goal::Event`.
//!
//! Every name here is listed in [`ALL`], which `catalog::EVENTS` includes, so a step that waits
//! for a name nobody reports fails the registry test. The place that reports each one is named
//! in its doc line, and the desktop's `tests/guide_highlights_solving.rs` checks that every name
//! has a call site.

/// The Solve button was pressed (or F5): the solve ran, or a background solve was started.
///
/// Reported by `callbacks/tier_actions/lifecycle.rs` after the refresh, so the verdict and the
/// solve state a step reads are already current for a small design.
pub const SOLVE_REQUESTED: &str = "solve_requested";

/// The auto-solve list was set to Off (`editor_command_bar.slint`).
pub const AUTO_SOLVE_OFF: &str = "auto_solve_off";

/// The auto-solve list was set to a time: 150 ms, 300 ms, 1 s or 3 s
/// (`editor_command_bar.slint`).
pub const AUTO_SOLVE_ON: &str = "auto_solve_on";

/// The overall verdict badge was opened: its list of reasons is on screen
/// (`verdict_badge.slint`).
pub const VERDICT_OPENED: &str = "verdict_opened";

/// A Deep Solve run started (`callbacks/solve_actions/deep_solve_run.rs`).
pub const DEEP_SOLVE_STARTED: &str = "deep_solve_started";

/// A Deep Solve run ended and its verdict is on screen
/// (`callbacks/solve_actions/deep_solve_run.rs`).
pub const DEEP_SOLVE_FINISHED: &str = "deep_solve_finished";

/// An entry of the Optimize tab's Objective list was chosen (`optimize_panel/mod.rs`).
pub const OPTIMIZE_PRESET_CHOSEN: &str = "optimize_preset_chosen";

/// The Optimize tab's angle ranges were opened (`optimize_panel/mod.rs`).
pub const OPTIMIZE_RANGES_OPENED: &str = "optimize_ranges_opened";

/// An Optimize run finished with a result a candidate of which can be applied
/// (`callbacks/solve_actions/optimize_outcome.rs`).
pub const OPTIMIZE_FINISHED: &str = "optimize_finished";

/// A row of the Optimize tab's candidate list was clicked (`optimize_panel/mod.rs`).
pub const OPTIMIZE_CANDIDATE_PICKED: &str = "optimize_candidate_picked";

/// The History tab's Variants view was opened (`editor_inspector/history_tab.slint`).
pub const VARIANTS_OPENED: &str = "variants_opened";

/// A variant was saved (`variants/actions.rs`).
pub const VARIANT_SAVED: &str = "variant_saved";

/// Two designs were compared from the Variants view, as pictures or as text
/// (`variants/actions.rs`).
pub const VARIANTS_COMPARED: &str = "variants_compared";

/// A snapshot of the design was taken (`callbacks/retarget_actions/snapshot.rs`).
pub const SNAPSHOT_TAKEN: &str = "snapshot_taken";

/// The Compare to Snapshot table opened (`callbacks/retarget_actions/snapshot.rs`).
pub const COMPARE_OPENED: &str = "compare_opened";

/// The visual compare window opened, from the snapshot table, the Retarget dialog, the Optimize
/// tab or the Variants view (`compare/wiring.rs`).
pub const COMPARE_WINDOW_OPENED: &str = "compare_window_opened";

/// The Edit as Text dialog showed a line-by-line comparison of its text with the saved file or
/// the snapshot (`raw_text/mod.rs`).
pub const RAW_TEXT_DIFF_SHOWN: &str = "raw_text_diff_shown";

/// Every event above, for `catalog::EVENTS` and for the tests of this area.
pub const ALL: &[&str] = &[
    SOLVE_REQUESTED,
    AUTO_SOLVE_OFF,
    AUTO_SOLVE_ON,
    VERDICT_OPENED,
    DEEP_SOLVE_STARTED,
    DEEP_SOLVE_FINISHED,
    OPTIMIZE_PRESET_CHOSEN,
    OPTIMIZE_RANGES_OPENED,
    OPTIMIZE_FINISHED,
    OPTIMIZE_CANDIDATE_PICKED,
    VARIANTS_OPENED,
    VARIANT_SAVED,
    VARIANTS_COMPARED,
    SNAPSHOT_TAKEN,
    COMPARE_OPENED,
    COMPARE_WINDOW_OPENED,
    RAW_TEXT_DIFF_SHOWN,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_listed_once_and_is_lower_case_words() {
        for (index, name) in ALL.iter().enumerate() {
            assert!(!ALL[..index].contains(name), "{name} is listed twice");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not lower_snake_case"
            );
        }
    }
}
