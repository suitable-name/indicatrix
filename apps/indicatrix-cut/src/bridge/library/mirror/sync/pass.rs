//! The mirror algorithm itself: one full pass over the remote catalogue, deciding
//! per design whether to skip, add or update.

use super::{catalogue::enumerate_remote_catalogue, design::sync_one_design};
use crate::bridge::library::mirror::options::{
    LibraryTransport, MirrorCounts, MirrorOptions, MirrorOutcome, MirrorProgress,
};
use indicatrix_vault::db::sqlite::Database;
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

/// The mirror algorithm itself -- generic over [`LibraryTransport`] so it's testable
/// without a socket (see that trait's doc comment). See the mirror module's own doc
/// comment for the full design: additive/update-only, `url`-keyed identity, two-tier
/// content-hash skipping, eager (capped) attachment fetching, and per-design
/// cancellation safety.
#[must_use]
pub(super) fn run_mirror_sync(
    db: &Arc<Mutex<Database>>,
    transport: &impl LibraryTransport,
    source_id: &str,
    options: MirrorOptions,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(MirrorProgress),
) -> MirrorOutcome {
    let summaries = match enumerate_remote_catalogue(transport) {
        Ok(list) => list,
        Err(outcome) => return outcome,
    };

    let mut counts = MirrorCounts {
        total_found: summaries.len(),
        ..MirrorCounts::default()
    };

    for (processed, summary) in summaries.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return MirrorOutcome::Cancelled(counts);
        }

        let existing_state = {
            let db = db.lock().unwrap_or_else(PoisonError::into_inner);
            db.get_mirror_state(&summary.url).ok().flatten()
        };

        let unchanged = existing_state
            .as_ref()
            .is_some_and(|state| state.summary_version == summary.version);
        if unchanged {
            counts.skipped_unchanged += 1;
            on_progress(MirrorProgress {
                processed: processed + 1,
                counts,
                current_title: summary.title.clone(),
            });
            continue;
        }

        let is_new = existing_state.is_none();
        match sync_one_design(
            db,
            transport,
            source_id,
            options,
            &summary,
            is_new,
            &mut counts,
        ) {
            Ok(()) => {
                if is_new {
                    counts.new_count += 1;
                } else {
                    counts.updated_count += 1;
                }
            }
            Err(()) => counts.failed += 1,
        }

        on_progress(MirrorProgress {
            processed: processed + 1,
            counts,
            current_title: summary.title.clone(),
        });
    }

    MirrorOutcome::Completed(counts)
}
