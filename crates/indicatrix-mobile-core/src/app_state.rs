//! [`AppState`]: everything the front end owns on its UI thread.
//!
//! That is the editor session, the settings, where the user is in the app and which
//! window class applies, plus the platform handle and the settings store behind them.

use std::sync::Arc;

use indicatrix_editor::{EditorSession, cut_sheet::SheetDetails};

use crate::{
    analysis::AnalysisState,
    cutting::CuttingState,
    design_props::PropsState,
    design_state::SolveState,
    document::DocumentState,
    hdr::HdrState,
    layout::{Destination, WindowClass},
    library::LibraryState,
    platform::Platform,
    render::RenderState,
    settings::{MobileSettings, SettingsStore},
    viewport::ViewportState,
};

/// The title shown for a design that has none.
const UNTITLED: &str = "Untitled Design";

/// An Android-style lifecycle event, as the front end reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lifecycle {
    /// The app left the foreground.
    Paused,
    /// The app is in the foreground again.
    Resumed,
    /// The app is about to be stopped or destroyed.
    Stopping,
}

/// What the About screen shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AboutInfo {
    /// The product name.
    pub app_name: &'static str,
    /// This app's version.
    pub app_version: &'static str,
    /// The version of the rendering and design engine (`indicatrix`).
    pub indicatrix_version: String,
    /// The engine's build identity, as the remote-render handshake compares it.
    pub build_id: String,
}

/// The app's state. See the module docs.
pub struct AppState {
    /// The design under edit with its undo history.
    pub session: EditorSession,
    /// The current settings. Change them through [`Self::update_settings`] so they are saved.
    pub settings: MobileSettings,
    /// Where the user is.
    pub destination: Destination,
    /// The layout family of the current window width.
    pub window_class: WindowClass,
    /// Where the design came from and how it is saved (`document`, lane M2-a).
    pub document: DocumentState,
    /// The last solve of the design (`design_state`, lane M2-c). A solve is current
    /// when its generation equals `session.current_generation()`.
    pub solve: SolveState,
    /// The tier selected in the tier list and the viewport, `None` for no selection.
    /// Setting it also clears `session.multi_selected` unless Select mode is on (the
    /// desktop's rule); every design surface reads it.
    pub selected_tier: Option<usize>,
    /// Camera and view state of the design viewport (`viewport`, lane M2-b).
    pub viewport: ViewportState,
    /// Material and lighting state that is not a setting (`design_props`, lane M2-d).
    pub props: PropsState,
    /// Cutting Mode and the done marks (`cutting`, wave M3).
    pub cutting: CuttingState,
    /// The progressive render and its governor (`render`, lane M4-a).
    pub render: RenderState,
    /// Metrics and tilt curves (`analysis`, lane M4-c).
    pub analysis: AnalysisState,
    /// The imported HDR environment map (`hdr`, lane M4-c).
    pub hdr: HdrState,
    /// The local library (`library`, lane M5-a).
    pub library: LibraryState,
    /// Cancellation and identity of immutable long-operation inputs.
    pub operations: crate::operations::OperationState,
    platform: Arc<dyn Platform>,
    store: SettingsStore,
}

impl AppState {
    /// A fresh state on `platform`: settings are read from the platform's private files
    /// directory, the session holds the start-up design, and the destination is the one
    /// the user left last.
    #[must_use]
    pub fn new(platform: Arc<dyn Platform>) -> Self {
        let (store, settings) = SettingsStore::open(platform.app_dirs());
        Self {
            session: EditorSession::fresh(),
            destination: settings.last_destination,
            settings,
            window_class: WindowClass::Compact,
            document: DocumentState::default(),
            solve: SolveState::default(),
            selected_tier: None,
            viewport: ViewportState::default(),
            props: PropsState::default(),
            cutting: CuttingState::default(),
            render: RenderState::default(),
            analysis: AnalysisState::default(),
            hdr: HdrState::default(),
            library: LibraryState::default(),
            operations: crate::operations::OperationState::default(),
            platform,
            store,
        }
    }

    /// The platform services.
    #[must_use]
    pub const fn platform(&self) -> &Arc<dyn Platform> {
        &self.platform
    }

    /// Records the window width in dp. Returns true when the window class changed.
    pub fn set_window_width(&mut self, width_dp: f32) -> bool {
        let class = WindowClass::from_width(width_dp);
        let changed = class != self.window_class;
        self.window_class = class;
        changed
    }

    /// Moves to `destination` and remembers it for the next start.
    pub fn set_destination(&mut self, destination: Destination) {
        self.destination = destination;
        if self.settings.last_destination != destination {
            self.update_settings(|settings| settings.last_destination = destination);
        }
    }

    /// Applies `edit` to the settings, then queues a (debounced) save.
    pub fn update_settings(&mut self, edit: impl FnOnce(&mut MobileSettings)) {
        edit(&mut self.settings);
        self.store.save(&self.settings);
    }

    /// Reacts to a lifecycle event: leaving the foreground or stopping writes any pending
    /// settings now. Resuming needs nothing yet.
    pub fn lifecycle(&mut self, event: Lifecycle) {
        self.render.lifecycle(event);
        match event {
            Lifecycle::Paused | Lifecycle::Stopping => {
                // In-flight work stops; a finished Optimize or Retarget result survives a
                // round trip to the file picker or the share sheet.
                self.operations.interrupt();
                self.analysis.cancel_all();
                // Android may kill a paused app without another callback: keep the
                // design (cheap when nothing changed) before the settings.
                if let Err(error) = crate::document::autosave(self) {
                    tracing::warn!(%error, "the design could not be autosaved");
                }
                self.store.flush();
            }
            Lifecycle::Resumed => {}
        }
    }

    /// Ends the app: autosaves the design, writes the settings and ends the settings
    /// writer. Call once when the run loop has ended, while the app's threads still
    /// exist; the state may be dropped any time after.
    pub fn shutdown(&mut self) {
        self.lifecycle(Lifecycle::Stopping);
        self.render.shutdown();
        self.analysis.shutdown();
        self.library.close();
        self.store.close();
    }

    /// The design's title (its first header line that is not a "by ..." line), or
    /// "Untitled Design" when it has none.
    #[must_use]
    pub fn design_title(&self) -> String {
        let title = SheetDetails::from_design(&self.session.design).title;
        if title.is_empty() {
            UNTITLED.to_owned()
        } else {
            title
        }
    }

    /// The version information for the About screen.
    #[must_use]
    pub fn about(&self) -> AboutInfo {
        AboutInfo {
            app_name: "Indicatrix",
            app_version: env!("CARGO_PKG_VERSION"),
            indicatrix_version: indicatrix::VERSION.to_owned(),
            build_id: indicatrix::BUILD_ID.to_owned(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::AppDirs,
        platform_mock::{MockPlatform, unique_temp_dir},
        settings::ThemeChoice,
    };

    fn state() -> (AppState, Arc<MockPlatform>) {
        let mock = Arc::new(MockPlatform::default());
        let platform: Arc<dyn Platform> = mock.clone();
        (AppState::new(platform), mock)
    }

    #[test]
    fn new_state_starts_on_design_with_the_fresh_session() {
        let (state, _mock) = state();
        assert_eq!(state.destination, Destination::Design);
        assert_eq!(state.window_class, WindowClass::Compact);
        assert_eq!(state.settings, MobileSettings::default());
        assert_eq!(state.session.design.meta.gear_teeth, 96);
    }

    #[test]
    fn window_width_reports_class_changes() {
        let (mut state, _mock) = state();
        assert!(!state.set_window_width(400.0));
        assert!(state.set_window_width(700.0));
        assert_eq!(state.window_class, WindowClass::Medium);
        assert!(!state.set_window_width(800.0));
        assert!(state.set_window_width(1000.0));
        assert!(state.set_window_width(f32::NAN));
        assert_eq!(state.window_class, WindowClass::Compact);
    }

    #[test]
    fn destination_is_stored_in_the_settings() {
        let (mut state, _mock) = state();
        state.set_destination(Destination::Library);
        assert_eq!(state.destination, Destination::Library);
        assert_eq!(state.settings.last_destination, Destination::Library);
    }

    #[test]
    fn update_settings_applies_the_edit() {
        let (mut state, _mock) = state();
        state.update_settings(|settings| settings.theme = ThemeChoice::Dark);
        assert_eq!(state.settings.theme, ThemeChoice::Dark);
    }

    #[test]
    fn pausing_flushes_the_settings_file() {
        let (mut state, mock) = state();
        state.update_settings(|settings| settings.battery_saver = true);
        state.lifecycle(Lifecycle::Paused);
        let path = mock.app_dirs().files.join("settings.toml");
        let text = std::fs::read_to_string(path).expect("settings file written by the flush");
        assert!(text.contains("battery_saver = true"), "{text}");
        state.lifecycle(Lifecycle::Resumed);
    }

    #[test]
    fn stopping_flushes_too() {
        let root = unique_temp_dir("appstate-stop");
        let dirs = AppDirs {
            files: root.join("files"),
            cache: root.join("cache"),
        };
        let platform: Arc<dyn Platform> = Arc::new(MockPlatform::new(dirs.clone()));
        let mut state = AppState::new(platform);
        state.update_settings(|settings| settings.walkthrough_done = true);
        state.lifecycle(Lifecycle::Stopping);
        assert!(dirs.files.join("settings.toml").is_file());
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn design_title_falls_back_to_untitled() {
        let (mut state, _mock) = state();
        assert_eq!(state.design_title(), "Untitled Design");
        state.session.design.meta.headers = vec!["by Jane".to_owned(), "  Barion  ".to_owned()];
        assert_eq!(state.design_title(), "Barion");
        state.session.design.meta.headers = vec!["by Jane".to_owned()];
        assert_eq!(state.design_title(), "Untitled Design");
    }

    #[test]
    fn about_reports_the_versions() {
        let (state, _mock) = state();
        let about = state.about();
        assert_eq!(about.app_name, "Indicatrix");
        assert_eq!(about.app_version, env!("CARGO_PKG_VERSION"));
        assert_eq!(about.indicatrix_version, indicatrix::VERSION);
        assert_eq!(about.build_id, indicatrix::BUILD_ID);
    }

    #[test]
    fn platform_accessor_returns_the_given_platform() {
        let (state, mock) = state();
        state.platform().keep_screen_on(true);
        assert_eq!(mock.keep_screen_on_history(), vec![true]);
    }
}
