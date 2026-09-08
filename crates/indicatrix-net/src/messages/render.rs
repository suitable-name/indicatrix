//! `-> RENDER`: [`RenderRequest`] and its [`StreamConfig`].
//!
//! Only compiled under this crate's `render` feature -- [`RenderRequest`] embeds
//! [`crate::scene::SceneState`], itself gated the same way, since a library-only
//! `indicatrix-worker` build never needs `indicatrix` at all.

use crate::scene::SceneState;
use serde::{Deserialize, Serialize};

/// Whether a `RENDER` request's full-resolution radiance is delivered progressively
/// (several small `FRAME`s as sampling proceeds) or only once, at the end.
///
/// Independent of [`StreamConfig::preview`]: a reduced-resolution `PREVIEW` (if
/// configured) is still sent on the cadence either way -- see [`StreamConfig`]'s docs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferMode {
    /// Emit a `FRAME` delta roughly every `cadence_ms` as sampling proceeds, plus a
    /// final one flushing whatever hasn't been sent yet.
    LiveProgressive,
    /// Emit exactly one `FRAME`, covering the whole requested sample range, once
    /// tracing finishes. `PROGRESS` (and `PREVIEW`, if configured) still arrive on the
    /// cadence in the meantime -- see [`StreamConfig`]'s docs.
    FinalOnly,
}

/// The reduced resolution a `PREVIEW` is rendered at, when [`StreamConfig::preview`] is
/// set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreviewConfig {
    pub width: u32,
    pub height: u32,
}

/// Per-request streaming configuration on a [`RenderRequest`].
///
/// So a viewer with an A100 on a LAN and one with a 2060 over hotel wifi can each pick a
/// cadence (and whether to bother with progressive delivery at all) that suits their own
/// link and hardware rather than share one hardcoded default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamConfig {
    pub transfer_mode: TransferMode,
    /// Target interval, in milliseconds, between emissions. Advisory, not a hard
    /// guarantee -- backpressure naturally widens the EFFECTIVE cadence when the
    /// requested one can't be sustained, reported back in
    /// [`super::stream::Stats::effective_cadence_ms`].
    pub cadence_ms: u32,
    /// When set, a cumulative, reduced-resolution `PREVIEW` is additionally sent on the
    /// cadence (see the crate's `messages` docs for why CUMULATIVE and display-only).
    /// Full resolution is still delivered via `FRAME` either way, so this is purely an
    /// extra, freely-droppable live look while the full-resolution result is pending.
    pub preview: Option<PreviewConfig>,
}

/// `-> RENDER`: a request to trace samples `[first_sample, first_sample + samples)`.
///
/// Sample ranges are disjoint across nodes and additive -- see the crate docs for why
/// that's what makes remote offload correct at all. `request_id` is chosen by the
/// client and echoed on every reply from this request onward -- see the crate's
/// `messages` docs on why that's what makes cancellation epochs mechanical.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderRequest {
    pub request_id: u32,
    pub scene: SceneState,
    pub first_sample: u32,
    pub samples: u32,
    pub stream: StreamConfig,
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
            width: 4,
            height: 4,
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
        }
    }

    #[test]
    fn render_request_round_trips_with_stream_config() {
        for stream in [
            StreamConfig {
                transfer_mode: TransferMode::LiveProgressive,
                cadence_ms: 250,
                preview: Some(PreviewConfig {
                    width: 64,
                    height: 64,
                }),
            },
            StreamConfig {
                transfer_mode: TransferMode::FinalOnly,
                cadence_ms: 1000,
                preview: None,
            },
        ] {
            let request = RenderRequest {
                request_id: 99,
                scene: scene(),
                first_sample: 10,
                samples: 20,
                stream,
            };
            let mut buf = Vec::new();
            write_message(&mut buf, &request).unwrap();
            let mut cursor = std::io::Cursor::new(buf);
            let decoded: RenderRequest = read_message(&mut cursor).unwrap();
            assert_eq!(request, decoded);
        }
    }
}
