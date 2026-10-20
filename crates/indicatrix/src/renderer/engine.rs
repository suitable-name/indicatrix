//! Serializable rendering choices shared by every front end.

/// The image-forming tracer. Analytical metrics use their own tracer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum RenderEngine {
    #[default]
    PathTraced,
    RayTraced,
}

impl RenderEngine {
    #[must_use]
    pub const fn index(self) -> u8 {
        match self {
            Self::PathTraced => 0,
            Self::RayTraced => 1,
        }
    }

    /// Unknown persisted values retain the established path tracer.
    #[must_use]
    pub const fn from_index(index: u8) -> Self {
        match index {
            1 => Self::RayTraced,
            _ => Self::PathTraced,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::PathTraced => "Path-Traced",
            Self::RayTraced => "Ray-Traced",
        }
    }
}

pub const AA_TAP_CHOICES: [u8; 4] = [1, 4, 8, 16];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default)
)]
pub struct RayOptions {
    pub aa_taps: u8,
    pub fire: bool,
}

impl Default for RayOptions {
    fn default() -> Self {
        Self {
            aa_taps: 8,
            fire: true,
        }
    }
}

impl RayOptions {
    /// Round an unsupported tap count up to a supported count, capped at sixteen.
    #[must_use]
    pub const fn sanitized(self) -> Self {
        let aa_taps = match self.aa_taps {
            0..=1 => 1,
            2..=4 => 4,
            5..=8 => 8,
            _ => 16,
        };
        Self {
            aa_taps,
            fire: self.fire,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default)
)]
pub struct EngineSpec {
    pub engine: RenderEngine,
    pub ray: RayOptions,
}

impl EngineSpec {
    #[must_use]
    pub const fn is_default(&self) -> bool {
        matches!(self.engine, RenderEngine::PathTraced)
    }
    #[must_use]
    pub const fn is_ray(&self) -> bool {
        matches!(self.engine, RenderEngine::RayTraced)
    }
    #[must_use]
    pub fn target_samples(&self, path_target: u32) -> u32 {
        if self.is_ray() {
            u32::from(self.ray.sanitized().aa_taps)
        } else {
            path_target
        }
    }
    #[must_use]
    pub const fn denoises(&self) -> bool {
        !self.is_ray()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_preserve_the_path_tracer_and_unknown_indices_are_safe() {
        let spec = EngineSpec::default();
        assert!(spec.is_default());
        assert!(spec.denoises());
        assert_eq!(spec.target_samples(512), 512);
        assert_eq!(RenderEngine::from_index(255), RenderEngine::PathTraced);
        for engine in [RenderEngine::PathTraced, RenderEngine::RayTraced] {
            assert_eq!(RenderEngine::from_index(engine.index()), engine);
        }
    }

    #[test]
    fn every_serialized_tap_count_has_a_bounded_target() {
        for aa_taps in 0..=255 {
            let spec = EngineSpec {
                engine: RenderEngine::RayTraced,
                ray: RayOptions {
                    aa_taps,
                    fire: true,
                },
            };
            assert!(AA_TAP_CHOICES.contains(&spec.ray.sanitized().aa_taps));
            assert_eq!(
                spec.target_samples(1000),
                u32::from(spec.ray.sanitized().aa_taps)
            );
            assert!(!spec.denoises());
        }
    }
}
