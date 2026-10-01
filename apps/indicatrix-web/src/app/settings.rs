//! The render and view settings, persisted as JSON under
//! `sessionStorage["indicatrix.settings.v1"]` (see [`super::persist`]).
//!
//! The payload, its defaults, clamping and the settings-to-scene conversion live in
//! `indicatrix_web_core::settings`, where they are tested natively; this module
//! re-exports them under the path the app has always used.

pub use indicatrix_web_core::settings::{
    RenderSettings, SessionPayload, lighting_options, material_options,
};
