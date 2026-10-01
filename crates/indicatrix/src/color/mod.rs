/// CIE 1931 colour matching and D65 spectral integration.
///
/// Holds the tabulated 2-degree standard observer colour-matching functions.
pub mod cie1931;
/// Gamut mapping from CIE XYZ into a target RGB working space.
pub mod gamut;
/// Optical metrics computed from a traced gemstone image.
///
/// Covers brilliance, fire, scintillation, windowing and extinction.
pub mod metrics;
/// RGB colour-space definitions (primaries, white points, transfer functions) and
/// tone-mapping operators.
pub mod space;

pub use space::{ColorSpace, ToneMap, TransferFunction};
