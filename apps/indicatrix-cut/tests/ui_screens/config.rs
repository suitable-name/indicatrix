//! What a screenshot run is told through the environment, read once before the throw-away
//! profile moves the working directory and redirects the user variables.

use crate::support::library_source::{ENV_LIBRARY, LibrarySource, configured_source};
use std::{fs, path::PathBuf, time::Duration};

/// Environment variable that overrides the output folder (default: the workspace `assets`).
pub const ENV_OUT: &str = "INDICATRIX_SCREENS_OUT";
/// Environment variable that sets the sample count a live render must reach before a shot.
pub const ENV_SPP: &str = "INDICATRIX_SCREENS_SPP";
/// Environment variable that sets how many seconds a shot waits for a live render.
pub const ENV_TIMEOUT: &str = "INDICATRIX_SCREENS_RENDER_TIMEOUT";
/// The live render's sample count when [`ENV_SPP`] is not set: four times the app's own
/// default of 256, so the published shots carry no visible noise.
pub const DEFAULT_SPP: u32 = 1024;
/// How long a shot waits for a live render when [`ENV_TIMEOUT`] is not set.
pub const DEFAULT_RENDER_TIMEOUT_SECS: u64 = 120;

/// The message of the abort when no library database is configured.
pub const MISSING_LIBRARY: &str = "set INDICATRIX_HARNESS_LIBRARY to the facet_diagrams.sqlite \
                                   to photograph (it is copied into the throw-away profile, \
                                   never opened)";

/// Text that must never be visible in a shot: the user's name and the parts of a profile path.
const PRIVATE_FRAGMENTS: [&str; 4] = ["\\users\\", "/users/", "appdata\\", "indicatrix-ui-screens"];

/// Everything the shots need from the environment.
pub struct Config {
    /// The folder the PNG files are written to (canonical).
    pub out_dir: PathBuf,
    /// The sample count a live render must reach before it is photographed.
    pub spp: u32,
    /// How long a shot waits for the live render to reach it.
    pub render_timeout: Duration,
    /// The database to photograph, `None` when [`ENV_LIBRARY`] is unset or not a file.
    pub library: Option<PathBuf>,
    /// A note about [`ENV_LIBRARY`] when it was set but unusable.
    pub library_note: Option<String>,
    /// Lower-case texts a shot must not show.
    pub private_words: Vec<String>,
}

impl Config {
    /// Reads the environment. Call it before `Harness::run`, which moves the working
    /// directory and redirects `USERNAME`, `USERPROFILE` and the temp folder.
    #[must_use]
    pub fn from_env() -> Self {
        let manifest = std::env::var_os("CARGO_MANIFEST_DIR")
            .map_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")), PathBuf::from);
        let wanted = std::env::var_os(ENV_OUT)
            .filter(|value| !value.is_empty())
            .map_or_else(
                || manifest.join("..").join("..").join("assets"),
                PathBuf::from,
            );
        // Created first so that canonicalising cannot fail on a missing folder; a failure of
        // either call leaves the path as given and the first write reports it.
        let _ = fs::create_dir_all(&wanted);
        let out_dir = fs::canonicalize(&wanted).unwrap_or(wanted);
        let spp = std::env::var(ENV_SPP)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_SPP);
        let seconds = std::env::var(ENV_TIMEOUT)
            .ok()
            .and_then(|text| text.trim().parse::<u64>().ok())
            .filter(|value| *value > 0)
            .unwrap_or(DEFAULT_RENDER_TIMEOUT_SECS);
        let (source, source_note) = configured_source();
        let library = match source {
            LibrarySource::Database(path) => Some(path),
            LibrarySource::Fixtures => None,
        };
        let library_note = source_note.or_else(|| {
            library
                .is_none()
                .then(|| format!("{ENV_LIBRARY} is not set"))
        });
        Self {
            out_dir,
            spp,
            render_timeout: Duration::from_secs(seconds),
            library,
            library_note,
            private_words: private_words(),
        }
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
