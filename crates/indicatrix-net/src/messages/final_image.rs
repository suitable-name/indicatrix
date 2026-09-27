//! `-> FINAL_IMAGE_REQUEST` (v14): ask for one finished, tone-mapped picture.
//!
//! Asks the server to render a whole sample range AND tone-map it, replying with one
//! 8-bit PNG instead of float radiance -- the "final picture only" transfer for still
//! exports and tilt videos.
//!
//! ```text
//! -> FinalImageRequest { request_id, scene, first_sample, samples, width, height, color_space, output }
//! <- PROGRESS          { request_id, samples_done }                 -- heartbeat, at least every 2 s
//! <- FINAL_IMAGE       { request_id, width, height, samples_done, encoding, payload_len } + raw PNG bytes
//! <- DONE              { request_id, cancelled: false, stats }      -- exactly once, after FINAL_IMAGE
//! ```
//!
//! # Reply semantics
//!
//! - `PROGRESS` heartbeats follow the same liveness rule as a `RENDER` stream (a client
//!   drops a silent connection after 8 s), so a server sends one at least every 2 s.
//! - On success the server sends exactly one `StreamEvent::FinalImage` followed by
//!   `DONE { cancelled: false }` with `stats.samples_done == samples`.
//! - A `CANCEL` for `request_id` ends the job with `DONE { cancelled: true }` and no
//!   `FINAL_IMAGE`; a failure ends it with `StreamEvent::Error` and no `DONE`, exactly as
//!   a `RENDER` stream ends on error.
//! - The PNG is what the viewer itself would have written from the same float sum: the
//!   server divides by `samples` and tone-maps with the same code path the GUI export
//!   uses, for `color_space`. It is therefore lossless relative to the delivered product.
//!
//! Only compiled under this crate's `render` feature, like `RenderRequest`.

use crate::scene::SceneState;
use serde::{Deserialize, Serialize};

/// The RGB colour space the server tone-maps into -- the wire twin of
/// `indicatrix::color::ColorSpace` (which has no serde derive). Variant order is
/// wire-load-bearing; append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WireColorSpace {
    /// IEC 61966-2-1 sRGB.
    Srgb,
    /// Display P3.
    DisplayP3,
    /// ITU-R BT.2020.
    Rec2020,
    /// `ACEScg` (scene-linear).
    AcesCg,
}

impl From<WireColorSpace> for indicatrix::color::ColorSpace {
    fn from(value: WireColorSpace) -> Self {
        match value {
            WireColorSpace::Srgb => Self::Srgb,
            WireColorSpace::DisplayP3 => Self::DisplayP3,
            WireColorSpace::Rec2020 => Self::Rec2020,
            WireColorSpace::AcesCg => Self::AcesCg,
        }
    }
}

impl From<indicatrix::color::ColorSpace> for WireColorSpace {
    fn from(value: indicatrix::color::ColorSpace) -> Self {
        match value {
            indicatrix::color::ColorSpace::Srgb => Self::Srgb,
            indicatrix::color::ColorSpace::DisplayP3 => Self::DisplayP3,
            indicatrix::color::ColorSpace::Rec2020 => Self::Rec2020,
            indicatrix::color::ColorSpace::AcesCg => Self::AcesCg,
        }
    }
}

/// The output format of a [`FinalImageRequest`]. Variant order is wire-load-bearing;
/// append only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinalOutput {
    /// An 8-bit RGBA PNG, non-interlaced, untagged (the viewer attaches an ICC profile
    /// for a wide-gamut `color_space` when it writes the file, as its own export does).
    PngRgba8,
}

/// `-> FINAL_IMAGE_REQUEST`: render `[first_sample, first_sample + samples)` of `scene`
/// and reply with the tone-mapped picture -- see the module doc comment for the reply
/// sequence.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FinalImageRequest {
    /// Client-chosen epoch id, echoed on every reply (same rules as `RenderRequest`).
    pub request_id: u32,
    /// The fully resolved scene to trace.
    pub scene: SceneState,
    /// First absolute sample index. Explicit so a server-side render of the same range
    /// is comparable (byte for byte) with a viewer-side one.
    pub first_sample: u32,
    /// Number of samples; the tone-mapping divisor.
    pub samples: u32,
    /// Output width in pixels. Must equal `scene.width` in v14 (a server refuses a
    /// mismatch); kept separate so a later version can add server-side scaling.
    pub width: u32,
    /// Output height in pixels. Must equal `scene.height` in v14, see [`Self::width`].
    pub height: u32,
    /// The colour space to tone-map into.
    pub color_space: WireColorSpace,
    /// The output format.
    pub output: FinalOutput,
}

impl FinalImageRequest {
    /// Whether the output size matches the scene, the only size v14 servers accept.
    #[must_use]
    pub const fn output_matches_scene(&self) -> bool {
        self.width == self.scene.width && self.height == self.scene.height
    }
}

#[cfg(test)]
mod tests {
    use super::{
        super::codec::{read_message, write_message},
        *,
    };

    fn scene() -> SceneState {
        use indicatrix::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };
        SceneState {
            width: 8,
            height: 6,
            yaw: 0.4,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: crate::scene::SceneEnvironment::Studio,
        }
    }

    #[test]
    fn final_image_request_round_trips_for_every_color_space() {
        for color_space in [
            WireColorSpace::Srgb,
            WireColorSpace::DisplayP3,
            WireColorSpace::Rec2020,
            WireColorSpace::AcesCg,
        ] {
            let request = FinalImageRequest {
                request_id: 3,
                scene: scene(),
                first_sample: 64,
                samples: 256,
                width: 8,
                height: 6,
                color_space,
                output: FinalOutput::PngRgba8,
            };
            assert!(request.output_matches_scene());
            let mut buf = Vec::new();
            write_message(&mut buf, &request).unwrap();
            let decoded: FinalImageRequest = read_message(&mut std::io::Cursor::new(buf)).unwrap();
            assert_eq!(decoded, request);
            let back: WireColorSpace = indicatrix::color::ColorSpace::from(color_space).into();
            assert_eq!(back, color_space);
        }
    }

    #[test]
    fn a_size_other_than_the_scenes_is_flagged() {
        let request = FinalImageRequest {
            request_id: 3,
            scene: scene(),
            first_sample: 0,
            samples: 1,
            width: 16,
            height: 6,
            color_space: WireColorSpace::Srgb,
            output: FinalOutput::PngRgba8,
        };
        assert!(!request.output_matches_scene());
    }
}
