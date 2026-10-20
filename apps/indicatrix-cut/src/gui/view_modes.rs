//! Typed desktop view modes at the boundary with Slint's integer properties.

use indicatrix::renderer::engine::RenderEngine;

/// Edit viewport choices. Existing discriminants are durable UI settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum EditViewMode {
    #[default]
    Solid = 0,
    PathTraced = 1,
    Both = 2,
    Diagram = 3,
    RayTraced = 4,
}

impl EditViewMode {
    /// Unknown persisted or toolkit values explicitly fall back to Solid.
    pub(crate) const fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::PathTraced,
            2 => Self::Both,
            3 => Self::Diagram,
            4 => Self::RayTraced,
            _ => Self::Solid,
        }
    }

    pub(crate) const fn raw(self) -> i32 {
        self as i32
    }

    pub(crate) const fn is_traced(self) -> bool {
        matches!(self, Self::PathTraced | Self::Both | Self::RayTraced)
    }

    pub(crate) const fn engine(self) -> RenderEngine {
        match self {
            Self::RayTraced => RenderEngine::RayTraced,
            _ => RenderEngine::PathTraced,
        }
    }
}

/// Live Render viewport choices, independent of the Edit viewport.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(i32)]
pub enum LiveViewMode {
    #[default]
    Solid = 0,
    PathTraced = 1,
    RayTraced = 2,
}

impl LiveViewMode {
    /// Unknown persisted or toolkit values explicitly fall back to Solid.
    pub(crate) const fn from_raw(raw: i32) -> Self {
        match raw {
            1 => Self::PathTraced,
            2 => Self::RayTraced,
            _ => Self::Solid,
        }
    }

    pub(crate) const fn raw(self) -> i32 {
        self as i32
    }

    pub(crate) const fn is_traced(self) -> bool {
        matches!(self, Self::PathTraced | Self::RayTraced)
    }

    pub(crate) const fn engine(self) -> RenderEngine {
        match self {
            Self::RayTraced => RenderEngine::RayTraced,
            _ => RenderEngine::PathTraced,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_known_mode_roundtrips_and_selects_its_engine() {
        for raw in 0..=4 {
            let mode = EditViewMode::from_raw(raw);
            assert_eq!(mode.raw(), raw);
            assert_eq!(mode.is_traced(), matches!(raw, 1 | 2 | 4));
            assert_eq!(mode.engine() == RenderEngine::RayTraced, raw == 4);
        }
        for raw in 0..=2 {
            let mode = LiveViewMode::from_raw(raw);
            assert_eq!(mode.raw(), raw);
            assert_eq!(mode.is_traced(), raw != 0);
            assert_eq!(mode.engine() == RenderEngine::RayTraced, raw == 2);
        }
        for raw in [-1, 5, i32::MAX] {
            assert_eq!(EditViewMode::from_raw(raw), EditViewMode::Solid);
            assert_eq!(LiveViewMode::from_raw(raw), LiveViewMode::Solid);
        }
    }
}
