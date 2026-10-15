//! Window size classes and the four top-level destinations of the mobile app.
//!
//! The front end asks [`WindowClass::from_width`] for the layout family that fits the
//! current window width (a phone in portrait is `Compact`, a small tablet or a phone
//! unfolded is `Medium`, a tablet or desktop window is `Expanded`) and keeps the user's
//! place in the app as a [`Destination`].

use serde::{Deserialize, Serialize};

/// The window width family, after the Material 3 window size classes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowClass {
    /// Narrower than [`WindowClass::COMPACT_MAX_DP`]: one pane, bottom navigation.
    Compact,
    /// From [`WindowClass::COMPACT_MAX_DP`] up to [`WindowClass::MEDIUM_MAX_DP`]: a
    /// navigation rail and one or two panes.
    Medium,
    /// [`WindowClass::MEDIUM_MAX_DP`] and wider: rail and side-by-side panes.
    Expanded,
}

impl WindowClass {
    /// Widths below this many dp are [`WindowClass::Compact`].
    pub const COMPACT_MAX_DP: f32 = 600.0;
    /// Widths below this many dp (and at least [`Self::COMPACT_MAX_DP`]) are
    /// [`WindowClass::Medium`]; wider ones are [`WindowClass::Expanded`].
    pub const MEDIUM_MAX_DP: f32 = 840.0;

    /// The class for a window `width_dp` wide. A width that is not a number or is
    /// negative counts as [`WindowClass::Compact`].
    #[must_use]
    pub fn from_width(width_dp: f32) -> Self {
        if width_dp >= Self::MEDIUM_MAX_DP {
            Self::Expanded
        } else if width_dp >= Self::COMPACT_MAX_DP {
            Self::Medium
        } else {
            // Below the compact limit, negative, or NaN (every comparison above is false).
            Self::Compact
        }
    }

    /// The class as the integer the Slint UI uses: 0 Compact, 1 Medium, 2 Expanded.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Compact => 0,
            Self::Medium => 1,
            Self::Expanded => 2,
        }
    }
}

/// A top-level place in the app, in navigation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum Destination {
    /// The design library.
    Library,
    /// The design editor (the start-up destination).
    #[default]
    Design,
    /// Cutting mode and the cutting sheet.
    Cut,
    /// Settings, help, about.
    More,
}

impl Destination {
    /// Every destination, in navigation order.
    pub const ALL: [Self; 4] = [Self::Library, Self::Design, Self::Cut, Self::More];

    /// The position in navigation order, 0 to 3.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Library => 0,
            Self::Design => 1,
            Self::Cut => 2,
            Self::More => 3,
        }
    }

    /// The destination at navigation position `index`; `None` outside 0 to 3.
    #[must_use]
    pub const fn from_index(index: i32) -> Option<Self> {
        match index {
            0 => Some(Self::Library),
            1 => Some(Self::Design),
            2 => Some(Self::Cut),
            3 => Some(Self::More),
            _ => None,
        }
    }

    /// The Title Case label shown in the navigation bar.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Library => "Library",
            Self::Design => "Design",
            Self::Cut => "Cut",
            Self::More => "More",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn window_class_boundaries() {
        assert_eq!(WindowClass::from_width(0.0), WindowClass::Compact);
        assert_eq!(WindowClass::from_width(599.9), WindowClass::Compact);
        assert_eq!(WindowClass::from_width(600.0), WindowClass::Medium);
        assert_eq!(WindowClass::from_width(839.9), WindowClass::Medium);
        assert_eq!(WindowClass::from_width(840.0), WindowClass::Expanded);
        assert_eq!(WindowClass::from_width(4000.0), WindowClass::Expanded);
    }

    #[test]
    fn window_class_odd_widths_are_compact() {
        assert_eq!(WindowClass::from_width(f32::NAN), WindowClass::Compact);
        assert_eq!(WindowClass::from_width(-1.0), WindowClass::Compact);
        assert_eq!(
            WindowClass::from_width(f32::NEG_INFINITY),
            WindowClass::Compact
        );
        assert_eq!(
            WindowClass::from_width(f32::INFINITY),
            WindowClass::Expanded
        );
    }

    #[test]
    fn window_class_indices() {
        assert_eq!(WindowClass::Compact.index(), 0);
        assert_eq!(WindowClass::Medium.index(), 1);
        assert_eq!(WindowClass::Expanded.index(), 2);
    }

    #[test]
    fn destination_index_round_trip() {
        for (position, destination) in Destination::ALL.into_iter().enumerate() {
            assert_eq!(
                destination.index(),
                i32::try_from(position).expect("a handful of destinations fit i32")
            );
            assert_eq!(
                Destination::from_index(destination.index()),
                Some(destination)
            );
        }
        assert_eq!(Destination::from_index(-1), None);
        assert_eq!(Destination::from_index(4), None);
    }

    #[test]
    fn destination_default_and_labels() {
        assert_eq!(Destination::default(), Destination::Design);
        let labels: Vec<_> = Destination::ALL.iter().map(|d| d.label()).collect();
        assert_eq!(labels, ["Library", "Design", "Cut", "More"]);
    }
}
