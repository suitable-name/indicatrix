//! Indicatrix for Android phones and tablets.
//!
//! The touch-first Slint UI (`ui/`) over [`indicatrix_mobile_core`], plus the two
//! implementations of [`indicatrix_mobile_core::platform::Platform`]:
//! `platform_android` (JNI calls into the Kotlin `MainActivity`, Android only) and
//! [`platform_desktop`] (the `desktop-preview` feature: the same UI in a desktop
//! window at a phone or tablet size).
//!
//! Entry points: `android_main` (in `android_entry`, called by the `NativeActivity`
//! glue) and the `indicatrix-mobile-preview` binary. Both end in [`run::run`].

#[cfg(target_os = "android")]
mod android_entry;
#[cfg(target_os = "android")]
mod platform_android;
#[cfg(feature = "desktop-preview")]
pub mod platform_desktop;

mod commands;
mod cut;
mod design;
mod library;
mod navigation;
mod push;
mod remote;
mod remote_jobs;
pub mod run;

slint::include_modules!();
