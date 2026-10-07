//! The requirements a command can have, and the plain-English reason shown when one fails.
//!
//! A command lists its requirements as a short slice of [`Check`]s. The first one that
//! fails is the reason the palette shows under the command. Putting the guide's lock
//! first in every list means a guide-locked command always says "Locked by the guide",
//! whatever else is also missing.

use super::state::{CommandState, Fact, GuideGroup};

/// The reason shown for a command whose controls the worked-example guide has locked.
pub const LOCKED_BY_GUIDE: &str = "Locked by the guide";

/// One requirement of a command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The editor is available.
    Editor,
    /// A design is open.
    Design,
    /// A tier is selected.
    TierSelected,
    /// There is something to undo.
    CanUndo,
    /// There is something to redo.
    CanRedo,
    /// Nothing else is solving: no Solve, Deep Solve or Optimize is running.
    Idle,
    /// A normal Solve is running.
    SolveRunning,
    /// Deep Solve is running.
    DeepSolveRunning,
    /// Optimize is running.
    OptimizeRunning,
    /// Deep Solve has what it needs to start.
    DeepSolveAvailable,
    /// Optimize has a free tier to move.
    OptimizeAvailable,
    /// A snapshot exists.
    HasSnapshot,
    /// The design has been saved or exported this session.
    HasLastSaved,
    /// A design is selected in the library list.
    LibrarySelected,
    /// The preview-image batch is not running.
    PreviewBatchIdle,
    /// The tilt-curve batch is not running.
    TiltBatchIdle,
    /// The Edit view is on screen.
    EditView,
    /// The viewport is not in Diagram mode.
    NotDiagramView,
    /// The interface is in Advanced mode, so switching to Simple does something.
    AdvancedInterfaceOn,
    /// The interface is in Simple mode, so switching to Advanced does something.
    SimpleInterfaceOn,
    /// The control the command shows exists only in the Advanced interface.
    NeedsAdvancedInterface,
    /// The guide leaves this group of controls usable.
    Guide(GuideGroup),
}

impl Check {
    /// Whether the requirement holds in `state`; the error is the reason it does not.
    ///
    /// # Errors
    ///
    /// Returns a short plain-English reason when the requirement does not hold.
    pub fn verdict(self, state: &CommandState) -> Result<(), &'static str> {
        let (holds, reason) = match self {
            Self::Editor => (state.has(Fact::Editor), "The editor is not ready yet"),
            Self::Design => (state.has(Fact::Design), "Open or create a design first"),
            Self::TierSelected => (state.has(Fact::TierSelected), "Select a tier first"),
            Self::CanUndo => (state.has(Fact::CanUndo), "Nothing to undo"),
            Self::CanRedo => (state.has(Fact::CanRedo), "Nothing to redo"),
            Self::Idle => (
                !state.has(Fact::Busy),
                "Wait for the running solve to finish",
            ),
            Self::SolveRunning => (state.has(Fact::SolveRunning), "No solve is running"),
            Self::DeepSolveRunning => (
                state.has(Fact::DeepSolveRunning),
                "Deep Solve is not running",
            ),
            Self::OptimizeRunning => (state.has(Fact::OptimizeRunning), "Optimize is not running"),
            Self::DeepSolveAvailable => (
                state.has(Fact::DeepSolveAvailable),
                "Needs a library design with printed proportions",
            ),
            Self::OptimizeAvailable => (
                state.has(Fact::OptimizeAvailable),
                "Needs at least one free tier",
            ),
            Self::HasSnapshot => (state.has(Fact::HasSnapshot), "Take a snapshot first"),
            Self::HasLastSaved => (state.has(Fact::HasLastSaved), "Nothing has been saved yet"),
            Self::LibrarySelected => (
                state.has(Fact::LibrarySelected),
                "Select a design in the library first",
            ),
            Self::PreviewBatchIdle => (
                !state.has(Fact::PreviewBatchRunning),
                "A preview batch is already running",
            ),
            Self::TiltBatchIdle => (
                !state.has(Fact::TiltBatchRunning),
                "A tilt-curve batch is already running",
            ),
            Self::EditView => (state.has(Fact::EditView), "Open the Edit tab first"),
            Self::NotDiagramView => (
                !state.has(Fact::DiagramView),
                "Not available in the Diagram view",
            ),
            Self::AdvancedInterfaceOn => (
                !state.has(Fact::SimpleInterface),
                "Already using the Simple interface",
            ),
            Self::SimpleInterfaceOn => (
                state.has(Fact::SimpleInterface),
                "Already using the Advanced interface",
            ),
            Self::NeedsAdvancedInterface => (
                !state.has(Fact::SimpleInterface),
                "Switch to the Advanced interface first",
            ),
            Self::Guide(group) => (state.has(Fact::Allows(group)), LOCKED_BY_GUIDE),
        };
        if holds { Ok(()) } else { Err(reason) }
    }
}

/// The first requirement in `checks` that fails in `state`, as its reason.
///
/// # Errors
///
/// Returns the reason of the first failing check.
pub fn first_failure(checks: &[Check], state: &CommandState) -> Result<(), &'static str> {
    checks.iter().try_for_each(|check| check.verdict(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ready_state_passes_every_check_except_the_opposite_interface_ones() {
        let state = CommandState::ready();
        // `ready()` is the Advanced interface, with nothing running as busy: the Simple-only
        // check is the one that must fail.
        assert_eq!(
            Check::SimpleInterfaceOn.verdict(&state),
            Err("Already using the Advanced interface")
        );
        for check in [
            Check::Editor,
            Check::Design,
            Check::TierSelected,
            Check::CanUndo,
            Check::CanRedo,
            Check::Idle,
            Check::SolveRunning,
            Check::DeepSolveRunning,
            Check::OptimizeRunning,
            Check::DeepSolveAvailable,
            Check::OptimizeAvailable,
            Check::HasSnapshot,
            Check::HasLastSaved,
            Check::LibrarySelected,
            Check::PreviewBatchIdle,
            Check::TiltBatchIdle,
            Check::EditView,
            Check::NotDiagramView,
            Check::AdvancedInterfaceOn,
            Check::NeedsAdvancedInterface,
        ] {
            assert_eq!(check.verdict(&state), Ok(()), "{check:?}");
        }
    }

    #[test]
    fn every_requirement_names_a_reason_when_it_fails() {
        let empty = CommandState::default();
        for check in [
            Check::Editor,
            Check::Design,
            Check::TierSelected,
            Check::CanUndo,
            Check::CanRedo,
            Check::SolveRunning,
            Check::DeepSolveRunning,
            Check::OptimizeRunning,
            Check::DeepSolveAvailable,
            Check::OptimizeAvailable,
            Check::HasSnapshot,
            Check::HasLastSaved,
            Check::LibrarySelected,
            Check::EditView,
            Check::SimpleInterfaceOn,
            Check::Guide(GuideGroup::Advanced),
        ] {
            let reason = check.verdict(&empty).unwrap_err();
            assert!(!reason.is_empty(), "{check:?}");
        }
    }

    #[test]
    fn the_busy_checks_fail_only_while_something_runs() {
        let busy = CommandState::ready().with(Fact::Busy, true);
        assert_eq!(
            Check::Idle.verdict(&busy),
            Err("Wait for the running solve to finish")
        );
        let batches = CommandState::ready()
            .with(Fact::PreviewBatchRunning, true)
            .with(Fact::TiltBatchRunning, true);
        assert!(Check::PreviewBatchIdle.verdict(&batches).is_err());
        assert!(Check::TiltBatchIdle.verdict(&batches).is_err());
    }

    #[test]
    fn a_guide_lock_says_so_and_names_no_other_reason() {
        for group in GuideGroup::ALL {
            let locked = CommandState::ready().with(Fact::Allows(group), false);
            assert_eq!(
                Check::Guide(group).verdict(&locked),
                Err(LOCKED_BY_GUIDE),
                "{group:?}"
            );
            assert_eq!(Check::Guide(group).verdict(&CommandState::ready()), Ok(()));
        }
    }

    #[test]
    fn the_first_failing_check_gives_the_reason() {
        let state = CommandState::ready()
            .with(Fact::Allows(GuideGroup::Advanced), false)
            .with(Fact::Design, false);
        // The guide comes first in the list, so its lock wins over the missing design.
        let checks = [Check::Guide(GuideGroup::Advanced), Check::Design];
        assert_eq!(first_failure(&checks, &state), Err(LOCKED_BY_GUIDE));
        // Without the lock the missing design is the reason.
        let unlocked = state.with(Fact::Allows(GuideGroup::Advanced), true);
        assert_eq!(
            first_failure(&checks, &unlocked),
            Err("Open or create a design first")
        );
        assert_eq!(first_failure(&[], &CommandState::default()), Ok(()));
    }

    #[test]
    fn the_diagram_view_blocks_the_viewport_only_commands() {
        let diagram = CommandState::ready().with(Fact::DiagramView, true);
        assert_eq!(
            Check::NotDiagramView.verdict(&diagram),
            Err("Not available in the Diagram view")
        );
    }

    #[test]
    fn the_interface_switch_checks_follow_the_current_mode() {
        let simple = CommandState::ready().with(Fact::SimpleInterface, true);
        assert_eq!(Check::SimpleInterfaceOn.verdict(&simple), Ok(()));
        assert_eq!(
            Check::AdvancedInterfaceOn.verdict(&simple),
            Err("Already using the Simple interface")
        );
    }

    #[test]
    fn a_control_that_only_the_advanced_interface_has_says_to_switch() {
        let simple = CommandState::ready().with(Fact::SimpleInterface, true);
        assert_eq!(
            Check::NeedsAdvancedInterface.verdict(&simple),
            Err("Switch to the Advanced interface first")
        );
        assert_eq!(
            Check::NeedsAdvancedInterface.verdict(&CommandState::ready()),
            Ok(())
        );
    }
}
