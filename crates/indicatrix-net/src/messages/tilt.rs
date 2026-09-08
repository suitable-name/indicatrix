//! `-> TILT_CURVES`: [`TiltCurvesRequest`] and its single reply [`TiltCurvesResponse`] --
//! a design's full tilt-performance sweep (brilliance/extinction/windowing across 4
//! axes, 181 points each), computed once and returned whole.
//!
//! Only compiled under this crate's `render` feature -- [`TiltCurvesRequest`] embeds
//! [`crate::scene::SceneState`], gated the same way `RenderRequest` is.
//!
//! Its own small request/response family rather than a [`super::stream::StreamEvent`]
//! stream: a `TiltCurvesRequest` produces exactly one reply, 8,688 bytes, at the end
//! (~1.36s total, measured, to compute the axis profile four times) -- nothing
//! progressive to report, so `RENDER`'s streaming machinery buys nothing here.
//! [`TiltCurvesResponse`] mirrors [`crate::library::LibraryResponse`]'s plain-enum shape
//! instead, and its `Error` case reuses `RenderRequest`'s `ErrorMsg` shape and error
//! codes rather than a second failure vocabulary.

use crate::{messages::ErrorMsg, scene::SceneState};
use serde::{Deserialize, Serialize};

/// Sample points per axis in a [`TiltCurvesResponse`]'s curves.
///
/// Index `i` holds the value at tilt-away-from-table-up `(i - 90)` degrees: index 90 is
/// table-up (tilt 0), indices 0/180 are the edge-on extremes at tilt -90/+90.
///
/// Pinned independently of `indicatrix`'s and `indicatrix_vault`'s own copies of this
/// constant, keeping wire shapes independent of any one crate's internal representation
/// (`apps/indicatrix-worker` maps between them); a drift shows up as disagreeing
/// constants rather than a silent shared change.
pub const TILT_CURVE_POINTS_PER_AXIS: usize = 181;

/// Axes per [`TiltCurvesResponse`]. Matches
/// `indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG`'s length (`[0.0, 45.0, 90.0,
/// 135.0]`); see [`TILT_CURVE_POINTS_PER_AXIS`] for why this crate pins its own copy.
pub const TILT_CURVE_AXIS_COUNT: usize = 4;

/// One axis's three tilt sweeps, each already a 0..100-scale percentage.
///
/// Matches `indicatrix::color::metrics::evaluate_full_axis_profile_at_azimuth`'s return
/// convention (`(brilliance_pct, extinction_pct, windowing_pct)`).
///
/// Stored as `Vec<f32>`, not `[f32; TILT_CURVE_POINTS_PER_AXIS]`, since serde's array
/// impls stop at length 32. Logically still exactly [`TILT_CURVE_POINTS_PER_AXIS`]
/// points per curve -- [`Self::from_arrays`] converts from the fixed-size array form.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AxisTiltCurves {
    pub brilliance_pct: Vec<f32>,
    pub extinction_pct: Vec<f32>,
    pub windowing_pct: Vec<f32>,
}

impl AxisTiltCurves {
    /// Builds one axis's curves from the fixed-size arrays
    /// `indicatrix::color::metrics::evaluate_full_axis_profile_at_azimuth` returns.
    #[must_use]
    pub fn from_arrays(
        brilliance_pct: [f32; TILT_CURVE_POINTS_PER_AXIS],
        extinction_pct: [f32; TILT_CURVE_POINTS_PER_AXIS],
        windowing_pct: [f32; TILT_CURVE_POINTS_PER_AXIS],
    ) -> Self {
        Self {
            brilliance_pct: brilliance_pct.to_vec(),
            extinction_pct: extinction_pct.to_vec(),
            windowing_pct: windowing_pct.to_vec(),
        }
    }
}

/// `-> TILT_CURVES`: a request for one design's full tilt-performance sweep.
///
/// Reuses [`SceneState`] as its geometry payload so `validate_scene` stays the one
/// shared validation path for both `RENDER` and `TILT_CURVES`.
///
/// Only FOUR fields actually feed the per-axis profile evaluation: `scene.planes`,
/// `scene.material`, `scene.light_yaw`, `scene.light_pitch`. Every other field
/// (`width`/`height`/`yaw`/`pitch`/`distance`/`exposure`/`max_bounces`/
/// `lighting_preset`/`girdle_frosted`) is IGNORED -- a tilt sweep has no single camera
/// frame; the evaluator derives its own 181 camera poses per axis. Ignored fields are
/// still validated unconditionally, to avoid a second validation path to keep in sync
/// with `RenderRequest`'s.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltCurvesRequest {
    pub request_id: u32,
    pub scene: SceneState,
}

/// A completed [`TiltCurvesRequest`]'s full sweep: [`TILT_CURVE_AXIS_COUNT`] axes, axis
/// `k` holding the sweep at `indicatrix::color::metrics::PROFILE_AZIMUTHS_DEG[k]`.
///
/// Same canonical shape `indicatrix_vault` stores per design (4 axes x 3 metrics x 181
/// points, 8,688 bytes total).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TiltCurvesResult {
    pub request_id: u32,
    pub axes: [AxisTiltCurves; TILT_CURVE_AXIS_COUNT],
}

/// `<- TILT_CURVES`'s single reply -- see the module doc comment for why this is its
/// own small request/response family, never a [`super::stream::StreamEvent`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum TiltCurvesResponse {
    /// The full sweep finished normally. Boxed like `ClientMessage::RenderRequest`:
    /// 8,688 bytes of curve data, far larger than this enum's other variants.
    Curves(Box<TiltCurvesResult>),
    /// A `CANCEL` for this `request_id` was honored before every axis finished --
    /// checkpointed once per axis boundary, bounding cancellation latency to roughly
    /// one axis's own duration.
    Cancelled { request_id: u32 },
    /// The request failed `validate_scene`, or the worker's computation panicked on
    /// pathological (but validation-passing) geometry. No `request_id` of its own: the
    /// peer only ever has one `TILT_CURVES` request outstanding at a time.
    Error(ErrorMsg),
}

#[cfg(test)]
mod tests {
    use super::{
        super::codec::{read_message, write_message},
        *,
    };

    fn sample_axis(base: f32) -> AxisTiltCurves {
        AxisTiltCurves {
            brilliance_pct: vec![base; TILT_CURVE_POINTS_PER_AXIS],
            extinction_pct: vec![base + 1.0; TILT_CURVE_POINTS_PER_AXIS],
            windowing_pct: vec![base + 2.0; TILT_CURVE_POINTS_PER_AXIS],
        }
    }

    fn sample_scene() -> SceneState {
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
    fn tilt_curves_request_round_trips() {
        let request = TiltCurvesRequest {
            request_id: 42,
            scene: sample_scene(),
        };
        let mut buf = Vec::new();
        write_message(&mut buf, &request).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: TiltCurvesRequest = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, request);
    }

    #[test]
    fn tilt_curves_response_curves_round_trips() {
        let response = TiltCurvesResponse::Curves(Box::new(TiltCurvesResult {
            request_id: 7,
            axes: [
                sample_axis(0.0),
                sample_axis(10.0),
                sample_axis(20.0),
                sample_axis(30.0),
            ],
        }));
        let mut buf = Vec::new();
        write_message(&mut buf, &response).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: TiltCurvesResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn tilt_curves_response_cancelled_round_trips() {
        let response = TiltCurvesResponse::Cancelled { request_id: 7 };
        let mut buf = Vec::new();
        write_message(&mut buf, &response).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: TiltCurvesResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, response);
    }

    #[test]
    fn tilt_curves_response_error_round_trips() {
        let response = TiltCurvesResponse::Error(ErrorMsg {
            code: 2,
            message: "scene.planes must not be empty".to_string(),
        });
        let mut buf = Vec::new();
        write_message(&mut buf, &response).unwrap();
        let mut cursor = std::io::Cursor::new(buf);
        let decoded: TiltCurvesResponse = read_message(&mut cursor).unwrap();
        assert_eq!(decoded, response);
    }
}
