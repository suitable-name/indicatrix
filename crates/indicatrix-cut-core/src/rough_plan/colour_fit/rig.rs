//! The rig profile with its light model.

use super::forward::{ForwardError, RigLighting};
use crate::rough_plan::locate::RigProfile;

/// A [`RigProfile`] (cameras, indices) plus the light model of the rig (backlight panels, the
/// holder, the measured white frames).
///
/// Kept apart from the profile on purpose: the profile is saved with the user's rig, and its
/// bytes must not change with the `zoning` feature.
#[derive(Debug, Clone, PartialEq)]
pub struct ColourRig {
    /// The cameras and the refractive indices of the stone and its surroundings.
    pub rig: RigProfile,
    /// How the rig is lit and what occludes the light.
    pub lighting: RigLighting,
}

impl ColourRig {
    /// A rig with its light model.
    #[must_use]
    pub const fn new(rig: RigProfile, lighting: RigLighting) -> Self {
        Self { rig, lighting }
    }

    /// Checks the profile and the light model.
    ///
    /// # Errors
    ///
    /// [`ForwardError::BadRig`] for an unusable profile, [`ForwardError::BadLighting`] for a
    /// panel with a zero normal or size, or a white frame of the wrong shape.
    pub fn validate(&self) -> Result<(), ForwardError> {
        self.rig.validate().map_err(ForwardError::BadRig)?;
        self.lighting.validate()
    }
}
