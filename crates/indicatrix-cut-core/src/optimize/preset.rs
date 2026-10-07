//! Named objective weightings ([`ObjectivePreset`]) for a front end that offers a
//! choice instead of four sliders. Each preset is just an [`ObjectiveWeights`]; the
//! optimizer sees nothing but the weights.

use super::objective::{ObjectiveWeights, ToneGoal};

/// A named way to weigh the optical measurements, the yield and the face-up tone against
/// each other.
///
/// All of them scale the same weights of [`ObjectiveWeights`]; only their relative
/// sizes matter (see [`ObjectiveWeights::score_with_yield`]). [`Self::Balanced`] is
/// today's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ObjectivePreset {
    /// Windowing, extinction and tilt brilliance count equally; yield does not count.
    /// The same weights as [`ObjectiveWeights::default`].
    #[default]
    Balanced,
    /// Favours the light returned to the eye as the stone is tilted.
    Brilliance,
    /// Favours avoiding the see-through areas of the pavilion.
    LowWindowing,
    /// Favours avoiding the dark areas where light is lost.
    LowExtinction,
    /// Also weighs how much of the rough the finished stone keeps.
    KeepWeight,
    /// Favours a lighter stone face-up, for dark rough.
    LightenDark,
    /// Favours a stronger face-up colour, for pale rough.
    IntensifyPale,
}

impl ObjectivePreset {
    /// Every preset, in the order a list should show them.
    pub const ALL: [Self; 7] = [
        Self::Balanced,
        Self::Brilliance,
        Self::LowWindowing,
        Self::LowExtinction,
        Self::KeepWeight,
        Self::LightenDark,
        Self::IntensifyPale,
    ];

    /// The weights this preset stands for.
    ///
    /// | preset | windowing | extinction | tilt brilliance | yield | tone |
    /// |---|---|---|---|---|---|
    /// | Balanced | 1 | 1 | 1 | 0 | 0 |
    /// | Brilliance | 1 | 1 | 4 | 0 | 0 |
    /// | Low windowing | 4 | 1 | 1 | 0 | 0 |
    /// | Low extinction | 1 | 4 | 1 | 0 | 0 |
    /// | Keep weight | 1 | 1 | 1 | 3 | 0 |
    /// | Lighten dark rough | 1 | 1 | 1 | 0 | 3, lighter |
    /// | Intensify pale rough | 1 | 1 | 1 | 0 | 3, deeper |
    #[must_use]
    pub const fn weights(self) -> ObjectiveWeights {
        let (windowing, extinction, tilt_brilliance, yield_weight, tone_weight, tone_goal) =
            match self {
                Self::Balanced => (1.0, 1.0, 1.0, 0.0, 0.0, ToneGoal::Lighter),
                Self::Brilliance => (1.0, 1.0, 4.0, 0.0, 0.0, ToneGoal::Lighter),
                Self::LowWindowing => (4.0, 1.0, 1.0, 0.0, 0.0, ToneGoal::Lighter),
                Self::LowExtinction => (1.0, 4.0, 1.0, 0.0, 0.0, ToneGoal::Lighter),
                Self::KeepWeight => (1.0, 1.0, 1.0, 3.0, 0.0, ToneGoal::Lighter),
                Self::LightenDark => (1.0, 1.0, 1.0, 0.0, 3.0, ToneGoal::Lighter),
                Self::IntensifyPale => (1.0, 1.0, 1.0, 0.0, 3.0, ToneGoal::Deeper),
            };
        ObjectiveWeights {
            windowing,
            extinction,
            tilt_brilliance,
            yield_weight,
            tone_weight,
            tone_goal,
        }
    }

    /// A short name for a list or a combo box.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced",
            Self::Brilliance => "Brilliance",
            Self::LowWindowing => "Low windowing",
            Self::LowExtinction => "Low extinction",
            Self::KeepWeight => "Keep weight",
            Self::LightenDark => "Lighten dark rough",
            Self::IntensifyPale => "Intensify pale rough",
        }
    }

    /// One plain sentence saying what the preset favours.
    #[must_use]
    pub const fn description(self) -> &'static str {
        match self {
            Self::Balanced => "Balanced: brightness, windowing and extinction weighed equally.",
            Self::Brilliance => {
                "Brilliance: favours the light returned to the eye as the stone tilts."
            }
            Self::LowWindowing => {
                "Low windowing: favours avoiding see-through areas in the pavilion."
            }
            Self::LowExtinction => {
                "Low extinction: favours avoiding dark areas where light is lost."
            }
            Self::KeepWeight => {
                "Keep weight: also weighs how much of the rough the finished stone keeps."
            }
            Self::LightenDark => {
                "Lighten dark rough: favours shorter light paths through the body colour, so the stone shows lighter face-up."
            }
            Self::IntensifyPale => {
                "Intensify pale rough: favours longer light paths through the body colour, so the stone shows a stronger colour face-up."
            }
        }
    }

    /// Position in [`Self::ALL`], for a combo box.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Balanced => 0,
            Self::Brilliance => 1,
            Self::LowWindowing => 2,
            Self::LowExtinction => 3,
            Self::KeepWeight => 4,
            Self::LightenDark => 5,
            Self::IntensifyPale => 6,
        }
    }

    /// The preset at `index` in [`Self::ALL`]; an index past the end gives
    /// [`Self::Balanced`].
    #[must_use]
    pub const fn from_index(index: usize) -> Self {
        match index {
            1 => Self::Brilliance,
            2 => Self::LowWindowing,
            3 => Self::LowExtinction,
            4 => Self::KeepWeight,
            5 => Self::LightenDark,
            6 => Self::IntensifyPale,
            _ => Self::Balanced,
        }
    }

    /// The preset whose weights are exactly `weights`, if there is one -- so a front
    /// end can show which preset the current sliders still match.
    #[must_use]
    pub fn matching(weights: &ObjectiveWeights) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|preset| preset.weights() == *weights)
    }
}
