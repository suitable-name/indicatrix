//! `-> FINAL_IMAGE_REQUEST` (v14): ask for one finished, tone-mapped picture.
//!
//! Asks the server to render a whole sample range AND tone-map it, replying with one
//! 8-bit PNG instead of float radiance -- the "final picture only" transfer for still
//! exports and tilt videos.
//!
//! ```text
//! -> FinalImageRequest { request_id, scene, first_sample, samples, width, height, color_space, output,
//!                        viewer_samples }
//! <- PROGRESS          { request_id, samples_done }                 -- heartbeat, at least every 2 s
//! <- PREVIEW           { .. }                                        -- optional; a coordinator sends a
//!                                                                       <= 360 px thumbnail about every 1 %
//! -> CONTRIBUTION (optional, v16) + payload -- the viewer's own share, see below
//! <- FINAL_IMAGE       { request_id, width, height, samples_done, encoding, payload_len } + raw PNG bytes
//! <- DONE              { request_id, cancelled: false, stats }      -- exactly once, after FINAL_IMAGE
//! ```
//!
//! # Viewer contribution (v16)
//!
//! `viewer_samples` reserves the LAST `viewer_samples` of `[first_sample, first_sample +
//! samples)` for the viewer itself: the server only plans `server_samples()` of it, and
//! the viewer renders the rest locally and uploads the sum as one `CONTRIBUTION` (see
//! `crate::messages::contribution`). `0` is today's behaviour (the server plans the
//! whole range). `viewer_samples` must be at most half of `samples`
//! ([`FinalImageRequest::viewer_share_valid`]), or the server refuses the request with
//! `VALIDATION_FAILED`. If the contribution does not arrive within the server's wait
//! (or arrives invalid), the server renders that tail itself and reports how many
//! samples it reclaimed in `Stats.reclaimed_samples` -- the export still succeeds
//! either way.
//!
//! # Reply semantics
//!
//! - `PROGRESS` heartbeats follow the same liveness rule as a `RENDER` stream (a client
//!   drops a silent connection after 8 s), so a server sends one at least every 2 s.
//! - `PREVIEW` frames are optional and only a coordinator job emits them (a plain worker
//!   never does): a box-downsampled thumbnail, long edge at most 360 px, sent when
//!   `samples_done` has advanced by about 1 % of `samples` since the previous one. A
//!   client that does not want them may ignore them.
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

/// The RGB color space the server tone-maps into -- the wire twin of
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
    /// The color space to tone-map into.
    pub color_space: WireColorSpace,
    /// The output format.
    pub output: FinalOutput,
    /// v16: the viewer renders the LAST `viewer_samples` of `[first_sample,
    /// first_sample+samples)` itself and uploads them as one `CONTRIBUTION`; the server
    /// plans only `samples - viewer_samples`. `0` = today's behaviour. Must be `<=
    /// samples / 2` (else `VALIDATION_FAILED`).
    #[serde(default)]
    pub viewer_samples: u32,
}

impl FinalImageRequest {
    /// Whether the output size matches the scene, the only size v14 servers accept.
    #[must_use]
    pub const fn output_matches_scene(&self) -> bool {
        self.width == self.scene.width && self.height == self.scene.height
    }

    /// The number of samples the SERVER plans -- `samples` minus the viewer's reserved
    /// tail.
    #[must_use]
    pub const fn server_samples(&self) -> u32 {
        self.samples.saturating_sub(self.viewer_samples)
    }

    /// Whether [`Self::viewer_samples`] is at most half of `samples` -- the only share
    /// a server accepts.
    #[must_use]
    pub const fn viewer_share_valid(&self) -> bool {
        self.viewer_samples <= self.samples / 2
    }

    /// `(first, count)` of the viewer-reserved tail, `None` when [`Self::viewer_samples`]
    /// is `0`.
    #[must_use]
    pub const fn reserved_range(&self) -> Option<(u32, u32)> {
        if self.viewer_samples == 0 {
            None
        } else {
            Some((
                self.first_sample + self.server_samples(),
                self.viewer_samples,
            ))
        }
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
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: Default::default(),
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
                viewer_samples: 32,
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
            viewer_samples: 0,
        };
        assert!(!request.output_matches_scene());
    }

    #[test]
    fn the_reserved_range_is_the_tail_and_share_over_half_is_invalid() {
        let request = FinalImageRequest {
            request_id: 3,
            scene: scene(),
            first_sample: 100,
            samples: 10,
            width: 8,
            height: 6,
            color_space: WireColorSpace::Srgb,
            output: FinalOutput::PngRgba8,
            viewer_samples: 4,
        };
        assert_eq!(request.server_samples(), 6);
        assert!(request.viewer_share_valid());
        assert_eq!(request.reserved_range(), Some((106, 4)));

        let no_viewer = FinalImageRequest {
            viewer_samples: 0,
            ..request.clone()
        };
        assert_eq!(no_viewer.server_samples(), 10);
        assert_eq!(no_viewer.reserved_range(), None);

        let over_half = FinalImageRequest {
            viewer_samples: 6,
            ..request
        };
        assert!(!over_half.viewer_share_valid());
    }
}
