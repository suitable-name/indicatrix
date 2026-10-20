//! Indicatrix's configuration of the crate's click-everything engine.
//!
//! The engine (enumerate, press, pump, record `panic` / `error-toast` / `main-closed` /
//! `window-invalid`, restore on change) is `uiprobe::smoke`; the types and [`run`] are
//! re-exported here. What stays Indicatrix's: the deny list [`INDICATRIX_DENY`] (the crate
//! default is only the generic words), the one function that builds every [`Smoke`]
//! ([`indicatrix_smoke`]) and the hosts that own the real app (`MainHost`, `SecondaryHost` and
//! the overlay and window hosts of the coverage scenarios).

use uiprobe::smoke::Smoke;

pub use uiprobe::{
    smoke::{DenyField, DenyMatch, DenyRule, SmokeHost, SmokeStats, run},
    take_panics,
};

/// Controls the engine never activates: they destroy data, leave the app, run a long job or
/// reach the network. Edit this list, not the engine. (The crate's `DEFAULT_DENY` has 11 generic
/// rules; this is the full 29 Indicatrix always had, in the same order.)
pub const INDICATRIX_DENY: &[DenyRule] = &[
    DenyRule::label(DenyMatch::Contains, "delete", "destroys data"),
    DenyRule::label(DenyMatch::Contains, "remove", "destroys data"),
    DenyRule::label(DenyMatch::Contains, "quit", "ends the app"),
    DenyRule::label(DenyMatch::Exact, "exit", "ends the app"),
    DenyRule::label(DenyMatch::Exact, "export", "runs an export"),
    DenyRule::label(DenyMatch::Contains, "start export", "runs an export"),
    DenyRule::label(DenyMatch::Contains, "run export", "runs an export"),
    DenyRule::label(DenyMatch::Contains, "overwrite", "overwrites a file"),
    DenyRule::label(DenyMatch::Contains, "discard", "discards work"),
    DenyRule::label(DenyMatch::Contains, "replace", "replaces data"),
    DenyRule::label(DenyMatch::Contains, "restore all", "mass change"),
    DenyRule::label(DenyMatch::Contains, "reset all", "mass change"),
    DenyRule::label(DenyMatch::Contains, "factory reset", "mass change"),
    DenyRule::label(DenyMatch::Contains, "clear all", "mass change"),
    DenyRule::label(DenyMatch::Contains, "purge", "destroys data"),
    DenyRule::label(DenyMatch::Contains, "regenerate all", "long batch job"),
    DenyRule::label(DenyMatch::Contains, "start render", "long render"),
    // Exact: "Render and Output" is a tab opener, not a render.
    DenyRule::label(DenyMatch::Exact, "render", "long render"),
    DenyRule::label(DenyMatch::StartsWith, "render now", "long render"),
    DenyRule::label(DenyMatch::Contains, "test connection", "network"),
    DenyRule::label(DenyMatch::Contains, "claim", "network"),
    DenyRule::label(DenyMatch::Contains, "enroll", "network"),
    DenyRule::label(DenyMatch::Contains, "sync", "network"),
    DenyRule::label(DenyMatch::Contains, "download", "network"),
    DenyRule::label(DenyMatch::Contains, "check for update", "network"),
    DenyRule::label(DenyMatch::Contains, "upload", "network"),
    DenyRule::label(
        DenyMatch::Contains,
        "check now",
        "network (GitHub release lookup)",
    ),
    DenyRule::label(
        DenyMatch::Contains,
        "check daily",
        "network (GitHub release lookup)",
    ),
    // Secondary windows: leaving them mid-pass would make the engine see a vanished window.
    DenyRule::label(
        DenyMatch::Contains,
        "close window",
        "closes the window under test",
    ),
];

/// The one place every Indicatrix [`Smoke`] is built: [`INDICATRIX_DENY`], 60 ms settle, blocked
/// controls NOT skipped (a covered control is still pressed, as before the migration), no
/// scope-may-close (the overlay and window passes switch it on).
#[must_use]
pub fn indicatrix_smoke(state: impl Into<String>) -> Smoke {
    let mut smoke = Smoke::new(state).settle_ms(60).skip_blocked(false);
    smoke.deny = INDICATRIX_DENY.to_vec();
    smoke
}
