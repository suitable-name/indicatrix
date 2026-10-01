//! Non-geometry `.gcs` elements: [`GcsIndex`] (the shared index-wheel setup),
//! [`GcsRender`]/[`GcsColor`] (preview material/display settings), and
//! [`GcsInfo`] (free-text design metadata).

/// The `<index .../>` element: the index-wheel setup shared by every tier.
///
/// Only `gear` is required. `base`, `symmetry` and `mirror` are optional and
/// "represent the current state of the UI in the app ... NOT the values for
/// symmetry/mirror as would be printed in a faceting diagram" (Gem Cut Studio
/// User's Manual v1.1.0, p.58); the parser defaults them to `0`, `1` and `0`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsIndex {
    /// Index-wheel tooth count. Matches `.asc`'s `g` line tooth count as a
    /// magnitude (`.gcs` never signs it).
    pub gear: u32,
    /// UI state: the base index the app was set to (manual pp.36-44). `0` in
    /// every corpus file. Not `.asc`'s gear offset.
    pub base: f64,
    /// UI state: the symmetry of the tier being cut, which "does not necessarily
    /// define the final gem's symmetry" (manual p.40). Not a design property.
    pub symmetry: u32,
    /// UI state: the ± mirror offset in index steps around the base index (manual
    /// pp.41-44), `0` to `5` in the corpus. Not a boolean mirror flag.
    pub mirror: u32,
}

/// The `<color .../>` child of [`GcsRender`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsColor {
    /// Red channel, `0.0..=1.0`.
    pub r: f64,
    /// Green channel, `0.0..=1.0`.
    pub g: f64,
    /// Blue channel, `0.0..=1.0`.
    pub b: f64,
}

/// The `<render>...</render>` element: material and display settings used by Gem
/// Cut Studio's own preview, not by anything this crate does.
///
/// Kept because [`GcsIndex::gear`] and [`Self::refractive_index`] are this
/// module's two strongest cross-checks against a sibling `.asc`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GcsRender {
    /// Material name as Gem Cut Studio's own material library names it (e.g.
    /// `"176 Corundum"`, or the literal `"(from file)"` seen in some samples).
    pub material: String,
    /// Refractive index. Matches `.asc`'s `I` line wherever the two files describe
    /// the same material. `0.0` when the attribute is absent.
    pub refractive_index: f64,
    /// Dispersion value, as Gem Cut Studio's material library records it.
    pub dispersion: f64,
    /// Clarity setting used by the preview renderer.
    pub clarity: f64,
    /// Density setting used by the preview renderer.
    pub density: f64,
    /// Lighting model name (only `"Random"` seen in the sampled corpus).
    pub lighting_model: String,
    /// Preview display color.
    pub color: GcsColor,
}

/// The `<info .../>` element: free-text design metadata.
///
/// Every field is `Option` because the real corpus has at least four distinct
/// attribute sets -- e.g. some files carry `shape`/`header2`/`header3` and no
/// `size_min`/`size_max`, others the reverse -- so no attribute here is reliably
/// present.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GcsInfo {
    /// Design title.
    pub title: Option<String>,
    /// Designer/author name.
    pub author: Option<String>,
    /// Free-text publication date/venue (e.g. `"Star Cuts 1 1998"`); several real
    /// samples embed a trailing blank line inside the attribute value itself.
    pub date: Option<String>,
    /// Shape name (e.g. `"Octagon"`).
    pub shape: Option<String>,
    /// Second free-text header line.
    pub header2: Option<String>,
    /// Third free-text header line.
    pub header3: Option<String>,
    /// Minimum recommended refractive index, as a string (real samples format it
    /// inconsistently, e.g. `"1.7"` next to `"2.1500001"`, so this is not parsed
    /// to a number here).
    pub ri_min: Option<String>,
    /// Maximum recommended refractive index, as a string; see [`Self::ri_min`].
    pub ri_max: Option<String>,
    /// Minimum recommended finished size, as a string (an alternative to
    /// `shape`/`header2`/`header3` seen in some samples).
    pub size_min: Option<String>,
    /// Maximum recommended finished size, as a string; see [`Self::size_min`].
    pub size_max: Option<String>,
    /// First free-text footer line.
    pub footer1: Option<String>,
    /// Second free-text footer line.
    pub footer2: Option<String>,
    /// Third free-text footer line, present in some samples.
    pub footer3: Option<String>,
    /// Fourth free-text footer line, present in a few samples.
    pub footer4: Option<String>,
}
