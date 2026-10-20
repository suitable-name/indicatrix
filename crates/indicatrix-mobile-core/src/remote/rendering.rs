//! One authenticated interactive session, with exact resolved geometry and bounded decoding.
use super::DeadlineStream;
use crate::{
    compute_policy::ComputePolicy,
    platform::Platform,
    settings::{RemoteSettings, RemoteTransfer},
};
use indicatrix::renderer::{
    engine::EngineSpec, env_map::EnvironmentMap, tonemap::tonemap_to_rgba_with_threads,
};
use indicatrix_net::{
    client::{self, Accumulator, ApplyOutcome, ClientError},
    framing::FramingError,
    messages::{
        self, Backend, MAX_ASSET_LEN, NetError, RenderCapability, RenderRequest, RequestIntent,
        StreamConfig, StreamEvent, TransferMode,
        envmap_asset::{encode_envmap_asset, envmap_asset_hash, envmap_asset_len},
    },
    scene::{HdrEnvironment, SceneEnvironment, SceneState},
};
use indicatrix_web_core::{
    display::{DenoiseFrame, Denoiser},
    scene::{CustomMaterialSpec, GeometrySceneSpec, OwnedScene},
};
use std::{
    fmt,
    sync::Arc,
    time::{Duration, Instant},
};

#[derive(Clone)]
pub(crate) struct RemoteConfig {
    pub platform: Arc<dyn Platform>,
    pub settings: RemoteSettings,
}

pub const MAX_REMOTE_PIXELS: u64 = 1024 * 1024;
/// One request per owned TLS connection; the epoch is owned by the caller.
pub(super) const REQUEST_ID: u32 = 1;

pub struct FrameUpdate {
    pub rgba: Option<Vec<u8>>,
    pub samples: u32,
    pub done: bool,
    pub denoised: bool,
}
pub struct RenderOutput {
    pub sum: Vec<glam::Vec3>,
    pub samples: u32,
}

/// Why a remote session ended without a result.
///
/// The split exists for the caller's retry decision: an [`Interrupted`](Self::Interrupted)
/// session lost its transport (a dropped socket, a stalled link, the app going to the
/// background) and may be started again from sample zero over a new connection; a
/// [`Failed`](Self::Failed) one was refused or is wrong in itself, and retrying repeats it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunError {
    /// The transport ended or the session was cancelled from this side.
    Interrupted(String),
    /// The endpoint refused, the data was invalid, or a precondition was not met.
    Failed(String),
}

impl RunError {
    /// The sentence for the user.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Interrupted(message) | Self::Failed(message) => message,
        }
    }
}

impl fmt::Display for RunError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl From<String> for RunError {
    fn from(message: String) -> Self {
        Self::Failed(message)
    }
}

impl From<&str> for RunError {
    fn from(message: &str) -> Self {
        Self::Failed(message.to_owned())
    }
}

impl From<RunError> for String {
    fn from(error: RunError) -> Self {
        match error {
            RunError::Interrupted(message) | RunError::Failed(message) => message,
        }
    }
}

/// A transport failure (socket error, truncated frame) is an interruption; anything else
/// the codec reports (an oversized frame, undecodable bytes) is a protocol failure.
pub(super) fn net_error(error: &NetError) -> RunError {
    let lost = matches!(
        error,
        NetError::Framing(FramingError::Io(_) | FramingError::TruncatedFrame)
    );
    if lost {
        RunError::Interrupted(error.to_string())
    } else {
        RunError::Failed(error.to_string())
    }
}

pub(super) fn framing_error(error: &FramingError) -> RunError {
    let message = error.to_string();
    if matches!(error, FramingError::Io(_) | FramingError::TruncatedFrame) {
        RunError::Interrupted(message)
    } else {
        RunError::Failed(message)
    }
}

pub(super) fn client_error(error: &ClientError) -> RunError {
    match error {
        ClientError::Net(net) => net_error(net),
        other => RunError::Failed(other.to_string()),
    }
}

/// The phone's effective lighting map, with the scene reference that names it.
///
/// The map is the exact `EnvironmentMap` the device renders with (rotation, exposure
/// and size reduction already applied), carried as a versioned float asset
/// (`IXENVMAP` v1, see `indicatrix_net::messages::envmap_asset`) so a server traces the
/// very same texels, not a re-encoded `.hdr`.
pub(super) struct HdrAsset {
    map: Arc<EnvironmentMap>,
    environment: HdrEnvironment,
}

impl HdrAsset {
    /// The asset for `spec`, `None` when the scene uses studio lighting.
    ///
    /// # Errors
    ///
    /// A sentence when the scene names a map that is not loaded, or the map cannot be
    /// carried (outside the format's bounds or over the transfer limit).
    pub(super) fn prepare(
        spec: &GeometrySceneSpec,
        hdr: Option<&Arc<EnvironmentMap>>,
    ) -> Result<Option<Self>, String> {
        if spec.scene.hdr_id.is_none() {
            return Ok(None);
        }
        let map = hdr.ok_or("The scene names a lighting map that is not loaded.")?;
        let too_big = || {
            "The lighting map is too large to send to a remote endpoint. Choose a smaller map or render on this device."
                .to_owned()
        };
        let width = u32::try_from(map.width()).map_err(|_| too_big())?;
        let height = u32::try_from(map.height()).map_err(|_| too_big())?;
        let len = envmap_asset_len(width, height).ok_or_else(too_big)?;
        if len > u64::from(MAX_ASSET_LEN) {
            return Err(too_big());
        }
        let content_hash = envmap_asset_hash(width, height, map.texels())
            .map_err(|error| format!("The lighting map cannot be sent: {error}."))?;
        Ok(Some(Self {
            map: Arc::clone(map),
            environment: HdrEnvironment {
                content_hash,
                width,
                height,
            },
        }))
    }

    /// The scene's environment reference.
    pub(super) const fn environment(&self) -> HdrEnvironment {
        self.environment
    }

    /// The asset's bytes, when `requested` names this map and it was not sent on this
    /// connection yet; any other request is a protocol violation and nothing substitute
    /// is sent. Marks the asset as sent.
    pub(super) fn bytes_for(
        &self,
        requested: [u8; 32],
        sent: &mut bool,
    ) -> Result<Vec<u8>, RunError> {
        if requested != self.environment.content_hash || *sent {
            return Err(RunError::Failed(
                "Endpoint requested an asset this scene does not use; no substitute lighting was sent."
                    .into(),
            ));
        }
        let bytes = encode_envmap_asset(
            self.environment.width,
            self.environment.height,
            self.map.texels(),
        )
        .map_err(|error| RunError::Failed(error.to_string()))?;
        *sent = true;
        Ok(bytes)
    }

    /// Answers a `NEED_ASSET`: uploads the map as an `ASSET` frame (see [`Self::bytes_for`]).
    pub(super) fn answer(
        &self,
        stream: &mut DeadlineStream<'_>,
        requested: [u8; 32],
        sent: &mut bool,
    ) -> Result<(), RunError> {
        let bytes = self.bytes_for(requested, sent)?;
        client::send_asset(stream, &bytes).map_err(|error| client_error(&error))
    }
}

/// Refuses a session that would move many megabytes over a metered network the user has
/// not allowed. `heavy` is true for a lighting-map upload or `FullData` frames.
///
/// A network the platform cannot classify counts as unmetered (and so does the desktop
/// preview), see `Platform::network_metered`.
///
/// # Errors
///
/// The sentence to show.
pub(super) fn metered_refusal(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    heavy: bool,
) -> Result<(), String> {
    if heavy && !settings.allow_metered && platform.network_metered() == Some(true) {
        return Err("This network is metered and this remote session would transfer many megabytes (a lighting map or FullData frames). Connect to Wi-Fi, switch Remote to DisplayOnly without a lighting map, or turn on Allow Metered Networks in Remote settings.".into());
    }
    Ok(())
}

/// What a connection is opened for; it sets the pixel budget and what counts as a heavy
/// transfer for the metered-network policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Purpose {
    /// A live preview: radiance frames (`full_data`) or tone-mapped pictures.
    Interactive { full_data: bool },
    /// One finished picture from a coordinator.
    Final,
    /// A durable server job: only the scene (and a lighting map) goes up; the server
    /// bounds the picture size itself.
    Job,
}

impl Purpose {
    const fn heavy(self) -> bool {
        matches!(self, Self::Interactive { full_data: true })
    }

    const fn pixel_budget(self) -> u64 {
        match self {
            Self::Job => JOB_MAX_PIXELS,
            Self::Interactive { .. } | Self::Final => MAX_REMOTE_PIXELS,
        }
    }
}

/// The largest picture the phone asks of a server job (the server enforces its own bound).
pub(super) const JOB_MAX_PIXELS: u64 = 4096 * 4096;

/// A connected, validated session: the stream, what the endpoint advertised, the scene
/// built locally and its wire form.
pub(super) struct Session<'a> {
    pub stream: DeadlineStream<'a>,
    pub capability: RenderCapability,
    pub owned: OwnedScene,
    pub scene: SceneState,
    pub asset: Option<HdrAsset>,
}

/// Builds the scene, applies the metered-network policy, connects and checks that the
/// endpoint can take this scene (pixel limit, zoning, HDR). Nothing is uploaded.
#[expect(
    clippy::too_many_arguments,
    reason = "one remote call takes the endpoint, the scene with its material recipe and lighting map, the purpose, the compute budget and the cancel hook; a bag struct would only rename them"
)]
pub(super) fn open_session<'a>(
    platform: &dyn Platform,
    settings: &RemoteSettings,
    spec: &GeometrySceneSpec,
    customs: &[CustomMaterialSpec],
    engine: EngineSpec,
    hdr: Option<&Arc<EnvironmentMap>>,
    purpose: Purpose,
    policy: &ComputePolicy,
    cancelled: &'a dyn Fn() -> bool,
) -> Result<Session<'a>, RunError> {
    let pixels = spec.scene.pixel_count();
    if pixels == 0 || pixels > purpose.pixel_budget() {
        return Err(if purpose == Purpose::Job {
            "The picture is larger than a server job allows (16,777,216 pixels). Choose a smaller size.".into()
        } else {
            RunError::Failed("Remote output exceeds the phone's 1,048,576-pixel decode budget. Choose a smaller image.".into())
        });
    }
    let asset = HdrAsset::prepare(spec, hdr)?;
    metered_refusal(platform, settings, purpose.heavy() || asset.is_some())?;
    let owned = {
        let _permit = policy
            .acquire(cancelled)
            .ok_or_else(|| RunError::Interrupted("Remote preparation cancelled.".into()))?;
        let mut owned = spec.build_with_customs(hdr.cloned(), customs)?;
        owned
            .set_engine(engine)
            .map_err(|reason| RunError::Failed(reason.message().into()))?;
        owned
    };
    let scene = scene_state(spec, &owned, asset.as_ref().map(HdrAsset::environment));
    let (stream, welcome) = super::connect(platform, &settings.address, cancelled)?;
    let capability = welcome
        .render
        .ok_or("The authenticated endpoint has no render capacity.")?;
    if purpose != Purpose::Job && pixels > u64::from(capability.max_pixels) {
        return Err("The endpoint's advertised pixel limit is smaller than this image.".into());
    }
    if !scene.engine.is_ray() && scene.material.zoning.is_some() && !welcome.zoning {
        return Err(
            "The endpoint does not advertise exact material zoning support; no scene was uploaded."
                .into(),
        );
    }
    if asset.is_some() && !capability.hdr {
        return Err("The endpoint does not advertise HDR lighting-map support; no scene was uploaded. Choose a studio lighting preset or render on this device.".into());
    }
    Ok(Session {
        stream,
        capability,
        owned,
        scene,
        asset,
    })
}

/// The caller owns connection/scene identity. No connection is opened until explicit
/// Remote selection. Disconnect/cancel discards the session; retry starts at sample zero.
///
/// `hdr` is the device's effective lighting map, required exactly when the scene uses
/// one: it is sent to the endpoint as a content-addressed asset when the endpoint asks
/// for it.
///
/// # Errors
///
/// [`RunError::Interrupted`] when the transport ended or `cancelled` fired,
/// [`RunError::Failed`] for a refusal or invalid data.
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
    denoise: bool,
    intent: RequestIntent,
    policy: &ComputePolicy,
    cancelled: &dyn Fn() -> bool,
    ready: &dyn Fn(String),
    frame: &dyn Fn(FrameUpdate),
) -> Result<RenderOutput, RunError> {
    check_sample_target(engine, samples)?;
    let display_only = settings.transfer == RemoteTransfer::DisplayOnly;
    let Session {
        mut stream,
        capability,
        owned,
        scene,
        asset,
    } = open_session(
        platform,
        settings,
        spec,
        customs,
        engine,
        hdr,
        Purpose::Interactive {
            full_data: !display_only,
        },
        policy,
        cancelled,
    )?;
    if display_only && !matches!(capability.backend, Backend::Coordinator { .. }) {
        return Err("Standalone workers do not support DisplayOnly. Select FullData in Remote settings, or connect to a coordinator.".into());
    }
    let denoise = denoise && owned.engine().denoises();
    if display_only && !denoise && !owned.engine().is_ray() {
        return Err("DisplayOnly endpoints deliver denoised pictures. Enable Denoise or explicitly choose FullData.".into());
    }
    let request = RenderRequest {
        request_id: REQUEST_ID,
        scene,
        first_sample: 0,
        samples,
        stream: StreamConfig {
            transfer_mode: if display_only {
                TransferMode::DisplayOnly
            } else {
                TransferMode::LiveProgressive
            },
            cadence_ms: settings
                .cadence_ms
                .max(capability.min_cadence_ms)
                .clamp(100, 10000),
            preview: None,
        },
        intent,
    };
    ready(format!(
        "Remote · {} · {:?} · {}",
        owned.engine().engine.label(),
        capability.backend,
        if display_only {
            "DisplayOnly"
        } else {
            "FullData"
        }
    ));
    let mut accumulator = Accumulator::new(spec.scene.width, spec.scene.height);
    accumulator.begin_request_for_range(REQUEST_ID, 0, samples);
    client::send_render_request(&mut stream, &request).map_err(|error| client_error(&error))?;
    let outcome = receive(
        &mut stream,
        &mut accumulator,
        &owned,
        asset.as_ref(),
        samples,
        display_only,
        denoise && !display_only,
        policy,
        cancelled,
        frame,
    );
    if outcome.is_err() {
        // The same worker owns TLS writes and reads, including the best-effort Cancel.
        // Write has a bounded socket timeout; closing also releases server session work.
        let _cancel = client::send_cancel(&mut stream.tls, REQUEST_ID);
    }
    outcome?;
    Ok(RenderOutput {
        sum: if display_only {
            Vec::new()
        } else {
            accumulator.buffer().to_vec()
        },
        samples,
    })
}

/// The longest raw payload the phone reads for one event: a full radiance frame at the
/// pixel budget (12 bytes per pixel) plus header slack.
pub(super) fn max_payload() -> u32 {
    u32::try_from(MAX_REMOTE_PIXELS * 12 + 65536).unwrap_or(u32::MAX)
}

#[expect(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one remote call takes the endpoint, the scene, its lighting map, the sample plan and the cancel and report hooks, and its loop is one state machine over the endpoint's events: every arm shares the last picture, the sample count and the upload flag, so splitting it would only pass them around"
)]
fn receive(
    stream: &mut DeadlineStream<'_>,
    accumulator: &mut Accumulator,
    scene: &OwnedScene,
    asset: Option<&HdrAsset>,
    target: u32,
    display_only: bool,
    denoise: bool,
    policy: &ComputePolicy,
    cancelled: &dyn Fn() -> bool,
    frame: &dyn Fn(FrameUpdate),
) -> Result<(), RunError> {
    let mut last_picture = None;
    let mut last_samples = 0;
    let mut uploaded = false;
    loop {
        stream.deadline = Instant::now() + Duration::from_secs(12);
        let event: StreamEvent =
            messages::read_control_message(stream).map_err(|error| net_error(&error))?;
        if matches!(&event, StreamEvent::Frame(_)) && display_only
            || matches!(&event, StreamEvent::DisplayFrame(_)) && !display_only
        {
            return Err(
                "Endpoint sent an image transfer mode different from the requested mode.".into(),
            );
        }
        if matches!(
            event,
            StreamEvent::Preview(_)
                | StreamEvent::FinalImage(_)
                | StreamEvent::BatchItemDone(_)
                | StreamEvent::BatchItemProgress(_)
                | StreamEvent::BatchItemFailed(_)
                | StreamEvent::BatchDone(_)
        ) {
            return Err("Endpoint sent an output mode not requested by this session.".into());
        }
        // Bound the raw payload before reading/allocating it. The shared accumulator
        // separately validates sample ranges, decoded dimensions and raw radiance size.
        let payload = read_payload(stream, &event, max_payload())?;
        let _permit = policy
            .acquire(cancelled)
            .ok_or_else(|| RunError::Interrupted("Remote session cancelled.".into()))?;
        let applied = accumulator
            .apply(&event, payload.as_deref())
            .map_err(|error| error.to_string())?;
        match applied {
            ApplyOutcome::FrameSummed { samples_done } => {
                let rgba = tonemap_to_rgba_with_threads(
                    accumulator.buffer(),
                    1.0 / samples_done.max(1) as f32,
                    1,
                );
                last_samples = samples_done;
                last_picture = Some(rgba.clone());
                frame(FrameUpdate {
                    rgba: Some(rgba),
                    samples: samples_done,
                    done: false,
                    denoised: false,
                });
            }
            ApplyOutcome::DisplayFrameReplaced => {
                let picture = accumulator
                    .last_display_frame()
                    .ok_or("Remote display picture is missing.")?;
                if picture.width != scene.width()
                    || picture.height != scene.height()
                    || picture.samples_done > target
                    || picture.samples_done < last_samples
                {
                    return Err("Remote display dimensions or sample count are invalid.".into());
                }
                let rgba = indicatrix_net::display::decode_rgba8(
                    picture.encoding,
                    picture.width,
                    picture.height,
                    &picture.bytes,
                )
                .map_err(|error| error.to_string())?;
                last_samples = picture.samples_done;
                last_picture = Some(rgba.clone());
                frame(FrameUpdate {
                    rgba: Some(rgba),
                    samples: last_samples,
                    done: false,
                    denoised: scene.engine().denoises(),
                });
            }
            ApplyOutcome::Progress { samples_done } => {
                if samples_done > target {
                    return Err("Remote progress exceeds the requested sample count.".into());
                }
                frame(FrameUpdate {
                    rgba: None,
                    samples: samples_done,
                    done: false,
                    denoised: false,
                });
            }
            ApplyOutcome::Done { cancelled: stopped } => {
                if stopped || cancelled() {
                    return Err(RunError::Interrupted("Remote rendering cancelled.".into()));
                }
                if last_samples != target || (!display_only && accumulator.samples_done() != target)
                {
                    return Err(
                        "Remote session ended without the complete requested sample count.".into(),
                    );
                }
                if denoise {
                    let rgba = Denoiser::single_thread().denoised_rgba(&DenoiseFrame {
                        guide_key: 1,
                        width: scene.width(),
                        height: scene.height(),
                        camera: scene.camera(),
                        planes: scene.planes(),
                        tools: scene.tools(),
                        n_d: scene.material().dispersion.n_d(),
                        sample_count: accumulator.samples_done(),
                        sum: accumulator.buffer(),
                    });
                    last_picture = Some(rgba);
                }
                frame(FrameUpdate {
                    rgba: last_picture,
                    samples: target,
                    done: true,
                    denoised: scene.engine().denoises()
                        && (denoise || accumulator.last_display_frame().is_some()),
                });
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
            ApplyOutcome::PreviewReplaced
            | ApplyOutcome::FinalImageReceived
            | ApplyOutcome::Pong { .. }
            | ApplyOutcome::StaleDropped => {}
        }
    }
}

/// Reads the raw payload frame `event` announces, bounded by `max` before anything is
/// allocated; `None` for an event without one.
pub(super) fn read_payload(
    stream: &mut DeadlineStream<'_>,
    event: &StreamEvent,
    max: u32,
) -> Result<Option<Vec<u8>>, RunError> {
    let Some(length) = event.payload_len() else {
        return Ok(None);
    };
    if length > max {
        return Err("Remote payload exceeds the phone memory budget.".into());
    }
    let bytes = indicatrix_net::framing::read_frame_bounded(stream, max)
        .map_err(|error| framing_error(&error))?;
    if bytes.len() != length as usize {
        return Err("Remote payload length disagrees with its header.".into());
    }
    Ok(Some(bytes))
}

pub(super) fn scene_state(
    spec: &GeometrySceneSpec,
    scene: &OwnedScene,
    hdr: Option<HdrEnvironment>,
) -> SceneState {
    let lighting = &spec.scene.lighting;
    // The girdle toggle and the design's frosted planes, as the wire carries them.
    let (girdle_frosted, frosted_planes) =
        indicatrix::geometry::finishes_wire_form(scene.planes(), scene.finishes());
    SceneState {
        width: scene.width(),
        height: scene.height(),
        yaw: spec.scene.camera.yaw,
        pitch: spec.scene.camera.pitch,
        distance: spec.scene.camera.distance,
        light_yaw: lighting.light_yaw,
        light_pitch: lighting.light_pitch,
        exposure: lighting.exposure,
        max_bounces: scene.max_bounces(),
        lighting_preset: lighting.preset(),
        material: scene.material().clone(),
        planes: scene.planes().to_vec(),
        girdle_frosted,
        // The tray radiance under this scene's own preset and light pose (exposure 1).
        backdrop: lighting.backdrop().resolve(
            lighting.preset(),
            lighting.light_yaw,
            lighting.light_pitch,
        ),
        environment: hdr.map_or(SceneEnvironment::Studio, SceneEnvironment::Hdr),
        surface_glare: lighting.surface_glare,
        tools: scene.tools().to_vec(),
        fluorescence: scene.fluorescence().clone(),
        head_shadow_deg: lighting.head_shadow_deg,
        frosted_planes,
        engine: scene.engine(),
    }
}

/// A deterministic request must stop at its AA count; repeated taps are not a
/// higher-quality remote result. This also applies to final and durable jobs.
pub(super) fn check_sample_target(engine: EngineSpec, samples: u32) -> Result<(), RunError> {
    if samples == 0 {
        return Err("Remote samples must be positive.".into());
    }
    if engine.is_ray() && samples > engine.target_samples(0) {
        return Err("Ray-Traced sample count exceeds the selected AA tap count.".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
