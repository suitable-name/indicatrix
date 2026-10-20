//! The render-time `Backdrop` mapping: the surface the stone lies on, as an albedo, a
//! resolved tray radiance and a settings-dialog pill index.
//!
//! The backdrop is a physical tray: a matte grey surface of albedo `rho` under the stone,
//! extending to the horizon and lit by the lighting rig, seen by every ray that leaves the
//! stone and by the camera alike (see `optics::raytracer::environment::tray`). [`Backdrop::resolve`]
//! turns a choice into the radiance the scene carries.
//!
//! This is the PURE half only -- the desktop's `settings::model::Backdrop`
//! (`apps/indicatrix-cut/src/settings/model/app_settings.rs`) carries `serde`
//! derives for its on-disk settings-file representation, and this crate's own `serde`
//! dependency is optional/feature-gated (off by default, on only for
//! `indicatrix-net`'s wire protocol), so adding an unconditional `Serialize`/
//! `Deserialize` here would grow the base dependency footprint for every consumer,
//! not just the desktop app. The desktop keeps its own `Backdrop` enum with the serde
//! derives, mirroring this one variant-for-variant and delegating `resolve`/`index`/
//! `from_index` here -- see that type's own doc comment.
use crate::optics::raytracer::{
    LightingPreset,
    environment::{TRAY_ALBEDO_BLACK, TRAY_ALBEDO_GREY, TRAY_ALBEDO_WHITE, tray_radiance},
};

/// Default light yaw of the desktop and the browser, degrees (the pose [`Backdrop::level`]'s
/// temporary shim resolves at).
const SHIM_LIGHT_YAW_DEG: f32 = 48.0;
/// Default light pitch of the desktop and the browser, degrees.
const SHIM_LIGHT_PITCH_DEG: f32 = 72.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// The lighting preset's own ground, as lit (nothing below the girdle for the grading
    /// dome: the ISO/GemRay convention).
    AsLit,
    /// A photographic 18 % grey card.
    Grey,
    /// White paper (90 % reflectance), the default.
    #[default]
    White,
    /// Black velvet (2 % reflectance).
    Black,
}

impl Backdrop {
    /// The tray's albedo; `0.0` for [`Self::AsLit`] (the preset's own ground).
    #[must_use]
    pub const fn albedo(self) -> f32 {
        match self {
            Self::AsLit => 0.0,
            Self::Grey => TRAY_ALBEDO_GREY,
            Self::White => TRAY_ALBEDO_WHITE,
            Self::Black => TRAY_ALBEDO_BLACK,
        }
    }

    /// The tray radiance (at exposure 1) handed to `EnvironmentSource::with_backdrop`:
    /// `albedo * E_rig / pi` of `preset` at the light pose (radians), `0.0` for As Lit.
    #[must_use]
    pub fn resolve(self, preset: LightingPreset, light_yaw: f32, light_pitch: f32) -> f32 {
        tray_radiance(self.albedo(), preset, light_yaw, light_pitch)
    }

    /// TEMPORARY shim for the wave-2 lanes (desktop, web-core, mobile-core), which still call
    /// `level()`: [`Self::resolve`] at the default preset and the default light pose. Each
    /// caller must switch to `resolve` with its own scene's preset and pose; this is deleted
    /// by lanes B3/B4.
    #[must_use]
    pub fn level(self) -> f32 {
        self.resolve(
            LightingPreset::default(),
            SHIM_LIGHT_YAW_DEG.to_radians(),
            SHIM_LIGHT_PITCH_DEG.to_radians(),
        )
    }

    /// Index into the settings dialog's pill row (`SettingsModel.backdrop_index`). Black is
    /// appended as 3 so persisted indices stay valid.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::AsLit => 0,
            Self::Grey => 1,
            Self::White => 2,
            Self::Black => 3,
        }
    }

    /// Inverse of [`Self::index`]; anything else is the default ([`Self::White`]).
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            0 => Self::AsLit,
            1 => Self::Grey,
            3 => Self::Black,
            _ => Self::White,
        }
    }
}
