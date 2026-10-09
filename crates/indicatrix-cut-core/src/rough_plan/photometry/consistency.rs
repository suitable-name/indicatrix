//! Consistency of the shooting data between the stone photo and its white and dark frames.
//!
//! Everything here is a warning for the import checklist, never an error: the pipeline still
//! runs on frames that disagree, the report just says so.

use super::{calibrate::CalibrationFrames, linear::LinearImage};

/// Which calibration frame a warning is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRole {
    /// The n-th white frame (0-based).
    White(usize),
    /// The n-th dark frame (0-based).
    Dark(usize),
}

/// One finding.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsistencyWarning {
    /// The exposure time differs between the stone photo and the frame.
    ExposureMismatch {
        /// The frame.
        frame: FrameRole,
        /// The stone photo's exposure time in seconds.
        stone: f64,
        /// The frame's exposure time in seconds.
        other: f64,
    },
    /// The ISO differs.
    IsoMismatch {
        /// The frame.
        frame: FrameRole,
        /// The stone photo's ISO.
        stone: u32,
        /// The frame's ISO.
        other: u32,
    },
    /// The white-balance mode differs.
    WhiteBalanceMismatch {
        /// The frame.
        frame: FrameRole,
        /// The stone photo's mode (EXIF `WhiteBalance`).
        stone: u16,
        /// The frame's mode.
        other: u16,
    },
    /// The f-number differs.
    ApertureMismatch {
        /// The frame.
        frame: FrameRole,
        /// The stone photo's f-number.
        stone: f64,
        /// The frame's f-number.
        other: f64,
    },
    /// The frame has another size than the stone photo.
    SizeMismatch {
        /// The frame.
        frame: FrameRole,
        /// The stone photo's width and height.
        stone: [usize; 2],
        /// The frame's width and height.
        other: [usize; 2],
    },
    /// A frame (or the stone photo) carries no shooting data, so it cannot be compared.
    MissingExif {
        /// The frame; `None` for the stone photo itself.
        frame: Option<FrameRole>,
    },
    /// The view has no white frame.
    NoWhiteFrame,
    /// A large part of the stone photo is clipped.
    Saturation {
        /// The clipped share of the pixels.
        fraction: f32,
    },
    /// The stone photo was not measured as linear (8-bit, or a 16-bit one without a linear
    /// profile), so colours will be less certain.
    NonLinearSource,
}

/// The findings for one view.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ConsistencyReport {
    /// The warnings, in a fixed order: white frames, then dark frames, then the photo itself.
    pub warnings: Vec<ConsistencyWarning>,
    /// The share of the stone photo's pixels with a clipped channel.
    pub saturated_fraction: f32,
}

/// Settings of [`check_view`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ConsistencyOptions {
    /// Exposure times and f-numbers differing by more than this relative amount are reported
    /// (default 0.5 %).
    pub relative_tolerance: f64,
    /// A pixel is clipped at this fraction of the full scale (default 0.98).
    pub saturation_fraction: f32,
    /// A saturation warning is raised above this share of clipped pixels (default 1 %).
    pub saturation_warn_fraction: f32,
}

impl Default for ConsistencyOptions {
    fn default() -> Self {
        Self {
            relative_tolerance: 0.005,
            saturation_fraction: 0.98,
            saturation_warn_fraction: 0.01,
        }
    }
}

fn differs(a: f64, b: f64, tolerance: f64) -> bool {
    (a - b).abs() > tolerance * a.abs().max(b.abs())
}

fn compare(
    stone: &LinearImage,
    other: &LinearImage,
    role: FrameRole,
    options: &ConsistencyOptions,
    out: &mut Vec<ConsistencyWarning>,
) {
    if stone.dims() != other.dims() {
        out.push(ConsistencyWarning::SizeMismatch {
            frame: role,
            stone: stone.dims(),
            other: other.dims(),
        });
    }
    let (a, b) = (&stone.meta, &other.meta);
    let nothing = |m: &super::linear::CaptureMeta| {
        m.exposure_time_s.is_none() && m.iso.is_none() && m.white_balance_mode.is_none()
    };
    if nothing(a) || nothing(b) {
        out.push(ConsistencyWarning::MissingExif { frame: Some(role) });
    }
    if let (Some(s), Some(o)) = (a.exposure_time_s, b.exposure_time_s)
        && differs(s, o, options.relative_tolerance)
    {
        out.push(ConsistencyWarning::ExposureMismatch {
            frame: role,
            stone: s,
            other: o,
        });
    }
    if let (Some(s), Some(o)) = (a.iso, b.iso)
        && s != o
    {
        out.push(ConsistencyWarning::IsoMismatch {
            frame: role,
            stone: s,
            other: o,
        });
    }
    if let (Some(s), Some(o)) = (a.f_number, b.f_number)
        && differs(s, o, options.relative_tolerance)
    {
        out.push(ConsistencyWarning::ApertureMismatch {
            frame: role,
            stone: s,
            other: o,
        });
    }
    if let (Some(s), Some(o)) = (a.white_balance_mode, b.white_balance_mode)
        && s != o
    {
        out.push(ConsistencyWarning::WhiteBalanceMismatch {
            frame: role,
            stone: s,
            other: o,
        });
    }
}

/// Compares the stone photo of a view with its calibration frames.
#[must_use]
pub fn check_view(
    stone: &LinearImage,
    frames: &CalibrationFrames,
    options: &ConsistencyOptions,
) -> ConsistencyReport {
    let mut warnings = Vec::new();
    if frames.white.is_empty() {
        warnings.push(ConsistencyWarning::NoWhiteFrame);
    }
    for (i, frame) in frames.white.iter().enumerate() {
        compare(stone, frame, FrameRole::White(i), options, &mut warnings);
    }
    for (i, frame) in frames.dark.iter().enumerate() {
        compare(stone, frame, FrameRole::Dark(i), options, &mut warnings);
    }
    let limit = stone.full_scale * options.saturation_fraction;
    let clipped = stone
        .pixels
        .iter()
        .filter(|p| p.iter().any(|&v| v >= limit))
        .count();
    let saturated_fraction = clipped as f32 / stone.pixels.len().max(1) as f32;
    if saturated_fraction > options.saturation_warn_fraction {
        warnings.push(ConsistencyWarning::Saturation {
            fraction: saturated_fraction,
        });
    }
    if stone.non_linear_source {
        warnings.push(ConsistencyWarning::NonLinearSource);
    }
    ConsistencyReport {
        warnings,
        saturated_fraction,
    }
}
