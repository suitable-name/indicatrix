//! The settings structs the mobile app persists in `settings.toml`.
//!
//! Every struct is `#[serde(default)]` as a whole, so a file written by an older build
//! (fewer keys) loads with the current default for each missing key, and keys a newer
//! build added are ignored.

use serde::{Deserialize, Serialize};

use crate::layout::Destination;

/// The most entries [`MobileSettings::recent`] keeps.
pub const MAX_RECENT: usize = 10;

/// The lighting rig a fresh install starts with: the same one the desktop app defaults to
/// (`DEFAULT_LIGHTING_RIG` in `apps/indicatrix-cut`).
pub const DEFAULT_LIGHTING_RIG: &str = "Light tent + black cards";

/// Which color theme the app follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum ThemeChoice {
    /// Follow the system's light or dark setting.
    #[default]
    System,
    /// Always dark.
    Dark,
    /// Always light.
    Light,
}

/// How hard the renderer works on a picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RenderQuality {
    /// Fewest samples: fastest, noisiest.
    Draft,
    /// The balanced default.
    #[default]
    Normal,
    /// Most samples: slowest, cleanest.
    Fine,
}

/// What the camera sees behind the stone (mirrors `indicatrix::render_setup::Backdrop`,
/// which has no serde support).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Backdrop {
    /// The environment's own ground, as lit.
    AsLit,
    /// A neutral grey card (the desktop default).
    #[default]
    Grey,
    /// A white light box.
    White,
}

/// How much of a remote render the phone receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RemoteTransfer {
    /// Only the finished display image.
    #[default]
    DisplayOnly,
    /// The full accumulated radiance data.
    FullData,
}

/// The library list's sort order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum LibrarySort {
    /// The catalogue's own order.
    #[default]
    CatalogueOrder,
    /// Alphabetical by title.
    Title,
    /// Newest entries first.
    Newest,
    /// Most recently edited first.
    RecentlyEdited,
}

/// Rendering preferences.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RenderSettings {
    /// The overall quality tier.
    pub quality: RenderQuality,
    /// Maximum light bounces per path.
    pub bounces: u32,
    /// Whether the denoiser runs on finished frames.
    pub denoise: bool,
    /// Samples per pixel for the live preview on a compact (phone) window.
    pub live_spp_compact: u32,
    /// Samples per pixel for the live preview on a larger window.
    pub live_spp_large: u32,
    /// The longest side, in pixels, of an exported picture.
    pub export_cap_px: u32,
    /// Samples per pixel of an exported picture.
    pub export_cap_spp: u32,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            quality: RenderQuality::Normal,
            bounces: 8,
            denoise: true,
            live_spp_compact: 64,
            live_spp_large: 128,
            export_cap_px: 1024,
            export_cap_spp: 256,
        }
    }
}

/// Lighting preferences for the rendered view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LightingSettings {
    /// The lighting rig, by its stable label.
    pub preset_name: String,
    /// Exposure multiplier.
    pub exposure: f32,
    /// Light yaw in degrees.
    pub azimuth_deg: f32,
    /// Light pitch (elevation) in degrees.
    pub elevation_deg: f32,
    /// What the camera sees behind the stone.
    pub backdrop: Backdrop,
    /// Angular radius of the viewer's head shadow in degrees (0 is off).
    pub head_shadow_deg: f32,
    /// The surface-glare scale in percent (`0.0..=100.0`); 100 is unscaled.
    #[serde(default = "default_surface_glare_pct")]
    pub surface_glare_pct: f32,
}

/// The unscaled surface glare (100 percent): the default of
/// [`LightingSettings::surface_glare_pct`] and of a settings file written before the field
/// existed.
const fn default_surface_glare_pct() -> f32 {
    100.0
}

impl Default for LightingSettings {
    fn default() -> Self {
        Self {
            preset_name: DEFAULT_LIGHTING_RIG.to_owned(),
            exposure: 1.0,
            azimuth_deg: 48.0,
            elevation_deg: 72.0,
            backdrop: Backdrop::Grey,
            head_shadow_deg: 16.0,
            surface_glare_pct: default_surface_glare_pct(),
        }
    }
}

/// The remote render worker the phone talks to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RemoteSettings {
    /// A name the user gave the worker.
    pub name: String,
    /// Its network address (`host:port`); empty when none is set up.
    pub address: String,
    /// How much of a render is sent back.
    pub transfer: RemoteTransfer,
    /// How often, in milliseconds, partial results are sent.
    pub cadence_ms: u32,
    /// Whether this phone has been enrolled with the worker.
    pub enrolled: bool,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            transfer: RemoteTransfer::DisplayOnly,
            cadence_ms: 500,
            enrolled: false,
        }
    }
}

/// Library list preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    /// The sort order.
    pub sort: LibrarySort,
    /// Whether the library shows rows instead of a grid.
    pub list_mode: bool,
}

/// What a recent-items entry points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecentKind {
    /// A library entry.
    Library {
        /// The entry's id in the library database.
        entry_id: i64,
    },
    /// A document opened from the file picker or another app.
    Document {
        /// The name the user saw.
        display_name: String,
    },
}

/// One entry of the recent-items list.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentItem {
    /// The design's title.
    pub title: String,
    /// When it was opened, in seconds since the Unix epoch.
    pub opened_at_unix: i64,
    /// What it points at.
    pub kind: RecentKind,
}

/// Everything the mobile app remembers between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the settings file; the preferences are independent switches"
)]
pub struct MobileSettings {
    /// The color theme.
    pub theme: ThemeChoice,
    /// Whether drag handles are drawn larger.
    pub large_handles: bool,
    /// Whether dragged angles snap to the index gear.
    pub snap_to_gear: bool,
    /// How long, in milliseconds, an automatic solve may take.
    pub auto_solve_budget_ms: u32,
    /// Whether the renderer backs off to save battery and heat.
    pub battery_saver: bool,
    /// Whether the first-run walkthrough has been shown.
    pub walkthrough_done: bool,
    /// The material the rendered stone starts with.
    pub material_name: String,
    /// The destination the app was last on.
    pub last_destination: Destination,
    /// Rendering preferences.
    pub render: RenderSettings,
    /// Lighting preferences.
    pub lighting: LightingSettings,
    /// The remote worker.
    pub remote: RemoteSettings,
    /// Library list preferences.
    pub library: LibrarySettings,
    /// Recently opened designs, newest first, at most [`MAX_RECENT`].
    pub recent: Vec<RecentItem>,
}

impl Default for MobileSettings {
    fn default() -> Self {
        Self {
            theme: ThemeChoice::System,
            large_handles: true,
            snap_to_gear: true,
            auto_solve_budget_ms: 300,
            battery_saver: false,
            walkthrough_done: false,
            material_name: "Diamond".to_owned(),
            last_destination: Destination::Design,
            render: RenderSettings::default(),
            lighting: LightingSettings::default(),
            remote: RemoteSettings::default(),
            library: LibrarySettings::default(),
            recent: Vec::new(),
        }
    }
}

impl RecentKind {
    /// Whether both entries point at the same thing (a library id, or a document name).
    fn same_target(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Library { entry_id: a }, Self::Library { entry_id: b }) => a == b,
            (Self::Document { display_name: a }, Self::Document { display_name: b }) => a == b,
            _ => false,
        }
    }
}

impl MobileSettings {
    /// Puts `item` at the front of [`Self::recent`], removing an older entry that points
    /// at the same thing and keeping at most [`MAX_RECENT`] entries.
    pub fn push_recent(&mut self, item: RecentItem) {
        self.recent.retain(|old| !old.kind.same_target(&item.kind));
        self.recent.insert(0, item);
        self.recent.truncate(MAX_RECENT);
    }
}
