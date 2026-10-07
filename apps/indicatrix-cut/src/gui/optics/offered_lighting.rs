//! Which lighting presets this build offers: the two UV lamps belong to the
//! `physical-color` feature ([`PHYSICS_COLOR_UI`]), like the fluorescence UI they light.
//!
//! The data layer still knows every preset (a scene, a job or a design file may name a UV
//! lamp), so the gate sits where a preset enters the desktop app from outside the combo box:
//! the settings file, a saved lighting preset, a design's own lighting and the combo's index.
//! A UV lamp reaching a build without the feature falls back to the editor default rig
//! ([`DEFAULT_LIGHTING_RIG`]), never an error.

use indicatrix::optics::LightingPreset;

use crate::{gui::optics::physics_state::PHYSICS_COLOR_UI, settings::model::DEFAULT_LIGHTING_RIG};

/// `preset` itself, or the editor default rig when it is a UV lamp and `uv_offered` is false.
#[must_use]
pub fn gate_preset(preset: LightingPreset, uv_offered: bool) -> LightingPreset {
    if uv_offered || !preset.is_uv_lamp() {
        preset
    } else {
        LightingPreset::from_label(DEFAULT_LIGHTING_RIG)
    }
}

/// [`gate_preset`] for this build ([`PHYSICS_COLOR_UI`]).
#[must_use]
pub fn offered(preset: LightingPreset) -> LightingPreset {
    gate_preset(preset, PHYSICS_COLOR_UI)
}

/// The preset a stored label names in this build: `LightingPreset::from_label`, gated.
#[must_use]
pub fn offered_from_label(label: &str) -> LightingPreset {
    offered(LightingPreset::from_label(label))
}

/// The preset the combo's index names in this build: `LightingPreset::from_index`, gated.
#[must_use]
pub fn offered_from_index(index: i32) -> LightingPreset {
    offered(LightingPreset::from_index(index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uv_lamps_fall_back_to_the_default_rig_without_the_feature() {
        let default = LightingPreset::from_label(DEFAULT_LIGHTING_RIG);
        assert_eq!(gate_preset(LightingPreset::UvLamp365, false), default);
        assert_eq!(gate_preset(LightingPreset::UvLamp395, false), default);
        assert!(!default.is_uv_lamp());
    }

    #[test]
    fn uv_lamps_are_kept_with_the_feature() {
        assert_eq!(
            gate_preset(LightingPreset::UvLamp365, true),
            LightingPreset::UvLamp365
        );
        assert_eq!(
            gate_preset(LightingPreset::UvLamp395, true),
            LightingPreset::UvLamp395
        );
    }

    #[test]
    fn visible_light_presets_are_never_changed() {
        for preset in LightingPreset::ALL {
            if !preset.is_uv_lamp() {
                assert_eq!(gate_preset(preset, false), preset);
                assert_eq!(gate_preset(preset, true), preset);
            }
        }
    }

    #[test]
    fn stored_labels_and_indices_follow_the_build() {
        let uv_label = LightingPreset::UvLamp365.label();
        let uv_index = LightingPreset::UvLamp395.index();
        let default = LightingPreset::from_label(DEFAULT_LIGHTING_RIG);
        let expect = |uv: LightingPreset| if PHYSICS_COLOR_UI { uv } else { default };
        assert_eq!(
            offered_from_label(uv_label),
            expect(LightingPreset::UvLamp365)
        );
        assert_eq!(
            offered_from_index(uv_index),
            expect(LightingPreset::UvLamp395)
        );
        assert_eq!(offered_from_index(9999), LightingPreset::from_index(9999));
    }
}
