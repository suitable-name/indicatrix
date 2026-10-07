//! Queue ordering: which job runs next, and how rows move.

use crate::state::JobState;

/// One row of the queue, as far as ordering is concerned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueEntry {
    /// The job id.
    pub id: i64,
    /// The job state.
    pub state: JobState,
}

/// The first `Queued` job in list order, if any.
#[must_use]
pub fn next_to_run(entries: &[QueueEntry]) -> Option<i64> {
    entries
        .iter()
        .find(|entry| entry.state == JobState::Queued)
        .map(|entry| entry.id)
}

/// The ids with `id` moved by `delta` places (negative is up), clamped to the ends. An
/// unknown id leaves the list unchanged.
#[must_use]
pub fn moved(ids: &[i64], id: i64, delta: i32) -> Vec<i64> {
    let mut out = ids.to_vec();
    let Some(from) = out.iter().position(|other| *other == id) else {
        return out;
    };
    let last = out.len() - 1;
    let target = (i64::try_from(from).unwrap_or(0) + i64::from(delta))
        .clamp(0, i64::try_from(last).unwrap_or(0));
    let to = usize::try_from(target).unwrap_or(from);
    let item = out.remove(from);
    out.insert(to, item);
    out
}

/// The ids with `id` moved to the top, the others keeping their order. An unknown id
/// leaves the list unchanged.
#[must_use]
pub fn moved_to_front(ids: &[i64], id: i64) -> Vec<i64> {
    let mut out = ids.to_vec();
    if let Some(from) = out.iter().position(|other| *other == id) {
        let item = out.remove(from);
        out.insert(0, item);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(id: i64, state: JobState) -> QueueEntry {
        QueueEntry { id, state }
    }

    #[test]
    fn next_to_run_takes_the_first_queued_row_in_list_order() {
        let entries = [
            entry(5, JobState::Running),
            entry(4, JobState::Paused),
            entry(9, JobState::Done),
            entry(3, JobState::Failed),
            entry(8, JobState::Cancelled),
            entry(7, JobState::Queued),
            entry(1, JobState::Queued),
        ];
        assert_eq!(next_to_run(&entries), Some(7));
        assert_eq!(next_to_run(&entries[..5]), None);
        assert_eq!(next_to_run(&[]), None);
    }

    #[test]
    fn moved_shifts_and_clamps_at_both_ends() {
        let ids = [10, 20, 30, 40];
        assert_eq!(moved(&ids, 30, -1), vec![10, 30, 20, 40]);
        assert_eq!(moved(&ids, 20, 1), vec![10, 30, 20, 40]);
        assert_eq!(moved(&ids, 10, -1), vec![10, 20, 30, 40]);
        assert_eq!(moved(&ids, 40, 1), vec![10, 20, 30, 40]);
        assert_eq!(moved(&ids, 40, -100), vec![40, 10, 20, 30]);
        assert_eq!(moved(&ids, 10, 100), vec![20, 30, 40, 10]);
        assert_eq!(moved(&ids, 20, 0), vec![10, 20, 30, 40]);
    }

    #[test]
    fn an_unknown_id_leaves_the_list_unchanged() {
        let ids = [1, 2, 3];
        assert_eq!(moved(&ids, 99, 1), vec![1, 2, 3]);
        assert_eq!(moved_to_front(&ids, 99), vec![1, 2, 3]);
        assert_eq!(moved(&[], 1, 1), Vec::<i64>::new());
        assert_eq!(moved_to_front(&[], 1), Vec::<i64>::new());
    }

    #[test]
    fn moved_to_front_keeps_the_others_in_order() {
        assert_eq!(moved_to_front(&[1, 2, 3, 4], 3), vec![3, 1, 2, 4]);
        assert_eq!(moved_to_front(&[1, 2, 3, 4], 1), vec![1, 2, 3, 4]);
        assert_eq!(moved_to_front(&[1, 2, 3, 4], 4), vec![4, 1, 2, 3]);
    }
}
