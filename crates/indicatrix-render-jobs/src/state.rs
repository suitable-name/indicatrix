//! The job state machine: what each row command does, and what a finished run leaves
//! behind. Pure functions, so the rules are tested without a window or a database.

/// Where a job is in its life.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobState {
    /// Waiting for its turn.
    Queued,
    /// Rendering now.
    Running,
    /// Stopped, and can be resumed.
    Paused,
    /// Finished and written.
    Done,
    /// Stopped by an error.
    Failed,
    /// Stopped for good (a restart can bring it back).
    Cancelled,
}

impl JobState {
    /// The word the database stores.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Paused => "paused",
            Self::Done => "done",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    /// Reads a database word.
    #[must_use]
    pub fn parse(word: &str) -> Option<Self> {
        match word {
            "queued" => Some(Self::Queued),
            "running" => Some(Self::Running),
            "paused" => Some(Self::Paused),
            "done" => Some(Self::Done),
            "failed" => Some(Self::Failed),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }

    /// The word shown to people.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Queued => "Queued",
            Self::Running => "Running",
            Self::Paused => "Paused",
            Self::Done => "Done",
            Self::Failed => "Failed",
            Self::Cancelled => "Cancelled",
        }
    }

    /// Whether the job is over for good: `Done` or `Cancelled`.
    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Done | Self::Cancelled)
    }
}

/// A command on one row of the job list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobCommand {
    /// Stop this job; the queue goes on.
    Pause,
    /// Put a paused or failed job back in line.
    Resume,
    /// Render the job again from the beginning.
    Restart,
    /// Stop the job for good.
    Cancel,
    /// Remove the job from the list.
    Delete,
    /// Move the job to the top so it is taken next.
    RunNext,
    /// Move the job one place up.
    MoveUp,
    /// Move the job one place down.
    MoveDown,
}

/// Why a running job is being stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopReason {
    /// The row's Pause button.
    Pause,
    /// The Pause Queue button.
    PauseQueue,
    /// The row's Cancel button.
    Cancel,
    /// The app is closing.
    AppClosing,
}

/// How a run ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunEnd {
    /// Everything was written.
    Finished,
    /// The render was asked to stop.
    Stopped,
    /// An error ended the run.
    Failed,
}

/// What a row command does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandEffect {
    /// Write a new state. `reset_progress` also clears the done frames and the result.
    SetState {
        /// The state to write.
        state: JobState,
        /// Whether the progress, result and error are cleared too.
        reset_progress: bool,
    },
    /// Ask the running job to stop for this reason.
    StopRunning(StopReason),
    /// Remove the row.
    Delete,
    /// Move the row to the top. `requeue` also sets its state to `Queued`.
    MoveToFront {
        /// Whether the state becomes `Queued`.
        requeue: bool,
    },
    /// Move the row by this many places (negative is up).
    Move(i32),
    /// The command does not apply; the sentence says why.
    Refused(&'static str),
}

/// What a row command does to a job in `state`.
#[must_use]
pub const fn command_effect(state: JobState, command: JobCommand) -> CommandEffect {
    use CommandEffect::{Delete, Move, MoveToFront, Refused, SetState, StopRunning};
    use JobCommand as C;
    use JobState as S;
    let requeue = SetState {
        state: S::Queued,
        reset_progress: false,
    };
    let restart = SetState {
        state: S::Queued,
        reset_progress: true,
    };
    match (state, command) {
        // Moving rows works for every state except a running job.
        (S::Running, C::MoveUp | C::MoveDown) => {
            Refused("A running job cannot be moved. Pause it first.")
        }
        (_, C::MoveUp) => Move(-1),
        (_, C::MoveDown) => Move(1),

        (S::Queued, C::Pause) => SetState {
            state: S::Paused,
            reset_progress: false,
        },
        (S::Running, C::Pause) => StopRunning(StopReason::Pause),
        (S::Paused, C::Pause) => Refused("This job is already paused."),
        (S::Done | S::Failed | S::Cancelled, C::Pause) => Refused("This job is not waiting."),

        (S::Paused | S::Failed, C::Resume) => requeue,
        (S::Queued, C::Resume) => Refused("This job is already waiting."),
        (S::Running, C::Resume | C::RunNext) => Refused("This job is already running."),
        (S::Done, C::Resume) => Refused("This job is finished. Use Restart to render it again."),
        (S::Cancelled, C::Resume) => {
            Refused("This job was cancelled. Use Restart to render it again.")
        }

        (S::Queued | S::Running, C::Restart) => {
            Refused("Only a stopped or finished job can be restarted.")
        }
        (S::Paused | S::Done | S::Failed | S::Cancelled, C::Restart) => restart,

        (S::Queued | S::Paused, C::Cancel) => SetState {
            state: S::Cancelled,
            reset_progress: false,
        },
        (S::Running, C::Cancel) => StopRunning(StopReason::Cancel),
        (S::Done | S::Failed | S::Cancelled, C::Cancel) => {
            Refused("Only a waiting, paused or running job can be cancelled.")
        }

        (S::Running, C::Delete) => Refused("A running job cannot be deleted. Cancel it first."),
        (_, C::Delete) => Delete,

        (S::Queued, C::RunNext) => MoveToFront { requeue: false },
        (S::Paused | S::Failed, C::RunNext) => MoveToFront { requeue: true },
        (S::Done | S::Cancelled, C::RunNext) => {
            Refused("Use Restart to render a finished or cancelled job again.")
        }
    }
}

/// The state a job is left in after a run.
#[must_use]
pub const fn state_after_run(end: RunEnd, stop: Option<StopReason>) -> JobState {
    match end {
        RunEnd::Finished => JobState::Done,
        RunEnd::Failed => JobState::Failed,
        RunEnd::Stopped => match stop {
            Some(StopReason::Cancel) => JobState::Cancelled,
            Some(StopReason::Pause | StopReason::PauseQueue | StopReason::AppClosing) | None => {
                JobState::Paused
            }
        },
    }
}

/// Whether the queue goes on with the next job after a run that was stopped for `stop`.
/// False only for Pause Queue and a closing app.
#[must_use]
pub const fn queue_continues_after(stop: Option<StopReason>) -> bool {
    !matches!(stop, Some(StopReason::PauseQueue | StopReason::AppClosing))
}

/// The state a job has when the app starts: a job that was running was interrupted.
#[must_use]
pub const fn state_on_app_start(state: JobState) -> JobState {
    match state {
        JobState::Running => JobState::Paused,
        other => other,
    }
}

/// The note kept with a job found running at start-up.
pub const INTERRUPTED_NOTE: &str = "The app closed while this job was running.";
/// The note kept with a job stopped because the app is closing.
pub const APP_CLOSING_NOTE: &str = "Stopped when the app closed.";

/// Which buttons a job row shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowActions {
    /// Show Pause.
    pub pause: bool,
    /// Show Resume.
    pub resume: bool,
    /// Show Restart.
    pub restart: bool,
    /// Show Cancel.
    pub cancel: bool,
    /// Show Delete.
    pub delete: bool,
    /// Show Run Next.
    pub run_next: bool,
    /// Show Up.
    pub move_up: bool,
    /// Show Down.
    pub move_down: bool,
}

const fn applies(state: JobState, command: JobCommand) -> bool {
    !matches!(command_effect(state, command), CommandEffect::Refused(_))
}

/// The buttons of a row: a button shows exactly when its command is not refused. Up is
/// hidden on the first row and Down on the last.
#[must_use]
pub const fn row_actions(state: JobState, is_first: bool, is_last: bool) -> RowActions {
    RowActions {
        pause: applies(state, JobCommand::Pause),
        resume: applies(state, JobCommand::Resume),
        restart: applies(state, JobCommand::Restart),
        cancel: applies(state, JobCommand::Cancel),
        delete: applies(state, JobCommand::Delete),
        run_next: applies(state, JobCommand::RunNext),
        move_up: applies(state, JobCommand::MoveUp) && !is_first,
        move_down: applies(state, JobCommand::MoveDown) && !is_last,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATES: [JobState; 6] = [
        JobState::Queued,
        JobState::Running,
        JobState::Paused,
        JobState::Done,
        JobState::Failed,
        JobState::Cancelled,
    ];
    const COMMANDS: [JobCommand; 8] = [
        JobCommand::Pause,
        JobCommand::Resume,
        JobCommand::Restart,
        JobCommand::Cancel,
        JobCommand::Delete,
        JobCommand::RunNext,
        JobCommand::MoveUp,
        JobCommand::MoveDown,
    ];

    /// The effect shapes of the spec table, without the refusal sentences.
    #[derive(Debug, PartialEq, Eq)]
    enum Want {
        Set(JobState, bool),
        Stop(StopReason),
        Del,
        Front(bool),
        Mv(i32),
        No,
    }

    fn shape(effect: CommandEffect) -> Want {
        match effect {
            CommandEffect::SetState {
                state,
                reset_progress,
            } => Want::Set(state, reset_progress),
            CommandEffect::StopRunning(reason) => Want::Stop(reason),
            CommandEffect::Delete => Want::Del,
            CommandEffect::MoveToFront { requeue } => Want::Front(requeue),
            CommandEffect::Move(by) => Want::Mv(by),
            CommandEffect::Refused(_) => Want::No,
        }
    }

    /// Spec section 2, one row per state, columns in `COMMANDS` order.
    fn table() -> [[Want; 8]; 6] {
        use JobState::{Cancelled, Paused, Queued};
        use Want::{Del, Front, Mv, No, Set, Stop};
        [
            // Queued
            [
                Set(Paused, false),
                No,
                No,
                Set(Cancelled, false),
                Del,
                Front(false),
                Mv(-1),
                Mv(1),
            ],
            // Running
            [
                Stop(StopReason::Pause),
                No,
                No,
                Stop(StopReason::Cancel),
                No,
                No,
                No,
                No,
            ],
            // Paused
            [
                No,
                Set(Queued, false),
                Set(Queued, true),
                Set(Cancelled, false),
                Del,
                Front(true),
                Mv(-1),
                Mv(1),
            ],
            // Done
            [No, No, Set(Queued, true), No, Del, No, Mv(-1), Mv(1)],
            // Failed
            [
                No,
                Set(Queued, false),
                Set(Queued, true),
                No,
                Del,
                Front(true),
                Mv(-1),
                Mv(1),
            ],
            // Cancelled
            [No, No, Set(Queued, true), No, Del, No, Mv(-1), Mv(1)],
        ]
    }

    #[test]
    fn every_state_and_command_pair_matches_the_spec_table() {
        let table = table();
        for (row, state) in STATES.into_iter().enumerate() {
            for (column, command) in COMMANDS.into_iter().enumerate() {
                assert_eq!(
                    shape(command_effect(state, command)),
                    table[row][column],
                    "{state:?} + {command:?}"
                );
            }
        }
    }

    #[test]
    fn every_refusal_is_one_sentence() {
        for state in STATES {
            for command in COMMANDS {
                if let CommandEffect::Refused(reason) = command_effect(state, command) {
                    assert!(reason.ends_with('.'), "{state:?} + {command:?}: {reason}");
                    assert!(reason.len() > 10, "{state:?} + {command:?}: {reason}");
                }
            }
        }
        assert_eq!(
            command_effect(JobState::Running, JobCommand::Delete),
            CommandEffect::Refused("A running job cannot be deleted. Cancel it first.")
        );
    }

    #[test]
    fn after_run_table() {
        use RunEnd::{Failed, Finished, Stopped};
        use StopReason::{AppClosing, Cancel, Pause, PauseQueue};
        let stops = [
            None,
            Some(Pause),
            Some(PauseQueue),
            Some(Cancel),
            Some(AppClosing),
        ];
        for stop in stops {
            assert_eq!(state_after_run(Finished, stop), JobState::Done);
            assert_eq!(state_after_run(Failed, stop), JobState::Failed);
        }
        assert_eq!(state_after_run(Stopped, Some(Pause)), JobState::Paused);
        assert_eq!(state_after_run(Stopped, Some(PauseQueue)), JobState::Paused);
        assert_eq!(state_after_run(Stopped, Some(Cancel)), JobState::Cancelled);
        assert_eq!(state_after_run(Stopped, Some(AppClosing)), JobState::Paused);
        assert_eq!(state_after_run(Stopped, None), JobState::Paused);
    }

    #[test]
    fn the_queue_stops_only_for_pause_queue_and_app_closing() {
        assert!(queue_continues_after(None));
        assert!(queue_continues_after(Some(StopReason::Pause)));
        assert!(queue_continues_after(Some(StopReason::Cancel)));
        assert!(!queue_continues_after(Some(StopReason::PauseQueue)));
        assert!(!queue_continues_after(Some(StopReason::AppClosing)));
    }

    #[test]
    fn only_running_changes_at_app_start() {
        for state in STATES {
            let expected = if state == JobState::Running {
                JobState::Paused
            } else {
                state
            };
            assert_eq!(state_on_app_start(state), expected);
        }
    }

    #[test]
    fn words_round_trip_and_terminal_states() {
        for state in STATES {
            assert_eq!(JobState::parse(state.as_str()), Some(state));
            assert!(state.label().eq_ignore_ascii_case(state.as_str()));
            assert!(state.label().starts_with(|c: char| c.is_ascii_uppercase()));
        }
        assert_eq!(JobState::parse("Queued"), None);
        assert_eq!(JobState::parse(""), None);
        for state in STATES {
            let terminal = matches!(state, JobState::Done | JobState::Cancelled);
            assert_eq!(state.is_terminal(), terminal, "{state:?}");
        }
    }

    #[test]
    fn row_actions_follow_the_command_table() {
        for state in STATES {
            let middle = row_actions(state, false, false);
            let want = |command| !matches!(shape(command_effect(state, command)), Want::No);
            assert_eq!(middle.pause, want(JobCommand::Pause));
            assert_eq!(middle.resume, want(JobCommand::Resume));
            assert_eq!(middle.restart, want(JobCommand::Restart));
            assert_eq!(middle.cancel, want(JobCommand::Cancel));
            assert_eq!(middle.delete, want(JobCommand::Delete));
            assert_eq!(middle.run_next, want(JobCommand::RunNext));
            assert_eq!(middle.move_up, want(JobCommand::MoveUp));
            assert_eq!(middle.move_down, want(JobCommand::MoveDown));

            let first = row_actions(state, true, false);
            assert!(!first.move_up);
            assert_eq!(first.move_down, middle.move_down);
            let last = row_actions(state, false, true);
            assert!(!last.move_down);
            assert_eq!(last.move_up, middle.move_up);
        }
        let running = row_actions(JobState::Running, false, false);
        assert!(running.pause && running.cancel);
        assert!(!running.delete && !running.resume && !running.move_up);
    }

    #[test]
    fn notes_are_the_spec_sentences() {
        assert_eq!(
            INTERRUPTED_NOTE,
            "The app closed while this job was running."
        );
        assert_eq!(APP_CLOSING_NOTE, "Stopped when the app closed.");
    }
}
