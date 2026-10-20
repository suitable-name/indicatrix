//! Persistent settings of the mobile app: the data model ([`MobileSettings`] and its
//! parts) and the crash-safe, debounced file store ([`SettingsStore`]).

mod model;
mod store;
#[cfg(test)]
mod tests;

pub use model::{
    Backdrop, DEFAULT_LIGHTING_RIG, LibrarySettings, LibrarySort, LightingSettings, MAX_RECENT,
    MobileSettings, RecentItem, RecentKind, RemoteSettings, RemoteTransfer, RenderBackend,
    RenderQuality, RenderSettings, ThemeChoice,
};
pub use store::SettingsStore;
