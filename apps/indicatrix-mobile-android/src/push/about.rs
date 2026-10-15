//! The About page: app name, versions and the build id.

use indicatrix_mobile_core::app_state::AppState;
use slint::ComponentHandle;

use crate::{About, AppWindow};

/// Pushes the version information from `state.about()`.
pub fn push(window: &AppWindow, state: &AppState) {
    let info = state.about();
    let about = window.global::<About>();
    about.set_app_name(info.app_name.into());
    about.set_app_version(info.app_version.into());
    about.set_indicatrix_version(info.indicatrix_version.into());
    about.set_build_id(info.build_id.into());
}
