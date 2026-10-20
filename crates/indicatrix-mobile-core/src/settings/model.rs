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

/// Execution policy. Automatic never uploads a scene or selects remote compute.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum RenderBackend {
    #[default]
    Automatic,
    DeviceCpu,
    DeviceGpu,
    Remote,
}

impl RenderBackend {
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Automatic => 0,
            Self::DeviceCpu => 1,
            Self::DeviceGpu => 2,
            Self::Remote => 3,
        }
    }
    #[must_use]
    pub const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Automatic),
            1 => Some(Self::DeviceCpu),
            2 => Some(Self::DeviceGpu),
            3 => Some(Self::Remote),
            _ => None,
        }
    }
}

/// The surface the stone lies on: a tray lit by the lighting rig, seen around the stone and
/// through its windows.
///
/// Mirrors `indicatrix::render_setup::Backdrop`, which has no serde support. Stored by
/// variant name; new variants are appended so older files stay valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Backdrop {
    /// The environment's own ground, as lit.
    AsLit,
    /// An 18 % grey card.
    Grey,
    /// White paper, 90 % reflectance (the default, like the desktop's).
    #[default]
    White,
    /// Black velvet, 2 % reflectance (appended last so stored names stay valid).
    Black,
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
    /// Image engine (0 Path-Traced, 1 Ray-Traced); unknown values keep old settings usable.
    #[serde(default)]
    pub engine: u8,
    pub backend: RenderBackend,
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
            engine: 0,
            backend: RenderBackend::Automatic,
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

impl RenderSettings {
    /// Fixed deterministic quality without changing the stored spectral sample targets.
    #[must_use]
    pub const fn engine_spec(&self) -> indicatrix::renderer::engine::EngineSpec {
        use indicatrix::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
        EngineSpec {
            engine: RenderEngine::from_index(self.engine),
            ray: RayOptions {
                aa_taps: match self.quality {
                    RenderQuality::Draft => 1,
                    RenderQuality::Normal => 4,
                    RenderQuality::Fine => 8,
                },
                fire: true,
            },
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
    /// The tray the stone lies on, lit by the rig (see [`Backdrop`]).
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
            backdrop: Backdrop::White,
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
    /// Whether a remote session may run on a metered mobile-data connection. Off by
    /// default: a lighting-map upload and `FullData` frames are many megabytes. A network
    /// whose metering the platform cannot tell is treated as unmetered.
    pub allow_metered: bool,
}

impl Default for RemoteSettings {
    fn default() -> Self {
        Self {
            name: String::new(),
            address: String::new(),
            transfer: RemoteTransfer::DisplayOnly,
            cadence_ms: 500,
            enrolled: false,
            allow_metered: false,
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

    /// Empties [`Self::recent`] and returns how many entries it held. The designs the
    /// entries pointed at are untouched.
    pub fn clear_recent(&mut self) -> usize {
        let held = self.recent.len();
        self.recent.clear();
        held
    }

    /// Removes the entries of [`Self::recent`] that point at the library entries `ids` and
    /// returns how many it removed. Call it when those entries are deleted: a line left in
    /// the list would still carry the deleted design's title (in `settings.toml` too) and
    /// could open nothing.
    pub fn forget_library_entries(&mut self, ids: &[i64]) -> usize {
        let before = self.recent.len();
        self.recent.retain(|item| {
            !matches!(item.kind, RecentKind::Library { entry_id } if ids.contains(&entry_id))
        });
        before - self.recent.len()
    }
}
