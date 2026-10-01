//! Body-colour presets: the nine fixed absorption-RGB triples a design (or the
//! custom-material editor) can pick from to recolour a material without authoring a
//! new one.
//!
//! Each triple is the same `[R, G, B]` absorption input [`super::GemMaterial::new_custom`]
//! and [`super::GemMaterial::with_body_colour`] take (expanded through
//! `absorption::legacy_rgb_bands`): a HIGH value in a channel absorbs that channel,
//! so "Blue" absorbs red and green. The table is data only -- the order is part of the
//! contract, since the desktop editor stores a swatch as its index into this table.

/// One body-colour preset: a stable machine key, the short label shown to a cutter,
/// and the absorption triple it applies.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BodyColourPreset {
    /// Stable lowercase identifier (`"yellow"`), never shown to a cutter.
    pub key: &'static str,
    /// Short display label (`"Yellow"`), e.g. for `Sapphire (Yellow)`.
    pub label: &'static str,
    /// The `[R, G, B]` absorption triple (see this module's doc comment).
    pub absorption_rgb: [f32; 3],
}

/// The nine body-colour presets, in their fixed order.
///
/// Clear, Blue, Red, Green, Violet, Yellow, Pink, Teal, Amber. The custom-material
/// editor's swatch row uses the same order
/// (index 0 "Clear" through 8 "Amber"), so an index into this table is also a valid
/// swatch index there.
pub const BODY_COLOUR_PRESETS: [BodyColourPreset; 9] = [
    BodyColourPreset {
        key: "clear",
        label: "Clear",
        absorption_rgb: [0.0, 0.0, 0.0],
    },
    BodyColourPreset {
        key: "blue",
        label: "Blue",
        absorption_rgb: [2.8, 1.2, 0.1],
    },
    BodyColourPreset {
        key: "red",
        label: "Red",
        absorption_rgb: [0.1, 2.5, 2.2],
    },
    BodyColourPreset {
        key: "green",
        label: "Green",
        absorption_rgb: [2.2, 0.2, 2.0],
    },
    BodyColourPreset {
        key: "violet",
        label: "Violet",
        absorption_rgb: [1.8, 1.6, 0.2],
    },
    BodyColourPreset {
        key: "yellow",
        label: "Yellow",
        absorption_rgb: [0.2, 0.4, 2.8],
    },
    BodyColourPreset {
        key: "pink",
        label: "Pink",
        absorption_rgb: [0.4, 2.2, 1.6],
    },
    BodyColourPreset {
        key: "teal",
        label: "Teal",
        absorption_rgb: [1.2, 0.4, 0.1],
    },
    BodyColourPreset {
        key: "amber",
        label: "Amber",
        absorption_rgb: [0.2, 0.6, 1.8],
    },
];

/// The index into [`BODY_COLOUR_PRESETS`] whose triple equals `rgb` exactly, or
/// `None` when `rgb` matches no preset (a colour authored elsewhere, e.g. a
/// hand-edited design file).
#[must_use]
pub fn preset_index_for_rgb(rgb: [f32; 3]) -> Option<usize> {
    BODY_COLOUR_PRESETS
        .iter()
        .position(|preset| preset.absorption_rgb == rgb)
}

/// The display label of the preset whose triple equals `rgb` exactly (see
/// [`preset_index_for_rgb`]), or `None` when it matches no preset.
#[must_use]
pub fn preset_label_for_rgb(rgb: [f32; 3]) -> Option<&'static str> {
    preset_index_for_rgb(rgb).map(|index| BODY_COLOUR_PRESETS[index].label)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::optics::absorption::legacy_rgb_bands;

    #[test]
    fn every_preset_finds_its_own_index() {
        for (index, preset) in BODY_COLOUR_PRESETS.iter().enumerate() {
            assert_eq!(preset_index_for_rgb(preset.absorption_rgb), Some(index));
            assert_eq!(
                preset_label_for_rgb(preset.absorption_rgb),
                Some(preset.label)
            );
        }
    }

    #[test]
    fn an_unlisted_triple_matches_no_preset() {
        assert_eq!(preset_index_for_rgb([0.5, 0.5, 0.5]), None);
        assert_eq!(preset_label_for_rgb([0.5, 0.5, 0.5]), None);
    }

    /// Summed absorption of a preset's expanded band set at `lambda_nm`.
    fn absorption_at(rgb: [f32; 3], lambda_nm: f32) -> f32 {
        legacy_rgb_bands(rgb)
            .iter()
            .map(|band| band.evaluate(lambda_nm))
            .sum()
    }

    /// A high channel value absorbs that channel, so Teal (blue-green) must absorb red
    /// most and Amber (orange-brown) must absorb blue most.
    #[test]
    fn teal_absorbs_red_and_amber_absorbs_blue() {
        let teal = BODY_COLOUR_PRESETS[7].absorption_rgb;
        let amber = BODY_COLOUR_PRESETS[8].absorption_rgb;
        assert_eq!(BODY_COLOUR_PRESETS[7].key, "teal");
        assert_eq!(BODY_COLOUR_PRESETS[8].key, "amber");

        let (teal_red, teal_green, teal_blue) = (
            absorption_at(teal, 620.0),
            absorption_at(teal, 540.0),
            absorption_at(teal, 450.0),
        );
        assert!(
            teal_red > teal_green && teal_red > teal_blue,
            "Teal must absorb most at 620 nm (red {teal_red}, green {teal_green}, blue {teal_blue})"
        );

        let (amber_red, amber_green, amber_blue) = (
            absorption_at(amber, 620.0),
            absorption_at(amber, 540.0),
            absorption_at(amber, 450.0),
        );
        assert!(
            amber_blue > amber_red && amber_blue > amber_green,
            "Amber must absorb most at 450 nm (red {amber_red}, green {amber_green}, blue {amber_blue})"
        );
    }

    #[test]
    fn keys_and_labels_are_unique() {
        for (i, a) in BODY_COLOUR_PRESETS.iter().enumerate() {
            for b in &BODY_COLOUR_PRESETS[i + 1..] {
                assert_ne!(a.key, b.key);
                assert_ne!(a.label, b.label);
            }
        }
    }
}
