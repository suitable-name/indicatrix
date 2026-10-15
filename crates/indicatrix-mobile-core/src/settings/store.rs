//! [`SettingsStore`]: loading `settings.toml` and writing it back from a writer thread.
//!
//! Loading never fails: a missing file gives the defaults, a file that does not parse is
//! renamed aside (`settings.toml.corrupt-<unix seconds>`) and the defaults are used.
//! Saving is debounced: every [`SettingsStore::save`] hands a snapshot to the writer
//! thread, which writes the newest snapshot once no newer one has arrived for 600 ms.
//! The file is written to `settings.toml.tmp` and renamed over the target, so a crash
//! never leaves a half-written file.

use std::{
    fs, io,
    path::{Path, PathBuf},
    sync::mpsc::{self, RecvTimeoutError, Sender},
    thread::JoinHandle,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use super::MobileSettings;
use crate::platform::AppDirs;

/// The file name below [`AppDirs::files`].
const FILE_NAME: &str = "settings.toml";
/// How long the writer waits after the last save request.
const DEBOUNCE: Duration = Duration::from_millis(600);
/// How long [`SettingsStore::flush`] waits for the writer.
const FLUSH_WAIT: Duration = Duration::from_secs(3);

enum Message {
    /// Write this snapshot after the debounce interval.
    Save(Box<MobileSettings>),
    /// Write a pending snapshot now, then answer on the channel.
    Flush(Sender<()>),
}

/// Owns `settings.toml` and its writer thread. See the module docs.
pub struct SettingsStore {
    path: PathBuf,
    sender: Option<Sender<Message>>,
    handle: Option<JoinHandle<()>>,
}

impl SettingsStore {
    /// Opens the store at `<dirs.files>/settings.toml` and returns it with the settings
    /// read from the file. Infallible: see the module docs for what happens to a missing
    /// or corrupt file.
    #[must_use]
    pub fn open(dirs: &AppDirs) -> (Self, MobileSettings) {
        if let Err(error) = fs::create_dir_all(&dirs.files) {
            tracing::warn!(%error, dir = %dirs.files.display(), "could not create the settings directory");
        }
        let path = dirs.files.join(FILE_NAME);
        let settings = load(&path);

        let (sender, receiver) = mpsc::channel();
        let writer_path = path.clone();
        let spawned = std::thread::Builder::new()
            .name("settings-writer".to_owned())
            .spawn(move || writer_loop(&receiver, &writer_path));
        let handle = match spawned {
            Ok(handle) => Some(handle),
            Err(error) => {
                tracing::error!(%error, "could not start the settings writer; settings will not be saved");
                None
            }
        };
        let store = Self {
            path,
            sender: Some(sender),
            handle,
        };
        (store, settings)
    }

    /// Asks the writer to save `settings` once no newer request arrives for 600 ms.
    pub fn save(&self, settings: &MobileSettings) {
        if let Some(sender) = &self.sender
            && sender
                .send(Message::Save(Box::new(settings.clone())))
                .is_err()
        {
            tracing::warn!("settings save dropped: the writer thread is gone");
        }
    }

    /// Writes a pending save now and waits (at most 3 s) for the writer to confirm.
    pub fn flush(&self) {
        let Some(sender) = &self.sender else {
            return;
        };
        let (ack_sender, ack_receiver) = mpsc::channel();
        if sender.send(Message::Flush(ack_sender)).is_err() {
            return;
        }
        if ack_receiver.recv_timeout(FLUSH_WAIT).is_err() {
            tracing::warn!("settings flush timed out");
        }
    }

    /// The settings file's path.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl SettingsStore {
    /// Writes anything pending and ends the writer thread; later saves are dropped (and
    /// logged). Idempotent. Call when the app shuts down, while the writer still runs:
    /// at process exit Windows ends the writer before thread-local destructors run, and
    /// a store dropped there could neither flush nor join.
    pub fn close(&mut self) {
        if self.sender.is_none() {
            return;
        }
        self.flush();
        // Closing the channel ends the writer, which writes anything still pending.
        drop(self.sender.take());
        if let Some(handle) = self.handle.take() {
            crate::tasks::join_briefly(handle, "settings-writer");
        }
    }
}

impl Drop for SettingsStore {
    fn drop(&mut self) {
        self.close();
    }
}

/// Reads the settings file; see the module docs for the failure behavior.
fn load(path: &Path) -> MobileSettings {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return MobileSettings::default(),
        Err(error) => {
            tracing::warn!(%error, path = %path.display(), "could not read the settings file; using defaults");
            return MobileSettings::default();
        }
    };
    match toml::from_str::<MobileSettings>(&text) {
        Ok(settings) => settings,
        Err(error) => {
            let seconds = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_secs());
            let mut aside = path.as_os_str().to_owned();
            aside.push(format!(".corrupt-{seconds}"));
            let aside = PathBuf::from(aside);
            match fs::rename(path, &aside) {
                Ok(()) => {
                    tracing::warn!(%error, moved_to = %aside.display(), "settings file was corrupt; moved aside, using defaults");
                }
                Err(rename_error) => {
                    tracing::warn!(%error, %rename_error, "settings file was corrupt and could not be moved aside; using defaults");
                }
            }
            MobileSettings::default()
        }
    }
}

/// Serializes `settings` and replaces the file at `path` with it.
fn write(path: &Path, settings: &MobileSettings) -> io::Result<()> {
    let text = toml::to_string_pretty(settings)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp = PathBuf::from(temp);
    fs::write(&temp, text)?;
    fs::rename(&temp, path)
}

fn write_logged(path: &Path, settings: &MobileSettings) {
    if let Err(error) = write(path, settings) {
        tracing::warn!(%error, path = %path.display(), "could not write the settings file");
    }
}

/// The writer thread: coalesces save requests and writes the newest after the debounce.
fn writer_loop(receiver: &mpsc::Receiver<Message>, path: &Path) {
    let mut pending: Option<Box<MobileSettings>> = None;
    let mut deadline: Option<Instant> = None;
    loop {
        let message = deadline.map_or_else(
            || receiver.recv().map_err(|_| RecvTimeoutError::Disconnected),
            |when| receiver.recv_timeout(when.saturating_duration_since(Instant::now())),
        );
        match message {
            Ok(Message::Save(settings)) => {
                pending = Some(settings);
                deadline = Some(Instant::now() + DEBOUNCE);
            }
            Ok(Message::Flush(ack)) => {
                if let Some(settings) = pending.take() {
                    write_logged(path, &settings);
                }
                deadline = None;
                let _ = ack.send(());
            }
            Err(RecvTimeoutError::Timeout) => {
                if let Some(settings) = pending.take() {
                    write_logged(path, &settings);
                }
                deadline = None;
            }
            Err(RecvTimeoutError::Disconnected) => {
                if let Some(settings) = pending.take() {
                    write_logged(path, &settings);
                }
                break;
            }
        }
    }
}
