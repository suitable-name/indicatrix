//! The unit a mesh file's numbers are in, and the suggestion shown when a rough's size is
//! not believable in the unit that was chosen.

/// The largest side, in mm, a rough is typically (the suggestion looks here first).
const TYPICAL_MM: (f64, f64) = (3.0, 200.0);

/// The largest side, in mm, the planner accepts at all (1 mm to its 2000 mm limit).
const BELIEVABLE_MM: (f64, f64) = (1.0, 2000.0);

/// A length unit a mesh file may use. The planner works in millimetres, so the points are
/// scaled by [`MeshUnit::factor_to_mm`] between parsing and import.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MeshUnit {
    /// Millimetres (the default; no scaling).
    #[default]
    Millimetre,
    /// Centimetres.
    Centimetre,
    /// Metres.
    Metre,
    /// Inches.
    Inch,
    /// Micrometres.
    Micrometre,
}

impl MeshUnit {
    /// Every unit, in the order of the window's unit box (index 0 is the default).
    pub const ALL: [Self; 5] = [
        Self::Millimetre,
        Self::Centimetre,
        Self::Metre,
        Self::Inch,
        Self::Micrometre,
    ];

    /// The unit at position `index` of the window's unit box; anything out of range is
    /// millimetres.
    pub fn from_index(index: i32) -> Self {
        usize::try_from(index)
            .ok()
            .and_then(|i| Self::ALL.get(i).copied())
            .unwrap_or_default()
    }

    /// How many millimetres one of this unit is.
    pub const fn factor_to_mm(self) -> f64 {
        match self {
            Self::Millimetre => 1.0,
            Self::Centimetre => 10.0,
            Self::Metre => 1000.0,
            Self::Inch => 25.4,
            Self::Micrometre => 0.001,
        }
    }

    /// The short label the window's unit box shows.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Millimetre => "mm",
            Self::Centimetre => "cm",
            Self::Metre => "m",
            Self::Inch => "inch",
            Self::Micrometre => "\u{b5}m",
        }
    }

    /// The unit written out, in the plural ("metres").
    pub const fn plural(self) -> &'static str {
        match self {
            Self::Millimetre => "millimetres",
            Self::Centimetre => "centimetres",
            Self::Metre => "metres",
            Self::Inch => "inches",
            Self::Micrometre => "micrometres",
        }
    }

    /// The unit, other than `current`, that would bring a file whose largest side is
    /// `raw_largest` (in the file's own numbers) to a believable size: first one that gives
    /// 3 to 200 mm, else one that gives 1 to 2000 mm. The units are tried in the order of
    /// [`MeshUnit::ALL`], so the answer never depends on anything else. `None` when no
    /// unit fits or the size is not a positive number.
    pub fn suggest(raw_largest: f64, current: Self) -> Option<Self> {
        if !(raw_largest.is_finite() && raw_largest > 0.0) {
            return None;
        }
        [TYPICAL_MM, BELIEVABLE_MM]
            .into_iter()
            .find_map(|(lo, hi)| {
                Self::ALL.into_iter().find(|&unit| {
                    unit != current && (lo..=hi).contains(&(raw_largest * unit.factor_to_mm()))
                })
            })
    }
}
