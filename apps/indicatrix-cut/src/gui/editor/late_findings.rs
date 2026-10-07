//! Manufacturability findings that reach the UI after their frame.
//!
//! The plan worker hands its frame to the render worker first and only then runs the costly
//! concave-tool check (check 6): a frame must not wait for a pass that builds one solid per
//! tool. The frame's rows are therefore pushed with the checks that need no solid, and
//! [`apply_late_findings`] brings the full pass in when it lands.

use super::{auto_solve, view};
use crate::{MainWindow, bridge::render_thread::RenderContext};
use indicatrix::geometry::meet_solver::SolvedTier;
use indicatrix_cut_core::{Design, ManufacturabilityWarning};
use std::sync::{Arc, Mutex, PoisonError, atomic::Ordering};

/// Whether findings computed for `generation` still describe the editor's design, whose
/// generation counter reads `live`. Findings for a design the cutter has edited since would
/// badge the wrong rows.
#[must_use]
const fn findings_are_current(generation: u64, live: u64) -> bool {
    generation == live
}

/// What [`apply_late_findings`] came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::gui) enum LatePush {
    /// Nothing is left to do: the rows were refreshed, or the findings describe a design the
    /// editor no longer has (or there is no editor to show them to).
    Settled,
    /// The editor state is held by a callback that is still running, so the rows were not
    /// touched. The findings are still owed: the caller must try again later, and must not
    /// count them as shown.
    Held,
}

/// What to do with late findings, see [`decide`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Decision {
    /// Show them.
    Show,
    /// The editor has moved on: they badge rows that are gone.
    Moved,
    /// The state is held: ask again.
    Held,
}

/// The [`Decision`] for findings computed for `generation`, given the editor's own generation
/// `live` -- `None` when its state could not be read because a callback holds it.
#[must_use]
const fn decide(live: Option<u64>, generation: u64) -> Decision {
    match live {
        None => Decision::Held,
        Some(live) if findings_are_current(generation, live) => Decision::Show,
        Some(_) => Decision::Moved,
    }
}

/// Pushes the manufacturability `findings` of the plan for `generation` (its `design` and
/// `solved` masts) into the tier table and the warning list, when that plan is still the
/// editor's design; otherwise does nothing.
///
/// `pub(in crate::gui)` because its caller, `gui::solid_sink::SlintSolidSink`, lives in
/// `gui`, outside this module. Runs on the UI thread. The editor state is only
/// `try_borrow`ed: a state held by a callback that is still running skips the refresh and
/// reports [`LatePush::Held`], so the caller keeps the findings as owed and tries again
/// (the row push of the frame already cancelled the debounced auto-solve, so nothing else
/// would bring the findings in).
pub(in crate::gui) fn apply_late_findings(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    generation: u64,
    design: &Design,
    solved: &[SolvedTier],
    findings: &[ManufacturabilityWarning],
) -> LatePush {
    let Some(state) = auto_solve::editor_state() else {
        return LatePush::Settled;
    };
    // The generation and the selection, copied out so the borrow ends here.
    let read = state.try_borrow().ok().map(|state| {
        (
            state.generation.load(Ordering::Relaxed),
            state.multi_selected.clone(),
        )
    });
    match decide(read.as_ref().map(|(live, _)| *live), generation) {
        Decision::Held => return LatePush::Held,
        Decision::Moved => return LatePush::Settled,
        Decision::Show => {}
    }
    let multi_selected = read.map(|(_, selected)| selected).unwrap_or_default();
    let custom_materials = render_ctx
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .custom_materials
        .as_ref()
        .clone();
    view::push_late_findings(
        ui,
        design,
        solved,
        &multi_selected,
        &custom_materials,
        findings,
    );
    LatePush::Settled
}

#[cfg(test)]
mod tests {
    use super::{Decision, decide, findings_are_current};

    /// Findings are shown only while the editor's design is still the one they were
    /// computed for.
    #[test]
    fn findings_describe_only_the_generation_they_were_computed_for() {
        assert!(findings_are_current(7, 7));
        assert!(!findings_are_current(7, 8), "one edit later");
        assert!(
            !findings_are_current(8, 7),
            "a plan for a design not yet reached"
        );
    }

    /// F4-11: a held editor state is not a verdict on the findings. It is "ask again", told
    /// apart from "the design moved on", which is "drop them".
    #[test]
    fn a_held_editor_state_is_retried_and_a_moved_design_is_dropped() {
        assert_eq!(decide(Some(7), 7), Decision::Show);
        assert_eq!(decide(Some(8), 7), Decision::Moved);
        assert_eq!(decide(Some(7), 8), Decision::Moved);
        assert_eq!(decide(None, 7), Decision::Held);
    }
}
