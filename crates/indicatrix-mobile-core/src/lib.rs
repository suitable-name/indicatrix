//! The UI-free core of the Indicatrix mobile app.
//!
//! Everything a phone or tablet front end needs that is not drawing: the app state
//! around the shared [`indicatrix_editor::EditorSession`], settings and private
//! paths, the window size class, the compute threads (render pool, solve, analysis)
//! and their governor, design files as bytes, cutting progress, and the
//! [`platform::Platform`] trait through which the front end provides files, sharing,
//! printing and the rest. No Slint, no JNI: the Android app
//! (`apps/indicatrix-mobile-android`) and any later iOS app sit on top of it, and the
//! whole crate is tested natively.
//!
//! Plan: `C:/temp/mcp2/android_app_implementation_plan_2026-10-13.md`.

pub mod analysis;
pub mod app_state;
pub mod cutting;
pub mod design_props;
pub mod design_state;
pub mod document;
pub mod hdr;
pub mod layout;
pub mod library;
pub mod platform;
pub mod platform_mock;
pub mod render;
pub mod settings;
pub mod tasks;
pub mod viewport;
