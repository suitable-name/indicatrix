//! The mirror algorithm itself: one full pass over the remote catalogue, deciding
//! per design whether to skip, add or update.

use super::{
    catalogue::enumerate_remote_catalogue,
    design::{SyncedDesign, sync_one_design},
};
use crate::bridge::library::mirror::options::{
    LibraryTransport, MirrorCounts, MirrorOptions, MirrorOutcome, MirrorProgress,
};
use indicatrix_net::library::DesignSummary;
use indicatrix_vault::{db::sqlite::Database, model::mirror::MirrorState};
use std::sync::{
    Arc, Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};
use tracing::warn;

/// What [`run_mirror_sync`] does with one remote design, given the mirror state stored
/// for its `url` (see [`decide`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Decision {
    /// The user deleted this mirrored design locally: never fetched, never re-created,
    /// whatever the remote versions say ("local delete wins").
    SkipDeleted,
    /// Neither remote version moved since the last sync.
    SkipUnchanged,
    /// Never mirrored, or a remote version moved: fetch and save.
    Sync,
}

/// The remote design's revision token as the wire summary states it
/// ([`DesignSummary::design_version`]), or `None` when the server could not state one (it
/// sends all zero bytes then): [`decide`] treats that as changed, since there is nothing to
/// compare against the stored token.
///
/// The token is what `sync_one_design` stored from the fetched record's `version`, so the
/// two are equal exactly while the design has not been edited on the remote since.
pub(super) fn remote_design_version(summary: &DesignSummary) -> Option<[u8; 32]> {
    (summary.design_version != [0u8; 32]).then_some(summary.design_version)
}

/// Classifies one remote design against its stored mirror `state`: a tombstone always
/// wins; otherwise the design is unchanged only while the summary hash matches AND the
/// revision token (`remote_design`) matches -- a remote edit to the angle table, notes,
/// attachments or ratios can leave the summary hash untouched and is caught only by the
/// second. A `remote_design` of `None` (no token stated) counts as moved.
pub(super) fn decide(
    state: Option<&MirrorState>,
    summary: &DesignSummary,
    remote_design: Option<[u8; 32]>,
) -> Decision {
    let Some(state) = state else {
        return Decision::Sync;
    };
    if state.deleted_locally {
        return Decision::SkipDeleted;
    }
    let summary_moved = state.summary_version != summary.version;
    let design_moved = remote_design.is_none_or(|version| version != state.design_version);
    if summary_moved || design_moved {
        Decision::Sync
    } else {
        Decision::SkipUnchanged
    }
}

/// Reads `summary`'s stored mirror state. `Err(())` (logged) when it cannot be read: that
/// is neither "never mirrored" nor "unchanged" -- treating it as new would re-create a
/// design the user deleted -- so the caller leaves the design for the next sync.
fn load_mirror_state(
    db: &Arc<Mutex<Database>>,
    summary: &DesignSummary,
) -> Result<Option<MirrorState>, ()> {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get_mirror_state(&summary.url)
        .map_err(|e| {
            warn!(
                "Mirror sync: cannot read the mirror state of {} ({e}); leaving it for the \
                 next sync",
                summary.url
            );
        })
}

/// The progress update after the `processed`-th (0-based) design of the pass.
fn progress_after(
    processed: usize,
    counts: MirrorCounts,
    summary: &DesignSummary,
) -> MirrorProgress {
    MirrorProgress {
        processed: processed + 1,
        counts,
        current_title: summary.title.clone(),
    }
}

/// The mirror algorithm itself -- generic over [`LibraryTransport`] so it's testable
/// without a socket (see that trait's doc comment). See the mirror module's own doc
/// comment for the full design: additive/update-only, `url`-keyed identity, two-tier
/// version-token skipping, tombstoned local deletes, eager (capped) attachment fetching,
/// and per-design cancellation safety.
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
            counts.orphaned_mirror_states = count_orphaned_mirror_states(db);
            return MirrorOutcome::Cancelled(counts);
        }

        let Ok(existing_state) = load_mirror_state(db, &summary) else {
            counts.failed += 1;
            on_progress(progress_after(processed, counts, &summary));
            continue;
        };

        match decide(
            existing_state.as_ref(),
            &summary,
            remote_design_version(&summary),
        ) {
            Decision::SkipUnchanged => {
                counts.skipped_unchanged += 1;
                on_progress(progress_after(processed, counts, &summary));
                continue;
            }
            // `orphaned_mirror_states` is the separate end-of-pass count of every mirror
            // state without a local design, whether or not the remote still lists it.
            Decision::SkipDeleted => {
                counts.skipped_deleted += 1;
                on_progress(progress_after(processed, counts, &summary));
                continue;
            }
            Decision::Sync => {}
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
            Ok(SyncedDesign::Saved) => {
                if is_new {
                    counts.new_count += 1;
                } else {
                    counts.updated_count += 1;
                }
            }
            Ok(SyncedDesign::SkippedLocalConflict) => counts.local_conflicts_skipped += 1,
            Err(()) => counts.failed += 1,
        }

        on_progress(progress_after(processed, counts, &summary));
    }

    counts.orphaned_mirror_states = count_orphaned_mirror_states(db);
    MirrorOutcome::Completed(counts)
}

/// [`indicatrix_vault::db::sqlite::Database::count_mirror_states_without_entry`], with
/// any error collapsed to `0` -- a failed count degrades the sync summary's "N designs
/// skipped (deleted locally)" line to silently reading `0`, rather than failing (or
/// even just not reporting) an otherwise-successful sync over a query this method
/// itself documents as never mutating anything.
fn count_orphaned_mirror_states(db: &Arc<Mutex<Database>>) -> u64 {
    db.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .count_mirror_states_without_entry()
        .unwrap_or(0)
}
