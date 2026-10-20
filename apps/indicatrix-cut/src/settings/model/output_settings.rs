//! The Render & Output tab's persisted and shared values: the output defaults of the still
//! export and the tilt-video export ([`OutputSettings`], `AppSettings::output`), the one
//! bounce ladder, the named sample ranges, and [`RemoteAvailability`].
//!
//! Pure data and logic (no Slint). Later waves read the constants and push them to Slint
//! through a global model property, so no Slint file keeps a literal copy.

use super::remote_endpoint::{ExportTransfer, TiltVideoCompute};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------
// The one bounce ladder.
// ---------------------------------------------------------------------------------------

/// The bounce-count rungs of every bounce picker (live view, still export, tilt video), in
/// pill order. Raised from the old 4/8/12/16/24 ladder per the `bounce_cost.rs` benchmark.
pub const BOUNCE_LADDER: [u32; 6] = [4, 8, 12, 24, 64, 128];

/// The index of the rung nearest to `bounces`. Exact matches map directly; anything else
/// (a retired 16-bounce rung, a hand-edited value) picks the nearest rung by absolute
/// distance, the lower rung on a tie. Never used to clamp the stored value itself.
#[must_use]
pub const fn bounce_index(bounces: u32) -> usize {
    let mut best_idx = 0usize;
    let mut best_dist = u32::MAX;
    let mut i = 0usize;
    while i < BOUNCE_LADDER.len() {
        let dist = BOUNCE_LADDER[i].abs_diff(bounces);
        if dist < best_dist {
            best_dist = dist;
            best_idx = i;
        }
        i += 1;
    }
    best_idx
}

/// The rung value at pill `index`, an out-of-range index limited to the ends (a negative
/// Slint index selects the first rung).
#[must_use]
pub const fn nearest_bounce_cap(index: i32) -> u32 {
    if index <= 0 {
        BOUNCE_LADDER[0]
    } else if index.cast_unsigned() as usize >= BOUNCE_LADDER.len() {
        BOUNCE_LADDER[BOUNCE_LADDER.len() - 1]
    } else {
        BOUNCE_LADDER[index.cast_unsigned() as usize]
    }
}

// ---------------------------------------------------------------------------------------
// Sample ranges, one per purpose.
// ---------------------------------------------------------------------------------------

/// The live view's target samples, as the power-of-two exponent range of the slider:
/// 2^3 = 8 up to 2^10 = 1024.
pub const LIVE_SAMPLES_EXPONENT_RANGE: (u32, u32) = (3, 10);

/// The still export's and tilt video's samples, as the exponent range of their sliders:
/// 2^3 = 8 up to 2^15 = 32768.
pub const EXPORT_SAMPLES_EXPONENT_RANGE: (u32, u32) = (3, 15);

/// The library thumbnails' samples per pixel (`AppSettings::preview_spp`), slider step 32.
pub const PREVIEW_SAMPLES_RANGE: (u32, u32) = (32, 512);

/// The library thumbnails' square size in pixels (`AppSettings::preview_size`), slider step 16.
pub const PREVIEW_SIZE_RANGE: (u32, u32) = (96, 256);

/// `exponent` limited to `range` (an inclusive exponent range from above).
#[must_use]
pub const fn clamp_exponent(exponent: u32, range: (u32, u32)) -> u32 {
    if exponent < range.0 {
        range.0
    } else if exponent > range.1 {
        range.1
    } else {
        exponent
    }
}

// ---------------------------------------------------------------------------------------
// Remote availability.
// ---------------------------------------------------------------------------------------

/// What the app knows about the remote, replacing the three ad-hoc flags
/// (`RemoteWorkerModel.live_remote_available` = configured, `ExportModel.remote_available` =
/// probed reachable and render-capable, `TiltVideoExportModel.remote_configured` =
/// configured; the job script's remote option now reads `configured` too).
///
/// Each surface uses the right half: pickers that only need a target to exist (live
/// compute, the tilt-video pill, job scripts) read [`Self::configured`]; the still export
/// (which probes before it starts) also requires [`Self::reachable_render_capable`]; see
/// [`Self::disabled_reason`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RemoteAvailability {
    /// A remote endpoint is set up in the settings.
    pub configured: bool,
    /// The last probe found the remote reachable AND render-capable (not library-only).
    /// Meaningless (and `false`) while nothing is configured.
    pub reachable_render_capable: bool,
}

impl RemoteAvailability {
    /// No remote at all.
    pub const NONE: Self = Self {
        configured: false,
        reachable_render_capable: false,
    };

    /// A configured remote whose probe has not found it usable (yet).
    #[must_use]
    pub const fn configured_only() -> Self {
        Self {
            configured: true,
            reachable_render_capable: false,
        }
    }

    /// The tooltip for a remote-only choice that is greyed out, `None` when it is offered.
    /// `probed` says whether this surface probes at all (the still export does, the live
    /// picker does not and therefore never blames reachability).
    #[must_use]
    pub const fn disabled_reason(self, probed: bool) -> Option<&'static str> {
        if !self.configured {
            Some("No remote is configured. Add one in the Remote dialog.")
        } else if probed && !self.reachable_render_capable {
            Some(
                "The remote is configured but was not reachable and render-capable when last checked.",
            )
        } else {
            None
        }
    }
}

// ---------------------------------------------------------------------------------------
// Output defaults.
// ---------------------------------------------------------------------------------------

/// The still export dialog's persisted values: it seeds its controls from them when it
/// opens and writes the values used back when an export starts ("last used").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StillOutput {
    /// Image-forming renderer; missing settings keep the spectral path tracer.
    pub engine: indicatrix::renderer::engine::EngineSpec,
    /// `ExportModel.selected_size_preset`: 0 = 1080p, 1 = 4K, 2 = custom.
    pub size_preset: i32,
    /// Custom width in pixels (used when `size_preset == 2`).
    pub custom_width: u32,
    /// Custom height in pixels.
    pub custom_height: u32,
    /// Samples as a power-of-two exponent, within [`EXPORT_SAMPLES_EXPONENT_RANGE`].
    pub sample_exponent: u32,
    /// Maximum ray bounces (a [`BOUNCE_LADDER`] rung value).
    pub bounces: u32,
    /// `ExportModel.selected_color_space_index`: 0 = sRGB, 1 = Display P3, 2 = Rec.2020.
    pub color_space_index: i32,
    /// Compute pill (local / remote / local + remote). Without a usable remote the export
    /// renders locally whatever this holds.
    pub compute: TiltVideoCompute,
    /// Transfer choice; `None` follows the remote endpoint's own `export_transfer`.
    pub transfer: Option<ExportTransfer>,
}

impl Default for StillOutput {
    fn default() -> Self {
        Self {
            engine: indicatrix::renderer::engine::EngineSpec {
                ray: indicatrix::renderer::engine::RayOptions {
                    aa_taps: 16,
                    fire: true,
                },
                ..Default::default()
            },
            size_preset: 0,
            custom_width: 1920,
            custom_height: 1080,
            sample_exponent: 10,
            bounces: BOUNCE_LADDER[2],
            color_space_index: 0,
            compute: TiltVideoCompute::Both,
            transfer: None,
        }
    }
}

/// The tilt-video export dialog's persisted values (same seed-and-write-back rule). The
/// compute pill is NOT repeated here: it already persists as
/// `AppSettings::tilt_video_compute_target`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TiltVideoOutput {
    /// Image-forming renderer; missing settings keep the spectral path tracer.
    pub engine: indicatrix::renderer::engine::EngineSpec,
    /// `TiltVideoExportModel.resolution_preset` (an index into
    /// `gui::tilt::video_export::params::RESOLUTION_PRESETS`, or the custom index).
    pub resolution_preset: i32,
    /// Custom width in pixels.
    pub custom_width: u32,
    /// Custom height in pixels.
    pub custom_height: u32,
    /// Samples as a power-of-two exponent, within [`EXPORT_SAMPLES_EXPONENT_RANGE`]; lower
    /// than the still default because it is multiplied by the frame count.
    pub sample_exponent: u32,
    /// Maximum ray bounces (a [`BOUNCE_LADDER`] rung value).
    pub bounces: u32,
    /// Colour space index, as for [`StillOutput::color_space_index`].
    pub color_space_index: i32,
    /// Transfer choice; `None` follows the remote endpoint's own `export_transfer`.
    pub transfer: Option<ExportTransfer>,
    /// The output folder-name template (`TiltVideoExportModel.filename_template`).
    pub filename_template: String,
}

/// The tilt-video folder-name template a fresh install starts with.
pub const DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE: &str =
    "tilt_video_{design}_{material}_axis{axis}_{width}x{height}_{timestamp}";

impl Default for TiltVideoOutput {
    fn default() -> Self {
        Self {
            engine: indicatrix::renderer::engine::EngineSpec {
                ray: indicatrix::renderer::engine::RayOptions {
                    aa_taps: 16,
                    fire: true,
                },
                ..Default::default()
            },
            resolution_preset: 8,
            custom_width: 512,
            custom_height: 512,
            sample_exponent: 6,
            bounces: BOUNCE_LADDER[2],
            color_space_index: 0,
            transfer: None,
            filename_template: DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE.to_string(),
        }
    }
}

/// `AppSettings::output`: the persisted output defaults. A settings file without the
/// table loads the defaults above (every struct and field is `serde(default)`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OutputSettings {
    /// The still export's defaults.
    pub still: StillOutput,
    /// The tilt video's defaults.
    pub tilt_video: TiltVideoOutput,
}

impl OutputSettings {
    /// Limits every value to what its control allows, so a hand-edited file cannot seed a
    /// dialog with an impossible slider position.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        self.still.engine.ray = self.still.engine.ray.sanitized();
        self.tilt_video.engine.ray = self.tilt_video.engine.ray.sanitized();
        let image_dim = |v: u32| v.clamp(16, 32768);
        self.still.size_preset = self.still.size_preset.clamp(0, 2);
        self.still.custom_width = image_dim(self.still.custom_width);
        self.still.custom_height = image_dim(self.still.custom_height);
        self.still.sample_exponent =
            clamp_exponent(self.still.sample_exponent, EXPORT_SAMPLES_EXPONENT_RANGE);
        self.still.bounces = self.still.bounces.max(1);
        self.still.color_space_index = self.still.color_space_index.clamp(0, 2);

        self.tilt_video.resolution_preset = self.tilt_video.resolution_preset.max(0);
        self.tilt_video.custom_width = self.tilt_video.custom_width.clamp(128, 8192);
        self.tilt_video.custom_height = self.tilt_video.custom_height.clamp(128, 8192);
        self.tilt_video.sample_exponent = clamp_exponent(
            self.tilt_video.sample_exponent,
            EXPORT_SAMPLES_EXPONENT_RANGE,
        );
        self.tilt_video.bounces = self.tilt_video.bounces.max(1);
        self.tilt_video.color_space_index = self.tilt_video.color_space_index.clamp(0, 2);
        if self.tilt_video.filename_template.trim().is_empty() {
            self.tilt_video.filename_template = DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE.to_string();
        }
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ladder_round_trips_and_unknown_values_pick_the_nearest_rung() {
        for (i, rung) in BOUNCE_LADDER.iter().enumerate() {
            assert_eq!(bounce_index(*rung), i);
            assert_eq!(
                nearest_bounce_cap(i32::try_from(i).expect("ladder index fits i32")),
                *rung
            );
        }
        assert_eq!(bounce_index(16), 2);
        assert_eq!(bounce_index(1), 0);
        assert_eq!(bounce_index(999), 5);
        assert_eq!(bounce_index(44), 3);
        assert_eq!(nearest_bounce_cap(-3), 4);
        assert_eq!(nearest_bounce_cap(40), 128);
    }

    #[test]
    fn remote_availability_disabled_reasons() {
        let up = RemoteAvailability {
            configured: true,
            reachable_render_capable: true,
        };
        assert_eq!(up.disabled_reason(true), None);
        assert!(RemoteAvailability::NONE.disabled_reason(false).is_some());
        // A surface that does not probe never blames reachability; one that probes does.
        assert_eq!(
            RemoteAvailability::configured_only().disabled_reason(false),
            None
        );
        assert!(
            RemoteAvailability::configured_only()
                .disabled_reason(true)
                .is_some()
        );
    }

    #[test]
    fn a_settings_file_without_output_loads_the_defaults() {
        use crate::settings::model::SettingsFile;
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert_eq!(parsed.settings.output, OutputSettings::default());
        let partial: SettingsFile =
            toml::from_str("[settings.output.still]\nsample_exponent = 12\n").expect("parse");
        assert_eq!(partial.settings.output.still.sample_exponent, 12);
        assert_eq!(
            partial.settings.output.tilt_video,
            TiltVideoOutput::default()
        );
    }

    #[test]
    fn output_values_are_limited_to_their_controls() {
        let mut wild = OutputSettings::default();
        wild.still.sample_exponent = 99;
        wild.tilt_video.custom_width = 1;
        wild.tilt_video.filename_template = "  ".to_string();
        let tame = wild.sanitised();
        assert_eq!(tame.still.sample_exponent, 15);
        assert_eq!(tame.tilt_video.custom_width, 128);
        assert_eq!(
            tame.tilt_video.filename_template,
            DEFAULT_TILT_VIDEO_FILENAME_TEMPLATE
        );
    }
}
