//! What a video run is told through the environment, read once before the throw-away profile
//! moves the working directory and redirects the user variables.
//!
//! Tutorials are picked by the harness itself: arguments after `--` that do not start with `-`
//! select scenarios (`video:<id>`) by name substring.

use crate::support::library_source::{ENV_LIBRARY, LibrarySource, configured_source};
use std::{fs, path::PathBuf};

/// Environment variable that overrides the output folder (default: the workspace `videos`).
pub const ENV_OUT: &str = "INDICATRIX_VIDEOS_OUT";
/// Environment variable that keeps the 3840 x 2160 capture instead of encoding 1920 x 1080.
pub const ENV_4K: &str = "INDICATRIX_VIDEOS_4K";
/// Environment variable that sets the frame rate (24 to 60, default 30).
pub const ENV_FPS: &str = "INDICATRIX_VIDEOS_FPS";
/// Environment variable that is rejected: a run with it set makes no video. Recordings need
/// visible user input only; synthetic `GuideModel.event` input is forbidden.
pub const ENV_INJECT: &str = "INDICATRIX_VIDEOS_ALLOW_INJECT";
/// The window's logical size.
pub const LOGICAL_SIZE: (u32, u32) = (1920, 1080);
/// The middle of the window, where the cursor starts (logical pixels).
pub const WINDOW_CENTER: (f32, f32) = (960.0, 540.0);
/// Physical pixels per logical pixel: the capture is 3840 x 2160, encoded down (V-D2).
pub const SCALE: f32 = 2.0;
/// The frame rate when [`ENV_FPS`] is not set or is out of range.
pub const DEFAULT_FPS: u32 = 30;
/// The size of the encoded video.
const ENCODE_SIZE: (u32, u32) = (1920, 1080);

/// Text that must never be visible in a video: the parts of a profile path, and the user's name.
const PRIVATE_FRAGMENTS: [&str; 4] = [
    "\\users\\",
    "/users/",
    "appdata\\",
    "indicatrix-tutorial-videos",
];

/// Everything the videos need from the environment.
pub struct Config {
    /// The folder the videos are written to (absolute).
    pub out_dir: PathBuf,
    /// Keep the 4K capture.
    pub four_k: bool,
    /// Frames per second.
    pub fps: u32,
    /// [`ENV_INJECT`] was set; the run is refused.
    pub allow_inject: bool,
    /// The database to copy into the profile, `None` for the fixture library.
    pub library: Option<PathBuf>,
    /// A note about [`ENV_LIBRARY`] when it was set but unusable.
    pub library_note: Option<String>,
    /// Lower-case texts a video must not show.
    pub private_words: Vec<String>,
}

impl Config {
    /// Reads the environment. Call it before `Harness::run`, which moves the working directory
    /// and redirects `USERNAME`, `USERPROFILE` and the temp folder.
    #[must_use]
    pub fn from_env() -> Self {
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
            .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
        let wanted = std::env::var_os(ENV_OUT)
            .filter(|value| !value.is_empty())
            .map_or_else(
                || manifest.join("..").join("..").join("videos"),
                PathBuf::from,
            );
        // Created first; a failure leaves the path as given and the first write reports it. The
        // folder is made absolute without `fs::canonicalize`, which gives a `\?\` verbatim
        // path on Windows that would then appear in ffmpeg's arguments, errors and the report.
        let _ = fs::create_dir_all(&wanted);
        let out_dir = if wanted.is_absolute() {
            wanted
        } else {
            std::env::current_dir().map_or_else(|_| wanted.clone(), |cwd| cwd.join(&wanted))
        };
        let fps = std::env::var(ENV_FPS)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|value| (24..=60).contains(value))
            .unwrap_or(DEFAULT_FPS);
        let (source, source_note) = configured_source();
        let library = match source {
            LibrarySource::Database(path) => Some(path),
            LibrarySource::Fixtures => None,
        };
        Self {
            out_dir,
            four_k: flag(ENV_4K),
            fps,
            allow_inject: flag(ENV_INJECT),
            library,
            library_note: source_note,
            private_words: private_words(),
        }
    }

    /// The MP4 of tutorial `id`.
    #[must_use]
    pub fn video_path(&self, id: &str) -> PathBuf {
        self.out_dir.join(format!("{id}.mp4"))
    }

    /// The size ffmpeg scales the capture to, `None` to keep it.
    #[must_use]
    pub const fn encode_size(&self) -> Option<(u32, u32)> {
        if self.four_k { None } else { Some(ENCODE_SIZE) }
    }

    /// The first private fragment `text` contains, if any.
    #[must_use]
    pub fn private_hit(&self, text: &str) -> Option<&str> {
        let lower = text.to_lowercase();
        self.private_words
            .iter()
            .find(|word| lower.contains(word.as_str()))
            .map(String::as_str)
    }

    /// A line for the report about the library the run uses.
    #[must_use]
    pub fn library_line(&self) -> String {
        match (&self.library, &self.library_note) {
            (Some(path), _) => format!(
                "library: a copy of {} (named by {ENV_LIBRARY}); it is never opened itself",
                path.display()
            ),
            (None, Some(note)) => format!("library: the fixture library ({note})"),
            (None, None) => format!("library: the fixture library ({ENV_LIBRARY} is not set)"),
        }
    }
}

/// Whether the environment variable is set to something other than empty, `0` or `false`.
fn flag(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| {
        let value = value.trim().to_lowercase();
        !(value.is_empty() || value == "0" || value == "false")
    })
}

/// The user's name (when it is long enough not to match everything) and the fixed fragments.
fn private_words() -> Vec<String> {
    let mut words: Vec<String> = PRIVATE_FRAGMENTS.iter().map(|s| (*s).to_owned()).collect();
    for variable in ["USERNAME", "USER", "LOGNAME"] {
        if let Ok(name) = std::env::var(variable) {
            let name = name.trim().to_lowercase();
            if name.chars().count() >= 3 && !words.contains(&name) {
                words.push(name);
            }
        }
    }
    words
}
