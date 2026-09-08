//! Debounced background settings writer -- saves on change without writing on every
//! drag event of a slider.
//!
//! Runs its own worker thread (a plain `thread::spawn` loop reading from a channel) so
//! UI-thread callbacks never block on disk I/O. Every `update()` call replaces the
//! in-memory snapshot and (re)starts a debounce window; the write only happens once
//! `DEBOUNCE` has elapsed since the *last* change, so a fast slider drag collapses
//! into a single write.

use super::{model::SettingsFile, store};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, mpsc},
    thread,
    time::{Duration, Instant},
};
use tracing::warn;

const DEBOUNCE: Duration = Duration::from_millis(600);
const POLL_INTERVAL: Duration = Duration::from_millis(100);

enum Msg {
    Changed(SettingsFile),
    FlushNow(SettingsFile),
    Shutdown,
}

/// Handle to the background settings writer. Cheaply cloneable (an `Arc` inside) so it
/// can be captured into every UI callback that changes a persisted setting.
#[derive(Clone)]
pub struct SettingsPersister {
    sender: mpsc::Sender<Msg>,
    current: Arc<Mutex<SettingsFile>>,
}

impl SettingsPersister {
    /// Spawns the worker thread and returns a handle seeded with `initial` (normally
    /// whatever `store::load_or_default` produced at startup).
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

    #[must_use]
    pub fn snapshot(&self) -> SettingsFile {
        self.current
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Forces an immediate write of the current in-memory state, bypassing the
    /// debounce wait. Called on window close so a change made in the last
    /// `DEBOUNCE` window before quitting isn't lost.
    pub fn flush(&self) {
        let snapshot = self.snapshot();
        let _ = self.sender.send(Msg::FlushNow(snapshot));
    }
}

impl Drop for SettingsPersister {
    fn drop(&mut self) {
        // Only meaningful for the last surviving clone, but harmless to send every time.
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
            Ok(Msg::FlushNow(settings)) => {
                write_settings(path, &settings);
                pending = None;
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
}
