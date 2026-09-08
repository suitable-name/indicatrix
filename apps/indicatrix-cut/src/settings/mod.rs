//! Settings persistence and lighting presets.
//!
//! `model` is pure data + pure logic (unit-testable without Slint or threads).
//! `store` handles the on-disk TOML file, including the "never block startup" fallback
//! behaviour. `persist` is the debounced background writer that `gui::mod` wires up to
//! the UI callbacks.

pub mod model;
pub mod persist;
pub mod store;

pub use model::{
    LightingPreset, LiveComputeTarget, LocalComputeTarget, LocalPreviewScale, PreviewScale,
    SettingsFile, WorkerSettings,
};
pub use persist::SettingsPersister;
