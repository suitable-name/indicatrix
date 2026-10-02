//! Writes a design file to disk as one atomic-as-possible operation: stage it under a
//! same-directory temp name (unique per writer, flushed to stable storage), back up
//! whatever already sits at the destination, then rename the temp file into place.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, Instant},
};

/// Distinguishes the temp names [`temp_sibling`] hands out within this process, so
/// two writers aimed at the same target never stage into the same file.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// How long a writer may hold the [`WriteGate`] before a later save stops waiting for
/// it: a writer thread that panicked never reports back, and the gate must not stay
/// shut for the rest of the session.
const WRITER_STALL_LIMIT: Duration = Duration::from_secs(120);

/// Admits one disk writer at a time. A request that arrives while a writer is in
/// flight is parked (the newest one wins -- an older parked request describes a
/// design the newer one already supersedes) and handed back by [`Self::release`] when
/// the running writer reports in, so two saves never race on the same files.
///
/// Pure bookkeeping over an opaque request type `T`: the caller owns the thread that
/// does the writing and calls [`Self::release`] from the UI thread when it finishes.
#[derive(Debug)]
pub(super) struct WriteGate<T> {
    /// When the writer now in flight was admitted, or `None` while idle.
    in_flight_since: Option<Instant>,
    /// The newest request parked behind the writer in flight.
    queued: Option<T>,
}

impl<T> WriteGate<T> {
    /// An idle gate.
    pub(super) const fn new() -> Self {
        Self {
            in_flight_since: None,
            queued: None,
        }
    }

    /// `Some(request)` when the writer may start now (the gate is marked in flight at
    /// `now`); `None` when a writer is already running and `request` was parked
    /// instead, replacing any older parked request.
    pub(super) fn admit(&mut self, request: T, now: Instant) -> Option<T> {
        let busy = self
            .in_flight_since
            .is_some_and(|since| now.saturating_duration_since(since) < WRITER_STALL_LIMIT);
        if busy {
            self.queued = Some(request);
            return None;
        }
        self.in_flight_since = Some(now);
        Some(request)
    }

    /// The running writer finished: hands back the parked request to start next (the
    /// gate stays in flight, marked at `now`), or `None` and an idle gate.
    pub(super) fn release(&mut self, now: Instant) -> Option<T> {
        let next = self.queued.take();
        self.in_flight_since = next.as_ref().map(|_| now);
        next
    }
}

/// Writes `text` to `path` as one atomic-as-possible operation: a read-only folder, a
/// full disk or an antivirus lock must never leave a truncated design file behind.
///
/// The text is staged under a same-directory temp sibling name first ([`temp_sibling`]
/// -- always the same filesystem as the real target, so the rename that follows is a
/// cheap, effectively-atomic same-volume operation, never one that could silently
/// fall back to copy+delete across volumes; unique per call, so concurrent writers
/// to one target cannot clobber each other's staging file) and flushed to stable
/// storage ([`write_synced`]) before the rename, so a power loss after the rename
/// cannot leave a zero-length file under the real name. Before the temp file is
/// renamed into place, a file ALREADY at `path` is copied to a `.bak` sibling
/// ([`backup_existing`]): a bad save (or this very save, if the design regressed
/// since the last one) must never overwrite the only copy of a design that was
/// already on disk.
///
/// # Errors
///
/// A ready-to-toast message naming exactly what is on disk afterward (always: nothing
/// was saved).
pub(super) fn write_file_atomically(path: &Path, text: &str) -> Result<(), String> {
    let staged = temp_sibling(path);
    if let Err(e) = write_synced(&staged, text) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "Failed to write {}: {e}. Nothing was saved.",
            path.display()
        ));
    }
    if let Err(message) = backup_existing(path) {
        let _ = std::fs::remove_file(&staged);
        return Err(message);
    }
    if let Err(e) = std::fs::rename(&staged, path) {
        let _ = std::fs::remove_file(&staged);
        return Err(format!(
            "Failed to finalize {}: {e}. Nothing was saved.",
            path.display()
        ));
    }
    Ok(())
}

/// A same-directory temp sibling of `path`, used to stage a write before the final
/// atomic-as-possible rename -- see [`write_file_atomically`]. Always a sibling
/// (never `std::env::temp_dir()`), so the rename that follows never crosses
/// filesystems.
///
/// Unique on every call (`<file>.<pid>.<counter>.tmp`): a fixed `<file>.tmp` let two
/// quick saves, or a save and an autosave, write into one staging file and rename
/// an interleaved result into place. A crash mid-write leaves one such file behind
/// instead of reusing it on the next save.
pub(super) fn temp_sibling(path: &Path) -> PathBuf {
    let suffix = format!(
        ".{}.{}.tmp",
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from(format!("save{suffix}")),
        |n| {
            let mut s = n.to_os_string();
            s.push(&suffix);
            s
        },
    );
    path.with_file_name(file_name)
}

/// Creates (or truncates) `path`, writes `contents` and flushes the file to stable
/// storage (`File::sync_all`) before returning, so the rename that follows publishes
/// bytes that are actually on disk.
///
/// # Errors
///
/// Any error from creating, writing or syncing the file.
pub(super) fn write_synced(path: &Path, contents: &str) -> std::io::Result<()> {
    let mut file = std::fs::File::create(path)?;
    file.write_all(contents.as_bytes())?;
    file.sync_all()
}

/// `path`'s `.bak` sibling -- e.g. `design.indicatrix` -> `design.indicatrix.bak`. One generation of backup only:
/// a second save in a row overwrites the `.bak` from the first, matching "keep the
/// PREVIOUS version" rather than an ever-growing history.
fn backup_sibling(path: &Path) -> PathBuf {
    let file_name = path.file_name().map_or_else(
        || std::ffi::OsString::from("save.bak"),
        |n| {
            let mut s = n.to_os_string();
            s.push(".bak");
            s
        },
    );
    path.with_file_name(file_name)
}

/// Never overwrites the only copy: copies `path` to its [`backup_sibling`]
/// before [`write_file_atomically`] renames a freshly staged temp file over it. A
/// no-op (`Ok(())`) when `path` doesn't exist yet -- a design's first save has
/// nothing to back up. Copies rather than renames `path` itself: `path` is left
/// completely untouched by this step either way, so a failed backup aborts the whole
/// save (see [`write_file_atomically`]) without having disturbed the file that was
/// already there.
///
/// # Errors
///
/// A ready-to-toast message naming the file that could not be backed up.
fn backup_existing(path: &Path) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let backup = backup_sibling(path);
    std::fs::copy(path, &backup).map_err(|e| {
        format!(
            "Failed to back up {} to {} before overwriting it: {e}. Nothing was saved.",
            path.display(),
            backup.display()
        )
    })?;
    Ok(())
}
