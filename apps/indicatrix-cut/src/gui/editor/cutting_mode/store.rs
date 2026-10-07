//! The library side of cutting mode: a design's done marks and index ticks.
//!
//! They live in the local library (`indicatrix_vault`'s `design_cut_progress` table), keyed by
//! the design's UUID and never written into the design file, so a copy of the file, a file sent
//! to someone else and a file opened on another computer all start with a clean slate while the
//! design on this computer remembers where the cutter was.
//!
//! Every function takes the already-normalised design UUID and returns plain `String` errors for
//! the caller to show; none of them touches the window.

use indicatrix_editor::cutting_mode::progress::{MarkChange, Progress};
use indicatrix_vault::db::sqlite::Database;
use std::{
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

/// The marks stored for design `uuid`.
pub(super) fn load(db: &Mutex<Database>, uuid: &str) -> Result<Progress, String> {
    let marks = db
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .cut_progress(uuid)
        .map_err(|err| format!("{err:#}"))?;
    Ok(Progress::from_marks(
        marks
            .into_iter()
            .map(|mark| (mark.step_key, mark.step_signature)),
    ))
}

/// Stores `changes` for design `uuid`, in order. `now` is Unix seconds. Stops at the first
/// change that fails; the ones before it stay stored.
pub(super) fn save(
    db: &Mutex<Database>,
    uuid: &str,
    changes: &[MarkChange],
    now: i64,
) -> Result<(), String> {
    let database = db.lock().unwrap_or_else(PoisonError::into_inner);
    changes
        .iter()
        .try_for_each(|change| save_one(&database, uuid, change, now))
}

fn save_one(database: &Database, uuid: &str, change: &MarkChange, now: i64) -> Result<(), String> {
    match change {
        MarkChange::Mark { key, signature } => database
            .mark_step_done(uuid, key, signature, now)
            .map_err(|err| format!("{err:#}")),
        MarkChange::Unmark { key } => database
            .unmark_step(uuid, key)
            .map(|_| ())
            .map_err(|err| format!("{err:#}")),
    }
}

/// Removes every mark of design `uuid`.
pub(super) fn clear(db: &Mutex<Database>, uuid: &str) -> Result<(), String> {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clear_cut_progress(uuid)
        .map(|_| ())
        .map_err(|err| format!("{err:#}"))
}

/// The time to store with a mark: Unix seconds now.
pub(super) fn unix_now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX)
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicatrix_cut_core::{ConstraintTier, Design, PreformSpec, ScheduleMeta};
    use indicatrix_editor::cutting_mode::{build_plan, progress::StepState};

    const DESIGN_A: &str = "0b9e6a4e-5f1d-4c7a-9a52-1e3f7d8c2b10";
    const DESIGN_B: &str = "7c3d2f19-8a64-4e0b-b1d5-9f2a6c4e8d73";

    fn database() -> Mutex<Database> {
        Mutex::new(Database::new(Some(":memory:")).expect("an in-memory database"))
    }

    fn plan() -> indicatrix_editor::cutting_mode::CuttingPlan {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let solved = design.solve().expect("every tier is pinned");
        build_plan(&design, &solved, &[]).expect("a plan")
    }

    #[test]
    fn nothing_is_stored_for_a_design_that_was_never_marked() {
        let db = database();
        assert_eq!(load(&db, DESIGN_A), Ok(Progress::default()));
    }

    #[test]
    fn marks_and_ticks_survive_a_reload_and_stay_with_their_design() {
        let db = database();
        let plan = plan();
        let mut progress = Progress::default();

        let done = progress.toggle_done(&plan.steps[0]);
        progress.apply(&done);
        let tick = progress.toggle_chip(&plan.steps[2], 1);
        progress.apply(&tick);
        let changes: Vec<MarkChange> = done.into_iter().chain(tick).collect();
        save(&db, DESIGN_A, &changes, 1_000).expect("saved");

        let again = load(&db, DESIGN_A).expect("loaded");
        assert_eq!(again, progress);
        assert_eq!(again.state(&plan.steps[0]), StepState::Done);
        assert!(again.chip_ticked(&plan.steps[2], 1));
        assert_eq!(again.resume_position(&plan.steps), 1);
        assert_eq!(load(&db, DESIGN_B), Ok(Progress::default()));
    }

    #[test]
    fn taking_a_mark_back_removes_it() {
        let db = database();
        let plan = plan();
        let mut progress = Progress::default();
        let mark = progress.toggle_done(&plan.steps[3]);
        progress.apply(&mark);
        save(&db, DESIGN_A, &mark, 5).expect("saved");

        let unmark = progress.toggle_done(&plan.steps[3]);
        save(&db, DESIGN_A, &unmark, 6).expect("saved");
        assert_eq!(load(&db, DESIGN_A), Ok(Progress::default()));
    }

    #[test]
    fn marking_again_replaces_the_stored_fingerprint() {
        let db = database();
        let plan = plan();
        let stale = vec![MarkChange::Mark {
            key: plan.steps[1].key.clone(),
            signature: "0000000000000000".to_owned(),
        }];
        save(&db, DESIGN_A, &stale, 1).expect("saved");
        let progress = load(&db, DESIGN_A).expect("loaded");
        assert_eq!(progress.state(&plan.steps[1]), StepState::Changed);

        let fresh = progress.mark_done(&plan.steps[1]);
        save(&db, DESIGN_A, &fresh, 2).expect("saved");
        let progress = load(&db, DESIGN_A).expect("loaded");
        assert_eq!(progress.state(&plan.steps[1]), StepState::Done);
    }

    #[test]
    fn reset_clears_one_design_only() {
        let db = database();
        let plan = plan();
        let mark = Progress::default().toggle_done(&plan.steps[0]);
        save(&db, DESIGN_A, &mark, 1).expect("saved");
        save(&db, DESIGN_B, &mark, 1).expect("saved");

        clear(&db, DESIGN_A).expect("cleared");
        assert_eq!(load(&db, DESIGN_A), Ok(Progress::default()));
        assert_ne!(load(&db, DESIGN_B), Ok(Progress::default()));
    }

    #[test]
    fn a_name_that_is_not_a_uuid_is_an_error_not_a_panic() {
        let db = database();
        assert!(load(&db, "not-a-uuid").is_err());
        let mark = MarkChange::Unmark {
            key: "t1".to_owned(),
        };
        assert!(save(&db, "not-a-uuid", &[mark], 1).is_err());
        assert!(clear(&db, "not-a-uuid").is_err());
    }
}
