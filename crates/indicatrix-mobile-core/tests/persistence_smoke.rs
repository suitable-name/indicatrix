//! Smoke test: a destination and a setting changed in one `AppState` are there again
//! when a new `AppState` opens over the same directories.

use std::sync::Arc;

use indicatrix_mobile_core::{
    app_state::AppState,
    layout::Destination,
    platform::{AppDirs, Platform},
    platform_mock::{MockPlatform, unique_temp_dir},
    settings::ThemeChoice,
};

#[test]
fn destination_and_setting_persist_across_restarts() {
    let root = unique_temp_dir("smoke");
    let dirs = AppDirs {
        files: root.join("files"),
        cache: root.join("cache"),
    };

    {
        let platform: Arc<dyn Platform> = Arc::new(MockPlatform::new(dirs.clone()));
        let mut state = AppState::new(platform);
        assert_eq!(state.destination, Destination::Design);
        state.set_destination(Destination::Cut);
        state.update_settings(|settings| settings.theme = ThemeChoice::Dark);
        // Dropping the state drops its settings store, which writes the pending save.
    }

    {
        let platform: Arc<dyn Platform> = Arc::new(MockPlatform::new(dirs));
        let state = AppState::new(platform);
        assert_eq!(state.destination, Destination::Cut);
        assert_eq!(state.settings.theme, ThemeChoice::Dark);
    }

    let _ = std::fs::remove_dir_all(root);
}
