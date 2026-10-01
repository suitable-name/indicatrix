//! Debounced background settings writer -- saves on change without writing on every
//! drag event of a slider.
//!
//! Runs its own worker thread (a plain `thread::spawn` loop reading from a channel) so
//! UI-thread callbacks never block on disk I/O. Every `update()` call replaces the
//! in-memory snapshot and (re)starts a debounce window; the write only happens once
//! `DEBOUNCE` has elapsed since the *last* change, so a fast slider drag collapses
//! into a single write.
//!
//! The in-memory snapshot is the only source of truth for what gets written: every
//! write of the settings file goes through [`SettingsPersister::update`], because
//! [`SettingsPersister::flush`] (called on every close path) writes that snapshot over
//! whatever is on disk. A writer that edits the file behind the persister's back would
//! have its change erased on exit.

use super::{model::SettingsFile, store};
use std::{
    cell::RefCell,
    path::PathBuf,
    sync::{Arc, Mutex, Weak, mpsc},
    thread,
    time::{Duration, Instant},
};
use tracing::warn;

const DEBOUNCE: Duration = Duration::from_millis(600);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// How long [`SettingsPersister::flush`] waits for the worker to confirm the write. A
/// settings file is a few kilobytes, so this only matters when the disk is wedged, and
/// it bounds how long closing the window can be held up then.
const FLUSH_WAIT: Duration = Duration::from_secs(3);

enum Msg {
    Changed(SettingsFile),
    /// Write this snapshot now, then signal on the sender.
    FlushNow(SettingsFile, mpsc::Sender<()>),
    Shutdown,
}

thread_local! {
    /// The persister UI-thread code without a handle of its own reaches through
    /// [`SettingsPersister::installed_for_this_thread`] -- see
    /// [`SettingsPersister::install_for_this_thread`].
    ///
    /// Held weakly: the application's own `Arc` stays the only owner that decides
    /// when the worker shuts down.
    static INSTALLED: RefCell<Weak<SettingsPersister>> = const { RefCell::new(Weak::new()) };
}

/// Handle to the background settings writer. Held in an `Arc` so it can be captured
/// into every UI callback that changes a persisted setting. Dropping a handle asks the
/// worker to shut down after it has written any pending change, so a clone must not
/// outlive the `Arc` that is meant to keep the worker running.
#[derive(Clone)]
pub struct SettingsPersister {
    sender: mpsc::Sender<Msg>,
    current: Arc<Mutex<SettingsFile>>,
}

impl SettingsPersister {
    /// Spawns the worker thread and returns a handle seeded with `initial` (normally
    /// whatever `store::load_with_outcome` produced at startup).
    #[must_use]
    pub fn spawn(path: PathBuf, initial: SettingsFile) -> Self {
        let (sender, receiver) = mpsc::channel::<Msg>();
        let current = Arc::new(Mutex::new(initial));
        thread::spawn(move || worker_loop(&path, &receiver));
        Self { sender, current }
    }

    /// Mutates the in-memory settings under `f` and schedules a debounced save. The
    /// mutation itself is applied synchronously (so `snapshot()` immediately reflects
    /// it); only the disk write is deferred.
    pub fn update(&self, f: impl FnOnce(&mut SettingsFile)) {
        let mut guard = self
            .current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&mut guard);
        let snapshot = guard.clone();
        drop(guard);
        let _ = self.sender.send(Msg::Changed(snapshot));
    }

    /// A copy of the current in-memory settings -- what the next write will contain.
    #[must_use]
    pub fn snapshot(&self) -> SettingsFile {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Forces an immediate write of the current in-memory state, bypassing the
    /// debounce wait, and returns once the worker has written it (or after
    /// [`FLUSH_WAIT`] if it has not). Called on window close so a change made in the
    /// last `DEBOUNCE` window before quitting isn't lost -- returning only after the
    /// write is what keeps the process from exiting with the write still queued.
    pub fn flush(&self) {
        let snapshot = self.snapshot();
        let (ack_sender, ack_receiver) = mpsc::channel();
        if self
            .sender
            .send(Msg::FlushNow(snapshot, ack_sender))
            .is_ok()
        {
            let _ = ack_receiver.recv_timeout(FLUSH_WAIT);
        }
    }

    /// Registers `handle` as the calling thread's ambient persister, for UI-thread code
    /// that has no handle threaded to it (the editor's recent-file list and "don't ask
    /// again" choices, reached from many call sites). Every write of the settings
    /// file must go through the persister -- see the module documentation -- so those
    /// writers reach it here instead of writing the file themselves.
    ///
    /// Replaces any handle installed earlier on this thread.
    pub fn install_for_this_thread(handle: &Arc<Self>) {
        INSTALLED.with(|slot| *slot.borrow_mut() = Arc::downgrade(handle));
    }

    /// The handle [`Self::install_for_this_thread`] registered on this thread, if one
    /// was and it is still alive.
    #[must_use]
    pub fn installed_for_this_thread() -> Option<Arc<Self>> {
        INSTALLED.with(|slot| slot.borrow().upgrade())
    }
}

impl Drop for SettingsPersister {
    fn drop(&mut self) {
        // Sent by every handle that drops; with the single `Arc` the application
        // keeps, that is exactly once, when the last owner goes away.
        let _ = self.sender.send(Msg::Shutdown);
    }
}

fn worker_loop(path: &std::path::Path, receiver: &mpsc::Receiver<Msg>) {
    let mut pending: Option<SettingsFile> = None;
    let mut last_change = Instant::now();

    loop {
        match receiver.recv_timeout(POLL_INTERVAL) {
            Ok(Msg::Changed(settings)) => {
                pending = Some(settings);
                last_change = Instant::now();
            }
            Ok(Msg::FlushNow(settings, ack)) => {
                write_settings(path, &settings);
                pending = None;
                let _ = ack.send(());
            }
            Ok(Msg::Shutdown) => {
                if let Some(settings) = pending.take() {
                    write_settings(path, &settings);
                }
                break;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(settings) = &pending
                    && last_change.elapsed() >= DEBOUNCE
                {
                    write_settings(path, settings);
                    pending = None;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn write_settings(path: &std::path::Path, settings: &SettingsFile) {
    if let Err(e) = store::save(path, settings) {
        warn!("Failed to save settings to {}: {e}", path.display());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_settings_path(tag: &str) -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "indicatrix-cut-persist-test-{tag}-{}-{n}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("settings.toml")
    }

    #[test]
    fn flush_writes_immediately_without_waiting_for_the_debounce() {
        let path = temp_settings_path("flush");
        let persister = SettingsPersister::spawn(path.clone(), SettingsFile::default());

        // 2.5 is exactly representable in both f32 and f64, so it round-trips through
        // TOML without the long decimal tail an arbitrary value like 1.9 would pick up.
        persister.update(|s| s.settings.exposure = 2.5);
        persister.flush();

        // Poll briefly rather than assume zero scheduling latency between threads.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Ok(contents) = std::fs::read_to_string(&path)
                && contents.contains("exposure = 2.5")
            {
                break;
            }
            assert!(Instant::now() < deadline, "flush did not persist in time");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn snapshot_reflects_update_immediately_even_before_the_write_lands() {
        let path = temp_settings_path("snapshot");
        let persister = SettingsPersister::spawn(path, SettingsFile::default());
        persister.update(|s| s.settings.max_bounces = 42);
        assert_eq!(persister.snapshot().settings.max_bounces, 42);
    }

    /// A recent file recorded through the persister is what `flush` writes on exit.
    /// The snapshot `flush` writes is the persister's own, so the entry must be in it;
    /// an entry written to the file behind the persister's back would be overwritten.
    #[test]
    fn a_recent_file_recorded_through_the_persister_survives_flush() {
        let path = temp_settings_path("recent-survives");
        let persister = SettingsPersister::spawn(path.clone(), SettingsFile::default());

        persister.update(|s| {
            s.settings
                .record_recent_native_file("saved-this-session.indicatrix.toml".to_owned());
        });
        persister.flush();

        // `flush` returns after the write, so no polling is needed.
        let on_disk = store::load_or_default(&path);
        assert_eq!(
            on_disk.settings.recent_native_files,
            vec!["saved-this-session.indicatrix.toml".to_owned()]
        );
    }

    /// The counterpart of the test above, pinning down why the persister is the only
    /// allowed writer: a file edited behind its back does not survive `flush`.
    #[test]
    fn flush_overwrites_a_file_written_behind_the_persisters_back() {
        let path = temp_settings_path("behind-the-back");
        let persister = SettingsPersister::spawn(path.clone(), SettingsFile::default());

        let mut direct = SettingsFile::default();
        direct
            .settings
            .record_recent_native_file("written-directly.indicatrix.toml".to_owned());
        store::save(&path, &direct).unwrap();
        persister.flush();

        assert_eq!(
            store::load_or_default(&path).settings.recent_native_files,
            Vec::<String>::new()
        );
    }

    #[test]
    fn the_installed_handle_is_per_thread_and_shares_the_persisters_state() {
        let path = temp_settings_path("installed");
        let persister = Arc::new(SettingsPersister::spawn(path, SettingsFile::default()));
        SettingsPersister::install_for_this_thread(&persister);

        let installed = SettingsPersister::installed_for_this_thread().expect("installed");
        assert!(Arc::ptr_eq(&installed, &persister));
        installed.update(|s| s.settings.max_bounces = 17);
        assert_eq!(persister.snapshot().settings.max_bounces, 17);

        // Another thread has its own (empty) slot.
        let elsewhere = thread::spawn(|| SettingsPersister::installed_for_this_thread().is_none())
            .join()
            .unwrap();
        assert!(elsewhere);
    }
}
