/// Body color and colorimetry from absorption spectra and illuminants.
pub mod body_color;
/// CIE 1931 color matching and D65 spectral integration.
///
/// Holds the tabulated 2-degree standard observer color-matching functions.
pub mod cie1931;
/// Gamut mapping from CIE XYZ into a target RGB working space.
pub mod gamut;
/// The CIE 15:2018 LED illuminants (tabulated spectra). Only with the `zoning` feature.
#[cfg(feature = "zoning")]
pub mod led;
/// Optical metrics computed from a traced gemstone image.
///
/// Covers brilliance, fire, scintillation, windowing and extinction.
pub mod metrics;
/// RGB color-space definitions (primaries, white points, transfer functions) and
/// tone-mapping operators.
pub mod space;

pub use body_color::{
    BodyColor, BodyColors, Illuminant, body_color, body_colors, color_change_delta_e, delta_e_2000,
    srgb_to_lab, xyz_to_lab,
};
pub use space::{ColorSpace, ToneMap, TransferFunction};
