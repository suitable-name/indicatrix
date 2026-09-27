//! The mirror algorithm itself: enumerating the remote catalogue
//! ([`catalogue`]), the additive/update-only per-design sync loop
//! ([`pass`]/[`design`]), and the worker-thread wrapper
//! ([`spawn_mirror_sync`]) the UI actually calls. See this group's own `mod.rs` doc
//! comment for the full design.

mod catalogue;
mod design;
mod pass;
#[cfg(test)]
mod tests;

use pass::run_mirror_sync;

use super::options::{
    MirrorHandle, MirrorOptions, MirrorOutcome, MirrorProgress, mirror_source_id,
};
use crate::{bridge::library::client::LibrarySession, settings::WorkerSettings};
use indicatrix_vault::db::sqlite::Database;
use slint::{ComponentHandle, Weak};
use std::{
    sync::{Arc, Mutex, atomic::AtomicBool},
    thread,
};

/// Spawns the mirror-sync worker thread against `worker`'s design library, writing into
/// `db`. `on_progress` is invoked on the UI event loop after each design examined;
/// `on_done` is invoked once, exactly once, when the sync completes, is cancelled, or
/// fails outright. Follows `bridge::export_thread::spawn_export`'s exact pattern (a
/// `thread::spawn` worker, an `Arc<AtomicBool>` cancel flag, results marshalled back via
/// `Weak::upgrade_in_event_loop`).
///
/// `db` is locked only for the duration of each individual database call inside the
/// sync loop (see [`pass::run_mirror_sync`]), never for the whole sync -- so the local
/// library UI (search, detail, import) stays responsive on the SAME database while a
/// multi-minute sync runs in the background.
///
/// Drives the sync against one [`LibrarySession`] -- built here, from `worker`, and held
/// for the whole sync -- rather than reconnecting per request, the whole point of this
/// module's held-connection task; see [`LibrarySession`]'s own doc comment for what that
/// buys (one handshake instead of thousands for a real catalogue) and how it behaves when
/// that held connection drops mid-sync.
pub fn spawn_mirror_sync<T, P, D>(
    ui_weak: Weak<T>,
    db: Arc<Mutex<Database>>,
    worker: WorkerSettings,
    options: MirrorOptions,
    on_progress: P,
    on_done: D,
) -> MirrorHandle
where
    T: ComponentHandle + 'static,
    P: Fn(&T, MirrorProgress) + Send + 'static + Clone,
    D: Fn(&T, MirrorOutcome) + Send + 'static,
{
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_worker = cancel.clone();
    let ui_weak_done = ui_weak.clone();

    thread::spawn(move || {
        let source_id = mirror_source_id(&worker);
        let session = LibrarySession::new(worker);
        let progress_ui_weak = ui_weak;
        let outcome = run_mirror_sync(
            &db,
            &session,
            &source_id,
            options,
            &cancel_worker,
            move |progress| {
                let on_progress = on_progress.clone();
                let _ =
                    progress_ui_weak.upgrade_in_event_loop(move |ui| on_progress(&ui, progress));
            },
        );
        let _ = ui_weak_done.upgrade_in_event_loop(move |ui| on_done(&ui, outcome));
    });

    MirrorHandle { cancel }
}
