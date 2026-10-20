//! The Render mode's decisions, kept apart from Slint so they can be tested natively.
//!
//! - [`trace_size`]: the raster the trace is drawn at (long edge 512 px on a phone, 768 px
//!   on a larger window, 384 px in battery saver; the Solid view's minimum aspect);
//! - [`RenderDriver`] and [`maintain`]: when the trace starts, restarts and stops, and why
//!   it could not start. A drag of the camera or the light cancels it once and restarts it
//!   once after the finger lifts (see [`render_drag`](super::render_drag)); [`live_solid`]
//!   says when the picture area shows the Solid raster at the live pose meanwhile;
//! - [`status_line`] and [`progress_fraction`]: what the overlay says about the trace;
//! - [`LightDrag`] and [`drag_light`]: one finger moving the light instead of the camera;
//! - the export options: [`size_options`], [`spp_options`], [`export_request`],
//!   [`ExportEstimate`] and [`export_file_name`].

use std::time::Duration;

use indicatrix_solid::preview::view::with_min_aspect;

use super::frame::{MIN_EDGE, SOLID_MIN_ASPECT};
use super::render_drag::Gate;
use crate::{
    app_state::AppState,
    design_props::{LightValues, lighting_view, set_light_values},
    layout::Destination,
    render::{self, ExportColorSpace, ExportRequest, PauseReason, RenderStatus, RenderView},
    settings::{LightingSettings, RenderQuality, RenderSettings},
};

/// The trace raster's long edge on a compact window, in px.
pub const TRACE_EDGE_COMPACT: u32 = 512;
/// The trace raster's long edge on a larger window, in px.
pub const TRACE_EDGE_LARGE: u32 = 768;
/// The trace raster's long edge in battery saver, in px.
pub const TRACE_EDGE_BATTERY_SAVER: u32 = 384;
/// The shortest interval between two trace restarts while a finger is down.
pub const DRAG_RESTART_INTERVAL: Duration = Duration::from_millis(150);
/// Degrees of light azimuth per logical px of horizontal drag.
pub const AZIMUTH_DEG_PER_PX: f32 = 0.4;
/// Degrees of light elevation per logical px of vertical drag (dragging up raises it).
pub const ELEVATION_DEG_PER_PX: f32 = 0.3;
/// The export sizes the sheet offers (long edge in px), before the settings' cap.
pub const SIZE_PRESETS: [u32; 2] = [512, 1024];
/// The export sample counts the sheet offers, before the settings' cap.
pub const SPP_PRESETS: [u32; 3] = [64, 128, 256];

/// The trace raster for a viewport of `view` logical px.
///
/// The view's aspect with the long
/// edge capped (see the constants), never narrower than the Solid raster's minimum aspect
/// (the camera's field of view is vertical, so a narrower raster would crop the stone's
/// sides; the picture is letterboxed instead).
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the edges are small, rounded, positive and at most the long edge"
)]
pub fn trace_size(view: (f32, f32), compact: bool, battery_saver: bool) -> (u32, u32) {
    let long_edge = if battery_saver {
        TRACE_EDGE_BATTERY_SAVER
    } else if compact {
        TRACE_EDGE_COMPACT
    } else {
        TRACE_EDGE_LARGE
    };
    let long = long_edge as f32;
    let (width, height) = (view.0.max(1.0), view.1.max(1.0));
    let (raster_w, raster_h) = if width >= height {
        (long, long * height / width)
    } else {
        (long * width / height, long)
    };
    let clamp = |edge: f32| (edge.round() as u32).clamp(MIN_EDGE, long_edge);
    with_min_aspect(
        (clamp(raster_w), clamp(raster_h)),
        SOLID_MIN_ASPECT,
        MIN_EDGE,
    )
}

/// Which touches are down on the picture; the restart gate coalesces while any is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct TouchState {
    pointer: bool,
    pinch: bool,
}

impl TouchState {
    /// A single pointer (finger or mouse button) went down or up.
    pub const fn set_pointer(&mut self, down: bool) {
        self.pointer = down;
    }

    /// A two-finger pinch began or ended.
    pub const fn set_pinch(&mut self, down: bool) {
        self.pinch = down;
    }

    /// Whether any touch is down.
    #[must_use]
    pub const fn is_down(self) -> bool {
        self.pointer || self.pinch
    }
}

/// What the restart gate decided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Start the trace now.
    Start,
    /// Too soon after the last start: ask again after this long.
    Wait(Duration),
}

/// Limits restarts while a finger is down to one per [`DRAG_RESTART_INTERVAL`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RestartGate {
    last_start: Option<Duration>,
}

impl RestartGate {
    /// Whether a restart may happen at `now` (the time since the app started).
    #[must_use]
    pub fn decide(&self, now: Duration, touching: bool) -> GateDecision {
        if !touching {
            return GateDecision::Start;
        }
        self.last_start.map_or(GateDecision::Start, |last| {
            let due = last + DRAG_RESTART_INTERVAL;
            due.checked_sub(now)
                .filter(|wait| !wait.is_zero())
                .map_or(GateDecision::Start, GateDecision::Wait)
        })
    }

    /// Records a start at `now`.
    pub const fn started(&mut self, now: Duration) {
        self.last_start = Some(now);
    }
}

/// What the trace was started with, to notice a change the engine cannot see (the view's
/// size, the sample target).
#[derive(Debug, Clone, PartialEq, Eq)]
struct RunKey {
    size: (u32, u32),
    target_spp: u32,
}

/// What a start attempt failed on; retried only when one of these changed.
#[derive(Debug, Clone, PartialEq)]
struct BlockKey {
    generation: u64,
    solved: bool,
    material: String,
    lighting: LightingSettings,
}

/// What [`maintain`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Maintain {
    /// Nothing to do: the trace is current (or the Render mode is not on show).
    Idle,
    /// A new trace epoch was started.
    Started,
    /// A restart is due but too soon after the last one (or the pose has not rested since
    /// the finger went up): call again after this long.
    Wait(Duration),
    /// A finger is moving the camera or the light: the trace is cancelled and nothing
    /// starts until it lifts. Nothing to schedule; the release asks again.
    Held,
    /// The trace could not start; the sentence says why. Not retried until the design,
    /// its solve, the material or the lighting changes.
    Blocked(String),
    /// The Render mode went off show and the trace was stopped.
    Stopped,
}

/// Starts, restarts and stops the trace for the Render mode. Kept in
/// [`ViewportState::driver`](super::ViewportState::driver); see [`maintain`].
#[derive(Debug, Default)]
pub struct RenderDriver {
    gate: RestartGate,
    active: bool,
    running: Option<RunKey>,
    blocked: Option<(BlockKey, String)>,
}

impl RenderDriver {
    /// Whether this driver started a trace that it has not stopped.
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.active
    }

    /// Why the last start attempt failed, while that still holds.
    #[must_use]
    pub fn blocked_message(&self) -> Option<&str> {
        self.blocked.as_ref().map(|(_, message)| message.as_str())
    }

    /// A drag moved the camera or the light: cancels the running trace once (flags only, no
    /// join) so the workers leave the cores to the UI, and drops the run key so that the
    /// next start is a start.
    fn hold(&mut self, state: &mut AppState) {
        if !state.viewport.render_live.is_stopped() {
            state.render.stop();
            self.running = None;
            state.viewport.render_live.mark_stopped();
        }
    }

    fn step(&mut self, state: &mut AppState, now: Duration, touching: bool) -> Maintain {
        if !wants(state) {
            state.viewport.render_live.reset();
            if self.active {
                state.render.stop();
                self.active = false;
                self.running = None;
                return Maintain::Stopped;
            }
            return Maintain::Idle;
        }
        // A drag in progress (camera or light) never builds a scene or starts an epoch.
        state.viewport.render_live.stamp(now);
        match state.viewport.render_live.gate(now, touching) {
            Gate::Hold => {
                self.hold(state);
                return Maintain::Held;
            }
            Gate::Wait(delay) => {
                self.hold(state);
                return Maintain::Wait(delay);
            }
            Gate::Idle | Gate::Go => {}
        }
        let owes = state.viewport.render_live.owes_restart();
        let compact = state.window_class == crate::layout::WindowClass::Compact;
        let size = trace_size(
            state.viewport.view_size,
            compact,
            state.settings.battery_saver,
        );
        let run_key = RunKey {
            size,
            target_spp: render::live_target_spp(state),
        };
        // A moved camera or light is known to need a restart: skip the scene comparison (it
        // describes the whole scene on the UI thread).
        if !owes && self.running.as_ref() == Some(&run_key) && !render::needs_restart(state) {
            return Maintain::Idle;
        }
        let key = block_key(state);
        if let Some((blocked, message)) = &self.blocked
            && *blocked == key
        {
            state.viewport.render_live.abandoned();
            return Maintain::Blocked(message.clone());
        }
        match self.gate.decide(now, touching) {
            GateDecision::Wait(delay) => Maintain::Wait(delay),
            GateDecision::Start => {
                let view = RenderView {
                    width: size.0,
                    height: size.1,
                };
                match render::start(state, view) {
                    Ok(epoch) => {
                        self.gate.started(now);
                        self.active = true;
                        self.running = Some(run_key);
                        self.blocked = None;
                        let camera = state.viewport.camera;
                        state.viewport.render_live.started(epoch, camera);
                        Maintain::Started
                    }
                    Err(message) => {
                        self.blocked = Some((key, message.clone()));
                        state.viewport.render_live.abandoned();
                        Maintain::Blocked(message)
                    }
                }
            }
        }
    }
}

/// What a failed start is keyed on.
fn block_key(state: &AppState) -> BlockKey {
    let generation = state.session.current_generation();
    BlockKey {
        generation,
        solved: state.solve.current_tiers(generation).is_some(),
        material: state.settings.material_name.clone(),
        lighting: state.settings.lighting.clone(),
    }
}

/// Whether the Render mode is on show and wants a trace: the mode is chosen, the Design
/// destination is up, there is a design and the viewport has a size.
#[must_use]
pub fn wants(state: &AppState) -> bool {
    state.viewport.mode.is_render()
        && state.destination == Destination::Design
        && state.document.has_design(&state.session.design)
        && state.viewport.view_size.0 >= 1.0
        && state.viewport.view_size.1 >= 1.0
}

/// Whether the Render mode shows the Solid raster at the live pose instead of the traced
/// picture.
///
/// That is the case when the camera moved and the traced picture of the new pose has not
/// landed yet. Needs a solve (without one the raster has nothing to draw and the status text shows).
#[must_use]
pub fn live_solid(state: &AppState) -> bool {
    let generation = state.session.current_generation();
    state.viewport.mode.is_render()
        && state.viewport.render_live.solid()
        && state.solve.current_tiers(generation).is_some()
}

/// A traced frame of `epoch` was put on screen. Returns whether the Solid raster gave way to
/// it, so the viewport must be pushed again.
pub fn frame_landed(state: &mut AppState, epoch: u64) -> bool {
    let camera = state.viewport.camera;
    state.viewport.render_live.frame_landed(epoch, camera)
}

/// Brings the trace in line with the state.
///
/// It starts it when the Render mode is on show and the scene (or the view) changed, and
/// stops it when the mode goes off show. While the camera or the light is being dragged it
/// cancels the trace once and starts nothing until the finger is up and the pose has rested
/// for [`SETTLE`](super::render_drag::SETTLE) (see `render_drag`); other restarts with a
/// finger down are limited to one per [`DRAG_RESTART_INTERVAL`].
///
/// `now` is the time since the app started. Cheap when nothing changed.
pub fn maintain(state: &mut AppState, now: Duration) -> Maintain {
    // The driver lives inside the viewport state; take it out so it can borrow the rest.
    let mut driver = std::mem::take(&mut state.viewport.driver);
    let touching = state.viewport.touch.is_down();
    let outcome = driver.step(state, now, touching);
    state.viewport.driver = driver;
    outcome
}

/// The overlay's status line.
#[must_use]
pub fn status_line(state: &AppState) -> String {
    let generation = state.session.current_generation();
    if state.solve.current_tiers(generation).is_none() {
        return if state.solve.is_solving() {
            "Solving the design...".to_owned()
        } else {
            "Solve the design to render it".to_owned()
        };
    }
    if state.viewport.light_drag.on {
        return "Drag to move the light".to_owned();
    }
    if state.viewport.render_live.owes_restart() && state.viewport.touch.is_down() {
        return "Release to render this view".to_owned();
    }
    match state.render.status() {
        RenderStatus::Idle => state
            .viewport
            .driver
            .blocked_message()
            .map_or_else(|| "Starting the render...".to_owned(), str::to_owned),
        RenderStatus::Tracing { samples, target }
            if state.settings.render.engine_spec().is_ray() =>
        {
            format!("Ray-Traced · Tap {samples} Of {target}")
        }
        RenderStatus::Done { .. } if state.settings.render.engine_spec().is_ray() => {
            "Ray-Traced · Done".to_owned()
        }
        RenderStatus::Tracing { samples, target } => format!("Rendering {samples} / {target}"),
        RenderStatus::Done { samples } => format!("Done, {samples} samples"),
        RenderStatus::Paused(PauseReason::User) => "Paused".to_owned(),
        RenderStatus::Paused(PauseReason::Background) => "Paused: App In Background".to_owned(),
        RenderStatus::Paused(PauseReason::Hot) => "Paused: Device Is Hot".to_owned(),
        RenderStatus::Failed(message) => message,
    }
}

/// How far the trace is, `0.0..=1.0`, when the status says; `None` while paused (keep the
/// last value) or idle.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "the fraction is within 0 to 1; f32 is plenty"
)]
pub fn progress_fraction(status: &RenderStatus) -> Option<f32> {
    match status {
        RenderStatus::Tracing { samples, target } => Some(if *target == 0 {
            0.0
        } else {
            (f64::from(*samples) / f64::from(*target)).clamp(0.0, 1.0) as f32
        }),
        RenderStatus::Done { .. } => Some(1.0),
        RenderStatus::Idle | RenderStatus::Failed(_) => Some(0.0),
        RenderStatus::Paused(_) => None,
    }
}

/// The Slint index of `quality` (`0` Draft, `1` Normal, `2` Fine).
#[must_use]
pub const fn quality_index(quality: RenderQuality) -> i32 {
    match quality {
        RenderQuality::Draft => 0,
        RenderQuality::Normal => 1,
        RenderQuality::Fine => 2,
    }
}

/// The quality after `quality` when the chip is tapped: Draft, Normal, Fine, Draft...
#[must_use]
pub const fn next_quality(quality: RenderQuality) -> RenderQuality {
    match quality {
        RenderQuality::Draft => RenderQuality::Normal,
        RenderQuality::Normal => RenderQuality::Fine,
        RenderQuality::Fine => RenderQuality::Draft,
    }
}

/// The chip's label for `quality`.
#[must_use]
pub const fn quality_label(quality: RenderQuality) -> &'static str {
    match quality {
        RenderQuality::Draft => "Draft",
        RenderQuality::Normal => "Normal",
        RenderQuality::Fine => "Fine",
    }
}

/// Whether the chosen lighting rig has a light whose direction a drag can move.
#[must_use]
pub fn light_drag_available(state: &AppState) -> bool {
    lighting_view(state).uses_light_direction
}

/// One finger moving the light: the Light chip turns it on, and the pointer events then go
/// here instead of to the camera.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LightDrag {
    /// Whether a drag moves the light.
    pub on: bool,
    last: Option<(f32, f32)>,
}

impl LightDrag {
    /// A finger went down at `(x, y)`.
    pub const fn begin(&mut self, x: f32, y: f32) {
        self.last = Some((x, y));
    }

    /// The finger moved to `(x, y)`: the movement since the last event, if a drag began.
    pub const fn moved(&mut self, x: f32, y: f32) -> Option<(f32, f32)> {
        let Some((last_x, last_y)) = self.last else {
            return None;
        };
        self.last = Some((x, y));
        Some((x - last_x, y - last_y))
    }

    /// The finger went up or the stream was cancelled.
    pub const fn end(&mut self) {
        self.last = None;
    }
}

/// The light values after a drag of `(dx, dy)` logical px (the core clamps them later).
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the unfused multiply-then-add is the documented arithmetic; mul_add would change the rounding"
)]
pub fn light_after_drag(values: LightValues, dx: f32, dy: f32) -> LightValues {
    LightValues {
        azimuth_deg: values.azimuth_deg + dx * AZIMUTH_DEG_PER_PX,
        elevation_deg: values.elevation_deg - dy * ELEVATION_DEG_PER_PX,
        ..values
    }
}

/// Moves the light by a drag of `(dx, dy)` logical px and stores the result in the
/// settings.
pub fn drag_light(state: &mut AppState, dx: f32, dy: f32) {
    let values = light_after_drag(lighting_view(state).values, dx, dy);
    set_light_values(state, values);
    state.viewport.render_live.light_moved();
}

/// The export sizes (long edge, px) the sheet offers under the settings' cap: the presets
/// at or below it, or the cap itself when it is below them all.
#[must_use]
pub fn size_options(cap_px: u32) -> Vec<u32> {
    options_under(&SIZE_PRESETS, cap_px)
}

/// The export sample counts the sheet offers under the settings' cap.
#[must_use]
pub fn spp_options(cap_spp: u32) -> Vec<u32> {
    options_under(&SPP_PRESETS, cap_spp)
}

fn options_under(presets: &[u32], cap: u32) -> Vec<u32> {
    let cap = cap.max(1);
    let options: Vec<u32> = presets.iter().copied().filter(|&p| p <= cap).collect();
    if options.is_empty() {
        vec![cap]
    } else {
        options
    }
}

/// The export color space the sheet's segmented control numbers (`0` sRGB, `1` Display P3).
#[must_use]
pub const fn color_space_from_index(index: i32) -> ExportColorSpace {
    if index == 1 {
        ExportColorSpace::DisplayP3
    } else {
        ExportColorSpace::Srgb
    }
}

/// The export request for the sheet's choices (indices past the end pick the last option;
/// the caps of `settings` bound both numbers).
#[must_use]
pub fn export_request(
    settings: &RenderSettings,
    size_index: usize,
    spp_index: usize,
    color_space: ExportColorSpace,
) -> ExportRequest {
    let sizes = size_options(settings.export_cap_px);
    let spps = spp_options(settings.export_cap_spp);
    let pick = |options: &[u32], index: usize| {
        options
            .get(index.min(options.len().saturating_sub(1)))
            .copied()
            .unwrap_or(1)
    };
    ExportRequest {
        long_edge_px: pick(&sizes, size_index).min(settings.export_cap_px.max(1)),
        spp: pick(&spps, spp_index).min(settings.export_cap_spp.max(1)),
        color_space,
    }
}

/// A measured time for one export size, scaled to other sizes (the cost is proportional to
/// the pixels times the samples).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExportEstimate {
    seconds: Option<f32>,
    long_edge_px: u32,
    spp: u32,
}

impl ExportEstimate {
    /// `seconds` (from `ExportJob::estimate_seconds`) for a job of `request`'s size.
    #[must_use]
    pub fn new(request: &ExportRequest, seconds: Option<f32>) -> Self {
        Self {
            seconds: seconds.filter(|s| s.is_finite() && *s >= 0.0),
            long_edge_px: request.long_edge_px.max(1),
            spp: request.spp.max(1),
        }
    }

    /// The estimate for `request`, if there is a measurement.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "an estimate in seconds does not need f64"
    )]
    pub fn for_request(&self, request: &ExportRequest) -> Option<f32> {
        let seconds = self.seconds?;
        let ratio = |a: u32, b: u32| f64::from(a.max(1)) / f64::from(b);
        let edge = ratio(request.long_edge_px, self.long_edge_px);
        let scale = edge * edge * ratio(request.spp, self.spp);
        Some((f64::from(seconds) * scale) as f32)
    }
}

/// The sentence under the export options.
#[must_use]
pub fn estimate_text(seconds: Option<f32>) -> String {
    match seconds {
        None => "Time estimate unavailable until the preview has run.".to_owned(),
        Some(s) if s < 10.0 => "Under 10 seconds.".to_owned(),
        Some(s) if s < 90.0 => format!("About {} seconds.", round_to_5(s)),
        Some(s) => {
            let minutes = (s / 60.0).round().max(2.0);
            format!("About {minutes:.0} minutes.")
        }
    }
}

/// `value` rounded to the nearest multiple of 5.
fn round_to_5(value: f32) -> f32 {
    (value / 5.0).round() * 5.0
}

/// A file size for the sheet: "850 KB", "1.4 MB".
#[must_use]
pub fn byte_size_text(bytes: usize) -> String {
    const KB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KB {
        format!("{bytes} bytes")
    } else if value < KB * KB {
        format!("{:.0} KB", value / KB)
    } else {
        format!("{:.1} MB", value / (KB * KB))
    }
}

/// The file name an exported picture suggests: the design's title with characters a file
/// name cannot hold replaced by spaces, then `.png`.
#[must_use]
pub fn export_file_name(title: &str) -> String {
    let cleaned: String = title
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '.' | '(' | ')') {
                c
            } else {
                ' '
            }
        })
        .collect();
    let words: Vec<&str> = cleaned.split_whitespace().collect();
    let joined = words.join(" ");
    let trimmed: String = joined.chars().take(60).collect();
    let trimmed = trimmed.trim().trim_matches('.').trim();
    if trimmed.is_empty() {
        "Indicatrix Render.png".to_owned()
    } else {
        format!("{trimmed}.png")
    }
}

#[cfg(test)]
#[path = "render_view_tests.rs"]
mod tests;
