//! The `DisplayOnly` still export: one finished picture asked of a coordinator.
//!
//! A coordinator (not a standalone worker) answers a `FinalImageRequest` with the whole
//! range rendered, averaged and tone-mapped for the chosen color space, as one 8-bit PNG.
//! The phone cannot re-tone-map it (it holds no radiance), so the picture is exactly what
//! the endpoint produced; only the ICC profile of a wide-gamut space is attached here, as
//! the local export does, so the file means the same thing everywhere.

use super::{
    DeadlineStream,
    rendering::{
        HdrAsset, Purpose, REQUEST_ID, RunError, Session, check_sample_target, client_error,
        net_error, open_session, read_payload,
    },
};
use crate::compute_policy::ComputePolicy;
use crate::{platform::Platform, settings::RemoteSettings};
use indicatrix::{
    color::ColorSpace,
    renderer::{engine::EngineSpec, env_map::EnvironmentMap},
};
use indicatrix_net::{
    client::{self, Accumulator, ApplyOutcome},
    display::decode_rgba8,
    messages::{self, Backend, FinalImageRequest, FinalOutput, StreamEvent},
};
use indicatrix_web_core::scene::{CustomMaterialSpec, GeometrySceneSpec};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

/// The most bytes of PNG the phone reads: the picture is at most 1,048,576 pixels, so
/// even an incompressible RGBA PNG stays under this.
const MAX_PNG_BYTES: u32 = 4 * 1024 * 1024 + 64 * 1024;

/// A coordinator's optional thumbnail preview: at most 360 x 360 pixels of 12-byte
/// radiance, so 2 MiB bounds it with room to spare.
const MAX_PREVIEW_BYTES: u32 = 2 * 1024 * 1024;

/// Renders `samples` of `spec` on a coordinator and returns the finished PNG with the ICC
/// profile of `color_space`. Blocks; `progress` receives 0.0 to 1.0.
///
/// # Errors
///
/// [`RunError::Interrupted`] when the transport ended or `cancelled` fired,
/// [`RunError::Failed`] when the endpoint is not a coordinator, refuses, or answers with
/// something other than the requested picture.
#[expect(
    clippy::too_many_arguments,
    reason = "one remote call takes the endpoint, the scene, its lighting map, the sample plan and the cancel and report hooks; a bag struct would only rename them"
)]
pub fn run(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    spec: &GeometrySceneSpec,
    customs: &[CustomMaterialSpec],
    engine: EngineSpec,
    hdr: Option<&Arc<EnvironmentMap>>,
    samples: u32,
    color_space: ColorSpace,
    policy: &ComputePolicy,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32),
) -> Result<Vec<u8>, RunError> {
    check_sample_target(engine, samples)?;
    let Session {
        mut stream,
        capability,
        scene,
        asset,
        ..
    } = open_session(
        platform,
        settings,
        spec,
        customs,
        engine,
        hdr,
        Purpose::Final,
        policy,
        cancelled,
    )?;
    if !matches!(capability.backend, Backend::Coordinator { .. }) {
        return Err("Standalone workers cannot deliver a finished picture. Select FullData in Remote settings, or connect to a coordinator.".into());
    }
    let (width, height) = (scene.width, scene.height);
    let request = FinalImageRequest {
        request_id: REQUEST_ID,
        scene,
        first_sample: 0,
        samples,
        width,
        height,
        color_space: color_space.into(),
        output: FinalOutput::PngRgba8,
        viewer_samples: 0,
    };
    let mut accumulator = Accumulator::new(width, height);
    accumulator.begin_request(REQUEST_ID);
    client::send_final_image_request(&mut stream, &request)
        .map_err(|error| client_error(&error))?;
    let outcome = receive(
        &mut stream,
        &mut accumulator,
        asset.as_ref(),
        samples,
        policy,
        cancelled,
        progress,
    );
    if outcome.is_err() {
        let _cancel = client::send_cancel(&mut stream.tls, REQUEST_ID);
    }
    outcome?;
    let picture = accumulator
        .final_image()
        .ok_or("The endpoint finished without sending the picture.")?;
    if picture.width != width || picture.height != height || picture.samples_done != samples {
        return Err("The finished picture does not match the request.".into());
    }
    let rgba = decode_rgba8(
        picture.encoding,
        picture.width,
        picture.height,
        &picture.bytes,
    )
    .map_err(|error| error.to_string())?;
    let _permit = policy
        .acquire(cancelled)
        .ok_or_else(|| RunError::Interrupted("The export was cancelled.".into()))?;
    indicatrix::render_setup::encode_png_with_icc(&rgba, width, height, color_space)
        .map_err(RunError::Failed)
}

fn receive(
    stream: &mut DeadlineStream<'_>,
    accumulator: &mut Accumulator,
    asset: Option<&HdrAsset>,
    samples: u32,
    policy: &ComputePolicy,
    cancelled: &dyn Fn() -> bool,
    progress: &dyn Fn(f32),
) -> Result<(), RunError> {
    let mut uploaded = false;
    let mut received = false;
    loop {
        stream.deadline = Instant::now() + Duration::from_secs(12);
        let event: StreamEvent =
            messages::read_control_message(stream).map_err(|error| net_error(&error))?;
        if matches!(
            event,
            StreamEvent::Frame(_)
                | StreamEvent::DisplayFrame(_)
                | StreamEvent::BatchItemDone(_)
                | StreamEvent::BatchItemProgress(_)
                | StreamEvent::BatchItemFailed(_)
                | StreamEvent::BatchDone(_)
        ) {
            return Err("Endpoint sent an output mode not requested by this export.".into());
        }
        let max = if matches!(event, StreamEvent::FinalImage(_)) {
            MAX_PNG_BYTES
        } else {
            MAX_PREVIEW_BYTES
        };
        let payload = read_payload(stream, &event, max)?;
        let _permit = policy
            .acquire(cancelled)
            .ok_or_else(|| RunError::Interrupted("The export was cancelled.".into()))?;
        match accumulator
            .apply(&event, payload.as_deref())
            .map_err(|error| error.to_string())?
        {
            ApplyOutcome::Progress { samples_done } => {
                if samples_done > samples {
                    return Err("Remote progress exceeds the requested sample count.".into());
                }
                progress((samples_done as f32 / samples as f32).min(1.0));
            }
            ApplyOutcome::FinalImageReceived => received = true,
            ApplyOutcome::Done { cancelled: stopped } => {
                if stopped || cancelled() {
                    return Err(RunError::Interrupted("The export was cancelled.".into()));
                }
                if !received {
                    return Err("The endpoint finished without sending the picture.".into());
                }
                progress(1.0);
                return Ok(());
            }
            ApplyOutcome::WorkerError => {
                if let StreamEvent::Error(error) = event {
                    return Err(error.message.into());
                }
            }
            ApplyOutcome::NeedAsset { content_hash } => {
                let asset = asset.ok_or(
                    "Endpoint requested an unadvertised asset; no substituted lighting was sent.",
                )?;
                asset.answer(stream, content_hash, &mut uploaded)?;
            }
            ApplyOutcome::CapabilityChanged => {
                if matches!(event, StreamEvent::CapabilityChanged { render: None }) {
                    return Err("Remote render capacity was withdrawn.".into());
                }
            }
            ApplyOutcome::FrameSummed { .. } | ApplyOutcome::DisplayFrameReplaced => {
                return Err("Endpoint sent an output mode not requested by this export.".into());
            }
            ApplyOutcome::PreviewReplaced
            | ApplyOutcome::Pong { .. }
            | ApplyOutcome::StaleDropped => {}
        }
    }
}
