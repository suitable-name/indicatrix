//! The decoded photo: linear RGB with its provenance.

use std::fmt;

/// What a photo was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    /// A camera RAW or DNG file: black and white level applied, demosaiced, no tone curve.
    Raw,
    /// A 16-bit PNG or TIFF.
    Tiff16,
    /// An 8-bit JPEG, PNG or TIFF.
    Rgb8,
}

/// The shooting data of one photo, as far as it could be read. Every field is `None` when the
/// file does not carry it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CaptureMeta {
    /// The exposure time in seconds.
    pub exposure_time_s: Option<f64>,
    /// The ISO speed.
    pub iso: Option<u32>,
    /// The f-number.
    pub f_number: Option<f64>,
    /// The EXIF `WhiteBalance` tag: 0 is automatic, 1 is manual.
    pub white_balance_mode: Option<u16>,
    /// The camera maker.
    pub camera_make: Option<String>,
    /// The camera model.
    pub camera_model: Option<String>,
}

/// One camera colour matrix from the file, tagged with the DNG/EXIF illuminant code it belongs
/// to (1 daylight, 17 standard light A, 21 D65, 23 D50, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct ColourMatrix {
    /// The illuminant code (the EXIF `LightSource` numbering).
    pub illuminant: u16,
    /// The matrix, flat and row-major, as the file stores it. In `color_matrices` it is the DNG
    /// `ColorMatrix`, which maps XYZ to camera RGB (three columns, one row per camera channel):
    /// the direction is XYZ to camera, so a caller that needs camera to XYZ must invert it. In
    /// `forward_matrices` it is the DNG `ForwardMatrix`, camera to XYZ (D50). The direction is
    /// fixed by which list the entry sits in.
    pub xyz_to_camera_or_forward: Vec<f32>,
}

/// What the camera says about its own colour, kept for the spectral calibration (lane P2).
#[derive(Debug, Clone, PartialEq)]
pub struct CameraColour {
    /// The white-balance multipliers the file records, in R, G, B, E order (0 when absent).
    /// They are NOT applied to the pixels: the pixels are camera-native.
    pub as_shot_wb: [f32; 4],
    /// The XYZ-to-camera matrix rawler resolved for this camera (rows R, G, B, E). Direction:
    /// XYZ to camera.
    pub xyz_to_cam: [[f32; 3]; 4],
    /// The inverse direction as rawler computes it (`RawImage::cam_to_xyz`): camera to XYZ,
    /// three rows (X, Y, Z) of four camera channels (R, G, B, E). Use this with
    /// `CameraResponse::from_camera_to_xyz`; the white it is normalised to is rawler's
    /// choice, so treat it as `MatrixXyzWhite::Native` unless the lane report says otherwise.
    pub camera_to_xyz: [[f32; 4]; 3],
    /// The DNG `ColorMatrix` entries the file carries (direction XYZ to camera), ordered by
    /// illuminant code.
    pub color_matrices: Vec<ColourMatrix>,
    /// The DNG `ForwardMatrix` entries the file carries, ordered by illuminant code. Empty
    /// when the file has none; rawler 0.8 exposes them only as raw tag values, which this
    /// version does not read (see the lane report).
    pub forward_matrices: Vec<ColourMatrix>,
}

/// What went wrong in the photometry pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PhotometryError {
    /// The file could not be read.
    Io(String),
    /// The decoder refused the file.
    Decode(String),
    /// The file is valid but of a kind this module does not handle (for example a CFA other
    /// than a 2 by 2 Bayer pattern).
    Unsupported(String),
    /// Two images that must match in size do not.
    SizeMismatch {
        /// The expected width and height.
        expected: [usize; 2],
        /// The width and height found.
        found: [usize; 2],
    },
    /// A frame list that must not be empty is.
    NoFrames,
    /// The exposure of an HDR input is neither given nor in its EXIF data.
    MissingExposure,
    /// Not enough usable data for an estimate.
    InsufficientData(String),
    /// An argument is out of range.
    Invalid(String),
}

impl fmt::Display for PhotometryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(text) => write!(f, "cannot read the file: {text}"),
            Self::Decode(text) => write!(f, "cannot decode the photo: {text}"),
            Self::Unsupported(text) => write!(f, "unsupported photo: {text}"),
            Self::SizeMismatch { expected, found } => write!(
                f,
                "image size {}x{} does not match {}x{}",
                found[0], found[1], expected[0], expected[1]
            ),
            Self::NoFrames => write!(f, "no frames"),
            Self::MissingExposure => write!(f, "an exposure is neither given nor in the EXIF data"),
            Self::InsufficientData(text) => write!(f, "not enough data: {text}"),
            Self::Invalid(text) => write!(f, "invalid argument: {text}"),
        }
    }
}

impl std::error::Error for PhotometryError {}

/// A photo in linear light: f32 RGB, row-major, origin at the top left.
#[derive(Debug, Clone, PartialEq)]
pub struct LinearImage {
    /// The width in pixels.
    pub width: usize,
    /// The height in pixels.
    pub height: usize,
    /// The pixels, `width * height` of them, red, green, blue. Camera-native for RAW, sRGB
    /// primaries for the other sources. Values are not clamped: noise can push a dark pixel
    /// slightly below zero, and an HDR merge can exceed `full_scale`.
    pub pixels: Vec<[f32; 3]>,
    /// The variance of each channel in the pixels' units squared, when known (an HDR merge or a
    /// resample fills it).
    pub variance: Option<Vec<[f32; 3]>>,
    /// The value at which the sensor (or the encoding) saturates: 1.0 for a decoded photo.
    pub full_scale: f32,
    /// What the photo was decoded from.
    pub source: SourceKind,
    /// True when the values went through an assumed transfer curve instead of being measured as
    /// linear: every 8-bit source and any 16-bit one without a linear ICC profile. The fit report
    /// shows reduced confidence for these.
    pub non_linear_source: bool,
    /// The shooting data.
    pub meta: CaptureMeta,
    /// The camera's own colour data (RAW only).
    pub colour: Option<CameraColour>,
}

impl LinearImage {
    /// An image from its pixels, with `full_scale` 1, no variance, no metadata and the
    /// non-linear flag set from `source` (everything but RAW).
    ///
    /// # Errors
    ///
    /// [`PhotometryError::Invalid`] when `pixels.len() != width * height` or the image is empty.
    pub fn from_pixels(
        width: usize,
        height: usize,
        pixels: Vec<[f32; 3]>,
        source: SourceKind,
    ) -> Result<Self, PhotometryError> {
        if width == 0 || height == 0 || pixels.len() != width * height {
            return Err(PhotometryError::Invalid(format!(
                "{} pixels for a {width}x{height} image",
                pixels.len()
            )));
        }
        Ok(Self {
            width,
            height,
            pixels,
            variance: None,
            full_scale: 1.0,
            source,
            non_linear_source: source != SourceKind::Raw,
            meta: CaptureMeta::default(),
            colour: None,
        })
    }

    /// An image of one colour.
    ///
    /// # Errors
    ///
    /// As [`from_pixels`](Self::from_pixels).
    pub fn filled(
        width: usize,
        height: usize,
        value: [f32; 3],
        source: SourceKind,
    ) -> Result<Self, PhotometryError> {
        Self::from_pixels(width, height, vec![value; width * height], source)
    }

    /// The index of pixel `(x, y)` in [`pixels`](Self::pixels).
    #[must_use]
    pub const fn index(&self, x: usize, y: usize) -> usize {
        y * self.width + x
    }

    /// The pixel at `(x, y)`.
    ///
    /// # Panics
    ///
    /// When the coordinates are outside the image.
    #[must_use]
    pub fn get(&self, x: usize, y: usize) -> [f32; 3] {
        self.pixels[self.index(x, y)]
    }

    /// Width and height.
    #[must_use]
    pub const fn dims(&self) -> [usize; 2] {
        [self.width, self.height]
    }

    /// The pixel is at or above the saturation level (`fraction` of `full_scale`) in any
    /// channel.
    #[must_use]
    pub fn is_saturated(&self, x: usize, y: usize, fraction: f32) -> bool {
        let limit = self.full_scale * fraction;
        self.get(x, y).iter().any(|&v| v >= limit)
    }

    /// Checks that `other` has the same size.
    ///
    /// # Errors
    ///
    /// [`PhotometryError::SizeMismatch`].
    pub fn require_same_size(&self, other: &Self) -> Result<(), PhotometryError> {
        if self.dims() == other.dims() {
            Ok(())
        } else {
            Err(PhotometryError::SizeMismatch {
                expected: self.dims(),
                found: other.dims(),
            })
        }
    }
}
