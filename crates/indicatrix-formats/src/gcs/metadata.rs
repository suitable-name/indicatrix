//! Non-geometry `.gcs` elements: [`GcsIndex`] (the shared index-wheel setup),
//! [`GcsRender`]/[`GcsColor`] (preview material/display settings), and
//! [`GcsInfo`] (free-text design metadata).

/// The `<index .../>` element: the index-wheel setup shared by every tier.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct GcsIndex {
    /// Index-wheel tooth count. Confirmed to match `.asc`'s `g` line tooth count
    /// (as a magnitude) -- see the module docs.
    pub gear: u32,
    /// Kept as-is; presumed analogous to `.asc`'s gear reference-angle field, but
    /// every sample in the corpus has `base = 0`, so this module has no non-zero
    /// real example to confirm that against. Treat as **unconfirmed**.
    pub base: f64,
    /// Kept as-is. Sometimes matches the design's real rotational symmetry order
    /// (per its `.asc` sibling), but is very often `1` regardless -- see the
    /// module docs. Treat as **unconfirmed** unless cross-checked against another
    /// source for the specific design at hand.
    pub symmetry: u32,
    /// Kept as-is. **Not a boolean.** Real corpus values include `0` through `5`,
    /// which rules out a simple flag matching `.asc`'s `y`/`n` mirror field. This
    /// module could not determine what the value represents -- see the module
    /// docs.
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
    /// Refractive index. Confirmed to match `.asc`'s `I` line in every pair
    /// checked where the two files describe the same material -- see the module
    /// docs' "Known discrepancies" for the ones that do not.
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
