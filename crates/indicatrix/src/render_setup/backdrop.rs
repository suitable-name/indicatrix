//! The render-time `Backdrop` mapping: what the camera sees behind the stone, as a
//! radiance level and a settings-dialog pill index.
//!
//! This is the PURE half only -- the desktop's `settings::model::Backdrop`
//! (`apps/indicatrix-cut/src/settings/model/app_settings.rs`) carries `serde`
//! derives for its on-disk settings-file representation, and this crate's own `serde`
//! dependency is optional/feature-gated (off by default, on only for
//! `indicatrix-net`'s wire protocol), so adding an unconditional `Serialize`/
//! `Deserialize` here would grow the base dependency footprint for every consumer,
//! not just the desktop app. The desktop keeps its own `Backdrop` enum with the serde
//! derives, mirroring this one variant-for-variant and delegating `level`/`index`/
//! `from_index` here -- see that type's own doc comment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Backdrop {
    /// The environment's own ground, as lit.
    AsLit,
    /// A neutral grey backdrop card (about sRGB 160), for like-for-like comparisons.
    #[default]
    Grey,
    /// A white light box.
    White,
}

impl Backdrop {
    /// Backdrop radiance handed to `EnvironmentSource::with_backdrop`.
    #[must_use]
    pub const fn level(self) -> f32 {
        match self {
            Self::AsLit => 0.0,
            Self::Grey => crate::optics::raytracer::BACKDROP_GREY,
            Self::White => crate::optics::raytracer::BACKDROP_WHITE,
        }
    }

    /// Index into the settings dialog's pill row (`SettingsModel.backdrop_index`).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::AsLit => 0,
            Self::Grey => 1,
            Self::White => 2,
        }
    }

    /// Inverse of [`Self::index`]; anything else is the default.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            0 => Self::AsLit,
            2 => Self::White,
            _ => Self::Grey,
        }
    }
}
